//! Phase 2 V0.1 → V0.2 plaintext-to-sealed migration tests.
//!
//! T0.2.0 close-out plan, HANDOFF.md "T0.2.0 Phase 2 — plan iterations 1
//! / 2 / 2.1 / 3". Tier 1 scaffolding (this file): detection-rule logic
//! against synthetic fixtures. Tier 2 (sibling file at integration time):
//! fixture-replay tests against the captured V0.1 fixture in
//! `tests/fixtures/v0_1_alpha_data_dir/` (per iteration 3 PASS evidence —
//! lance 4.0 reads V0.1 (lance 0.15) fragments end-to-end).
//!
//! ## Test list (post-migration-loop-scaffolding amendment 2026-05-10 — 13 vault-storage tests)
//!
//! Two layers, two amendments. Both surfaced + approved per
//! `feedback_floor_forecast_is_pre_declaration_not_estimate.md`.
//!
//! ### Detector layer (7 tests)
//!
//! Iteration 2 §5 forecast: 4 actionable detector tests + 1 Tier-2 fixture-
//! replay = 5. Amended +2 to pin all 6 named outcomes from iteration 2.1
//! §1 (defense-in-depth before migration loop module consumes the detector).
//!
//! - `detect_v0_1_shape_migrate`               — marker present + LANC magic
//! - `detect_post_swap_marker_cleanup`         — marker present + sealed framing
//! - `detect_half_state_corruption_fail_closed`  — marker present + empty
//! - `detect_third_party_data_fail_closed`     — marker absent + LANC magic
//! - `detect_v0_2_clean_no_op`                 — marker absent + sealed framing  [+1 amendment]
//! - `detect_first_run_install_no_op`          — marker absent + empty           [+1 amendment]
//! - `tier_2_real_v0_1_fixture_returns_v0_1_shape_migrate` — Tier 2 realism gate
//!
//! ### Migration loop layer (6 tests, scaffolding stub-driven)
//!
//! Iteration 1 §5 forecast: 4 migration-loop tests (covering the 4
//! actionable detector outcomes from iteration 1's pre-iteration-2.1
//! 4-state framing). Amended +2 to cover the 2 no-op detector outcomes
//! that iteration 2.1 §1 added to the 6-state rule
//! (`migration_no_op_on_v0_2_clean`, `migration_no_op_on_post_swap_marker_cleanup`)
//! — same defense-in-depth rationale as the detector-layer +2 amendment.
//!
//! - `migration_succeeds_on_v0_1_shape`                  — V0_1ShapeMigrate path
//! - `migration_no_op_on_v0_2_clean`                     — V0_2CleanNoOp                   [+1 amendment]
//! - `migration_no_op_on_first_run_install`              — FirstRunInstallNoOp
//! - `migration_no_op_on_post_swap_marker_cleanup`       — PostSwapMarkerCleanup           [+1 amendment]
//! - `migration_fails_closed_on_half_state_corruption`   — HalfStateCorruptionFailClosed
//! - `migration_fails_closed_on_third_party_data`        — ThirdPartyDataFailClosed
//!
//! Cookie-recovery tests (3 per iteration 2 §4) and vault-tauri dialog-
//! format test (1 per iteration 1 §5) stay deferred to subsequent passes.
//!
//! ## Tier 1a strategy (per iteration 2.1 §2)
//!
//! Synthetic V0.1-shape data is produced via `LanceVectorStore::open` +
//! one `upsert` — lance 4.0 writes a real `.lance` fragment with `LANC`
//! magic at file END. This is byte-compatible with V0.1 (lance 0.15)
//! fragments per the iteration 3 spike's PASS evidence (lance 4.0 reads
//! V0.1 fragments end-to-end).
//!
//! Sealed-shape data is produced via raw byte writes
//! (`[0x01, 0x00, ...]`) under `<table>.lance/data/`. The detector only
//! checks the first two bytes for the sealed-framing prefix per ADR-008
//! amendment, so a minimal raw fixture suffices for Tier 1.
//!
//! ## Layer status (current)
//!
//! - **Detector layer (7 tests):** implemented, all PASS. Production
//!   `detect_v0_1_state` lives in `crates/vault-storage/src/migration.rs`.
//! - **Migration loop layer (6 tests, `#[ignore]`'d):** scaffolding,
//!   stub-driven. Marked `#[ignore = "scaffolding stub: impl lands in
//!   Phase 2 step-(b)"]` per standard Rust convention + the codebase's
//!   pre-existing 17-ignored-tests pattern, so the workspace test gate
//!   stays green during scaffolding. All six tests fail when run via
//!   `cargo test ... -- --ignored` because the production
//!   `migrate_v0_1_to_sealed_if_needed` stub returns Err uniformly; the
//!   two fail-closed tests assert error-message substrings so the stub
//!   error doesn't trivially satisfy `is_err()`.
//!
//!   **Contract-class commitment:** the migration loop implementation
//!   milestone (next session, per HANDOFF.md "T0.2.0 Phase 2" plan
//!   iteration 1 §1) MUST remove all 6 `#[ignore]` annotations as part
//!   of the impl deliverable. After removal the tests run live + must
//!   PASS; ignore-removal is the impl-trigger contract.

