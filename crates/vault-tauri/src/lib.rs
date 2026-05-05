//! `vault-tauri` library — testable utility functions consumed by the
//! Tauri shell binary at `src/main.rs`.
//!
//! ## ADR-003 lib→bin conversion (T0.1.11 Phase 3)
//!
//! Per ADR-003, vault-tauri shipped as a library skeleton at T0.1.1 and
//! converts to a binary at T0.1.11. Phase 3 interpretation: ADR-003's
//! "converts to binary" reads as "add binary target," not "remove library
//! target." Keeping the library alongside the binary lets us unit-test
//! env-var parsing, OS dispatch, and integrity-failure formatting WITHOUT
//! launching the Tauri runtime — standard Rust pattern for testable apps.
//!
//! ## What lives here vs in main.rs
//!
//! - **lib.rs (this file):** pure functions that take inputs and return
//!   outputs — testable in isolation. ADR-032 env-var parsing, ADR-019
//!   OS-aware dylib filename dispatch, ADR-020 integrity-failure dialog
//!   text formatting.
//! - **main.rs:** Tauri Builder orchestration — builds the AppConfig from
//!   resolved paths, launches `Application::start_with_mcp`, manages the
//!   ApplicationHandle in Tauri state. Thin glue on top of these utilities.

#![forbid(unsafe_code)]

pub mod commands;

use std::env::VarError;
use std::path::PathBuf;

use thiserror::Error;
use vault_core::VaultError;
use vault_storage::SqlCipherKey;

/// Configuration errors surfaced before the Tauri runtime starts.
///
/// Each variant maps to a fatal-dialog message in main.rs and exits the
/// process with a distinct non-zero code so wrapper scripts can
/// distinguish "VAULT_KEY missing" from "model integrity failed."
#[derive(Debug, Error)]
pub enum ConfigError {
    /// VAULT_KEY environment variable is not set. Per ADR-032 (T0.1.11
    /// Phase 3, branch (B) lock): vault-tauri sources its SQLCipher
    /// passphrase from the VAULT_KEY env var for V0.1 founder-only
    /// dogfood. Future-cohort secret-source migration deferred to V0.2
    /// alpha-distribution task per ADR-032 forward-pointer.
    #[error("VAULT_KEY environment variable must be set before launching vault-tauri")]
    VaultKeyUnset,

    /// VAULT_KEY environment variable is set but empty. Treating empty
    /// as unset would let SqlCipherKey::new("") through, producing a
    /// vault encrypted with the empty passphrase — silently wrong.
    /// Failing closed here.
    #[error("VAULT_KEY environment variable is set but empty")]
    VaultKeyEmpty,

    /// Host OS is not one of the three supported V0.1 platforms (Linux /
    /// macOS / Windows per ADR-029 BRD amendment to "Mac or Windows" +
    /// `[ubuntu-latest, windows-latest, macos-latest]` CI matrix landed
    /// at T0.1.11 Phase 1).
    #[error("unsupported platform: {0}")]
    UnsupportedPlatform(String),
}

/// Read the SQLCipher passphrase from the `VAULT_KEY` environment
/// variable per ADR-032 branch (B) lock.
///
/// Production callers use [`parse_vault_key`] which delegates to
/// `std::env::var`. Tests pass a closure to [`parse_vault_key_from`] to
/// avoid mutating the process env (env-var-mutation tests race when
/// cargo runs multiple test binaries in parallel — closure-based tests
/// don't).
pub fn parse_vault_key() -> Result<SqlCipherKey, ConfigError> {
    parse_vault_key_from(|name| std::env::var(name))
}

/// Inner [`parse_vault_key`] that takes an env-var getter closure for
/// testability. Production [`parse_vault_key`] delegates here with
/// `std::env::var`.
pub fn parse_vault_key_from<F>(getter: F) -> Result<SqlCipherKey, ConfigError>
where
    F: Fn(&str) -> Result<String, VarError>,
{
    match getter("VAULT_KEY") {
        Ok(value) if !value.is_empty() => Ok(SqlCipherKey::new(&value)),
        Ok(_) => Err(ConfigError::VaultKeyEmpty),
        Err(_) => Err(ConfigError::VaultKeyUnset),
    }
}

