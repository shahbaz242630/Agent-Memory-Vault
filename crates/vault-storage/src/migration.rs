//! V0.1 plaintext → V0.2 sealed-at-rest migration.
//!
//! T0.2.0 close-out plan Phase 2 (HANDOFF.md). Implements the 6-state
//! detection rule from "T0.2.0 Phase 2 — plan iteration 2.1" §1 and the
//! migration loop from iteration 1 §1 + iteration 2 §1.
//!
//! The detector is metadata-only per ADR-018 spirit: it inspects file
//! presence + magic-byte prefixes/suffixes, never decoding any Lance
//! fragment. Decoding is the migration loop's job once the detector
//! returns [`MigrationDetectorOutcome::V0_1ShapeMigrate`] (lands in
//! Phase 2 step-(b)).

use std::io::SeekFrom;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::{info, instrument, trace, warn};

use vault_core::error::{VaultError, VaultResult};

use crate::vector_store::{LanceVectorStore, VectorStore, ALPHA_WARNING_FILENAME};

/// LANC magic — last 4 bytes of every Lance fragment file. Verified
/// empirically across V0.1 lance 0.15 and V0.2 lance 4.0 by the iteration
/// 3 spike (`crates/vault-storage/examples/v0_1_lance_compat_spike.rs`)
/// and by direct fixture inspection (see `tests/fixtures/v0_1_alpha_data_dir/README.md`).
const LANC_MAGIC: &[u8; 4] = b"LANC";

/// Sealed-framing prefix — first 2 bytes of every file written through
/// the at-rest sealing path: `version_byte (0x01) || granularity (0x00)`.
/// See ADR-008 amendment + [`crate::sealed_object_store`] (`TOTAL_FRAMING_LEN`,
/// `VERSION_BYTE`, `GRANULARITY_PER_FILE`).
const SEALED_FRAMING_PREFIX: &[u8; 2] = &[0x01, 0x00];

/// Outcome of the V0.1 → V0.2 migration detector. Six named states per
/// HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2.1" §1's detection-rule
/// table — variant names map 1:1 to the table's `Outcome` column.
///
/// ## State table (verbatim from iteration 2.1 §1)
///
/// | Marker file | Disk content                                | Outcome |
/// |---|---|---|
/// | Present | `.lance` ends with `LANC` (`4C 41 4E 43`)      | `V0_1ShapeMigrate` |
/// | Present | Sealed framing (`0x01 0x00`)                   | `PostSwapMarkerCleanup` |
/// | Present | Empty / mixed                                  | `HalfStateCorruptionFailClosed` |
/// | Absent  | `.lance` ends with `LANC`                      | `ThirdPartyDataFailClosed` |
/// | Absent  | Sealed framing                                 | `V0_2CleanNoOp` |
/// | Absent  | Empty                                          | `FirstRunInstallNoOp` |
///
/// `Marker file` = `<lance_data_dir>/<ALPHA_WARNING_FILENAME>`.
/// `Disk content` = `.lance` files under `<lance_data_dir>/<table>.lance/data/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationDetectorOutcome {
    /// V0.1-shape data detected (ADR-010 marker present + LANC magic at
    /// file END inside `<table>.lance/data/`). Phase 2 migration loop runs.
    V0_1ShapeMigrate,
    /// Phase 2 step-(b)-succeeded crash recovery. Marker present + sealed
    /// framing in `<table>.lance/data/`: the directory swap completed but
    /// the marker delete crashed mid-flight. Resolution: delete marker,
    /// return `NoMigrationNeeded`.
    PostSwapMarkerCleanup,
    /// Marker present but `<table>.lance/data/` is empty or mixed. Aborted
    /// write mid-creation; cannot safely recover. Surface to caller —
    /// migration fails closed rather than risk further corruption.
    HalfStateCorruptionFailClosed,
    /// V0.1-shape data without ADR-010 marker. Either corrupt (marker
    /// deleted by something) or non-Memory-Vault writer. Surface to caller
    /// — fail closed; do NOT migrate third-party data.
    ThirdPartyDataFailClosed,
    /// Clean post-migration V0.2 state — marker absent + sealed framing.
    /// Return `NoMigrationNeeded`.
    V0_2CleanNoOp,
    /// First-run V0.2 install — no marker, empty data dir. Return
    /// `NoMigrationNeeded`; vault-tauri's setup() will create a fresh
    /// sealed store.
    FirstRunInstallNoOp,
}

