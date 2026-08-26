//! Cross-process consolidator lockfile (RAII).
//!
//! Memory Vault's locked-next-arc Step 4 contract: the consolidator runs at
//! most once per vault at any moment — scheduled nightly OR manual via
//! `vault-cli consolidate run`. Two callers must NOT clobber each other's
//! state (Phase 3 merges are per-merge transactional, but K-means topic
//! discovery + REPORT writes are not, and an overlap would race the
//! atomic-rename REPORT artifact write at Commit 4).
//!
//! ## Mechanism
//!
//! [`ConsolidatorLock::try_acquire`] atomically creates `.consolidator.lock`
//! under the vault root using `OpenOptions::new().create_new(true)`. The
//! kernel-atomic `O_CREAT | O_EXCL` semantics (POSIX) / `CREATE_NEW`
//! (Windows) guarantee that two concurrent acquire attempts can never both
//! succeed — exactly one returns Ok, the other returns
//! [`VaultError::ConsolidatorBusy`].
//!
//! ## Stale lockfile policy (REVISED — ADR-SEC-012, 2026-08-26)
//!
//! If the holder crashes or is killed without dropping the guard, the lockfile
//! persists. **A lock whose owner is provably gone is now reclaimed
//! automatically**; a lock whose owner is alive is never touched.
//!
//! ### What this replaces, and why
//!
//! The previous policy was: *"we do NOT auto-take-over a stale lock …
//! Operators remove `.consolidator.lock` by hand after verifying no
//! consolidator is running."* Its reasoning was sound — blindly stealing a
//! lock risks racing a still-running orphan — but it assumed an **operator**.
//!
//! Memory Vault is a desktop app. Its user is not an operator, has no shell,
//! and never sees the vault directory. What they get is one opaque
//! `maintenance_vault_busy` string and a vault that silently stops
//! consolidating — forever, with no route back.
//!
//! That is not theoretical. On 2026-08-26 the founder's vault was found
//! holding a `.vault.lock` written by a process that died on **2026-07-26**.
//! Nothing had run since. It would have killed the first scheduled nightly
//! consolidation, and every one after it, in silence.
//!
//! ### Why this is safe now when it was not before
//!
//! The old objection — "we cannot tell a dead owner from a stalled one" —
//! was correct as long as the check was a PID lookup, which is defeated by
//! PID reuse. It is answered by not asking about PIDs at all: the owner holds
//! the lockfile OPEN, and the OS refuses a conflicting open while the owner
//! lives. Liveness becomes something the kernel enforces rather than
//! something we infer. See [`holder_is_gone`].
//!
//! The guarantee is unchanged and non-negotiable: **two writers must never
//! hold one vault**. `a_live_lock_is_never_reclaimed` pins it. If that test
//! ever fails, revert this mechanism rather than tuning it.
//!
//! ### Platform scope
//!
//! OS-enforced liveness is Windows-only, because POSIX has no mandatory
//! locking and the same probe there would call every live lock stale. Other
//! platforms keep the previous conservative behaviour. V0.2 ships Windows-only
//! (ADR-092: macOS/Linux backends are unit-tested builders, live behaviour
//! CI/beta-pending), so this covers every shipping platform today. A proper
//! advisory-lock backend is the follow-up when those platforms go live.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use chrono::Utc;
use vault_core::{VaultError, VaultResult};

/// `FILE_SHARE_READ | FILE_SHARE_DELETE`.
///
/// **READ** so a would-be acquirer can still read the forensic payload for its
/// error message. **DELETE** because without it this guard cannot delete its
/// own lockfile: Windows refuses `remove_file` while any handle is open unless
/// that handle shares delete access, and Rust drops struct fields only AFTER
/// the `Drop::drop` body has run — so the handle is still open at exactly the
/// moment `Drop` tries to remove the file. Omitting this flag made the lock
/// permanent, which is strictly worse than the stale-lock bug ADR-SEC-012 set
/// out to fix. Caught by the pre-existing
/// `drop_releases_lockfile_so_subsequent_acquire_succeeds` and
/// `drop_releases_lockfile_after_panic_unwind` tests on the first run.
///
/// **WRITE is deliberately still not shared**, which is what keeps
/// [`holder_is_gone`] meaningful: the liveness probe asks for write access, and
/// a live owner must refuse it.
#[cfg(windows)]
const LOCK_SHARE_MODE: u32 = 0x0000_0001 | 0x0000_0004;

