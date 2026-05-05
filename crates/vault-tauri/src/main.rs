//! `vault-tauri` binary entry point — Tauri shell that wraps the V0.1
//! composition root from vault-app. T0.1.11 Phase 3.
//!
//! ## ADR cross-references
//!
//! - **ADR-003:** library→binary conversion lands at T0.1.11 (this commit).
//!   Library target retained at `src/lib.rs` for testable utilities (env-var
//!   parsing, OS dispatch, integrity-failure formatting); main.rs is thin
//!   Tauri Builder orchestration on top.
//! - **ADR-019:** bundled libonnxruntime dylib resolved via
//!   `app.path().resolve(filename, BaseDirectory::Resource)`. Production
//!   path. Dev-mode override via `VAULT_ORT_LIB_PATH` env var (so founder
//!   running `cargo run -p vault-tauri` can point at the test-fixture
//!   dylib without needing an installer).
//! - **ADR-020:** model + tokenizer SHA-256 verification at
//!   `Application::new` — failure surfaces as fatal Tauri dialog via
//!   tauri-plugin-dialog, exits non-zero before any UI loads.
//! - **ADR-029:** branch (2) Windows-dogfood lock; founder runs
//!   `cargo run -p vault-tauri` on Windows 11 dev machine for V0.1
//!   founder-only dogfood (T0.1.12).
//! - **ADR-030:** vault-tauri spawns ONLY our own vault-mcp child via
//!   `Application::start_with_mcp` (which embeds the rmcp stdio server
//!   in-process per T0.1.10 wiring) — no user-controlled
//!   StdioServerParameters surface, no external-MCP-server-config UI in
//!   V0.1. Outcome shape (a) per ADR-026 forward-pointer.
//! - **ADR-032:** SQLCipher passphrase sourced from `VAULT_KEY` env var
//!   for V0.1 founder-only dogfood. Branch (B) per Spike 1
//!   (keyring-core ecosystem mid-migration; multi-user cohort secret
//!   source revisits at V0.2 alpha-distribution task).
//!
//! ## Boot flow (Phase 3 minimal — no UI commands yet)
//!
//! 1. Read `VAULT_KEY` → `SqlCipherKey` (ADR-032). Fail-closed dialog if
//!    unset/empty, exit code 2.
//! 2. Resolve `VAULT_ORT_LIB_PATH` (dev) or
//!    `app.path().resolve(BaseDirectory::Resource)` (production) for the
//!    libonnxruntime dylib (ADR-019).
//! 3. Resolve bundled model.onnx + tokenizer.json paths (ADR-019/020).
//! 4. Resolve per-user data dir via `app.path().app_data_dir()`.
//! 5. Build `AppConfig` from resolved paths.
//! 6. `Application::new(&config).await` — fail-closed dialog on integrity
//!    failure (ADR-020), exit code 1.
//! 7. `Application::start_with_mcp(authorized_boundaries).await` —
//!    spawns retry worker + rmcp stdio server + signal handlers; returns
//!    `ApplicationHandle`.
//! 8. `app_handle.manage(handle)` per ADR-003 register-shape pick
//!    (register `Application` directly, Tauri auto-Arc-wraps internally).
//! 9. Tauri main runtime takes over; webview loads `dist/index.html`
//!    placeholder. Phase 4 replaces with real UI.
//!
//! ## V0.1 authorized_boundaries
//!
//! Hardcoded `vec![Boundary::new("default")?]` — V0.1 founder-only
//! single-user, single boundary. Multi-boundary management UI lands at
//! Phase 4 / V0.2 per BRD §5.11 settings-view evolution.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use vault_app::{AppConfig, Application};
use vault_core::Boundary;
use vault_storage::SqlCipherKey;
use vault_tauri::{
    dylib_filename_for_os, format_startup_failure_dialog, parse_vault_key, ConfigError,
};

/// Exit code for ConfigError::VaultKeyUnset / VaultKeyEmpty (ADR-032).
const EXIT_CONFIG_ERROR: i32 = 2;
/// Exit code for Application startup failures including ADR-020
/// ModelIntegrityFailed.
const EXIT_STARTUP_FAILURE: i32 = 1;
/// V0.1 hardcoded boundary per inline-architectural decision in this
/// commit (HANDOFF.md commit body). Phase 4 may extend.
const V0_1_DEFAULT_BOUNDARY: &str = "default";

