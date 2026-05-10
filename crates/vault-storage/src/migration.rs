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
use std::path::Path;

use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::{instrument, trace};

use vault_core::error::VaultResult;

use crate::vector_store::ALPHA_WARNING_FILENAME;

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
// Migration loop — scaffolding stub (Tier 1, pre-implementation)
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

/// One-shot V0.1 plaintext → V0.2 sealed-at-rest migration.
///
/// Detect → branch on outcome:
/// - [`MigrationDetectorOutcome::V0_1ShapeMigrate`] → run the migration
///   loop (steps 1-13 per HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1"
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
///   `Err(VaultError::Storage)` carrying a diagnostic message; caller
///   surfaces a fatal-startup dialog.
///
/// Idempotent on every-launch invocation: `NoMigrationNeeded` on every
/// call after the first sealed write.
///
/// ## Tier 1 scaffolding stub (this file, pre-implementation)
///
/// Returns `Err(VaultError::Storage(...))` unconditionally. Integration
/// tests in `tests/migration_v0_1_to_sealed.rs` compile against this
/// signature and fail at the assertion step (per the spec-driven phase
/// rhythm — scaffold failing tests, hold for review, implement next).
pub async fn migrate_v0_1_to_sealed_if_needed(
    _vector_dir: &Path,
    _dimension: usize,
    _at_rest_key: &[u8; 32],
) -> VaultResult<MigrationOutcome> {
    Err(vault_core::error::VaultError::Storage(
        "migration loop not yet implemented (Tier 1 scaffolding stub — \
         see HANDOFF.md T0.2.0 Phase 2 close-out plan iteration 1 §1)"
            .into(),
    ))
}
