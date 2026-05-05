// vault-tauri build script. Per Tauri 2.x convention this calls
// tauri_build::build() which processes tauri.conf.json + capabilities/*.json
// and emits the generated context that tauri::generate_context!() consumes
// at compile time.
//
// Per ADR-003: this file lands at T0.1.11 Phase 3 alongside the lib→bin
// conversion. Without it, tauri::generate_context!() fails to resolve.

fn main() {
    tauri_build::build();
}