/// Open the lockfile as the new owner, keeping the handle.
///
/// On Windows the handle is opened with [`LOCK_SHARE_MODE`] (read + delete,
/// never write). That is what makes [`holder_is_gone`] work: while this process
/// lives, the OS refuses any other process's attempt to open the file for
/// writing; the instant the process
/// dies — cleanly, killed, or blue-screened — the kernel closes the handle and
/// that refusal stops. Liveness is therefore enforced by the OS rather than
/// inferred by us.
fn create_owned(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(windows)]
    opts.share_mode(LOCK_SHARE_MODE);
    opts.open(path)
}

/// Is the lockfile's owner definitely gone?
///
/// # Windows — an OS-enforced answer
///
/// Tries to open the existing lockfile for WRITE with no sharing. A live owner
/// holds it under [`LOCK_SHARE_MODE`], which shares read and delete but NOT
/// write, so this is refused. Once the owner's
/// process ends, the kernel releases its handle and the open succeeds. There is
/// no PID inspection, so this cannot be fooled by PID reuse — the classic bug
/// in hand-rolled stale-lock detection, where the OS recycles a dead process's
/// number onto something unrelated and a liveness check says "still running".
///
/// # Everywhere else — deliberately always `false`
///
/// POSIX has no mandatory locking: opening a file another process holds open
/// simply succeeds. The same probe would therefore report EVERY lock as stale
/// and hand out concurrent write access to one vault, which is far worse than
/// the problem it set out to fix. Non-Windows platforms keep the previous
/// conservative behaviour (report busy, let a human intervene) until a proper
/// advisory-lock backend exists. V0.2 ships Windows-only; macOS and Linux are
/// unit-tested builders whose live behaviour is CI/beta-pending.
#[cfg(windows)]
fn holder_is_gone(path: &Path) -> bool {
    let mut opts = OpenOptions::new();
    // `write(true)` WITHOUT `truncate(true)`: this must never damage a lockfile
    // that turns out to be live.
    opts.write(true).share_mode(0);
    opts.open(path).is_ok()
}

#[cfg(not(windows))]
fn holder_is_gone(_path: &Path) -> bool {
    false
}

/// Filename of the lockfile under the vault root.
///
/// Hidden (leading-dot) so it doesn't appear in casual directory listings
/// of the user's vault; consistent with other dotfile conventions like
/// `.git/`. The lockfile is removed on graceful drop; if it persists past
/// the consolidator's run, the previous run crashed.
pub(crate) const LOCKFILE_NAME: &str = ".consolidator.lock";

/// Filename of the vault-owner lock (ADR-SEC-002). At most one live
/// `vault-cli daemon` owns a vault at a time — this replaces the implicit
/// single-writer guard the DuckDB exclusive file lock provided before the graph
/// moved in-memory (ADR-SEC-002). Distinct from [`LOCKFILE_NAME`], which
/// serializes consolidation runs.
pub const VAULT_LOCKFILE_NAME: &str = ".vault.lock";

/// RAII guard for the consolidator's cross-process lockfile.
///
/// Acquired by [`Self::try_acquire`]; released on drop by removing the
/// lockfile. Drop is best-effort — if removal fails (e.g., file already
/// gone, permission error), a `tracing::warn!` fires but the drop completes
/// (guards must not panic).
///
/// The guard is `!Send` by virtue of holding no thread-bound state, but
/// callers should still hold it for the duration of one consolidation run.
/// Cloning is intentionally not implemented — multiple guards for the same
/// lockfile would defeat the single-writer invariant.
#[derive(Debug)]
pub struct ConsolidatorLock {
    path: PathBuf,
    /// `true` once the guard has acquired the lockfile and is responsible
    /// for cleanup on drop. `false` only in the `try_acquire` error path
    /// before the guard struct is constructed; the field stays for forward
    /// readability of the Drop impl.
    held: bool,
    /// The open handle, held for the guard's whole lifetime (ADR-SEC-012).
    ///
    /// On Windows its mere existence is what tells a later acquirer that this
    /// owner is still alive, and the kernel closing it on process death is what
    /// makes a crashed owner's lock self-identifying as stale. On other
    /// platforms it is inert but harmless, and is kept unconditionally so the
    /// struct does not need `#[cfg]` on a field — which would push platform
    /// conditionals into every construction site and every test.
    _handle: Option<File>,
}