/// Internal classification of `<table>.lance/data/` content. Combines
/// with the marker-file presence to yield one of the 6 named outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentState {
    /// At least one `.lance` file under `<table>.lance/data/` ends with
    /// LANC magic.
    V0_1Lance,
    /// At least one `.lance` file under `<table>.lance/data/` starts with
    /// the sealed-framing prefix; no LANC magic anywhere.
    SealedV0_2,
    /// `<lance_data_dir>` does not exist, contains no `<x>.lance/` table
    /// dir with a populated `data/` subdir, or all `.lance` files match
    /// neither signal.
    Empty,
}

/// Detect the V0.1 / V0.2 / corrupted state of a Lance data directory
/// per the 6-state rule in [`MigrationDetectorOutcome`]'s docs.
///
/// `lance_data_dir` is the directory that contains the optional ALPHA
/// marker file plus the `<table>.lance/` table subdir — i.e., the same
/// path passed to [`crate::LanceVectorStore::open`] /
/// [`crate::LanceVectorStore::open_with_at_rest_key`].
///
/// I/O failures during the scan (permission denied on the parent dir,
/// read failure on a corrupt disk sector, etc.) bubble up as
/// [`vault_core::error::VaultError::Io`] — the caller should surface
/// them as a fatal startup error rather than treat them as
/// `NoMigrationNeeded` (fail closed on uncertainty).
#[instrument(skip(lance_data_dir), fields(lance_data_dir = %lance_data_dir.display()))]
pub async fn detect_v0_1_state(lance_data_dir: &Path) -> VaultResult<MigrationDetectorOutcome> {
    let marker_path = lance_data_dir.join(ALPHA_WARNING_FILENAME);
    let marker_present = fs::try_exists(&marker_path).await?;
    let content_state = classify_content(lance_data_dir).await?;

    let outcome = match (marker_present, content_state) {
        (true, ContentState::V0_1Lance) => MigrationDetectorOutcome::V0_1ShapeMigrate,
        (true, ContentState::SealedV0_2) => MigrationDetectorOutcome::PostSwapMarkerCleanup,
        (true, ContentState::Empty) => MigrationDetectorOutcome::HalfStateCorruptionFailClosed,
        (false, ContentState::V0_1Lance) => MigrationDetectorOutcome::ThirdPartyDataFailClosed,
        (false, ContentState::SealedV0_2) => MigrationDetectorOutcome::V0_2CleanNoOp,
        (false, ContentState::Empty) => MigrationDetectorOutcome::FirstRunInstallNoOp,
    };

    trace!(
        marker_present,
        ?content_state,
        ?outcome,
        "migration detector classified",
    );

    Ok(outcome)
}