use std::fs;
use std::path::Path;

use vault_core::{Boundary, MemoryId};
use vault_storage::{
    detect_v0_1_state, migrate_v0_1_to_sealed_if_needed, LanceVectorStore,
    MigrationDetectorOutcome, MigrationOutcome, VectorStore, ALPHA_WARNING_FILENAME,
};

const EMBEDDING_DIM: usize = 384;
/// Stand-in K3 at-rest key for migration-loop scaffolding tests. Real key
/// derivation (BLAKE3-from-master_key) lands at the production wiring
/// site (vault-tauri main.rs step 5b); the scaffolding only needs a
/// 32-byte buffer to satisfy the API.
const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

// ─── State 1: marker present + LANC-magic data ────────────────────────────

#[tokio::test]
async fn detect_v0_1_shape_migrate() {
    let tmp = tempfile::tempdir().unwrap();
    create_v0_1_shape_data(tmp.path()).await;
    assert_alpha_marker_present(tmp.path());

    let outcome = detect_v0_1_state(tmp.path()).await.unwrap();
    assert_eq!(outcome, MigrationDetectorOutcome::V0_1ShapeMigrate);
}

// ─── State 2: marker present + sealed framing ─────────────────────────────

#[tokio::test]
async fn detect_post_swap_marker_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    write_alpha_marker(tmp.path());
    write_sealed_shape_data(tmp.path());

    let outcome = detect_v0_1_state(tmp.path()).await.unwrap();
    assert_eq!(outcome, MigrationDetectorOutcome::PostSwapMarkerCleanup);
}

// ─── State 3: marker present + empty/mixed ────────────────────────────────

#[tokio::test]
async fn detect_half_state_corruption_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    write_alpha_marker(tmp.path());
    // No table dir; just the marker. Aborted write mid-creation.

    let outcome = detect_v0_1_state(tmp.path()).await.unwrap();
    assert_eq!(
        outcome,
        MigrationDetectorOutcome::HalfStateCorruptionFailClosed
    );
}

// ─── State 4: marker absent + LANC-magic data ─────────────────────────────

#[tokio::test]
async fn detect_third_party_data_fail_closed() {
    let tmp = tempfile::tempdir().unwrap();
    create_v0_1_shape_data(tmp.path()).await;
    delete_alpha_marker(tmp.path());

    let outcome = detect_v0_1_state(tmp.path()).await.unwrap();
    assert_eq!(outcome, MigrationDetectorOutcome::ThirdPartyDataFailClosed);
}

// ─── State 5: marker absent + sealed framing ──────────────────────────────

#[tokio::test]
async fn detect_v0_2_clean_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    write_sealed_shape_data(tmp.path());
    // No marker — clean post-migration V0.2 state.

    let outcome = detect_v0_1_state(tmp.path()).await.unwrap();
    assert_eq!(outcome, MigrationDetectorOutcome::V0_2CleanNoOp);
}

// ─── State 6: marker absent + empty ───────────────────────────────────────

#[tokio::test]
async fn detect_first_run_install_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    // Empty dir — first-run V0.2 install, no migration needed.

    let outcome = detect_v0_1_state(tmp.path()).await.unwrap();
    assert_eq!(outcome, MigrationDetectorOutcome::FirstRunInstallNoOp);
}

