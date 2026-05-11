//! Phase 2 V0.1 → V0.2 plaintext-to-sealed migration tests.
//!
//! T0.2.0 close-out plan, HANDOFF.md "T0.2.0 Phase 2 — plan iterations 1
//! / 2 / 2.1 / 3". Tier 1 scaffolding (this file): detection-rule logic
//! against synthetic fixtures. Tier 2 (sibling file at integration time):
//! fixture-replay tests against the captured V0.1 fixture in
//! `tests/fixtures/v0_1_alpha_data_dir/` (per iteration 3 PASS evidence —
//! lance 4.0 reads V0.1 (lance 0.15) fragments end-to-end).
//!
//! ## Test list (post-migration-loop-impl, 2026-05-11 — 16 vault-storage tests)
//!
//! Three layers, all live. Floor amendments at the scaffolding milestone
//! (2026-05-10) surfaced + approved per
//! `feedback_floor_forecast_is_pre_declaration_not_estimate.md`. The
//! migration loop impl milestone (this commit) drops all 6 `#[ignore]`
//! annotations from the scaffolding tests + adds 3 cookie-recovery
//! tests per iteration 2 §4 — net +3 over the post-scaffolding floor.
//!
//! ### Detector layer (7 tests)
//!
//! - `detect_v0_1_shape_migrate`                            — marker present + LANC magic
//! - `detect_post_swap_marker_cleanup`                      — marker present + sealed framing
//! - `detect_half_state_corruption_fail_closed`             — marker present + empty
//! - `detect_third_party_data_fail_closed`                  — marker absent + LANC magic
//! - `detect_v0_2_clean_no_op`                              — marker absent + sealed framing
//! - `detect_first_run_install_no_op`                       — marker absent + empty
//! - `tier_2_real_v0_1_fixture_returns_v0_1_shape_migrate`  — Tier 2 realism gate
//!
//! ### Migration loop layer (6 tests, all PASS live)
//!
//! - `migration_succeeds_on_v0_1_shape`                  — V0_1ShapeMigrate path
//! - `migration_no_op_on_v0_2_clean`                     — V0_2CleanNoOp
//! - `migration_no_op_on_first_run_install`              — FirstRunInstallNoOp
//! - `migration_no_op_on_post_swap_marker_cleanup`       — PostSwapMarkerCleanup
//! - `migration_fails_closed_on_half_state_corruption`   — HalfStateCorruptionFailClosed
//! - `migration_fails_closed_on_third_party_data`        — ThirdPartyDataFailClosed
//!
//! ### Cookie-recovery layer (3 tests per iteration 2 §4)
//!
//! - `cookie_recovery_resumes_step_b_when_temp_dir_exists_and_vector_dir_missing`  — state 1
//! - `cookie_recovery_restores_from_backup_when_temp_dir_gone`                      — state 2
//! - `cookie_recovery_restarts_when_step_a_did_not_happen`                          — state 3
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
//! ## Test isolation: subdirectory pattern for migration tests
//!
//! Migration tests use a SUBDIRECTORY inside `tempfile::tempdir()`:
//! `vector_dir = tmp.path().join("vault")`. This keeps the cookie file
//! (`vault.vault_migration_in_progress`), temp dir
//! (`vault.v0_1_migration_in_progress`), and backup dir
//! (`vault.v0_1_backup`) — all named via `with_extension(...)` siblings
//! of vector_dir — INSIDE the tempdir, so they're cleaned up
//! automatically when `TempDir` drops. Without this, sibling files
//! would leak to the system temp dir under parallel test runs
//! (`RUST_TEST_THREADS=4` per ADR-038 layer 3 sibling).

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use vault_core::{Boundary, MemoryId};
use vault_storage::{
    detect_v0_1_state, migrate_v0_1_to_sealed_if_needed, LanceVectorStore,
    MigrationDetectorOutcome, MigrationOutcome, VectorStore, ALPHA_WARNING_FILENAME,
};

