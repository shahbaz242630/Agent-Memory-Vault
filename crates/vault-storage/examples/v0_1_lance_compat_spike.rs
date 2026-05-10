//! Spike: lance 4.0 backward-compat read of V0.1 (lance 0.15) fragments.
//!
//! T0.2.0 Phase 2 iteration 3 — runtime confirmation per
//! `feedback_runtime_confirmation_after_web_spike.md`.
//!
//! ## Question
//!
//! Does lance 4.0 (lancedb = 0.27.2 in workspace since Phase 0a) successfully
//! read V0.1's lance 0.15 fragments? Phase 2 migration step 3 reads V0.1
//! source via `LanceVectorStore::open(vector_dir, dimension)` then iterates
//! rows. If lance 4.0 cannot read V0.1's fragments (file-format generation
//! drift between lance 0.15 and lance 4.0), the migration plan is blocked
//! and we pick from iteration 2.1 §3 alternatives.
//!
//! ## Pass criteria
//!
//! - `LanceVectorStore::open(fixture_lance_dir, 384)` succeeds.
//! - `validate_readable()` succeeds (per ADR-018, exercises full data-decode
//!   of the row with smallest UUID — NOT metadata-only — so this catches
//!   "metadata opens but row decode fails" silent-corruption modes).
//! - `count(None)` = 5 (the fixture has exactly 5 rows per its README).
//! - `count(Some("default"))` = 5 (all 5 rows are boundary `default`).
//! - `search()` returns at least 1 hit (decoded id surfaces as evidence).
//!
//! ## Fail outcomes (iteration 2.1 §3)
//!
//! If any check fails, Phase 2 must pick one of:
//!   (a) Bundle a lance 0.15 reader as a dev-dep just for migration (heavy).
//!   (b) Build a one-shot migration tool from the V0.1 binary that exports
//!       to a lance-version-neutral format (Parquet, JSON); V0.2 reads that.
//!   (c) Document migration-not-supported for V0.1 founder data; founder
//!       accepts data loss; new alpha-cohort installs are first-run-only.
//!
//! ## Run
//!
//! ```text
//! cargo run --example v0_1_lance_compat_spike -p vault-storage --release
//! ```
//!
//! Exit code: 0 = PASS, non-zero = FAIL (different code per failing step).
//!
//! ## Side effects
//!
//! Reads from `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/`
//! (the checked-in Tier 2 fixture). To avoid mutating the fixture
//! (`LanceVectorStore::open` refreshes the ALPHA marker file), the spike
//! deep-copies the `lance/` subdir into a tempdir and operates on the copy.
//! No state outside the tempdir is touched.

use std::path::{Path, PathBuf};

use vault_core::Boundary;
use vault_storage::{LanceVectorStore, VectorStore};

const EXPECTED_DIMENSION: usize = 384;
const EXPECTED_ROW_COUNT: usize = 5;

