//! Phase 1 keychain spike — T0.2.0 close-out plan iteration 1.
//!
//! See HANDOFF.md "Phase 1 keychain spike — methodology declaration" + the
//! OQ #1 partial-resolution table for crate-selection context. Spike runs the
//! load-bearing leg (compile-and-run on Windows) of the hybrid spike-before-
//! lock methodology after Leg 1 web-research locked the candidate stack to
//! `keyring-core 1.0.0` + `windows-native-keyring-store 1.0.0` (`open-source-
//! cooperative` org, April 2026 coordinated release).
//!
//! ## Run modes (PowerShell on Windows per `feedback_cargo_on_windows_use_powershell.md`)
//!
//! **Interactive (full four assertions, default):**
//!
//!   cargo run -p vault-app --example keychain_spike --release
//!
//! **Non-interactive (assertions 1-3 only; leaves entry in place for offline
//! Credential Manager verification; SKIPS cleanup):**
//!
//!   cargo run -p vault-app --example keychain_spike --release -- --no-interactive
//!
//! **Cleanup-only (deletes the spike's keychain entry; idempotent):**
//!
//!   cargo run -p vault-app --example keychain_spike --release -- --cleanup
//!
//! The non-interactive split exists because assertion 4 is fundamentally a
//! human-out-of-band check (open Credential Manager, look for the entry).
//! Driving it from a non-TTY harness is theatre — `stdin.read_line` returns
//! `Ok(0)` immediately on EOF and the cleanup-then-PASS path runs without
//! anyone ever having seen the entry. Splitting the modes keeps assertion 4
//! honest: --no-interactive proves 1-3 + leaves on-OS evidence, the human
//! confirms 4, then --cleanup removes it.
//!
//! ## Four assertions (failure-conditions per methodology declaration)
//!
//! 1. **Round-trip byte-identity** — write 32 random bytes, read back, byte-equal.
//! 2. **Wrong-account-fails-closed** — read with mismatched account → `Err`.
//!    Silent success or empty-bytes return is a security failure mode → STOP.
//! 3. **Process-restart-survives** — child process re-reads same entry,
//!    asserts byte-identical against parent's expected hex, exits 0.
//! 4. **Manual Credential Manager UI verification** — namespace + account
//!    printed to stdout; human confirms the entry is visible in Windows
//!    Credential Manager (Control Panel → User Accounts → Credential
//!    Manager → Windows Credentials) BEFORE cleanup deletes it.
//!
//! ## Stop-and-escalate triggers
//!
//! Any assertion failure → exit 1 with diagnostic message. Do not improvise
//! workarounds (per `feedback_dont_propose_relaxation_for_speed.md`). ADR-040
//! drafts from this spike's runtime evidence at Phase 1 close.
//!
//! ## Deps (workspace-promoted at Phase 1 close)
//!
//! Per `feedback_spike_methodology_explicit.md` discipline, `keyring-core`,
//! `windows-native-keyring-store`, `getrandom`, and `hex` were spike-local
//! dev-deps at Phase 1 spike runtime (2026-05-09); they promoted to
//! `[workspace.dependencies]` + vault-app `[dependencies]` at Phase 1 close
//! alongside production wiring of `vault_app::keychain`. The spike file
//! continues to consume them via the workspace-deps path and stays as
//! executable documentation per the ADR-008 line 696 spike-retention pattern.