impl ConsolidatorLock {
    /// Attempt to atomically acquire the consolidator lock at
    /// `<vault_root>/.consolidator.lock`.
    ///
    /// # Errors
    ///
    /// - [`VaultError::ConsolidatorBusy`] — the lockfile already exists.
    ///   Carries forensic context (path + PID-of-holder when readable from
    ///   the existing lockfile) so the operator can investigate.
    /// - [`VaultError::Io`] — non-`AlreadyExists` I/O failure (permissions,
    ///   disk full, parent directory missing, etc.).
    pub fn try_acquire(vault_root: &Path) -> VaultResult<Self> {
        Self::try_acquire_named(vault_root, LOCKFILE_NAME)
    }

    /// Like [`Self::try_acquire`] but with a caller-chosen lockfile name. Used
    /// for the vault-owner lock ([`VAULT_LOCKFILE_NAME`], ADR-SEC-002) — at most
    /// one live daemon per vault — distinct from the consolidator run lock.
    ///
    /// # Errors
    ///
    /// Same as [`Self::try_acquire`]: [`VaultError::ConsolidatorBusy`] when the
    /// lockfile already exists, [`VaultError::Io`] otherwise.
    pub fn try_acquire_named(vault_root: &Path, lockfile_name: &str) -> VaultResult<Self> {
        let path = vault_root.join(lockfile_name);
        match Self::claim(&path) {
            Ok(guard) => Ok(guard),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Read the existing lockfile's forensic payload for the
                // error message. Best-effort — if the read fails (race
                // with the holder releasing it, permission error), fall
                // back to a generic message.
                let context = std::fs::read_to_string(&path)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "holder context unavailable".to_string());

                // ADR-SEC-012 — reclaim a lock whose owner is provably gone.
                //
                // Before this, a lockfile left behind by a crash was permanent:
                // the previous policy said "we do NOT auto-take-over a stale
                // lock ... operators remove it by hand". That is a defensible
                // rule for a server with an operator. This is a desktop app,
                // and its user is not an operator — they get one opaque
                // `maintenance_vault_busy` string and a vault that quietly
                // stops consolidating, forever, with no way to recover.
                //
                // Found live on the founder's machine 2026-08-26: a
                // `.vault.lock` from a process that died on 2026-07-26 would
                // have silently killed the FIRST scheduled nightly run.
                if holder_is_gone(&path) {
                    tracing::warn!(
                        path = %path.display(),
                        stale_holder = %context,
                        "reclaiming a lock whose owner is gone (ADR-SEC-012); \
                         the previous owner did not shut down cleanly"
                    );
                    // Remove + re-claim. If anything goes wrong here, fall
                    // through to the busy error rather than looping: a second
                    // process may have legitimately won the race between the
                    // probe and the remove, and reporting busy is always the
                    // safe direction to be wrong in.
                    if std::fs::remove_file(&path).is_ok() {
                        if let Ok(guard) = Self::claim(&path) {
                            return Ok(guard);
                        }
                    }
                }

                Err(VaultError::ConsolidatorBusy(format!(
                    "lockfile at {} already held: {context}. If no other Memory \
                     Vault process is running, this lock was left behind by one \
                     that did not exit cleanly; removing the file releases it.",
                    path.display()
                )))
            }
            Err(e) => Err(VaultError::Io(e)),
        }
    }

    /// Create the lockfile, write the forensic payload, and keep the handle.
    ///
    /// Split out so the stale-reclaim path can re-run exactly the same
    /// acquisition rather than a near-copy that could drift from it.
    fn claim(path: &Path) -> std::io::Result<Self> {
        let mut file = create_owned(path)?;
        // Forensic payload — PID + ISO-8601 timestamp. Best-effort; we own
        // the lockfile by virtue of the successful create_new even if the
        // write fails, so log + continue.
        let payload = format!(
            "pid={} acquired_at={}\n",
            std::process::id(),
            Utc::now().to_rfc3339()
        );
        if let Err(e) = file.write_all(&payload.into_bytes()) {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "consolidator lockfile acquired but forensic payload write failed"
            );
        }
        // Flush so a reader (including `holder_is_gone`'s caller reading the
        // payload for an error message) sees the PID rather than an empty file.
        let _ = file.flush();
        Ok(Self {
            path: path.to_path_buf(),
            held: true,
            _handle: Some(file),
        })
    }

    /// Path of the lockfile this guard owns. Exposed for diagnostics + tests.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ConsolidatorLock {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        // Close our own handle BEFORE removing the file.
        //
        // Rust drops fields only after this body returns, so `self._handle`
        // would otherwise still be open here. On Windows an open handle blocks
        // `remove_file` unless it shares delete access; `LOCK_SHARE_MODE` does
        // share it, so this is belt-and-braces rather than load-bearing — but
        // it makes the ordering explicit instead of resting on a flag several
        // dozen lines away, and it is what a reader would expect to see.
        drop(self._handle.take());

        if let Err(e) = std::fs::remove_file(&self.path) {
            // Best-effort cleanup. Common failure modes: another tool
            // raced and removed it (benign); permission error (would have
            // also blocked acquire so unlikely here); file already gone
            // (benign). Log at warn so operators see stale-file events
            // without panicking the drop.
            tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "failed to remove consolidator lockfile on drop"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn try_acquire_creates_lockfile_with_pid_and_timestamp() {
        let tmp = TempDir::new().unwrap();
        let guard = ConsolidatorLock::try_acquire(tmp.path()).unwrap();

        let lockfile_path = tmp.path().join(LOCKFILE_NAME);
        assert!(
            lockfile_path.exists(),
            "lockfile MUST exist at {} after successful acquire",
            lockfile_path.display()
        );

        let contents = std::fs::read_to_string(&lockfile_path).unwrap();
        assert!(
            contents.contains("pid="),
            "lockfile forensic payload MUST include 'pid=' prefix; got: {contents}"
        );
        assert!(
            contents.contains("acquired_at="),
            "lockfile forensic payload MUST include 'acquired_at=' prefix; got: {contents}"
        );

        drop(guard);
    }

    #[test]
    fn try_acquire_returns_busy_when_lockfile_already_exists() {
        let tmp = TempDir::new().unwrap();
        let _first = ConsolidatorLock::try_acquire(tmp.path()).unwrap();

        let second = ConsolidatorLock::try_acquire(tmp.path());
        let err = second.expect_err("second acquire MUST fail with ConsolidatorBusy");

        match err {
            VaultError::ConsolidatorBusy(msg) => {
                assert!(
                    msg.contains(LOCKFILE_NAME),
                    "ConsolidatorBusy message MUST name the lockfile path; got: {msg}"
                );
                assert!(
                    msg.contains("pid=") || msg.contains("holder context unavailable"),
                    "ConsolidatorBusy message MUST carry forensic context; got: {msg}"
                );
            }
            other => panic!("expected ConsolidatorBusy, got: {other:?}"),
        }
    }

    #[test]
    fn drop_releases_lockfile_so_subsequent_acquire_succeeds() {
        let tmp = TempDir::new().unwrap();
        {
            let _first = ConsolidatorLock::try_acquire(tmp.path()).unwrap();
            // first guard goes out of scope here -> Drop removes the lockfile
        }

        let lockfile_path = tmp.path().join(LOCKFILE_NAME);
        assert!(
            !lockfile_path.exists(),
            "lockfile MUST be removed after guard drop; still exists at {}",
            lockfile_path.display()
        );

        // Acquiring again must succeed cleanly.
        let _second = ConsolidatorLock::try_acquire(tmp.path())
            .expect("acquire after drop MUST succeed; got error");
    }

    #[test]
    fn drop_releases_lockfile_after_panic_unwind() {
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();

        // Panic inside a closure; the guard is constructed BEFORE the
        // panic and goes out of scope during unwind, exercising the
        // panic-unwind drop path.
        let result = std::panic::catch_unwind(|| {
            let _guard = ConsolidatorLock::try_acquire(&tmp_path).unwrap();
            panic!("simulated inner failure mid-consolidation");
        });
        assert!(
            result.is_err(),
            "inner panic should propagate to catch_unwind"
        );

        // Lockfile must be gone after unwind.
        let lockfile_path = tmp_path.join(LOCKFILE_NAME);
        assert!(
            !lockfile_path.exists(),
            "lockfile MUST be removed on Drop even under panic unwind; \
             still exists at {}",
            lockfile_path.display()
        );

        // Subsequent acquire on the same path must succeed.
        let _retry = ConsolidatorLock::try_acquire(&tmp_path)
            .expect("acquire after panic-unwind drop MUST succeed");
    }

    #[test]
    fn try_acquire_propagates_non_already_exists_io_error_as_io_variant() {
        // Point at a vault_root that doesn't exist as a directory — the
        // join() succeeds but OpenOptions::open will fail with NotFound
        // (parent dir doesn't exist) rather than AlreadyExists.
        let bogus_root = std::path::PathBuf::from("/this/path/definitely/does/not/exist/vault");
        let err = ConsolidatorLock::try_acquire(&bogus_root)
            .expect_err("acquire under non-existent parent MUST fail");
        match err {
            VaultError::Io(io_err) => {
                assert_ne!(
                    io_err.kind(),
                    std::io::ErrorKind::AlreadyExists,
                    "non-existent parent MUST surface as a non-AlreadyExists io kind"
                );
            }
            other => panic!("expected VaultError::Io, got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    //   ADR-SEC-012 — stale-lock reclaim
    // ------------------------------------------------------------------

    /// The property that must NEVER break: a lock held by a live guard in
    /// this process is not reclaimable.
    ///
    /// This is the test that keeps ADR-SEC-012 honest. The whole point of the
    /// lock is that two writers cannot touch one vault at once; a stale-lock
    /// feature that guesses wrong turns a rare inconvenience into corruption.
    /// If this ever fails, the reclaim logic must be reverted, not tuned.
    #[test]
    fn a_live_lock_is_never_reclaimed() {
        let tmp = TempDir::new().unwrap();
        let _held = ConsolidatorLock::try_acquire(tmp.path()).expect("first acquire");

        let second = ConsolidatorLock::try_acquire(tmp.path());
        assert!(
            matches!(second, Err(VaultError::ConsolidatorBusy(_))),
            "a lock held by a LIVE guard must stay held, got: {second:?}"
        );
    }

    /// Releasing normally must leave nothing behind for the next acquirer.
    #[test]
    fn dropping_the_guard_frees_the_lock_for_a_later_acquire() {
        let tmp = TempDir::new().unwrap();
        {
            let _g = ConsolidatorLock::try_acquire(tmp.path()).expect("acquire");
        }
        assert!(
            !tmp.path().join(LOCKFILE_NAME).exists(),
            "drop must remove the lockfile"
        );
        ConsolidatorLock::try_acquire(tmp.path()).expect("re-acquire after clean release");
    }

    /// A lockfile with no live owner — exactly what a crash leaves behind —
    /// is reclaimed rather than blocking forever.
    ///
    /// Simulated by writing the lockfile directly: no process holds a handle
    /// on it, which is precisely the post-crash state (the kernel closed the
    /// dead owner's handle, the file itself survived).
    ///
    /// Windows-only: [`holder_is_gone`] deliberately answers `false`
    /// everywhere else, because POSIX has no mandatory locking and the same
    /// probe there would declare every live lock stale. See its docs.
    #[cfg(windows)]
    #[test]
    fn an_ownerless_lockfile_is_reclaimed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(LOCKFILE_NAME);

        // A crashed owner's leftovers: plausible payload, no open handle.
        std::fs::write(
            &path,
            "pid=999999 acquired_at=2026-07-26T05:46:21Z
",
        )
        .expect("write stale lockfile");

        let guard = ConsolidatorLock::try_acquire(tmp.path())
            .expect("an ownerless lock must be reclaimed, not reported busy");

        let contents = std::fs::read_to_string(&path).expect("read reclaimed lockfile");
        assert!(
            contents.contains(&format!("pid={}", std::process::id())),
            "the reclaimed lock must record THIS process as owner, got: {contents:?}"
        );
        drop(guard);
    }

    /// The same reclaim applies to the vault-owner lock, which is the one
    /// that actually bit us: `.vault.lock` left by a process that died on
    /// 2026-07-26 would have silently killed the first scheduled nightly run
    /// a month later.
    #[cfg(windows)]
    #[test]
    fn an_ownerless_vault_lock_is_reclaimed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(VAULT_LOCKFILE_NAME);
        std::fs::write(
            &path,
            "pid=31092 acquired_at=2026-07-26T05:46:21.568342100+00:00
",
        )
        .expect("write stale vault lock");

        ConsolidatorLock::try_acquire_named(tmp.path(), VAULT_LOCKFILE_NAME)
            .expect("an ownerless vault lock must be reclaimed");
    }

    /// A busy error has to tell the user what to do about it. The old message
    /// gave a path and a payload and left a non-technical user with nothing
    /// actionable.
    #[test]
    fn busy_error_explains_how_to_recover() {
        let tmp = TempDir::new().unwrap();
        let _held = ConsolidatorLock::try_acquire(tmp.path()).expect("first acquire");

        match ConsolidatorLock::try_acquire(tmp.path()) {
            Err(VaultError::ConsolidatorBusy(msg)) => {
                assert!(
                    msg.contains("did not exit cleanly"),
                    "busy error must explain the stale case, got: {msg}"
                );
                assert!(
                    msg.contains("removing the file"),
                    "busy error must say what removes the lock, got: {msg}"
                );
            }
            other => panic!("expected ConsolidatorBusy, got: {other:?}"),
        }
    }
}