/// Walk `<lance_data_dir>/<table>.lance/data/*.lance` and classify the
/// content state. LANC-at-end takes precedence: a real V0.1 file's
/// embedded-UUID prefix could in principle collide with `[0x01, 0x00]`,
/// so we MUST check the trailing magic before accepting the prefix as
/// sealed-shape evidence.
async fn classify_content(lance_data_dir: &Path) -> VaultResult<ContentState> {
    if !fs::try_exists(lance_data_dir).await? {
        return Ok(ContentState::Empty);
    }

    let mut found_sealed = false;

    let mut top = fs::read_dir(lance_data_dir).await?;
    while let Some(top_entry) = top.next_entry().await? {
        let table_dir = top_entry.path();

        if !top_entry.file_type().await?.is_dir() {
            continue;
        }
        if table_dir.extension().and_then(|s| s.to_str()) != Some("lance") {
            continue;
        }

        let data_dir = table_dir.join("data");
        if !fs::try_exists(&data_dir).await? {
            continue;
        }

        let mut data = fs::read_dir(&data_dir).await?;
        while let Some(file_entry) = data.next_entry().await? {
            let file_path = file_entry.path();
            if !file_entry.file_type().await?.is_file() {
                continue;
            }
            if file_path.extension().and_then(|s| s.to_str()) != Some("lance") {
                continue;
            }

            if file_ends_with(&file_path, LANC_MAGIC).await? {
                return Ok(ContentState::V0_1Lance);
            }
            if file_starts_with(&file_path, SEALED_FRAMING_PREFIX).await? {
                found_sealed = true;
            }
        }
    }

    Ok(if found_sealed {
        ContentState::SealedV0_2
    } else {
        ContentState::Empty
    })
}

/// Read the last `marker.len()` bytes of `path` and compare to `marker`.
/// Files smaller than the marker return `Ok(false)` — a 1-byte file simply
/// cannot carry a 4-byte magic.
async fn file_ends_with(path: &Path, marker: &[u8]) -> VaultResult<bool> {
    let mut file = fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    if len < marker.len() as u64 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-(marker.len() as i64))).await?;
    let mut buf = vec![0u8; marker.len()];
    file.read_exact(&mut buf).await?;
    Ok(buf == marker)
}

/// Read the first `marker.len()` bytes of `path` and compare to `marker`.
async fn file_starts_with(path: &Path, marker: &[u8]) -> VaultResult<bool> {
    let mut file = fs::File::open(path).await?;
    let len = file.metadata().await?.len();
    if len < marker.len() as u64 {
        return Ok(false);
    }
    let mut buf = vec![0u8; marker.len()];
    file.read_exact(&mut buf).await?;
    Ok(buf == marker)
}

// ────────────────────────────────────────────────────────────────────────
// Migration loop
// ────────────────────────────────────────────────────────────────────────

/// Outcome of [`migrate_v0_1_to_sealed_if_needed`]. Locked surface per
/// HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1" §1.
///
/// `NoMigrationNeeded` covers all detector outcomes that produce a no-op
/// migration: `V0_2CleanNoOp` (already migrated), `FirstRunInstallNoOp`
/// (clean V0.2 install), and `PostSwapMarkerCleanup` (Phase 2 step-(b)-
/// succeeded crash recovery — marker is deleted as a side effect of the
/// no-op return path).
///
/// `Migrated { rows_migrated }` is returned only when the detector
/// returns `V0_1ShapeMigrate` and the full read-V0.1 → write-sealed →
/// atomic-swap → marker-cleanup loop succeeds end-to-end.
///
/// Detector outcomes `HalfStateCorruptionFailClosed` and
/// `ThirdPartyDataFailClosed` map to `Err(VaultError::Storage)` —
/// surfaced rather than silently treated as no-op. The caller (vault-
/// tauri main.rs setup() step 5b) shows a fatal-startup dialog and
/// exits non-zero per ADR-040 fail-closed discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// No migration was needed. Detector classified the data dir as
    /// already-V0.2 (clean post-migration, post-swap recovery, or
    /// first-run install).
    NoMigrationNeeded,
    /// Migration ran end-to-end. `rows_migrated` is the row count
    /// preserved from the V0.1 source through the sealed write.
    Migrated {
        /// Number of memory rows successfully copied from the V0.1
        /// plaintext fragment(s) into the new sealed store.
        rows_migrated: u64,
    },
}

