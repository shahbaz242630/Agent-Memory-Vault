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
//! ## Stale lockfile policy
//!
//! If the holder crashes / is killed without dropping the guard, the
//! lockfile persists. The next acquire attempt returns
//! [`VaultError::ConsolidatorBusy`] **explicitly** — we do NOT auto-take-over
//! a stale lock. Reasoning: blindly stealing the lock risks racing a
//! still-running orphan process (e.g., a long-stalled SQLite syscall the
//! OS hasn't yet reaped). Operators remove `.consolidator.lock` by hand
//! after verifying no consolidator is running; the forensic content
//! (PID + acquired_at timestamp) inside the file makes this verification
//! trivial.
//!
//! Auto-stale recovery (e.g., if PID is dead and acquired_at > 1h ago)
//! is forward-task for V1.0+ multi-device sync where automated takeover
//! becomes operationally necessary.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use vault_core::{VaultError, VaultResult};

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
        match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(mut file) => {
                // Forensic payload — PID + ISO-8601 timestamp. Best-effort;
                // we own the lockfile by virtue of the successful
                // create_new even if the write fails, so log + continue.
                let payload = format!(
                    "pid={} acquired_at={}\n",
                    std::process::id(),
                    Utc::now().to_rfc3339()
                );
                if let Err(e) = file.write_all(payload.as_bytes()) {
                    tracing::warn!(
                        path = %path.display(),
                        error = %e,
                        "consolidator lockfile acquired but forensic payload write failed"
                    );
                }
                Ok(Self { path, held: true })
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                // Read the existing lockfile's forensic payload for the
                // error message. Best-effort — if the read fails (race
                // with the holder releasing it, permission error), fall
                // back to a generic message.
                let context = std::fs::read_to_string(&path)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "holder context unavailable".to_string());
                Err(VaultError::ConsolidatorBusy(format!(
                    "lockfile at {} already held: {context}",
                    path.display()
                )))
            }
            Err(e) => Err(VaultError::Io(e)),
        }
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
}