const EMBEDDING_DIM: usize = 384;
/// Stand-in already-derived at-rest key for migration-loop tests.
///
/// Per the Phase 2 signature-fix amendment (2026-05-11):
/// `LanceVectorStore::open_with_at_rest_key` consumes the already-derived
/// at-rest key directly; the canonical production K3 derivation site is
/// `vault_app::keychain::derive_at_rest_key`. Migration tests pass this
/// 32-byte buffer as the at-rest key without modeling the master_key→K3
/// step (the K3 contract itself is pinned by vault-app's
/// `derive_at_rest_key_is_deterministic_and_uses_k3_kdf_context` test;
/// here we only exercise the seal/unseal round-trip with a fixed key).
const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

/// Migration tests use a SUBDIRECTORY inside the tempdir so that the
/// cookie file + temp_dir + backup_dir siblings (named by
/// `vector_dir.with_extension(...)`) all live INSIDE the tempdir — and
/// get cleaned up automatically when `TempDir` drops. Without this, the
/// migration's sibling files leak into the system temp dir under
/// parallel test runs (`RUST_TEST_THREADS=4` per ADR-038 layer 3).
fn vector_dir_under(tmp: &TempDir) -> PathBuf {
    tmp.path().join("vault")
}

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
async fn migration_succeeds_on_v0_1_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    create_v0_1_shape_data(&vector_dir).await;

    let outcome = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
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
    assert!(
        !vector_dir.join(ALPHA_WARNING_FILENAME).exists(),
        "ALPHA marker must be deleted after a successful migration",
    );
    // Sealed-shape sanity: at least one .lance file in the new vector_dir
    // starts with the sealed framing prefix (not PAR1 magic).
    assert_post_migration_sealed_shape(&vector_dir);
}

#[tokio::test]
async fn migration_no_op_on_v0_2_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    write_sealed_shape_data(&vector_dir);
    // No marker — clean post-migration V0.2 state.

    let outcome = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    assert_eq!(outcome, MigrationOutcome::NoMigrationNeeded);
}

#[tokio::test]
async fn migration_no_op_on_first_run_install() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    // Vector_dir does not exist (first-run V0.2 install).

    let outcome = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    assert_eq!(outcome, MigrationOutcome::NoMigrationNeeded);
}

#[tokio::test]
async fn migration_no_op_on_post_swap_marker_cleanup() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    write_alpha_marker(&vector_dir);
    write_sealed_shape_data(&vector_dir);
    // Marker + sealed framing → step-(b)-succeeded crash recovery.

    let outcome = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    assert_eq!(outcome, MigrationOutcome::NoMigrationNeeded);
    assert!(
        !vector_dir.join(ALPHA_WARNING_FILENAME).exists(),
        "ALPHA marker must be deleted by post-swap-marker-cleanup recovery",
    );
}

#[tokio::test]
async fn migration_fails_closed_on_half_state_corruption() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    write_alpha_marker(&vector_dir);
    // Marker present + empty data — aborted-write-mid-creation.

    let err = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .expect_err("half-state corruption must fail closed");

    // The implementation must surface the corruption type by name
    // (Phase 2 dialog will quote the message verbatim per
    // `feedback_quote_locked_artefacts_dont_paraphrase.md`).
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("half-state") || msg.contains("half state"),
        "error must name half-state corruption (got {err})",
    );
}

#[tokio::test]
async fn migration_fails_closed_on_third_party_data() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    create_v0_1_shape_data(&vector_dir).await;
    delete_alpha_marker(&vector_dir);
    // V0.1-shape data without ADR-010 marker — third-party or corrupted.

    let err = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
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

/// Walk every `.lance` file under `<vector_dir>/<table>.lance/data/` and
/// assert at least one starts with the sealed framing prefix
/// (`0x01, 0x00`) and zero contain `LANC` magic at the file end. This is
/// the disk-shape proof that migration successfully wrote sealed bytes
/// in place of V0.1 plaintext fragments.
fn assert_post_migration_sealed_shape(vector_dir: &Path) {
    let mut found_sealed = false;
    for entry in fs::read_dir(vector_dir).unwrap().flatten() {
        let table_dir = entry.path();
        if table_dir.extension().and_then(|s| s.to_str()) != Some("lance") {
            continue;
        }
        let data_dir = table_dir.join("data");
        if !data_dir.exists() {
            continue;
        }
        for f in fs::read_dir(&data_dir).unwrap().flatten() {
            let p = f.path();
            if p.extension().and_then(|s| s.to_str()) != Some("lance") {
                continue;
            }
            let bytes = fs::read(&p).unwrap();
            assert!(
                bytes.len() < 4 || &bytes[bytes.len() - 4..] != b"LANC",
                "post-migration file {} still ends with LANC magic — \
                 plaintext V0.1 fragment survived the migration",
                p.display()
            );
            if bytes.len() >= 2 && bytes[0] == 0x01 && bytes[1] == 0x00 {
                found_sealed = true;
            }
        }
    }
    assert!(
        found_sealed,
        "no sealed-shape .lance files found under {} after migration",
        vector_dir.display()
    );
}

