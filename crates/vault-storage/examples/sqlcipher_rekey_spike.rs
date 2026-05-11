//! Spike: SQLCipher PRAGMA rekey on the rusqlite + bundled-sqlcipher-vendored-openssl
//! chain (ADR-006), for ADR-041 V0.1 VAULT_KEY → V0.2 keychain SQLCipher
//! passphrase bridge.
//!
//! T0.2.0 ADR-041 plan iteration 2 §1.A — compile-and-run methodology
//! (Stage C kill-mid-rekey atomicity probe DROPPED per iteration 2 lock;
//! assume non-atomic and design around with snapshot per §3 step 5).
//!
//! ## Question
//!
//! Does SQLCipher's `PRAGMA rekey` work correctly through the rusqlite +
//! bundled-sqlcipher-vendored-openssl chain (ADR-006) on this dep stack?
//! Web research alone wouldn't catch any rusqlite-passthrough or
//! bundled-SQLCipher quirk; only empirical run gives us the answer.
//!
//! ## Stages
//!
//! - **Stage A — basic rekey:** create fresh SQLCipher file with passphrase
//!   K1, insert a row, `PRAGMA rekey K2`, close, reopen with K2, read row
//!   back. PASS = row content matches.
//! - **Stage B — wrong-key reopen post-rekey:** same setup as Stage A, then
//!   reopen with WRONG passphrase. PASS = subsequent query fails closed
//!   (NOT silent allow with garbage data).
//! - **Read-only-file probe:** validates ADR-041 plan iteration 2 §6 test 7
//!   methodology (c) viability — set vault.db read-only, attempt
//!   `PRAGMA rekey`, expect clean rejection (NOT silent in-memory rekey
//!   that returns Ok without disk write). If probe shows silent-success,
//!   methodology (c) is non-viable and falls back to (a) corrupted-fixture.
//!
//! ## Pass criteria
//!
//! Exit 0 with all three stages PASS.
//!
//! ## Fail outcomes
//!
//! - Stage A FAIL → bridge mechanism changes from PRAGMA rekey to manual
//!   `ATTACH DATABASE 'new.db' KEY 'K2'; INSERT INTO new SELECT * FROM main;`
//!   pattern (slower, more code, different fault modes). Triggers ADR-041
//!   plan iteration 3 per iteration 2 §7.
//! - Stage B FAIL → wrong-key-after-rekey leaks. ADR-041 stops here;
//!   SQLCipher passphrase bridging is not viable on this dep chain.
//! - Read-only probe FAIL (rekey-returns-Ok-but-disk-unchanged) → §6 test 7
//!   methodology flips from (c) to (a) corrupted-fixture. Minor scope
//!   shift; not iteration-3-worthy unless additional fault-injection
//!   design questions surface.
//!
//! ## Run
//!
//! ```text
//! cargo run --example sqlcipher_rekey_spike -p vault-storage --release
//! ```
//!
//! Exit code: 0 = all PASS, non-zero = at least one FAIL.
//!
//! ## Side effects
//!
//! Operates entirely within `tempfile::tempdir()` allocations. No state
//! outside the tempdir is touched.

use rusqlite::Connection;
use tempfile::tempdir;

const K1: &str = "spike-passphrase-K1-old-V0.1-key";
const K2: &str = "spike-passphrase-K2-new-V0.2-key";
const WRONG: &str = "spike-passphrase-wrong-not-K1-K2";

fn main() {
    let stages = [
        ("Stage A — basic rekey + reopen with new key", run_stage_a()),
        (
            "Stage B — reopen with WRONG key fails closed",
            run_stage_b(),
        ),
        (
            "Read-only probe (§6 test 7 methodology check)",
            run_readonly_probe(),
        ),
    ];

    let mut all_pass = true;
    for (name, result) in &stages {
        match result {
            Ok(()) => eprintln!("  PASS  {name}"),
            Err(e) => {
                eprintln!("  FAIL  {name}");
                eprintln!("        {e}");
                all_pass = false;
            }
        }
    }

    if all_pass {
        eprintln!();
        eprintln!("ALL STAGES PASS — ADR-041 spike clear; bridge implementation proceeds.");
        std::process::exit(0);
    } else {
        eprintln!();
        eprintln!("AT LEAST ONE STAGE FAILED — see fail outcomes in module docs for branch.");
        std::process::exit(1);
    }
}

