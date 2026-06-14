//! Build script for `vault-storage`.
//!
//! ## Why this exists — DuckDB 1.4 + Windows Restart Manager link fix
//!
//! DuckDB 1.4 (libduckdb-sys 1.10503.1, bundled) added `AdditionalLockInfo`,
//! which — on Windows — calls the Restart Manager API (`RmStartSession`,
//! `RmEndSession`, `RmRegisterResources`, `RmGetList`) to report *which*
//! process is holding a lock on a database file when an open fails.
//!
//! Those symbols live in `rstrtmgr.lib`, but `libduckdb-sys`'s bundled-cc
//! build script (`build_bundled_cc.rs`) does NOT emit a
//! `cargo:rustc-link-lib` directive for it on MSVC. Any target that pulls the
//! `AdditionalLockInfo` object out of the static `duckdb.lib` archive then
//! fails to link with:
//!
//! ```text
//! error LNK2019: unresolved external symbol RmStartSession
//!   referenced in function "...duckdb::AdditionalLockInfo(...)"
//! fatal error LNK1120: 4 unresolved externals
//! ```
//!
//! The encrypted-`ATTACH` code path (graph-at-rest encryption) pulls that
//! object, so this directive is load-bearing for the encrypted graph store.
//! Emitting it here propagates the link requirement to every downstream
//! binary (`vault-cli`, `vault-tauri`) and to this crate's examples/tests.
//! It is a no-op for targets whose linker never pulls the object.
//!
//! TECH DEBT: remove this once `libduckdb-sys` emits the `rstrtmgr` link
//! directive itself for the bundled MSVC build (upstream omission).

fn main() {
    // Re-run only when this script itself changes (no source inputs).
    println!("cargo:rerun-if-changed=build.rs");

    // `CARGO_CFG_WINDOWS` is the cross-compilation-safe way to detect a
    // Windows *target* from a build script (mirrors libduckdb-sys's own
    // `win_target()` helper); plain `cfg!(windows)` would key off the host.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // `rustc-link-lib` applies to this crate's *library* target and
        // propagates to DEPENDENT crates' final links (vault-cli,
        // vault-tauri) — so their binaries link rstrtmgr when their code
        // paths pull `AdditionalLockInfo`.
        println!("cargo:rustc-link-lib=dylib=rstrtmgr");

        // `rustc-link-lib` is NOT re-applied to this package's own example
        // / test / bench targets (it targets the lib only). The encrypted-
        // graph SECURITY TESTS + spike examples live in vault-storage and
        // link rstrtmgr only via this `rustc-link-arg`, which Cargo applies
        // to this package's binaries, examples, tests, and benches.
        println!("cargo:rustc-link-arg=rstrtmgr.lib");
    }
}