// ─── Tier 2: real V0.1 fixture replay ─────────────────────────────────────
//
// Tier 2 closes the synthetic-vs-real realism gate that iteration 2's
// three-tier strategy was designed to provide:
//
//   Tier 1 — synthetic fixtures (above): regression every CI run.
//   Tier 2 — captured V0.1-binary-emitted fixture (this test): realism.
//   Tier 3 — founder one-shot smoke on actual vault: distribution check.
//
// Each tier catches a distinct failure class. If THIS test fails, it
// means synthetic Tier 1 shape has diverged from what the V0.1 binary
// actually emits to disk — a hard signal to STOP and diagnose, not
// fix-forward into detector code (per the user direction at Tier 2 plan
// kickoff: "the failure IS the signal that synthetic-shape assumptions
// need correction").

#[tokio::test]
async fn tier_2_real_v0_1_fixture_returns_v0_1_shape_migrate() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("v0_1_alpha_data_dir")
        .join("lance");

    assert!(
        fixture.exists(),
        "Tier 2 fixture not found at {}: did the working tree get the \
         `tests/fixtures/v0_1_alpha_data_dir/` checked out? See \
         `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md` \
         for capture provenance + re-capture procedure.",
        fixture.display(),
    );

    // Detector is read-only by contract (file presence + magic-byte peeks;
    // never opens via LanceVectorStore, which WOULD write the ALPHA marker
    // and rewrite the fixture as a side effect). Pointing at the checked-in
    // fixture directly is safe; no tempdir copy needed.
    let outcome = detect_v0_1_state(&fixture).await.unwrap();
    assert_eq!(
        outcome,
        MigrationDetectorOutcome::V0_1ShapeMigrate,
        "Tier 2 fixture-replay failed: detector returned {outcome:?} but \
         the captured V0.1 fixture (commit 1d72aac MSI capture, 5 rows) \
         should classify as V0_1ShapeMigrate. This is the realism gap \
         that iteration 2's three-tier strategy is designed to catch — \
         Tier 1 synthetic fixtures may have diverged from V0.1-binary- \
         emitted reality. STOP and diagnose before any detector or \
         synthetic-fixture change.",
    );
}

// ─── Migration loop: scaffolding (stub-driven failures) ──────────────────