/// Sentinel cookie file written before the atomic dir-swap pair and
/// deleted after. On next launch, presence of the cookie triggers the
/// crash-recovery state machine ([`run_cookie_recovery`]) instead of the
/// 6-state detector run. JSON-encoded; serde handles `PathBuf` cross-
/// platform (NTFS backslashes preserve verbatim).
///
/// Per HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2" §2 calibration B.
#[derive(Debug, Serialize, Deserialize)]
struct MigrationCookie {
    /// Path to the in-progress sealed temp dir written by the new
    /// `LanceVectorStore::open_with_at_rest_key` call.
    temp_dir: PathBuf,
    /// Path to where the old V0.1 vector_dir was renamed for backup.
    backup_dir: PathBuf,
}

/// Build the per-vault cookie sibling path.
///
/// **Plan amendment vs iteration 2 §2 literal text:** iteration 2 §2 names
/// the cookie at `vector_dir.parent().join(".vault_migration_in_progress")`
/// — a single file in the parent dir. This works for production (one
/// vault per `%APPDATA%/com.memoryvault.dev/`) but collides under
/// parallel test runs (every `tempfile::tempdir()` shares `/tmp/` as
/// parent). The amendment is to use `vector_dir.with_extension(...)` so
/// the cookie is a per-vault sibling — symmetric with the temp_dir +
/// backup_dir naming convention from iteration 1 §1 step 1. Production
/// semantics unchanged (one cookie per vault, in parent dir, JSON-
/// encoded paths); test isolation gained.
fn cookie_path_for(vector_dir: &Path) -> VaultResult<PathBuf> {
    if vector_dir.file_name().is_none() {
        return Err(VaultError::Storage(format!(
            "migration: vector_dir has no file_name component: {}",
            vector_dir.display()
        )));
    }
    Ok(vector_dir.with_extension("vault_migration_in_progress"))
}

fn temp_dir_for(vector_dir: &Path) -> PathBuf {
    vector_dir.with_extension("v0_1_migration_in_progress")
}

fn backup_dir_for(vector_dir: &Path) -> PathBuf {
    vector_dir.with_extension("v0_1_backup")
}