#[tokio::main]
async fn main() {
    let fixture_lance = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("v0_1_alpha_data_dir")
        .join("lance");

    eprintln!("== T0.2.0 Phase 2 iteration 3 — lance 4.0 reads V0.1 (0.15) ==");
    eprintln!("Source fixture: {}", fixture_lance.display());
    if !fixture_lance.exists() {
        eprintln!("FAIL — fixture path does not exist; spike cannot run.");
        eprintln!("       Re-capture per crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md");
        std::process::exit(10);
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let dst = tmp.path().join("lance");
    copy_dir_recursive(&fixture_lance, &dst).expect("copy fixture");
    eprintln!("Working copy:   {}", dst.display());
    eprintln!();

    // Step 1 — open via the production V0.1 plaintext path. This is the
    // exact call Phase 2 step 3 will make to read the V0.1 source.
    eprintln!("[1/5] LanceVectorStore::open(dim={EXPECTED_DIMENSION})");
    let store = match LanceVectorStore::open(&dst, EXPECTED_DIMENSION).await {
        Ok(s) => {
            eprintln!("       OK — store opened");
            s
        }
        Err(e) => {
            eprintln!("       FAIL — open: {e}");
            print_alternatives();
            std::process::exit(1);
        }
    };

    // Step 2 — validate_readable. ADR-018 contract: exercises full data-
    // decode of the row with smallest UUID, ORDER BY id ASC LIMIT 1. NOT
    // metadata-only. The 2026-04-30 LanceDB-corruption spike showed that
    // metadata + row-count can both succeed on a store whose fragment
    // data is corrupted to unreadability; this check is the load-bearing
    // signal that lance 4.0 actually decodes V0.1's fragment bytes.
    eprintln!("[2/5] validate_readable() — full data-decode path (ADR-018)");
    if let Err(e) = store.validate_readable().await {
        eprintln!("       FAIL — validate_readable: {e}");
        eprintln!("       (lance 4.0 may open table metadata but mis-decode rows)");
        print_alternatives();
        std::process::exit(2);
    }
    eprintln!("       OK — full decode succeeded");

    // Step 3 — count(None) = 5
    eprintln!("[3/5] count(None) — expect {EXPECTED_ROW_COUNT}");
    let total = match store.count(None).await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("       FAIL — count(None): {e}");
            print_alternatives();
            std::process::exit(3);
        }
    };
    eprintln!("       count = {total}");
    if total != EXPECTED_ROW_COUNT {
        eprintln!("       FAIL — expected {EXPECTED_ROW_COUNT}, got {total}");
        print_alternatives();
        std::process::exit(4);
    }

    // Step 4 — count(Some(default)) = 5. Per the fixture README, all 5
    // captured rows are boundary `default`.
    eprintln!("[4/5] count(Some(\"default\")) — expect {EXPECTED_ROW_COUNT}");
    let default_boundary = Boundary::default_name();
    let scoped = match store.count(Some(&default_boundary)).await {
        Ok(n) => n,
        Err(e) => {
            eprintln!("       FAIL — count(Some(default)): {e}");
            print_alternatives();
            std::process::exit(5);
        }
    };
    eprintln!("       count(default) = {scoped}");
    if scoped != EXPECTED_ROW_COUNT {
        eprintln!("       FAIL — expected {EXPECTED_ROW_COUNT}, got {scoped}");
        print_alternatives();
        std::process::exit(6);
    }

    // Step 5 — search returns a hit (first row's id surfaces as
    // evidence-of-decode for the human-readable PASS line). Probe is
    // L2-normalised (1/sqrt(d) per component), so cosine distance is
    // well-defined against any non-zero stored embedding.
    eprintln!("[5/5] search(top=1) — dump first hit's id as decode evidence");
    let probe = vec![1.0_f32 / (EXPECTED_DIMENSION as f32).sqrt(); EXPECTED_DIMENSION];
    let authorized = [default_boundary];
    let hits = match store.search(&probe, 1, &authorized).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("       FAIL — search: {e}");
            print_alternatives();
            std::process::exit(7);
        }
    };
    match hits.first() {
        Some((id, score)) => {
            eprintln!("       hit: id={id} cosine_distance={score:.4}");
        }
        None => {
            eprintln!("       FAIL — search returned 0 rows even though count={total}");
            print_alternatives();
            std::process::exit(8);
        }
    }

    eprintln!();
    eprintln!("PASS — lance 4.0 reads V0.1 (lance 0.15) fragments end-to-end.");
    eprintln!("       Phase 2 implementation proceeds per iteration 2 §5.");
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn print_alternatives() {
    eprintln!();
    eprintln!("Iteration 2.1 §3 alternative migration strategies for iteration 3:");
    eprintln!("  (a) Bundle a lance 0.15 reader as a dev-dep just for migration (heavy).");
    eprintln!("  (b) Build a one-shot migration tool from the V0.1 binary that exports");
    eprintln!("      to a lance-version-neutral format (Parquet, JSON); V0.2 reads that.");
    eprintln!("  (c) Document migration-not-supported for V0.1 founder data; founder");
    eprintln!("      accepts data loss; new alpha-cohort installs are first-run-only.");
}