// ─── Cookie-recovery state machine (iteration 2 §4) ───────────────────────
//
// Three tests exercising the named visible-states from "T0.2.0 Phase 2 —
// plan iteration 2" §2 calibration B (the cookie-recovery state-machine
// table). Each test simulates a crash mid-migration by setting up the
// post-crash filesystem state (some combination of vector_dir, temp_dir,
// backup_dir, cookie file presence/absence) and asserts that the next
// invocation of `migrate_v0_1_to_sealed_if_needed` performs the
// documented recovery action.
//
// **Cookie file format note:** the `MigrationCookie` struct is private
// to the `migration` module. Tests construct an equivalent JSON
// representation directly via `serde_json::json!` — if the production
// struct's fields are renamed, these tests fail loudly at the cookie-
// read step (mismatched field name surfaces as "missing field
// `temp_dir`" / "missing field `backup_dir`" in the deserialise error
// chain). That fail-loud is the contract pin.

fn cookie_path_for_test(vector_dir: &Path) -> PathBuf {
    vector_dir.with_extension("vault_migration_in_progress")
}

fn temp_dir_for_test(vector_dir: &Path) -> PathBuf {
    vector_dir.with_extension("v0_1_migration_in_progress")
}

fn backup_dir_for_test(vector_dir: &Path) -> PathBuf {
    vector_dir.with_extension("v0_1_backup")
}

/// Write a synthetic cookie file mirroring `MigrationCookie`'s on-disk
/// JSON shape. The migration module's private struct is intentionally
/// not re-exported; tests reproduce its serialised form here. If the
/// production field names drift from `temp_dir` + `backup_dir`, all
/// three cookie-recovery tests fail at the `serde_json::from_slice`
/// step inside `read_cookie_file`.
fn write_test_cookie(cookie_path: &Path, temp_dir: &Path, backup_dir: &Path) {
    let json = serde_json::json!({
        "temp_dir": temp_dir,
        "backup_dir": backup_dir,
    })
    .to_string();
    fs::write(cookie_path, json).unwrap();
}