#[tokio::test]
#[ignore = "scaffolding stub: impl lands in Phase 2 step-(b)"]
async fn migration_succeeds_on_v0_1_shape() {
    let tmp = tempfile::tempdir().unwrap();
    create_v0_1_shape_data(tmp.path()).await;

    let outcome = migrate_v0_1_to_sealed_if_needed(tmp.path(), EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    match outcome {
        MigrationOutcome::Migrated { rows_migrated } => {
            assert_eq!(
                rows_migrated, 1,
                "create_v0_1_shape_data inserts exactly 1 row"
            );
        }
        other => panic!("expected Migrated, got {other:?}"),
    }
    // Post-migration disk-state assertions (reached only after stub flips
    // to real implementation):
    assert!(
        !tmp.path().join(ALPHA_WARNING_FILENAME).exists(),
        "ALPHA marker must be deleted after a successful migration",
    );
}

#[tokio::test]
#[ignore = "scaffolding stub: impl lands in Phase 2 step-(b)"]
async fn migration_no_op_on_v0_2_clean() {
    let tmp = tempfile::tempdir().unwrap();
    write_sealed_shape_data(tmp.path());
    // No marker — clean post-migration V0.2 state.

    let outcome = migrate_v0_1_to_sealed_if_needed(tmp.path(), EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    assert_eq!(outcome, MigrationOutcome::NoMigrationNeeded);
}

#[tokio::test]
#[ignore = "scaffolding stub: impl lands in Phase 2 step-(b)"]
async fn migration_no_op_on_first_run_install() {
    let tmp = tempfile::tempdir().unwrap();
    // Empty dir — first-run V0.2 install.

    let outcome = migrate_v0_1_to_sealed_if_needed(tmp.path(), EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    assert_eq!(outcome, MigrationOutcome::NoMigrationNeeded);
}

#[tokio::test]
#[ignore = "scaffolding stub: impl lands in Phase 2 step-(b)"]
async fn migration_no_op_on_post_swap_marker_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    write_alpha_marker(tmp.path());
    write_sealed_shape_data(tmp.path());
    // Marker + sealed framing → step-(b)-succeeded crash recovery.

    let outcome = migrate_v0_1_to_sealed_if_needed(tmp.path(), EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    assert_eq!(outcome, MigrationOutcome::NoMigrationNeeded);
    assert!(
        !tmp.path().join(ALPHA_WARNING_FILENAME).exists(),
        "ALPHA marker must be deleted by post-swap-marker-cleanup recovery",
    );
}

#[tokio::test]
#[ignore = "scaffolding stub: impl lands in Phase 2 step-(b)"]
async fn migration_fails_closed_on_half_state_corruption() {
    let tmp = tempfile::tempdir().unwrap();
    write_alpha_marker(tmp.path());
    // Marker present + empty data — aborted-write-mid-creation.

    let err = migrate_v0_1_to_sealed_if_needed(tmp.path(), EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .expect_err("half-state corruption must fail closed");

    // Tightened beyond a bare `is_err()` so the stub's "not yet implemented"
    // error doesn't trivially satisfy the assertion at scaffolding time.
    // The implementation must surface the corruption type by name (Phase 2
    // dialog will quote the message verbatim per `feedback_quote_locked_artefacts_dont_paraphrase.md`).
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("half-state") || msg.contains("half state"),
        "error must name half-state corruption (got {err})",
    );
}

#[tokio::test]
#[ignore = "scaffolding stub: impl lands in Phase 2 step-(b)"]
async fn migration_fails_closed_on_third_party_data() {
    let tmp = tempfile::tempdir().unwrap();
    create_v0_1_shape_data(tmp.path()).await;
    delete_alpha_marker(tmp.path());
    // V0.1-shape data without ADR-010 marker — third-party or corrupted.

    let err = migrate_v0_1_to_sealed_if_needed(tmp.path(), EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .expect_err("third-party data must fail closed");

    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("third-party") || msg.contains("third party"),
        "error must name third-party data (got {err})",
    );
}

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Tier 1a strategy (iteration 2.1 §2): produce a real V0.1-shape Lance
/// fragment by opening `LanceVectorStore` and upserting one row. This:
/// 1. Writes the ADR-010 ALPHA marker file (via `LanceVectorStore::open`'s
///    side effect — control #4 in the V0.1 plaintext compensating set).
/// 2. Creates `<dir>/memories.lance/data/<uuid>.lance` containing the
///    upserted row, ending with `LANC` magic (verified by iteration 3 spike).
async fn create_v0_1_shape_data(dir: &Path) {
    let store = LanceVectorStore::open(dir, EMBEDDING_DIM).await.unwrap();
    let id = MemoryId::new();
    let embedding = vec![0.1_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
    let boundary = Boundary::default_name();
    store.upsert(&id, &embedding, &boundary).await.unwrap();
}

/// Write a minimal sealed-shape file: `<dir>/memories.lance/data/0001.lance`
/// starting with the sealed framing prefix `0x01 0x00` per ADR-008
/// amendment. Detector checks the first two bytes; the trailing bytes are
/// arbitrary non-LANC junk so the LANC-at-end check correctly returns false.
fn write_sealed_shape_data(dir: &Path) {
    let table_data = dir.join("memories.lance").join("data");
    fs::create_dir_all(&table_data).unwrap();
    fs::write(
        table_data.join("0001.lance"),
        // 0x01 = version_byte, 0x00 = granularity (per-file).
        // Trailing 0xab/0xcd/0xef are arbitrary non-LANC junk.
        [0x01_u8, 0x00, 0xab, 0xcd, 0xef],
    )
    .unwrap();
}

fn write_alpha_marker(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join(ALPHA_WARNING_FILENAME),
        b"placeholder marker content for Tier 1 scaffolding fixture",
    )
    .unwrap();
}

fn delete_alpha_marker(dir: &Path) {
    fs::remove_file(dir.join(ALPHA_WARNING_FILENAME)).unwrap();
}

fn assert_alpha_marker_present(dir: &Path) {
    assert!(
        dir.join(ALPHA_WARNING_FILENAME).exists(),
        "ADR-010 marker file should have been written by LanceVectorStore::open"
    );
}
