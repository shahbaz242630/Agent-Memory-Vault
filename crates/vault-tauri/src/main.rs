//! `vault-tauri` binary entry point — Tauri shell that wraps the V0.1
//! composition root from vault-app. T0.1.11 Phase 4b.
//!
//! ## ADR cross-references
//!
//! - **ADR-003:** library→binary conversion lands at T0.1.11 Phase 3.
//!   Library target retained at `src/lib.rs` for testable utilities
//!   (env-var parsing, OS dispatch, integrity-failure formatting,
//!   resource-path env-var override checks); main.rs is thin Tauri
//!   Builder orchestration on top.
//! - **ADR-019:** bundled libonnxruntime dylib resolved via
//!   `app.path().resolve(filename, BaseDirectory::Resource)`. Production
//!   path. Dev-mode override via `VAULT_ORT_LIB_PATH` env var (testable
//!   via `vault_tauri::env_override_for`).
//! - **ADR-020:** model + tokenizer SHA-256 verification at
//!   `Application::new` — failure surfaces as fatal Tauri dialog via
//!   tauri-plugin-dialog, exits non-zero before any UI loads.
//! - **ADR-029:** branch (2) Windows-dogfood lock; founder runs
//!   `cargo run -p vault-tauri` on Windows 11 dev machine for V0.1
//!   founder-only dogfood (T0.1.12).
//! - **ADR-030:** vault-tauri spawns no external child MCP process — no
//!   user-controlled StdioServerParameters surface, no external-MCP-
//!   server-config UI in V0.1. Outcome shape (a) per ADR-026 forward-
//!   pointer. Phase 4b adds source-grep regression test in
//!   `lib.rs::tests::main_rs_does_not_register_external_mcp_spawn_command_per_adr_030`.
//! - **ADR-032:** SQLCipher passphrase sourced from `VAULT_KEY` env var
//!   for V0.1 founder-only dogfood. **Retired at T0.2.0 Phase 1
//!   (2026-05-09)** per ADR-040 + ADR-040 amendment: master_key now
//!   sourced from Windows Credential Manager via `vault_app::keychain::
//!   read_or_init_master_key`; SqlCipherKey + at-rest key derived as
//!   domain-separated BLAKE3 subkeys. Pre-Phase-1 callers reading
//!   VAULT_KEY env var are removed.
//! - **ADR-034 (Phase 5b fix-forward, 2026-05-05):** V0.1 vault-tauri is
//!   UI-only — no MCP server bound inside the Tauri process. Phase 5
//!   founder smoke surfaced that `Application::start_with_mcp` calls
//!   `rmcp::ServiceExt::serve(server, stdio()).await` which blocks on
//!   JSON-RPC `initialize` from a non-existent peer when launched as a
//!   Tauri UI app, hanging Tauri's setup() hook. Phase 5b replaces the
//!   call with `Application::start()` (worker-only, no MCP transport
//!   bind). AI-client MCP integration deferred to V0.2 alpha-distribution
//!   subcommand-split task. T0.1.12 founder dogfood is UI-only for V0.1.
//! - **ADR-038 (T0.2.0 Phase 0a fix-forward, 2026-05-07):** the binary's
//!   process environment MUST have `LANCE_MEM_POOL_SIZE=268435456` (256
//!   MiB) set BEFORE this binary launches, so lance/datafusion's
//!   `merge_insert` JOIN path is bounded. This cannot be set inside Rust
//!   code — ADR-002 forbids `unsafe_code` workspace-wide, and rustc 1.80+
//!   marks `std::env::set_var` as `unsafe`. The shell-level launcher is
//!   the correct semantic home: lance reads the var lazily on first
//!   datafusion-plan construction, so it must already be in the
//!   environment when the binary starts. Dev runs via `cargo` pick this
//!   up from `.cargo/config.toml`'s `[env]` block; CI runs pick it up
//!   from `.github/workflows/ci.yml`'s top-level `env:` block; V0.2
//!   alpha-distribution launchers (T0.2.14) MUST set it via WiX MSI
//!   pre-args (Windows), Info.plist `LSEnvironment` (macOS .app), or a
//!   `.desktop` `Exec` wrapper (Linux). See ADR-038 in HANDOFF.md and
//!   the struct-field doc on
//!   `vault_storage::vector_store::LanceVectorStore::upsert_lock`.
//! - **Phase 4a HIGH findings cleared at Phase 4b:** line 170
//!   `Boundary::default_name()` swap; line 191 `tauri::Builder::run`
//!   match + `eprintln!` + `std::process::exit`; lines 122-131
//!   `?`-propagation → match + `show_fatal_dialog_and_exit` routing;
//!   phantom `_force_sqlcipher_key_import_visible` deletion;
//!   `resolve_*` + `format_config_error_dialog` extracted to lib.rs
//!   for testability.

