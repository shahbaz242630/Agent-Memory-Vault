// Forced-include (/FI) shim for the GitHub Actions windows-2025 runner image
// after its 2026-06-08→06-15 migration to Visual Studio 2026 (MSVC 14.51).
//
// WHAT BROKE: VS 2026 REMOVED `stdext::checked_array_iterator` from the MSVC
// standard library headers entirely (a long-deprecated non-Standard extension),
// but the toolchain still DEFINES the legacy `_SECURE_SCL` macro. DuckDB's
// bundled `fmt` (~v5.x, vendored inside libduckdb-sys) guards its use of that
// removed type with a bare `#ifdef _SECURE_SCL`:
//
//     #ifdef _SECURE_SCL
//     template <typename T> using checked_ptr = stdext::checked_array_iterator<T*>;
//     ...
//     #else
//     template <typename T> using checked_ptr = T*;   // <- what Linux/macOS use
//     #endif
//
// Because `_SECURE_SCL` is still defined, the bundled C++ build takes the first
// branch and fails to compile with `error C2061: syntax error: identifier
// 'checked_array_iterator'`. The `_SILENCE_STDEXT_ARR_ITERS_DEPRECATION_WARNING`
// macro does NOT help — the type is gone, not merely deprecated.
//
// THE FIX: include the STL core header that sets `_SECURE_SCL` (it has an
// include guard, so it is only ever processed once), then `#undef` the macro.
// Every later `#include <yvals.h>` is a no-op (guard already set), so the macro
// stays undefined for the rest of the translation unit and fmt falls back to its
// raw-pointer branch — the exact code path Linux and macOS already compile, so
// it is known-good. Release `/MD` builds do not want checked iterators anyway.
//
// SCOPE: wired only via `CXXFLAGS_x86_64_pc_windows_msvc` in `.github/workflows/
// ci.yml`, which cc-rs (libduckdb-sys's bundled build) reads but CMake (the
// llama-cpp-sys-2 Vulkan build) does not — so this touches nothing but the
// DuckDB compile. No-op on non-MSVC toolchains. Remove once libduckdb-sys
// vendors a newer fmt or the bundled fmt drops the `stdext` usage.
#if defined(_MSC_VER)
#include <yvals.h>
#ifdef _SECURE_SCL
#undef _SECURE_SCL
#endif
#endif