/// One-shot V0.1 plaintext → V0.2 sealed-at-rest migration.
///
/// **Caller MUST pass the already-derived at-rest key** (`K3(master_key)`
/// per ADR-008 amendment K3 KDF). Canonical production derivation site:
/// [`vault_app::keychain::derive_at_rest_key`] per ADR-040 amendment.
/// The 32-byte key is forwarded verbatim into
/// [`LanceVectorStore::open_with_at_rest_key`] for the sealed write.
///
/// Detect → branch on outcome:
/// - [`MigrationDetectorOutcome::V0_1ShapeMigrate`] → run the migration
///   loop (steps 1-10 per HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1"
///   §1 + iteration 2 §2's cookie-file calibration), return
///   `Migrated { rows_migrated }`.
/// - [`MigrationDetectorOutcome::PostSwapMarkerCleanup`] → delete the
///   ALPHA marker file (Phase 2 step-(b)-succeeded crash recovery),
///   return `NoMigrationNeeded`.
/// - [`MigrationDetectorOutcome::V0_2CleanNoOp`] /
///   [`MigrationDetectorOutcome::FirstRunInstallNoOp`] → return
///   `NoMigrationNeeded`, no side effects.
/// - [`MigrationDetectorOutcome::HalfStateCorruptionFailClosed`] /
///   [`MigrationDetectorOutcome::ThirdPartyDataFailClosed`] → return
///   `Err(VaultError::Storage)` carrying a diagnostic message naming the
///   failure mode (test assertions pin substring). Caller surfaces a
///   fatal-startup dialog.
///
/// **Cookie-recovery precedence:** if a migration cookie file exists
/// (left over from a previous run that crashed mid-swap), the cookie
/// state machine runs FIRST and the 6-state detector is bypassed. See
/// [`run_cookie_recovery`] for the recovery rules.
///
/// Idempotent on every-launch invocation: `NoMigrationNeeded` on every
/// call after the first sealed write.
#[instrument(
    skip(vector_dir, at_rest_key),
    fields(vector_dir = %vector_dir.display(), dimension)
)]
pub async fn migrate_v0_1_to_sealed_if_needed(
    vector_dir: &Path,
    dimension: usize,
    at_rest_key: &[u8; 32],
) -> VaultResult<MigrationOutcome> {
    let cookie_path = cookie_path_for(vector_dir)?;
    if fs::try_exists(&cookie_path).await? {
        return run_cookie_recovery(vector_dir, dimension, at_rest_key, &cookie_path).await;
    }

    let outcome = detect_v0_1_state(vector_dir).await?;
    match outcome {
        MigrationDetectorOutcome::V0_1ShapeMigrate => {
            run_migration(vector_dir, dimension, at_rest_key, &cookie_path).await
        }
        MigrationDetectorOutcome::PostSwapMarkerCleanup => {
            // Marker present + sealed framing — Phase 2 step-(b)-
            // succeeded crash recovery. Delete marker, no-op.
            let marker = vector_dir.join(ALPHA_WARNING_FILENAME);
            if fs::try_exists(&marker).await? {
                // Marker may be read-only (V0.1 ADR-010 control #4 sets
                // read-only). Clear the bit before remove on Windows.
                clear_readonly_if_needed(&marker).await?;
                fs::remove_file(&marker).await?;
            }
            info!(
                vector_dir = %vector_dir.display(),
                "V0.1 → V0.2 post-swap marker cleanup complete (ALPHA marker deleted)"
            );
            Ok(MigrationOutcome::NoMigrationNeeded)
        }
        MigrationDetectorOutcome::V0_2CleanNoOp | MigrationDetectorOutcome::FirstRunInstallNoOp => {
            Ok(MigrationOutcome::NoMigrationNeeded)
        }
        MigrationDetectorOutcome::HalfStateCorruptionFailClosed => {
            Err(VaultError::Storage(format!(
                "V0.1 → V0.2 migration aborted: half-state corruption detected at {} \
                 (ALPHA marker present + Lance data dir empty/mixed). Aborted-write \
                 mid-creation; cannot safely auto-recover. Manual intervention required \
                 — restore from backup OR delete the ALPHA marker if the data dir is \
                 known-empty. See HANDOFF.md T0.2.0 Phase 2 plan iteration 2.1 §1.",
                vector_dir.display(),
            )))
        }
        MigrationDetectorOutcome::ThirdPartyDataFailClosed => Err(VaultError::Storage(format!(
            "V0.1 → V0.2 migration aborted: third-party data detected at {} \
             (Lance data present without ADR-010 ALPHA marker). Either the marker \
             was removed by an external tool, or the data dir contains non-Memory- \
             Vault writes. Manual intervention required to avoid migrating unknown \
             data. See HANDOFF.md T0.2.0 Phase 2 plan iteration 2.1 §1.",
            vector_dir.display(),
        ))),
    }
}