#![forbid(unsafe_code)]
// Phase 5e fix-forward (T0.1.12 dogfood Finding #2): mark the binary as
// Windows GUI subsystem (not console subsystem) for release builds. Without
// this attribute, Windows allocates a console window alongside the Tauri
// UI on every launch — a stray "black terminal" window that looks broken to
// any user. Standard Tauri 2 starter-template line that was dropped during
// T0.1.11 Phase 3 lib→bin conversion. `cfg_attr(not(debug_assertions), ...)`
// preserves the console for debug builds (so println / tracing is visible
// during dev) while hiding it for release/MSI distribution.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use vault_app::keychain::{
    bridge_or_init_master_key, derive_at_rest_key, derive_sqlcipher_passphrase,
    PRODUCTION_NAMESPACE, VAULT_ID,
};
use vault_app::{AppConfig, Application};
use vault_tauri::{
    dylib_filename_for_os, env_override_for, format_keychain_error_dialog,
    format_startup_failure_dialog,
};

/// Exit code for keychain provenance failures (ADR-040 + ADR-040 amendment;
/// retains the same numeric code that V0.1 used for VAULT_KEY config errors,
/// since wrapper scripts and CI keyed on `2 = config-class error` regardless
/// of the underlying provenance mechanism).
const EXIT_CONFIG_ERROR: i32 = 2;
/// Exit code for Application startup failures including ADR-020
/// ModelIntegrityFailed.
const EXIT_STARTUP_FAILURE: i32 = 1;

