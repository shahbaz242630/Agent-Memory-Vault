//! "Delete everything" command — ADR-SEC-008.
//!
//! Exposes [`vault_app::erase_vault`] to the Settings tab: destroy the
//! master_key in the OS credential store (cryptographic erasure, BRD
//! §11.5.4), then remove the vault's data files.
//!
//! ## Why this command exists at all
//!
//! Uninstalling Memory Vault does NOT remove vault data — Windows Installer
//! leaves user data behind by design, and deleting someone's memories as a
//! side effect of a routine uninstall is unrecoverable data loss. Keeping
//! the data is the right default; keeping it with no deliberate way to
//! destroy it is not. This is that deliberate way.
//!
//! ## ADR-086 (UI content policy)
//!
//! User-facing strings here name no vendor, model, or database engine. The
//! deliberate ADR-086 carve-out for security specifics (encryption standard,
//! OS credential store) DOES apply to this surface — a user deleting their
//! data is entitled to know the key is destroyed in the operating system's
//! credential store, because that is the substance of the guarantee.
//!
//! ## Honest reporting
//!
//! The command reports what actually happened. It never claims success on
//! the key step when that step failed, because a failed key destruction
//! means the data is still readable — the single thing the user is trying
//! to prevent. Files that could not be deleted are reported separately and
//! explicitly do NOT downgrade the confidentiality result: once the key is
//! gone those bytes are undecryptable ciphertext.

use tauri::State;
use vault_app::keychain::{PRODUCTION_NAMESPACE, VAULT_ID};
use vault_app::Application;

/// Inner implementation, `Application`-only so it is testable without a
/// Tauri runtime (same pattern as the other command modules).
///
/// # Errors
///
/// Returns an opaque error code when cryptographic erasure fails. The
/// caller MUST treat this as "the vault was NOT erased".
pub async fn erase_everything_inner(app: &Application) -> Result<serde_json::Value, String> {
    let vault_root = app.vault_root().to_path_buf();

    // Blocking work (credential store + recursive file removal) off the
    // async runtime, per BRD §2.
    let outcome = tokio::task::spawn_blocking(move || {
        vault_app::erase_vault(&vault_root, PRODUCTION_NAMESPACE, VAULT_ID)
    })
    .await
    .map_err(|_| "erasure_failed".to_string())?
    .map_err(|e| {
        tracing::error!(
            target: "vault_tauri::erasure",
            error = %e,
            "erasure FAILED; the vault is still readable"
        );
        "erasure_failed".to_string()
    })?;

    // NOTE: no audit row. The audit log lives inside the vault we just
    // destroyed — see the `vault_app::erasure` module docs. The operation
    // is recorded in the application log, which survives.

    Ok(serde_json::json!({
        "key_destroyed": outcome.key_destroyed,
        "entries_removed": outcome.entries_removed,
        "undeletable_count": outcome.undeletable.len(),
        // Always true once we reach here: the key step either succeeded or
        // returned Err above. Sent explicitly so the UI never has to infer
        // the security outcome from the file counts.
        "data_is_unrecoverable": outcome.data_is_unrecoverable(),
    }))
}

/// Permanently destroy every memory in this vault.
///
/// Irreversible by design and by mechanism: the key is destroyed, so the
/// data cannot be recovered from a backup of the data directory either.
///
/// # Errors
///
/// `"erasure_failed"` when the key could not be destroyed. No files are
/// removed in that case — erasure either succeeds at the step that matters
/// or does nothing at all.
#[tauri::command]
pub async fn erase_everything(app: State<'_, Application>) -> Result<serde_json::Value, String> {
    erase_everything_inner(&app).await
}