#[cfg(not(windows))]
fn main() {
    eprintln!(
        "Phase 1 keychain spike is Windows-only by design \
         (per ADR-029 V0.1 dogfood pattern — runtime confirmation happens \
         on the founder's actual hardware, which is Windows). macOS / Linux \
         runtime confirmation is per-platform-crate-add at T0.2.0.x sub-task \
         or T0.2.14 Stub-Installer-adjacent per HANDOFF.md OQ #1 partial \
         resolution. Run from a Windows host."
    );
    std::process::exit(2);
}

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use keyring_core::Entry;
    use windows_native_keyring_store::Store;

    const SPIKE_NAMESPACE: &str = "com.memoryvault.spike.v0.2";
    const SPIKE_ACCOUNT: &str = "spike-test-vault";
    const WRONG_ACCOUNT: &str = "spike-test-vault-WRONG";

    let args: Vec<String> = std::env::args().collect();
    let no_interactive = args.iter().any(|a| a == "--no-interactive");
    let cleanup_only = args.iter().any(|a| a == "--cleanup");

    // Cleanup-only branch — idempotent delete-and-exit.
    if cleanup_only {
        keyring_core::set_default_store(
            Store::new().map_err(|e| format!("Store::new failed: {e}"))?,
        );
        match Entry::new(SPIKE_NAMESPACE, SPIKE_ACCOUNT) {
            Ok(e) => match e.delete_credential() {
                Ok(()) => eprintln!(
                    "[--cleanup] OK — entry '{SPIKE_NAMESPACE}' / '{SPIKE_ACCOUNT}' deleted."
                ),
                Err(err) => eprintln!(
                    "[--cleanup] WARN — delete_credential returned: {err}. \
                     Entry may already be gone (idempotent), or manual deletion via \
                     Credential Manager required."
                ),
            },
            Err(err) => eprintln!("[--cleanup] WARN — Entry::new failed: {err}"),
        }
        keyring_core::unset_default_store();
        return Ok(());
    }

    // Child-process branch (assertion 3 second-half). The parent spawns the
    // same executable with `--child-read <hex>` and waits on exit code; the
    // child re-opens the keychain entry and asserts byte-identity against the
    // hex argument. Separate process = process-restart simulation; if the
    // entry is in-memory-only / process-cache-only, this branch fails.
    if args.len() >= 2 && args[1] == "--child-read" {
        if args.len() < 3 {
            return Err("child: --child-read requires expected-key-hex arg".into());
        }
        let expected =
            hex::decode(&args[2]).map_err(|e| format!("child: bad expected-hex: {e}"))?;

        keyring_core::set_default_store(
            Store::new().map_err(|e| format!("child: Store::new failed: {e}"))?,
        );
        let entry = Entry::new(SPIKE_NAMESPACE, SPIKE_ACCOUNT)
            .map_err(|e| format!("child: Entry::new failed: {e}"))?;
        let read_back = entry
            .get_secret()
            .map_err(|e| format!("child: get_secret failed: {e}"))?;
        keyring_core::unset_default_store();

        if read_back != expected {
            return Err(format!(
                "child: process-restart-survives FAILED — \
                 read {} bytes from keychain, did not match parent's expected key \
                 (length-equal: {})",
                read_back.len(),
                read_back.len() == expected.len()
            )
            .into());
        }
        eprintln!("child: process-restart-survives OK");
        return Ok(());
    }

    eprintln!("=== Phase 1 keychain spike — Memory Vault T0.2.0 ===");
    eprintln!();
    eprintln!("Stack: keyring-core 1.0.0 + windows-native-keyring-store 1.0.0");
    eprintln!("Namespace: {SPIKE_NAMESPACE}");
    eprintln!("Account:   {SPIKE_ACCOUNT}");
    eprintln!();

    keyring_core::set_default_store(Store::new().map_err(|e| format!("Store::new failed: {e}"))?);

    // Run the four assertions. The closure captures any failure as a
    // structured error; cleanup runs afterward regardless.
    let assertions: Result<(), Box<dyn std::error::Error>> = (|| {
        // ── Assertion 1: Round-trip byte-identity ────────────────────────
        eprintln!("[1/4] Round-trip byte-identity:");
        let mut master_key = [0u8; 32];
        getrandom::getrandom(&mut master_key).map_err(|e| format!("getrandom failed: {e}"))?;
        let entry = Entry::new(SPIKE_NAMESPACE, SPIKE_ACCOUNT)
            .map_err(|e| format!("Entry::new failed: {e}"))?;
        entry
            .set_secret(&master_key)
            .map_err(|e| format!("set_secret failed: {e}"))?;
        let read_back = entry
            .get_secret()
            .map_err(|e| format!("get_secret failed: {e}"))?;
        if read_back != master_key {
            return Err(format!(
                "Assertion 1 FAILED — round-trip not byte-identical \
                 (wrote 32 bytes, read {} bytes; bytes differ)",
                read_back.len()
            )
            .into());
        }
        eprintln!("      OK — 32 bytes written + read byte-identical.");

        // ── Assertion 2: Wrong-account-fails-closed ──────────────────────
        eprintln!("[2/4] Wrong-account-fails-closed:");
        let wrong = Entry::new(SPIKE_NAMESPACE, WRONG_ACCOUNT)
            .map_err(|e| format!("Entry::new(wrong) failed: {e}"))?;
        match wrong.get_secret() {
            Err(_) => {
                eprintln!("      OK — wrong account returned Err (fail-closed).");
            }
            Ok(bytes) => {
                return Err(format!(
                    "Assertion 2 FAILED — wrong account returned Ok({} bytes); \
                     SECURITY FAILURE — should have returned Err. \
                     STOP and escalate per methodology failure-conditions clause.",
                    bytes.len()
                )
                .into());
            }
        }

        // ── Assertion 3: Process-restart-survives via child process ──────
        eprintln!("[3/4] Process-restart-survives via child process:");
        let exe = std::env::current_exe().map_err(|e| format!("current_exe failed: {e}"))?;
        let expected_hex = hex::encode(master_key);
        let status = std::process::Command::new(&exe)
            .arg("--child-read")
            .arg(&expected_hex)
            .status()
            .map_err(|e| format!("child spawn failed: {e}"))?;
        if !status.success() {
            return Err(format!(
                "Assertion 3 FAILED — child process exited non-zero ({:?}); \
                 entry is in-memory-only / process-isolated, NOT persisted to OS. \
                 ADR-032 amendment needs re-architecting.",
                status.code()
            )
            .into());
        }
        eprintln!("      OK — child process re-read entry byte-identical.");

        // ── Assertion 4: Manual Credential Manager UI verification ───────
        if no_interactive {
            eprintln!(
                "[4/4] Manual Credential Manager UI verification — SKIPPED (--no-interactive)."
            );
            eprintln!("      Entry is LEFT IN PLACE for offline verification.");
            eprintln!("      Verify via: Control Panel → User Accounts → Credential Manager");
            eprintln!("                  → Windows Credentials");
            eprintln!("      Look for entry whose target/username contains '{SPIKE_NAMESPACE}'");
            eprintln!("      and account '{SPIKE_ACCOUNT}'.");
            eprintln!();
            eprintln!("      After verification, run with --cleanup to delete:");
            eprintln!(
                "      cargo run -p vault-app --example keychain_spike --release -- --cleanup"
            );
        } else {
            eprintln!("[4/4] Manual Credential Manager UI verification:");
            eprintln!("      Open: Control Panel → User Accounts → Credential Manager");
            eprintln!("            → Windows Credentials");
            eprintln!("      Look for entry whose target/username contains '{SPIKE_NAMESPACE}'");
            eprintln!("      and account '{SPIKE_ACCOUNT}'.");
            eprintln!();
            eprintln!("      MANUAL — confirm entry is visible BEFORE this spike");
            eprintln!("      completes cleanup. Press Enter to continue.");
            let mut _line = String::new();
            std::io::stdin()
                .read_line(&mut _line)
                .map_err(|e| format!("stdin read failed: {e}"))?;
            eprintln!("      OK — assumed-confirmed; proceeding to cleanup.");
        }

        Ok(())
    })();

    // Cleanup — INTERACTIVE mode only. --no-interactive intentionally leaves
    // the entry in place so the human can verify it offline before the next
    // --cleanup invocation.
    if !no_interactive {
        match Entry::new(SPIKE_NAMESPACE, SPIKE_ACCOUNT) {
            Ok(e) => match e.delete_credential() {
                Ok(()) => eprintln!("[cleanup] OK — spike keychain entry deleted."),
                Err(err) => eprintln!(
                    "[cleanup] WARN — delete_credential failed: {err}. \
                     Manually delete entry '{SPIKE_NAMESPACE}' / '{SPIKE_ACCOUNT}' \
                     via Credential Manager."
                ),
            },
            Err(err) => eprintln!(
                "[cleanup] WARN — Entry::new for cleanup failed: {err}. \
                 Manually delete entry '{SPIKE_NAMESPACE}' / '{SPIKE_ACCOUNT}' \
                 via Credential Manager."
            ),
        }
    }

    keyring_core::unset_default_store();

    match assertions {
        Ok(()) => {
            eprintln!();
            if no_interactive {
                eprintln!("=== KEYCHAIN SPIKE: ASSERTIONS 1-3 PASS ===");
                eprintln!("=== ASSERTION 4 (manual UI) PENDING — verify, then run --cleanup. ===");
            } else {
                eprintln!("=== KEYCHAIN SPIKE: ALL FOUR ASSERTIONS PASS ===");
            }
            Ok(())
        }
        Err(e) => {
            eprintln!();
            eprintln!("=== KEYCHAIN SPIKE: FAILURE — {e}");
            eprintln!("=== Stop and escalate per methodology failure-conditions clause.");
            Err(e)
        }
    }
}