fn main() {
    tracing_subscriber::fmt::init();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            vault_tauri::commands::add_memory,
            vault_tauri::commands::search_memories,
            vault_tauri::commands::update_memory,
            vault_tauri::commands::delete_memory,
        ])
        .setup(|app| {
            // 1. Resolve libonnxruntime dylib path per ADR-019.
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

            // 2. Resolve bundled model + tokenizer paths per ADR-019/020.
            //    Phase 4b HIGH fix: ?-propagation → fatal-dialog routing for
            //    UX consistency with the surrounding setup() failure paths.
            let model_path = match resolve_model_path(app.handle()) {
                Ok(p) => p,
                Err(e) => show_fatal_dialog_and_exit(
                    app.handle(),
                    "Memory Vault — Model Resource Resolution Failed",
                    &format!(
                        "Could not locate model.onnx.\n\nDetails: {e}\n\n\
                         For dev runs, set VAULT_MODEL_PATH. For installed builds, reinstall."
                    ),
                    EXIT_STARTUP_FAILURE,
                ),
            };
            let tokenizer_path = match resolve_tokenizer_path(app.handle()) {
                Ok(p) => p,
                Err(e) => show_fatal_dialog_and_exit(
                    app.handle(),
                    "Memory Vault — Tokenizer Resource Resolution Failed",
                    &format!(
                        "Could not locate tokenizer.json.\n\nDetails: {e}\n\n\
                         For dev runs, set VAULT_TOKENIZER_PATH. For installed builds, reinstall."
                    ),
                    EXIT_STARTUP_FAILURE,
                ),
            };

            // 3. Per-user data directory.
            let data_dir = match app.path().app_data_dir() {
                Ok(p) => p,
                Err(e) => show_fatal_dialog_and_exit(
                    app.handle(),
                    "Memory Vault — Data Directory Unavailable",
                    &format!("Could not locate per-user data directory.\n\nDetails: {e}"),
                    EXIT_STARTUP_FAILURE,
                ),
            };
            if let Err(e) = std::fs::create_dir_all(&data_dir) {
                show_fatal_dialog_and_exit(
                    app.handle(),
                    "Memory Vault — Data Directory Creation Failed",
                    &format!(
                        "Could not create per-user data directory at {}.\n\nDetails: {e}",
                        data_dir.display()
                    ),
                    EXIT_STARTUP_FAILURE,
                );
            }
            let metadata_path = data_dir.join("vault.db");
            let vector_dir = data_dir.join("lance");
            let graph_path = data_dir.join("graph.duckdb");

            // 4. Source master_key per ADR-040 + ADR-041. The bridge
            //    composes ADR-040's keychain logic with the V0.1 → V0.2
            //    SQLCipher passphrase bridge (ADR-041 plan iteration 2):
            //    - Keychain entry present → return existing master_key
            //      (V0.2 second-launch path; identical to read_or_init).
            //    - Keychain absent + no V0.1 vault.db → fresh-init via
            //      read_or_init's first-run path.
            //    - Keychain absent + V0.1 vault.db present → V0.1 bridge:
            //      verify VAULT_KEY env var unlocks vault.db → generate
            //      new master_key → keychain write FIRST → snapshot
            //      vault.db → PRAGMA rekey to new keychain-derived
            //      passphrase → close+reopen+verify (post-write
            //      verification invariant per ADR-041 §10) → cleanup
            //      snapshot. Fail-closed with rollback at any step.
            //
            //    This step needs `data_dir` (to detect V0.1 vault.db)
            //    so it runs AFTER step 3 (data_dir resolution), unlike
            //    pre-ADR-041 ordering where keychain was step 1. Step
            //    renumbering 1-4 reflects the new ordering.
            //
            //    The master_key is then split into two domain-separated
            //    BLAKE3 subkeys per ADR-040 amendment option β:
            //    - `sqlcipher_passphrase` (hex-encoded → SqlCipherKey)
            //    - `at_rest_key` (32 bytes → AppConfig.at_rest_key,
            //      consumed by Application::new's
            //      StorageBackend::open_with_at_rest_key per Phase 2).
            //
            //    Replaces the V0.1 VAULT_KEY env var path (ADR-032
            //    retired in Phase 1; ADR-041 bridges existing V0.1
            //    vaults forward).
            //    VAULT_KEY env var is read here (once, at the call site)
            //    rather than inside the bridge so the bridge stays a pure
            //    fn of its inputs — avoids hidden env dependency + keeps
            //    tests pure. Empty string treated as unset (matches the
            //    bridge's Some-with-non-empty-content discipline).
            let vault_key_env = std::env::var("VAULT_KEY").ok();
            let v0_1_vault_key = vault_key_env.as_deref().filter(|s| !s.is_empty());
            let master_key = match bridge_or_init_master_key(
                &data_dir,
                PRODUCTION_NAMESPACE,
                VAULT_ID,
                v0_1_vault_key,
            ) {
                Ok(k) => k,
                Err(err) => {
                    show_fatal_dialog_and_exit(
                        app.handle(),
                        "Memory Vault — Keychain Access Failed",
                        &format_keychain_error_dialog(&err),
                        EXIT_CONFIG_ERROR,
                    );
                }
            };
            let key = derive_sqlcipher_passphrase(&master_key);
            let at_rest_key = derive_at_rest_key(&master_key);
            // master_key drops here — the two derived subkeys carry the
            // keying material forward; Zeroizing wipes the master_key
            // bytes on Drop per BRD §11.5.3.
            drop(master_key);

            // 5. Build AppConfig from resolved paths + the derived subkeys.
            let config = AppConfig {
                metadata_path,
                vector_dir,
                graph_path,
                key,
                model_path,
                tokenizer_path,
                ort_lib_path,
                at_rest_key,
            };

            // 6. Construct Application and spawn the cascading retry
            //    worker. Per ADR-034 (T0.1.11 Phase 5b): V0.1 vault-tauri
            //    is UI-only — no MCP server bound inside the Tauri
            //    process. `start_with_mcp` would call rmcp's
            //    `ServiceExt::serve(server, stdio()).await` which blocks
            //    on JSON-RPC `initialize` from a peer that doesn't exist
            //    when launched as a Tauri UI app, hanging Tauri's setup()
            //    hook indefinitely. `start()` spawns only the retry
            //    worker (no rmcp transport bind), keeping the UI
            //    responsive. AI-client MCP integration deferred to V0.2
            //    alpha-distribution task (subcommand-split design per
            //    ADR-034 cross-link).
            //
            //    `start()` is sync but spawns `tokio::spawn(worker.run)`
            //    which requires a tokio runtime in scope. Tauri provides
            //    one inside `tauri::async_runtime::block_on`, which we
            //    enter just to construct Application::new (async) and
            //    call start() within the runtime context.
            let app_handle = app.handle().clone();
            let (application, _shutdown_sender) = tauri::async_runtime::block_on(async move {
                let application = match Application::new(&config).await {
                    Ok(a) => a,
                    Err(e) => show_fatal_dialog_and_exit(
                        &app_handle,
                        "Memory Vault — Fatal Error",
                        &format_startup_failure_dialog(&e),
                        EXIT_STARTUP_FAILURE,
                    ),
                };

                let shutdown_sender = application.start();
                (application, shutdown_sender)
            });

            // 7. Manage Application (for Tauri commands) + the worker
            //    shutdown Sender (held to keep the watch channel alive
            //    for the worker's lifetime; dropping it signals worker
            //    exit via the watch::changed() Err arm — which is fine
            //    on Tauri close, but holding it explicitly is the
            //    deliberate lifecycle).
            app.manage(application);
            app.manage(_shutdown_sender);

            Ok(())
        });

    // Phase 4b HIGH fix: tauri::Builder::run().expect(...) → match Result.
    // Tauri Builder failure means the dialog plugin may not be available,
    // so we use eprintln + exit (degraded path) rather than the dialog
    // routing the rest of setup() uses.
    if let Err(e) = builder.run(tauri::generate_context!()) {
        eprintln!("Memory Vault failed to start the Tauri runtime: {e}");
        std::process::exit(EXIT_STARTUP_FAILURE);
    }
}

