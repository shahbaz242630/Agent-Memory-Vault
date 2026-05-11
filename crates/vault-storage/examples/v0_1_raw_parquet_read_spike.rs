//! Spike: empirical confirmation that V0.1 lance 0.15 `.lance` files are
//! NOT raw Parquet — they end with LANC magic and do NOT start with PAR1.
//!
//! T0.2.0 Phase 3 sub-task (b) per HANDOFF.md iteration 4 §4 (amended
//! 2026-05-11 post-README-recon). The fixture README at
//! `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md`
//! lines 61-73 already documents the file-format finding via capture-time
//! inspection ("⚠️ CRITICAL V0.1 file-format finding"). This spike is
//! defensive evidence-doubling per the spike-before-lock discipline:
//! empirically re-confirms the same property via locally-runnable byte
//! inspection, so future readers can re-run the proof without relying on
//! README text alone.
//!
//! ## Question
//!
//! Are V0.1's `.lance` data files raw Parquet containers (PAR1 magic at
//! file start), as iteration 4 §4 initially framed the spike methodology
//! around? Or are they Lance's own binary format (LANC magic at file end),
//! as the fixture README's capture-time inspection documents?
//!
//! ## Pass criteria
//!
//! - First 4 bytes of `<fixture>/data/<smallest-uuid>.lance` are NOT
//!   `b"PAR1"` (`0x50 0x41 0x52 0x31`).
//! - Last 4 bytes of the same file ARE `b"LANC"` (`0x4C 0x41 0x4E 0x43`).
//!
//! Both must hold for the spike to PASS. PASS = README finding confirmed
//! empirically; outcome (ii) locked per iteration 4 §4 amendment
//! (plaintext `LanceVectorStore::open` retained via
//! `#[cfg(feature = "v0_1_migration")]` gate for migration.rs source-path
//! read; production binaries built without the feature have NO plaintext
//! callable).
//!
//! ## Fail outcomes
//!
//! Any FAIL = unexpected drift from README. Investigation needed before
//! proceeding with sub-task (b) implementation. Distinct exit codes per
//! failing assertion so scripts can branch on the specific finding.
//!
//! ## Run
//!
//! ```text
//! cargo run --example v0_1_raw_parquet_read_spike -p vault-storage --release
//! ```
//!
//! Exit code: 0 = PASS (README confirmed), non-zero = unexpected drift.
//!
//! ## Side effects
//!
//! Reads from `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/`
//! (the checked-in Tier 2 fixture). Read-only — no fixture mutation, no
//! tempdir copy needed (the spike only reads byte slices, never opens via
//! Lance API).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

/// Smallest UUID in the fixture's `data/` subdir for deterministic
/// targeting. The 5 `.lance` files are one-fragment-per-row (per fixture
/// README line 56); if THIS file's byte format matches the README, all 5
/// should by transitivity (same V0.1 writer, same fragment-file format).
const FIXTURE_LANCE_FILE: &str = "505875b1-57a0-41fe-a10c-e7dd00283ae1.lance";

/// PAR1 magic — first 4 bytes of every raw Parquet container. If present,
/// iteration 4 §4's original framing (raw `parquet::arrow::ArrowReader`)
/// would have been viable. Per fixture README empirical capture-time
/// inspection: absent.
const PAR1_MAGIC: &[u8; 4] = b"PAR1";

/// LANC magic — last 4 bytes of every Lance fragment file (V0.1 lance
/// 0.15 emit, confirmed by fixture README + production detector in
/// `crates/vault-storage/src/migration.rs:29`). The exit-code-distinct
/// presence assertion is the load-bearing signal that V0.1 fragments are
/// Lance binary format, not Parquet.
const LANC_MAGIC: &[u8; 4] = b"LANC";