/// Execute the V0.1 → V0.2 migration loop on a confirmed-V0.1 source dir.
///
/// Steps 1-10 per HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1" §1 with
/// iteration 2 §2 cookie-file + locked-rename-ordering calibration:
///
/// 1. Pre-clean orphan temp/backup dirs from any earlier non-cookie-tracked failures.
/// 2. Open V0.1 source via plaintext `LanceVectorStore::open`.
/// 3. Scan all rows via `scan_all_rows_for_migration` (in-tree pattern locked at iteration 2 §1).
/// 4. Open temp dir as sealed via `open_with_at_rest_key`.
/// 5. Bulk-insert each row into the sealed dest.
/// 6. Drop both store handles to release Lance file locks.
/// 7. Write cookie file (JSON of {temp_dir, backup_dir}) — opens the rename-pair window.
/// 8. Atomic dir swap (locked Windows-correct ordering per iteration 2 §2 calibration C):
///    8a. `rename(vector_dir → backup_dir)` — succeeds because backup_dir didn't exist yet.
///    8b. `rename(temp_dir → vector_dir)` — succeeds because vector_dir was just renamed away.
/// 9. Delete cookie file — closes the rename-pair window.
/// 10. Delete ALPHA marker if it survived into the new sealed dir.
/// 11. Best-effort backup cleanup (WARN-not-Err per iteration 1 §1 step 9).
/// 12. Emit one-time INFO log naming row count.
async fn run_migration(
    vector_dir: &Path,
    dimension: usize,
    at_rest_key: &[u8; 32],
    cookie_path: &Path,
) -> VaultResult<MigrationOutcome> {
    let temp_dir = temp_dir_for(vector_dir);
    let backup_dir = backup_dir_for(vector_dir);

    // Step 1: defensive pre-clean. The cookie-presence check already
    // returned absent (else run_cookie_recovery would have run); any
    // residual temp/backup is an orphan from a non-cookie-tracked
    // failure (e.g., a process crash before step 7 wrote the cookie).
    if fs::try_exists(&temp_dir).await? {
        fs::remove_dir_all(&temp_dir).await?;
    }
    if fs::try_exists(&backup_dir).await? {
        fs::remove_dir_all(&backup_dir).await?;
    }

    // Steps 2-6: read every row + bulk-write through sealed dest in an
    // inner block so both LanceVectorStore handles drop (releasing all
    // Lance file locks) before we attempt any rename.
    let rows_migrated = {
        let source = LanceVectorStore::open(vector_dir, dimension).await?;
        let rows = source.scan_all_rows_for_migration().await?;
        let n = rows.len() as u64;

        let dest =
            LanceVectorStore::open_with_at_rest_key(&temp_dir, dimension, at_rest_key).await?;
        for (id, embedding, boundary) in &rows {
            dest.upsert(id, embedding, boundary).await?;
        }
        n
    };

    // Step 7: write the cookie BEFORE the rename pair.
    write_cookie_file(cookie_path, &temp_dir, &backup_dir).await?;

    // Steps 8a + 8b: locked rename ordering (Windows-correct).
    // Per iteration 2 §2 calibration C — `std::fs::rename` on NTFS
    // rejects rename-to-existing-destination; ordering must be
    // (vector_dir → backup_dir) THEN (temp_dir → vector_dir) so that
    // each destination is empty at rename time.
    fs::rename(vector_dir, &backup_dir).await.map_err(|e| {
        VaultError::Storage(format!(
            "migration step 8a: rename(vector_dir → backup_dir) failed: {e}. \
             vector_dir={}, backup_dir={}. Cookie remains at {} for next-launch \
             recovery.",
            vector_dir.display(),
            backup_dir.display(),
            cookie_path.display(),
        ))
    })?;
    fs::rename(&temp_dir, vector_dir).await.map_err(|e| {
        VaultError::Storage(format!(
            "migration step 8b: rename(temp_dir → vector_dir) failed AFTER step 8a \
             succeeded: {e}. temp_dir={}, vector_dir={}, backup_dir={}. V0.1 data \
             is preserved at backup_dir; cookie at {} will trigger recovery on next \
             launch.",
            temp_dir.display(),
            vector_dir.display(),
            backup_dir.display(),
            cookie_path.display(),
        ))
    })?;

    // Step 9: cookie deleted — atomic-swap window closed.
    fs::remove_file(cookie_path).await.map_err(|e| {
        VaultError::Storage(format!(
            "migration step 9: cookie file delete failed at {}: {e}. \
             Migration logically succeeded; next launch will surface this as a \
             cookie-recovery state.",
            cookie_path.display()
        ))
    })?;

    // Step 10: delete ALPHA marker if it survived. The sealed open()
    // does NOT write the marker (see vector_store::open_with_at_rest_key
    // doc: "Does NOT write the V0.1 ALPHA warning file"), so this
    // should be a no-op in practice — defense-in-depth only.
    let marker = vector_dir.join(ALPHA_WARNING_FILENAME);
    if fs::try_exists(&marker).await? {
        clear_readonly_if_needed(&marker).await?;
        fs::remove_file(&marker).await?;
    }

    // Step 11: best-effort backup cleanup. Failure is WARN-not-Err per
    // iteration 1 §1 step 9 — the migration logically succeeded; the
    // backup dir is auxiliary.
    if let Err(e) = fs::remove_dir_all(&backup_dir).await {
        warn!(
            error = %e,
            backup_dir = %backup_dir.display(),
            "post-migration backup cleanup failed (best-effort, migration logically complete)"
        );
    }

    // Step 12: one-time INFO log per iteration 1 §1 step 10 (verbatim).
    info!(
        rows_migrated,
        "V0.1 → V0.2 migration complete: {rows_migrated} rows migrated, V0.1 plaintext data deleted"
    );

    Ok(MigrationOutcome::Migrated { rows_migrated })
}