fn main() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 1. Source SqlCipherKey from VAULT_KEY env var per ADR-032.
            let key = match parse_vault_key() {
                Ok(k) => k,
                Err(err) => {
                    show_fatal_dialog_and_exit(
                        app.handle(),
                        "Memory Vault — Configuration Required",
                        &format_config_error_dialog(&err),
                        EXIT_CONFIG_ERROR,
                    );
                }
            };

            // 2. Resolve libonnxruntime dylib path per ADR-019.
            //    Dev-mode override via VAULT_ORT_LIB_PATH; production
            //    falls through to app.path().resolve(BaseDirectory::Resource).
            let ort_lib_path = match resolve_ort_lib_path(app.handle()) {
                Ok(p) => p,
                Err(e) => {
                    show_fatal_dialog_and_exit(
                        app.handle(),
                        "Memory Vault — Resource Resolution Failed",
                        &format!(
                            "Could not locate libonnxruntime dylib.\n\n\
                             Details: {e}\n\n\
                             For dev runs, set VAULT_ORT_LIB_PATH to the path of \
                             the dylib (e.g. crates/vault-embedding/test-fixtures/\
                             bge-small-en-v1.5/libonnxruntime.{{dll,dylib,so}}).\n\
                             For installed builds, reinstall to recover."
                        ),
                        EXIT_STARTUP_FAILURE,
                    );
                }
            };

            // 3. Resolve bundled model + tokenizer paths per ADR-019/020.
            //    Same dev-mode env-var override pattern.
            let model_path = resolve_model_path(app.handle())
                .map_err(|e| Box::<dyn std::error::Error>::from(format!("model path: {e}")))?;
            let tokenizer_path = resolve_tokenizer_path(app.handle())
                .map_err(|e| Box::<dyn std::error::Error>::from(format!("tokenizer path: {e}")))?;

            // 4. Per-user data directory per Tauri convention.
            let data_dir: PathBuf = app
                .path()
                .app_data_dir()
                .map_err(|e| Box::<dyn std::error::Error>::from(format!("app_data_dir: {e}")))?;
            std::fs::create_dir_all(&data_dir)?;
            let metadata_path = data_dir.join("vault.db");
            let vector_dir = data_dir.join("lance");
            let graph_path = data_dir.join("graph.duckdb");

            // 5. Build AppConfig from resolved paths.
            let config = AppConfig {
                metadata_path,
                vector_dir,
                graph_path,
                key,
                model_path,
                tokenizer_path,
                ort_lib_path,
            };

            // 6 + 7. Construct Application and start the MCP-host
            // lifecycle. Block on the Tauri async runtime so that
            // setup() resolves to a synchronous Result for Tauri's
            // initialization machinery.
            //
            // Error handling: on any startup failure (ADR-020 model
            // integrity, missing data dir permissions, MCP bind, etc.)
            // we show a fatal Tauri dialog and exit non-zero before
            // any UI loads. show_fatal_dialog_and_exit diverges (`-> !`)
            // so the function never returns from those branches.
            let app_handle = app.handle().clone();
            let handle = tauri::async_runtime::block_on(async move {
                let application = match Application::new(&config).await {
                    Ok(a) => a,
                    Err(e) => show_fatal_dialog_and_exit(
                        &app_handle,
                        "Memory Vault — Fatal Error",
                        &format_startup_failure_dialog(&e),
                        EXIT_STARTUP_FAILURE,
                    ),
                };

                let boundaries = vec![Boundary::new(V0_1_DEFAULT_BOUNDARY)
                    .expect("'default' is a valid boundary literal")];
                match application.start_with_mcp(boundaries).await {
                    Ok(h) => h,
                    Err(e) => show_fatal_dialog_and_exit(
                        &app_handle,
                        "Memory Vault — Fatal Error",
                        &format_startup_failure_dialog(&e),
                        EXIT_STARTUP_FAILURE,
                    ),
                }
            });

            // 8. Manage the handle so commands can access it later (Phase 4).
            //    ADR-003 register-shape pick: register the handle directly,
            //    Tauri auto-Arc-wraps internally per Spike 1 finding.
            app.manage(handle);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Render a fatal dialog and terminate the process. **Diverges** —
/// callers should treat the return type as `!` (the function never
/// returns to the caller). Used by setup() bail-out paths to surface a
/// user-visible reason before exit.
///
/// Note: not annotated `-> !` because `app.handle().dialog()` /
/// `block_on` interleave means we use `std::process::exit` after the
/// dialog dismisses; the function still terminates the process before
/// returning.
fn show_fatal_dialog_and_exit(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    exit_code: i32,
) -> ! {
    app.dialog().message(body).title(title).blocking_show();
    std::process::exit(exit_code);
}

fn format_config_error_dialog(err: &ConfigError) -> String {
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

/// Resolve libonnxruntime dylib path per ADR-019. Dev-mode override via
/// `VAULT_ORT_LIB_PATH` env var (so founder running `cargo run -p
/// vault-tauri` can point at `crates/vault-embedding/test-fixtures/
/// bge-small-en-v1.5/libonnxruntime.{dll,dylib,so}`); production falls
/// through to `app.path().resolve(BaseDirectory::Resource)`.
fn resolve_ort_lib_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("VAULT_ORT_LIB_PATH") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let filename =
        dylib_filename_for_os(std::env::consts::OS).map_err(|e| format!("OS dispatch: {e}"))?;
    app.path()
        .resolve(filename, tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("resolve {filename}: {e}"))
}

/// Resolve bundled model.onnx path. Dev-mode override via
/// `VAULT_MODEL_PATH` env var; production falls through to
/// `app.path().resolve("models/model.onnx", BaseDirectory::Resource)`.
fn resolve_model_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("VAULT_MODEL_PATH") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    app.path()
        .resolve("models/model.onnx", tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("resolve model.onnx: {e}"))
}

/// Resolve bundled tokenizer.json path. Dev-mode override via
/// `VAULT_TOKENIZER_PATH` env var; production falls through to
/// `app.path().resolve("models/tokenizer.json", BaseDirectory::Resource)`.
fn resolve_tokenizer_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("VAULT_TOKENIZER_PATH") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    app.path()
        .resolve(
            "models/tokenizer.json",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| format!("resolve tokenizer.json: {e}"))
}

// Suppress unused-import warnings for the SqlCipherKey re-export when
// inline-tested via lib.rs. The type IS used (in AppConfig construction
// at line "key,") but rustc's import-tracking sometimes flags it as
// unused under cfg(test) for binaries with a sibling lib.rs.
#[allow(dead_code)]
fn _force_sqlcipher_key_import_visible() {
    let _: Option<SqlCipherKey> = None;
}