/// Stage A — basic rekey works end-to-end.
fn run_stage_a() -> Result<(), String> {
    let tmp = tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let path = tmp.path().join("vault.db");

    // Setup with K1.
    {
        let conn = Connection::open(&path).map_err(|e| format!("open initial: {e}"))?;
        conn.pragma_update(None, "key", K1)
            .map_err(|e| format!("set key K1: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, content TEXT NOT NULL); \
             INSERT INTO t (id, content) VALUES (42, 'spike-stage-a-row');",
        )
        .map_err(|e| format!("create+insert: {e}"))?;

        // Rekey to K2.
        conn.pragma_update(None, "rekey", K2)
            .map_err(|e| format!("PRAGMA rekey: {e}"))?;
        // Connection drops → close.
    }

    // Reopen with K2; verify row content survives.
    let conn = Connection::open(&path).map_err(|e| format!("reopen with K2: {e}"))?;
    conn.pragma_update(None, "key", K2)
        .map_err(|e| format!("set key K2: {e}"))?;
    let content: String = conn
        .query_row("SELECT content FROM t WHERE id = 42", [], |row| row.get(0))
        .map_err(|e| format!("read row after rekey+reopen: {e}"))?;

    if content != "spike-stage-a-row" {
        return Err(format!("content mismatch after rekey: got {content:?}"));
    }
    Ok(())
}

/// Stage B — wrong-key reopen post-rekey fails closed.
fn run_stage_b() -> Result<(), String> {
    let tmp = tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let path = tmp.path().join("vault.db");

    // Setup with K1, rekey to K2.
    {
        let conn = Connection::open(&path).map_err(|e| format!("open initial: {e}"))?;
        conn.pragma_update(None, "key", K1)
            .map_err(|e| format!("set key K1: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, content TEXT NOT NULL); \
             INSERT INTO t (id, content) VALUES (42, 'spike-stage-b-row');",
        )
        .map_err(|e| format!("create+insert: {e}"))?;
        conn.pragma_update(None, "rekey", K2)
            .map_err(|e| format!("PRAGMA rekey: {e}"))?;
    }

    // Reopen with WRONG key — must fail closed at first query.
    let conn = Connection::open(&path).map_err(|e| format!("reopen: {e}"))?;
    // pragma_update with wrong key may "succeed" at the SET level
    // (SQLCipher defers verification to the first query); the TRUE check
    // is the subsequent query.
    let _ = conn.pragma_update(None, "key", WRONG);
    let result = conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
        row.get::<_, i64>(0)
    });
    match result {
        Ok(_) => Err(
            "CRITICAL: wrong-key reopen post-rekey returned a successful row read \
                      — AEAD verification is NOT enforced on this dep chain. ADR-041 \
                      blocked; SQLCipher passphrase bridging is not viable."
                .into(),
        ),
        Err(_) => Ok(()), // expected
    }
}

/// Read-only-file probe — validates §6 test 7 methodology (c) viability.
///
/// Sets vault.db read-only after creation. Attempts `PRAGMA rekey`. Three
/// possible outcomes:
/// 1. **Rekey returns Err** — methodology (c) is viable; tests can use
///    filesystem permissions to inject rekey failures.
/// 2. **Rekey returns Ok AND reopen with new key succeeds** — read-only
///    perms didn't actually prevent the rekey on disk. Methodology (c)
///    not viable; falls back to (a) corrupted-fixture.
/// 3. **Rekey returns Ok BUT reopen with new key fails (and reopen with
///    old key succeeds)** — silent in-memory rekey; methodology (c) not
///    viable; falls back to (a).
fn run_readonly_probe() -> Result<(), String> {
    let tmp = tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let path = tmp.path().join("vault.db");

    // Setup with K1.
    {
        let conn = Connection::open(&path).map_err(|e| format!("open initial: {e}"))?;
        conn.pragma_update(None, "key", K1)
            .map_err(|e| format!("set key K1: {e}"))?;
        conn.execute_batch(
            "CREATE TABLE t (id INTEGER PRIMARY KEY); INSERT INTO t (id) VALUES (1);",
        )
        .map_err(|e| format!("create+insert: {e}"))?;
    }

    // Set vault.db read-only.
    let mut perms = std::fs::metadata(&path)
        .map_err(|e| format!("metadata: {e}"))?
        .permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&path, perms).map_err(|e| format!("set readonly: {e}"))?;

    // Attempt rekey on read-only file.
    let conn = Connection::open(&path).map_err(|e| format!("reopen for rekey: {e}"))?;
    conn.pragma_update(None, "key", K1)
        .map_err(|e| format!("set key K1 (reopen): {e}"))?;
    let rekey_result = conn.pragma_update(None, "rekey", K2);
    drop(conn);

    // Restore writable so tempdir can clean up.
    let mut perms = std::fs::metadata(&path)
        .map_err(|e| format!("metadata for restore: {e}"))?
        .permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&path, perms).map_err(|e| format!("restore writable: {e}"))?;

    match rekey_result {
        Err(_) => Ok(()), // Outcome 1 — methodology (c) viable
        Ok(()) => {
            // Determine outcome 2 vs 3 — does reopen-with-K2 succeed?
            let conn = Connection::open(&path).map_err(|e| format!("reopen post-Ok-rekey: {e}"))?;
            let _ = conn.pragma_update(None, "key", K2);
            let k2_works = conn
                .query_row("SELECT count(*) FROM sqlite_master", [], |row| {
                    row.get::<_, i64>(0)
                })
                .is_ok();
            if k2_works {
                Err(
                    "Outcome 2: rekey reported Ok AND reopen with K2 succeeds — \
                     read-only perms did NOT prevent rekey on disk. Methodology \
                     (c) NOT viable; ADR-041 plan iteration 2 §6 test 7 falls \
                     back to (a) corrupted-fixture."
                        .into(),
                )
            } else {
                Err("Outcome 3: rekey reported Ok but reopen with K2 fails — \
                     silent in-memory rekey without disk write. Methodology \
                     (c) NOT viable; ADR-041 plan iteration 2 §6 test 7 falls \
                     back to (a) corrupted-fixture."
                    .into())
            }
        }
    }
}