/// Crash-recovery state machine triggered by cookie-presence on launch.
///
/// State table verbatim from HANDOFF.md "T0.2.0 Phase 2 — plan iteration
/// 2" §2 calibration B:
///
/// | Visible state                                                 | Recovery action |
/// |---|---|
/// | temp_dir exists with sealed framing + vector_dir does not exist | Resume from step 8b — `rename(temp_dir, vector_dir)`; delete cookie; return `Migrated`. |
/// | backup_dir exists + vector_dir does not exist + temp_dir gone   | Restore from backup — `rename(backup_dir, vector_dir)`; delete cookie; restart migration normally. |
/// | vector_dir exists with V0.1-shape data                          | Step 8a didn't happen yet — delete cookie + any orphaned temp/backup; restart migration normally. |
/// | Any other state                                                 | Surface as cookie-recovery-fail-closed; require manual intervention. |
async fn run_cookie_recovery(
    vector_dir: &Path,
    dimension: usize,
    at_rest_key: &[u8; 32],
    cookie_path: &Path,
) -> VaultResult<MigrationOutcome> {
    let cookie = read_cookie_file(cookie_path).await?;

    let vector_exists = fs::try_exists(vector_dir).await?;
    let temp_exists = fs::try_exists(&cookie.temp_dir).await?;
    let backup_exists = fs::try_exists(&cookie.backup_dir).await?;

    // State 1: resume from step 8b.
    if temp_exists && !vector_exists {
        // Sanity-check: the temp_dir must be sealed-shape before we
        // promote it. Refuse to rename an unverified dir into vector_dir.
        let temp_state = classify_content(&cookie.temp_dir).await?;
        if !matches!(temp_state, ContentState::SealedV0_2) {
            return Err(VaultError::Storage(format!(
                "cookie recovery: temp_dir at {} is not sealed-shape — refusing \
                 to promote unverified content into vector_dir. Manual intervention \
                 required.",
                cookie.temp_dir.display()
            )));
        }
        fs::rename(&cookie.temp_dir, vector_dir)
            .await
            .map_err(|e| {
                VaultError::Storage(format!(
                    "cookie recovery (state 1): rename(temp_dir → vector_dir) failed: {e}"
                ))
            })?;
        // Cookie + leftover backup cleanup; both best-effort.
        let _ = fs::remove_file(cookie_path).await;
        if backup_exists {
            let _ = fs::remove_dir_all(&cookie.backup_dir).await;
        }

        // Re-derive row count from the now-promoted vector_dir.
        let store =
            LanceVectorStore::open_with_at_rest_key(vector_dir, dimension, at_rest_key).await?;
        let rows_migrated = store.count(None).await? as u64;
        info!(
            rows_migrated,
            vector_dir = %vector_dir.display(),
            "V0.1 → V0.2 migration resumed (cookie state 1: step 8b recovery)"
        );
        return Ok(MigrationOutcome::Migrated { rows_migrated });
    }

    // State 2: restore from backup, restart migration normally.
    if backup_exists && !vector_exists && !temp_exists {
        fs::rename(&cookie.backup_dir, vector_dir)
            .await
            .map_err(|e| {
                VaultError::Storage(format!(
                    "cookie recovery (state 2): rename(backup_dir → vector_dir) failed: {e}"
                ))
            })?;
        let _ = fs::remove_file(cookie_path).await;
        info!(
            vector_dir = %vector_dir.display(),
            "V0.1 → V0.2 migration restoring from backup (cookie state 2); restarting migration"
        );
        // Recurse — Box::pin is required for async recursion (returned
        // future would otherwise be infinitely-sized).
        return Box::pin(migrate_v0_1_to_sealed_if_needed(
            vector_dir,
            dimension,
            at_rest_key,
        ))
        .await;
    }

    // State 3: vector_dir present with V0.1-shape data — step 8a never
    // ran; cookie is stale. Clean up and restart.
    if vector_exists {
        let outcome = detect_v0_1_state(vector_dir).await?;
        if matches!(outcome, MigrationDetectorOutcome::V0_1ShapeMigrate) {
            let _ = fs::remove_file(cookie_path).await;
            if temp_exists {
                let _ = fs::remove_dir_all(&cookie.temp_dir).await;
            }
            if backup_exists {
                let _ = fs::remove_dir_all(&cookie.backup_dir).await;
            }
            info!(
                vector_dir = %vector_dir.display(),
                "V0.1 → V0.2 migration cookie was stale (cookie state 3: step 8a never ran); restarting"
            );
            return Box::pin(migrate_v0_1_to_sealed_if_needed(
                vector_dir,
                dimension,
                at_rest_key,
            ))
            .await;
        }
    }

    // State 4: any other → fail closed. Tier 3 founder snapshot is the
    // safety net per iteration 1 §4.
    Err(VaultError::Storage(format!(
        "cookie recovery fail-closed: unrecognised crash state. Cookie at {}; \
         vector_dir.exists={vector_exists}, temp_dir.exists={temp_exists} ({}), \
         backup_dir.exists={backup_exists} ({}). Manual intervention required — \
         see HANDOFF.md T0.2.0 Phase 2 plan iteration 2 §2 calibration B.",
        cookie_path.display(),
        cookie.temp_dir.display(),
        cookie.backup_dir.display(),
    )))
}