/// Resolve the platform-specific filename for the bundled libonnxruntime
/// dylib per ADR-019 `load-dynamic` strategy.
///
/// Returns the relative path under `BaseDirectory::Resource` (Phase 5
/// installer-mode) or under the `VAULT_ORT_LIB_PATH` env var override
/// (Phase 3 dev-mode boot — main.rs reads this env var if set,
/// otherwise calls `app.path().resolve(BaseDirectory::Resource)`).
///
/// **ADR-019 amendment (T0.1.11 Phase 3):** the v3-vintage
/// "PathResolver::resolve_resource" wording was the JS API name; the
/// Rust API is `app.path().resolve(p, BaseDirectory::Resource)`. This
/// function returns just the relative path; main.rs wires the resolve
/// call. See HANDOFF.md ADR-019 cross-link for the corrected wording.
pub fn dylib_filename_for_os(os: &str) -> Result<&'static str, ConfigError> {
    match os {
        "windows" => Ok("libs/onnxruntime.dll"),
        "macos" => Ok("libs/libonnxruntime.dylib"),
        "linux" => Ok("libs/libonnxruntime.so"),
        other => Err(ConfigError::UnsupportedPlatform(other.to_string())),
    }
}

/// Resolve a dev-mode env-var override to a `PathBuf` if set + non-empty.
///
/// Used by the resource-resolution functions in `main.rs` so the founder
/// running `cargo run -p vault-tauri` can point at the test-fixture
/// dylib / model / tokenizer without needing an installer. Production
/// builds set no env var and fall through to `app.path().resolve(...,
/// BaseDirectory::Resource)`.
///
/// Extracted from `main.rs` at T0.1.11 Phase 4b for testability per
/// multi-agent code-review HIGH finding "extract pure functions to
/// lib.rs for testing without launching Tauri."
pub fn env_override_for(env_var_name: &str) -> Option<PathBuf> {
    match std::env::var(env_var_name) {
        Ok(v) if !v.is_empty() => Some(PathBuf::from(v)),
        _ => None,
    }
}

/// Format a [`ConfigError`] as the fatal-dialog body shown by
/// `main.rs::show_fatal_dialog_and_exit` when configuration parsing
/// fails before Application::new is reached. Matches the `setup()`
/// hook's `format_startup_failure_dialog` pattern (which handles
/// post-Application-construction failures).
///
/// Extracted from `main.rs` at T0.1.11 Phase 4b for testability per
/// multi-agent code-review HIGH finding.
pub fn format_config_error_dialog(err: &ConfigError) -> String {
    match err {
        ConfigError::VaultKeyUnset => "VAULT_KEY environment variable must be set before launching vault-tauri.\n\n\
             Per ADR-032 (T0.1.11 Phase 3 SQLCipher key source), V0.1 alpha sources \
             the SQLCipher passphrase from this env var.\n\n\
             Set it via:\n  PowerShell: $env:VAULT_KEY = \"your-passphrase\"\n  Bash: export VAULT_KEY=your-passphrase".to_string(),
        ConfigError::VaultKeyEmpty => "VAULT_KEY environment variable is set but empty.\n\n\
             Empty passphrases would silently encrypt the vault with the empty key. \
             Failing closed.\n\n\
             Set VAULT_KEY to a non-empty passphrase and relaunch.".to_string(),
        ConfigError::UnsupportedPlatform(os) => format!(
            "Memory Vault does not support the current platform: {os}.\n\n\
             V0.1 supports Linux, macOS, and Windows per BRD §5.11 (amended at T0.1.11 \
             Phase 1 / ADR-029)."
        ),
    }
}