fn main() {
    let fixture_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("v0_1_alpha_data_dir")
        .join("lance")
        .join("memories.lance")
        .join("data")
        .join(FIXTURE_LANCE_FILE);

    eprintln!("== T0.2.0 Phase 3 sub-task (b) — V0.1 .lance byte-format spike ==");
    eprintln!("Source fragment: {}", fixture_file.display());

    if !fixture_file.exists() {
        eprintln!("FAIL — fixture path does not exist; spike cannot run.");
        eprintln!("       Verify Tier 2 fixture is committed (e27e6dc or later) and");
        eprintln!("       Cargo.toml's package root is `crates/vault-storage/`.");
        std::process::exit(10);
    }

    let mut file = match File::open(&fixture_file) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("FAIL — File::open: {e}");
            std::process::exit(1);
        }
    };

    let mut first_4 = [0u8; 4];
    if let Err(e) = file.read_exact(&mut first_4) {
        eprintln!("FAIL — read first 4 bytes: {e}");
        std::process::exit(2);
    }

    if let Err(e) = file.seek(SeekFrom::End(-4)) {
        eprintln!("FAIL — seek to end-4: {e}");
        std::process::exit(3);
    }
    let mut last_4 = [0u8; 4];
    if let Err(e) = file.read_exact(&mut last_4) {
        eprintln!("FAIL — read last 4 bytes: {e}");
        std::process::exit(4);
    }

    eprintln!(
        "First 4 bytes: {:02x} {:02x} {:02x} {:02x}  (ASCII: {:?})",
        first_4[0],
        first_4[1],
        first_4[2],
        first_4[3],
        std::str::from_utf8(&first_4).unwrap_or("(non-UTF8)")
    );
    eprintln!(
        "Last 4 bytes:  {:02x} {:02x} {:02x} {:02x}  (ASCII: {:?})",
        last_4[0],
        last_4[1],
        last_4[2],
        last_4[3],
        std::str::from_utf8(&last_4).unwrap_or("(non-UTF8)")
    );
    eprintln!();

    // Assertion 1 — first 4 bytes are NOT PAR1.
    if &first_4 == PAR1_MAGIC {
        eprintln!("UNEXPECTED — first 4 bytes ARE PAR1 magic.");
        eprintln!("This contradicts the fixture README's CRITICAL V0.1 file-format finding.");
        eprintln!("Either:");
        eprintln!("  - The fixture has been replaced with a Parquet-format capture");
        eprintln!("  - The README finding was inaccurate at capture time");
        eprintln!("Investigate before proceeding with sub-task (b).");
        std::process::exit(5);
    }
    eprintln!("[1/2] PASS — first 4 bytes are NOT PAR1 magic.");

    // Assertion 2 — last 4 bytes ARE LANC.
    if &last_4 != LANC_MAGIC {
        eprintln!("UNEXPECTED — last 4 bytes are NOT LANC magic.");
        eprintln!("Expected: 4c 41 4e 43 (per fixture README lines 61-73 and migration.rs:29).");
        eprintln!(
            "Got:      {:02x} {:02x} {:02x} {:02x}",
            last_4[0], last_4[1], last_4[2], last_4[3]
        );
        eprintln!("File-format finding has drifted from README. Investigate.");
        std::process::exit(6);
    }
    eprintln!("[2/2] PASS — last 4 bytes ARE LANC magic.");

    eprintln!();
    eprintln!("ALL PASS — V0.1 .lance file format empirically confirmed:");
    eprintln!("  NOT Parquet (no PAR1 magic at start)");
    eprintln!("  IS  Lance binary (LANC magic at end)");
    eprintln!();
    eprintln!("Outcome (ii) locked per iteration 4 §4 amendment: plaintext");
    eprintln!("LanceVectorStore::open retained via #[cfg(feature = \"v0_1_migration\")]");
    eprintln!("gate. Production binaries built without the feature have NO plaintext");
    eprintln!("callable; migration.rs is the sole legitimate consumer of the gated path.");
    eprintln!();
    eprintln!("Double-anchor lock: README capture-time inspection + this spike's");
    eprintln!("locally-runnable runtime confirmation. Future readers can re-run via");
    eprintln!("the cargo command in the module docs above.");
}