/// Build a sealed-shape directory at `dir` by opening it via
/// `open_with_at_rest_key` and inserting `n_rows` synthetic rows. Used
/// by cookie-recovery test 1 to set up "temp_dir exists with sealed
/// framing" state. Drop releases all Lance file locks before the test
/// hands the path off to the migration loop.
async fn seed_sealed_dir(dir: &Path, n_rows: usize) {
    let store = LanceVectorStore::open_with_at_rest_key(dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();
    let boundary = Boundary::default_name();
    let embedding = vec![0.1_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
    for _ in 0..n_rows {
        store
            .upsert(&MemoryId::new(), &embedding, &boundary)
            .await
            .unwrap();
    }
}

/// State 1: temp_dir exists with sealed framing + vector_dir does not exist.
/// Recovery: rename(temp_dir, vector_dir); delete cookie; return Migrated
/// with row count re-derived from the now-promoted vector_dir.
#[tokio::test]
async fn cookie_recovery_resumes_step_b_when_temp_dir_exists_and_vector_dir_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    let temp_dir = temp_dir_for_test(&vector_dir);
    let backup_dir = backup_dir_for_test(&vector_dir);
    let cookie_path = cookie_path_for_test(&vector_dir);

    // Crash simulation: step 8a succeeded (vector_dir → backup_dir),
    // step 8b succeeded (temp_dir → vector_dir would have happened) BUT
    // we crashed BEFORE the rename. So we have temp_dir + backup_dir
    // present, vector_dir missing, cookie still present.
    seed_sealed_dir(&temp_dir, 3).await;
    seed_sealed_dir(&backup_dir, 3).await; // simulating the V0.1 backup (content shape doesn't matter for state 1)
    write_test_cookie(&cookie_path, &temp_dir, &backup_dir);
    assert!(!vector_dir.exists());

    let outcome = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    match outcome {
        MigrationOutcome::Migrated { rows_migrated } => {
            assert_eq!(
                rows_migrated, 3,
                "expected re-derived row count from promoted temp_dir"
            );
        }
        other => panic!("expected Migrated (state 1 resume), got {other:?}"),
    }
    assert!(
        vector_dir.exists(),
        "vector_dir must exist after step 8b resume"
    );
    assert!(
        !cookie_path.exists(),
        "cookie must be deleted after recovery"
    );
    assert!(!temp_dir.exists(), "temp_dir must be gone (renamed away)");
    assert!(!backup_dir.exists(), "leftover backup must be cleaned up");
}

/// State 2: backup_dir exists + vector_dir missing + temp_dir gone.
/// Recovery: rename(backup_dir, vector_dir); delete cookie; restart
/// migration normally.
#[tokio::test]
async fn cookie_recovery_restores_from_backup_when_temp_dir_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    let temp_dir = temp_dir_for_test(&vector_dir);
    let backup_dir = backup_dir_for_test(&vector_dir);
    let cookie_path = cookie_path_for_test(&vector_dir);

    // Crash simulation: step 8a succeeded (V0.1 vector_dir → backup_dir),
    // crash before step 8b. temp_dir was already deleted in a separate
    // best-effort cleanup race, leaving only backup_dir + cookie.
    create_v0_1_shape_data(&backup_dir).await;
    assert_alpha_marker_present(&backup_dir);
    write_test_cookie(&cookie_path, &temp_dir, &backup_dir);
    assert!(!vector_dir.exists());
    assert!(!temp_dir.exists());

    let outcome = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    // State 2 restores backup → vector_dir, then re-runs migration.
    // Net effect: the V0.1 data is migrated end-to-end.
    match outcome {
        MigrationOutcome::Migrated { rows_migrated } => {
            assert_eq!(
                rows_migrated, 1,
                "create_v0_1_shape_data inserts exactly 1 row"
            );
        }
        other => panic!("expected Migrated (state 2 restore-then-migrate), got {other:?}"),
    }
    assert!(
        vector_dir.exists(),
        "vector_dir must exist after restore + migration"
    );
    assert!(
        !cookie_path.exists(),
        "cookie must be deleted after recovery"
    );
    assert!(
        !vector_dir.join(ALPHA_WARNING_FILENAME).exists(),
        "marker must be gone post-migration"
    );
    assert_post_migration_sealed_shape(&vector_dir);
}

/// State 3: vector_dir exists with V0.1-shape data; cookie is stale
/// (step 8a never ran). Recovery: delete cookie + any orphaned
/// temp_dir/backup_dir; restart migration normally.
#[tokio::test]
async fn cookie_recovery_restarts_when_step_a_did_not_happen() {
    let tmp = tempfile::tempdir().unwrap();
    let vector_dir = vector_dir_under(&tmp);
    let temp_dir = temp_dir_for_test(&vector_dir);
    let backup_dir = backup_dir_for_test(&vector_dir);
    let cookie_path = cookie_path_for_test(&vector_dir);

    // Crash simulation: cookie was written but step 8a never executed
    // (process died between cookie write and the first rename). V0.1
    // data still in vector_dir; no temp/backup yet.
    create_v0_1_shape_data(&vector_dir).await;
    write_test_cookie(&cookie_path, &temp_dir, &backup_dir);
    assert!(vector_dir.exists());
    assert!(!temp_dir.exists());
    assert!(!backup_dir.exists());

    let outcome = migrate_v0_1_to_sealed_if_needed(&vector_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
        .await
        .unwrap();

    match outcome {
        MigrationOutcome::Migrated { rows_migrated } => {
            assert_eq!(
                rows_migrated, 1,
                "create_v0_1_shape_data inserts exactly 1 row"
            );
        }
        other => panic!("expected Migrated (state 3 restart), got {other:?}"),
    }
    assert!(vector_dir.exists());
    assert!(!cookie_path.exists(), "stale cookie must be deleted");
    assert!(!vector_dir.join(ALPHA_WARNING_FILENAME).exists());
    assert_post_migration_sealed_shape(&vector_dir);
}