/// Format a `VaultError` as a user-facing fatal-dialog message body for
/// ADR-020 integrity-failure surfacing.
///
/// Special-cases [`VaultError::ModelIntegrityFailed`] with reinstall
/// guidance per ADR-020's "Reinstall to recover" specification (HANDOFF
/// .md line 875 forward-pointer to T0.1.11). All other startup failures
/// get a generic "details: {err}" body with reinstall guidance — the
/// specific recovery procedure for non-integrity failures is
/// out-of-scope for V0.1 founder-only alpha (revisit at V0.2 alpha
/// cohort task when external-user error UX matters).
pub fn format_startup_failure_dialog(err: &VaultError) -> String {
    match err {
        VaultError::ModelIntegrityFailed {
            file,
            expected,
            actual,
        } => format!(
            "Memory Vault cannot start: model integrity check failed.\n\n\
             File: {file}\n\
             Expected SHA-256: {expected}\n\
             Actual SHA-256:   {actual}\n\n\
             Reinstall to recover."
        ),
        other => format!(
            "Memory Vault cannot start.\n\n\
             Details: {other}\n\n\
             Reinstall to recover."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------
    // ADR-032 tests (Phase 3 floor breach: +3 → +5; pre-declared in
    // commit body. The +2 over the v4 floor are below — VAULT_KEY-unset
    // and VAULT_KEY-empty paths.)
    // -----------------------------------------------------------------

    /// ADR-032 branch (B): VAULT_KEY unset → ConfigError::VaultKeyUnset.
    /// main.rs catches this variant and shows the
    /// "VAULT_KEY environment variable must be set" fatal dialog.
    ///
    /// Note: assertion can't `{result:?}`-format because SqlCipherKey
    /// (the Ok type) deliberately doesn't impl Debug per its zeroize-on-
    /// drop secrets discipline (vault-storage/src/key.rs:9-10). Format
    /// only the err side via `.err()` which is `Option<ConfigError>`.
    #[test]
    fn parse_vault_key_returns_unset_err_when_env_var_missing() {
        let result = parse_vault_key_from(|_| Err(VarError::NotPresent));
        assert!(
            matches!(result, Err(ConfigError::VaultKeyUnset)),
            "ADR-032 branch (B): missing VAULT_KEY MUST surface as \
             ConfigError::VaultKeyUnset; got err={:?}",
            result.err()
        );
    }

    /// ADR-032 branch (B): VAULT_KEY set but empty → ConfigError::
    /// VaultKeyEmpty. Treating empty as unset would silently let
    /// SqlCipherKey::new("") through, producing a vault encrypted with
    /// the empty passphrase. Failing closed.
    #[test]
    fn parse_vault_key_returns_empty_err_when_env_var_empty() {
        let result = parse_vault_key_from(|_| Ok(String::new()));
        assert!(
            matches!(result, Err(ConfigError::VaultKeyEmpty)),
            "ADR-032 branch (B): empty VAULT_KEY MUST fail closed as \
             ConfigError::VaultKeyEmpty (not silently accept and \
             encrypt-with-empty-passphrase); got err={:?}",
            result.err()
        );
    }

    /// ADR-032 branch (B): VAULT_KEY set with non-empty value →
    /// SqlCipherKey constructed successfully. SqlCipherKey doesn't
    /// implement PartialEq / Debug per its zeroize-on-drop secrets
    /// discipline (vault-storage/src/key.rs), so the assertion is
    /// limited to "Ok variant returned."
    #[test]
    fn parse_vault_key_returns_ok_when_env_var_set() {
        let result = parse_vault_key_from(|_| Ok("test-passphrase".to_string()));
        assert!(
            result.is_ok(),
            "ADR-032 branch (B): non-empty VAULT_KEY MUST construct \
             SqlCipherKey successfully; got {:?}",
            result.err()
        );
    }

    // -----------------------------------------------------------------
    // ADR-019 dylib path resolution (v4 floor item — OS dispatch test)
    // -----------------------------------------------------------------

    /// ADR-019 OS dispatch: each supported V0.1 platform per ADR-029
    /// BRD amendment maps to its canonical libonnxruntime filename.
    /// Unsupported platforms surface as ConfigError::UnsupportedPlatform.
    #[test]
    fn dylib_filename_dispatches_correctly_per_os() {
        // Three V0.1 first-class platforms per ADR-029 + Phase 1 CI
        // matrix [ubuntu-latest, windows-latest, macos-latest].
        assert_eq!(
            dylib_filename_for_os("windows").unwrap(),
            "libs/onnxruntime.dll",
            "Windows dylib filename must match scripts/setup-dev-env.ps1's \
             extracted onnxruntime.dll"
        );
        assert_eq!(
            dylib_filename_for_os("macos").unwrap(),
            "libs/libonnxruntime.dylib",
            "macOS dylib filename must match scripts/setup-dev-env.sh's \
             Darwin branch ORT_LIB_NAME"
        );
        assert_eq!(
            dylib_filename_for_os("linux").unwrap(),
            "libs/libonnxruntime.so",
            "Linux dylib filename must match scripts/setup-dev-env.sh's \
             Linux branch ORT_LIB_NAME"
        );

        // Unsupported platform → typed error, not panic.
        let err = dylib_filename_for_os("freebsd").unwrap_err();
        assert!(
            matches!(err, ConfigError::UnsupportedPlatform(ref s) if s == "freebsd"),
            "Unsupported OS MUST surface as ConfigError::UnsupportedPlatform \
             with the OS name preserved for diagnostics; got {err:?}"
        );
    }

    // -----------------------------------------------------------------
    // ADR-020 integrity-fatal-dialog wiring (v4 floor item)
    // -----------------------------------------------------------------

    /// ADR-020: ModelIntegrityFailed produces a dialog body that
    /// includes the file path + expected/actual SHA-256 + reinstall
    /// guidance. Pinning the format here prevents drift between the
    /// dialog text and what HANDOFF.md ADR-020 specified
    /// ("Reinstall to recover").
    #[test]
    fn format_startup_failure_dialog_includes_integrity_details() {
        let err = VaultError::ModelIntegrityFailed {
            file: "model.onnx".to_string(),
            expected: "abc123".to_string(),
            actual: "def456".to_string(),
        };
        let dialog = format_startup_failure_dialog(&err);

        assert!(
            dialog.contains("model integrity check failed"),
            "ADR-020 dialog body must announce integrity-check failure; got: {dialog}"
        );
        assert!(
            dialog.contains("model.onnx"),
            "ADR-020 dialog body must include the failing file path for \
             diagnostics; got: {dialog}"
        );
        assert!(
            dialog.contains("abc123") && dialog.contains("def456"),
            "ADR-020 dialog body must include both expected and actual \
             SHA-256 for verifying tampering vs. corruption; got: {dialog}"
        );
        assert!(
            dialog.contains("Reinstall"),
            "ADR-020 dialog body must include 'Reinstall' recovery \
             guidance per HANDOFF.md ADR-020 line 875 specification; \
             got: {dialog}"
        );
    }

    // -----------------------------------------------------------------
    // Phase 4b — extracted-utilities tests (per multi-agent code-review
    // HIGH finding: extract pure functions to lib.rs for testability)
    // -----------------------------------------------------------------

    /// `env_override_for` returns Some(path) when env var is set + non-empty,
    /// None when unset OR set-but-empty. Tests use real env-var get since
    /// the function delegates to `std::env::var`; tests are structured to
    /// avoid the env-var-race issue (each test uses a unique var name).
    #[test]
    fn env_override_for_returns_some_when_var_set_and_none_when_unset_or_empty() {
        // Use unique env-var names per test to avoid races with parallel
        // test execution. These names are scoped to this test and not
        // used by production code.
        let unique_set = "VAULT_TAURI_TEST_ENV_OVERRIDE_SET_42";
        let unique_unset = "VAULT_TAURI_TEST_ENV_OVERRIDE_UNSET_42";
        let unique_empty = "VAULT_TAURI_TEST_ENV_OVERRIDE_EMPTY_42";

        // Ensure clean state.
        std::env::remove_var(unique_set);
        std::env::remove_var(unique_unset);
        std::env::remove_var(unique_empty);

        // Unset → None.
        assert_eq!(
            env_override_for(unique_unset),
            None,
            "env_override_for MUST return None when env var is unset"
        );

        // Empty → None (production path: empty value treated as no override
        // so we fall through to BaseDirectory::Resource).
        std::env::set_var(unique_empty, "");
        assert_eq!(
            env_override_for(unique_empty),
            None,
            "env_override_for MUST return None when env var is set-but-empty \
             (treats empty as no override)"
        );
        std::env::remove_var(unique_empty);

        // Set → Some(path).
        std::env::set_var(unique_set, "C:/test/path/libonnxruntime.dll");
        assert_eq!(
            env_override_for(unique_set),
            Some(PathBuf::from("C:/test/path/libonnxruntime.dll")),
            "env_override_for MUST return Some(PathBuf) when env var is set + non-empty"
        );
        std::env::remove_var(unique_set);
    }

    /// `format_config_error_dialog` covers all three ConfigError variants
    /// with appropriate user-facing messages. Pinning the body content
    /// here prevents drift between the dialog text and what main.rs's
    /// `show_fatal_dialog_and_exit` callers rely on.
    #[test]
    fn format_config_error_dialog_covers_all_three_variants() {
        // VaultKeyUnset — references VAULT_KEY env var name + ADR-032 +
        // both PowerShell and Bash setup commands.
        let unset_dialog = format_config_error_dialog(&ConfigError::VaultKeyUnset);
        assert!(
            unset_dialog.contains("VAULT_KEY"),
            "VaultKeyUnset dialog must reference the env var name; got: {unset_dialog}"
        );
        assert!(
            unset_dialog.contains("ADR-032"),
            "VaultKeyUnset dialog must reference ADR-032 for source-of-truth; got: {unset_dialog}"
        );
        assert!(
            unset_dialog.contains("PowerShell") && unset_dialog.contains("Bash"),
            "VaultKeyUnset dialog must include both PowerShell and Bash setup \
             commands so cross-shell founders can recover; got: {unset_dialog}"
        );

        // VaultKeyEmpty — explains fail-closed posture.
        let empty_dialog = format_config_error_dialog(&ConfigError::VaultKeyEmpty);
        assert!(
            empty_dialog.contains("empty"),
            "VaultKeyEmpty dialog must announce empty-passphrase rejection; got: {empty_dialog}"
        );
        assert!(
            empty_dialog.contains("Failing closed"),
            "VaultKeyEmpty dialog must explain fail-closed posture so the \
             founder doesn't think Memory Vault is broken; got: {empty_dialog}"
        );

        // UnsupportedPlatform — includes the unsupported OS name.
        let unsupported_dialog =
            format_config_error_dialog(&ConfigError::UnsupportedPlatform("freebsd".to_string()));
        assert!(
            unsupported_dialog.contains("freebsd"),
            "UnsupportedPlatform dialog must include the unsupported OS \
             name for diagnostics; got: {unsupported_dialog}"
        );
        assert!(
            unsupported_dialog.contains("ADR-029"),
            "UnsupportedPlatform dialog must reference ADR-029 (BRD amendment \
             that locked V0.1 platform list); got: {unsupported_dialog}"
        );
    }

    // -----------------------------------------------------------------
    // Phase 4b — ADR-030 negative regression test (source-grep)
    // -----------------------------------------------------------------

    /// **ADR-030 outcome shape (a) regression check.** vault-tauri MUST
    /// NOT expose a Tauri command that takes user-controlled input and
    /// passes it into `StdioServerParameters` or spawns external MCP
    /// servers. Adding such a command requires ADR-030 amendment first
    /// (V1.0 connectors task per ADR-026 forward-pointer).
    ///
    /// Mechanism per Shahbaz Phase 4b v2 review clarification 2:
    /// **Rust-side source-grep against main.rs via `include_str!`** —
    /// deterministic, no Tauri runtime needed, no false negatives from
    /// macro-generation paths (in V0.1 Tauri commands are listed
    /// directly in `main.rs`'s `.invoke_handler(tauri::generate_handler![
    /// commands::add_memory, ...])`; if a future contributor adds
    /// `commands::spawn_external_mcp_server` to that list, the source
    /// text contains the forbidden substring and this test catches it).
    #[test]
    fn main_rs_does_not_register_external_mcp_spawn_command_per_adr_030() {
        let main_rs = include_str!("main.rs");

        // Forbidden patterns are COMMAND-NAME-style identifiers — what
        // would appear in a `#[tauri::command] fn <name>` declaration
        // OR a `tauri::generate_handler![commands::<name>]` registration
        // line. The API type `StdioServerParameters` is deliberately
        // NOT in this list because main.rs's own doc comment references
        // the term in negative form (per ADR-030 outcome (a) cross-link).
        // Substring match against the API type name would false-positive
        // on the legitimate doc reference.
        let forbidden_substrings = [
            "spawn_external_mcp",
            "configure_mcp_server",
            "add_mcp_server",
            "external_mcp_server",
            "configure_external",
            "stdio_server_params",
        ];

        for substring in forbidden_substrings {
            assert!(
                !main_rs.contains(substring),
                "ADR-030 outcome (a) regression: main.rs contains '{}' which \
                 suggests external-MCP-server-spawn UI surface. Per ADR-030 \
                 amendment 2026-05-05 (T0.1.11 Phase 2): vault-tauri must \
                 spawn ONLY our own vault-mcp child via in-process stdio; \
                 user-controlled input MUST NOT flow into StdioServerParameters. \
                 Adding such a surface requires V1.0 connectors task per \
                 ADR-026 forward-pointer.",
                substring
            );
        }
    }
}