async fn write_cookie_file(
    cookie_path: &Path,
    temp_dir: &Path,
    backup_dir: &Path,
) -> VaultResult<()> {
    let cookie = MigrationCookie {
        temp_dir: temp_dir.to_path_buf(),
        backup_dir: backup_dir.to_path_buf(),
    };
    let json = serde_json::to_vec(&cookie)
        .map_err(|e| VaultError::Storage(format!("migration: cookie serialize: {e}")))?;
    fs::write(cookie_path, json)
        .await
        .map_err(|e| VaultError::Storage(format!("migration: cookie write: {e}")))?;
    Ok(())
}

async fn read_cookie_file(cookie_path: &Path) -> VaultResult<MigrationCookie> {
    let bytes = fs::read(cookie_path)
        .await
        .map_err(|e| VaultError::Storage(format!("migration: cookie read: {e}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| VaultError::Storage(format!("migration: cookie deserialize: {e}")))
}

/// V0.1's `LanceVectorStore::open` writes the ALPHA marker file with the
/// read-only attribute set (cross-platform `Permissions::set_readonly(true)`,
/// per ADR-010 compensating control #4). On Windows, attempting to delete
/// a read-only file fails with ACCESS_DENIED unless we clear the bit
/// first; on Unix the same call clears write bits and `unlink` works
/// regardless. Calling `set_readonly(false)` first is harmless for both
/// platforms.
async fn clear_readonly_if_needed(path: &Path) -> VaultResult<()> {
    let mut perms = fs::metadata(path).await?.permissions();
    if perms.readonly() {
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(path, perms).await?;
    }
    Ok(())
}