/// Render a fatal dialog and terminate the process. **Diverges** —
/// the function never returns to the caller (`-> !`).
fn show_fatal_dialog_and_exit(
    app: &tauri::AppHandle,
    title: &str,
    body: &str,
    exit_code: i32,
) -> ! {
    app.dialog().message(body).title(title).blocking_show();
    std::process::exit(exit_code);
}

/// Resolve libonnxruntime dylib path per ADR-019. Dev-mode override via
/// `VAULT_ORT_LIB_PATH` env var (testable via `env_override_for`);
/// production falls through to `app.path().resolve(BaseDirectory::Resource)`.
fn resolve_ort_lib_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = env_override_for("VAULT_ORT_LIB_PATH") {
        return Ok(p);
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
    if let Some(p) = env_override_for("VAULT_MODEL_PATH") {
        return Ok(p);
    }
    app.path()
        .resolve("models/model.onnx", tauri::path::BaseDirectory::Resource)
        .map_err(|e| format!("resolve model.onnx: {e}"))
}

/// Resolve bundled tokenizer.json path. Dev-mode override via
/// `VAULT_TOKENIZER_PATH` env var; production falls through to
/// `app.path().resolve("models/tokenizer.json", BaseDirectory::Resource)`.
fn resolve_tokenizer_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Some(p) = env_override_for("VAULT_TOKENIZER_PATH") {
        return Ok(p);
    }
    app.path()
        .resolve(
            "models/tokenizer.json",
            tauri::path::BaseDirectory::Resource,
        )
        .map_err(|e| format!("resolve tokenizer.json: {e}"))
}
