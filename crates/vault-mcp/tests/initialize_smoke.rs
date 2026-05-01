//! `rmcp 1.5.0` API-surface smoke test (T0.1.9 Phase 1, runtime-confirmation
//! per the spike-methodology rule).
//!
//! ## Phase 1 scope (this file)
//!
//! Plan §2 / §7 step 8 specified "boot `StdioServer`, send JSON-RPC
//! `initialize`, assert response shape" to surface rmcp 1.5.0 API drift
//! between web research (Spike 1) and runtime. **Phase 1 lands the
//! API-surface variant** of that smoke: imports the rmcp types we
//! depend on (`ServiceExt::serve`, `transport::stdio`,
//! `IntoTransport`-via-`tokio::io::duplex`) and verifies they compile +
//! resolve. The compile step IS the API-drift surface — if rmcp 1.5.0
//! renamed `transport::stdio()` or changed `ServiceExt::serve`'s
//! signature, the build fails loudly the same way the ort↔ONNX
//! Runtime version coupling surfaced at T0.1.7 Phase 1.
//!
//! ## Phase 2 scope (deferred)
//!
//! The full JSON-RPC `initialize` round-trip lands in Phase 2 when
//! `#[tool_router(server_handler)]` macros are wired on `StdioServer`
//! alongside the real adapter bodies. The macro wiring requires
//! deciphering rmcp 1.5.0's macro contract (`Parameters<T>` wrapper,
//! return-type mapping, schemars-or-not for typed params), which is
//! non-trivial without good docs and is best done alongside the
//! handler bodies it serves.
//!
//! ## What this test pins
//!
//! 1. `rmcp::transport::stdio` exists and is callable.
//! 2. `rmcp::ServiceExt` is the trait carrying `serve()`.
//! 3. `tokio::io::duplex()` produces stream halves usable as rmcp
//!    transports (via `IntoTransport`-for-`AsyncRead+AsyncWrite`).
//! 4. The Phase 2 "boot a server over a duplex" pattern is mechanically
//!    feasible with rmcp 1.5.0.

mod common;

use common::make_test_server;

// =============================================================================
// 1. rmcp imports compile — proves the API surface we depend on exists
// =============================================================================

/// Static API-surface check: every import path the Phase 2 wiring will
/// use is resolvable in rmcp 1.5.0. This compiles only if rmcp's API
/// hasn't drifted from Spike 1's reading. Each import is referenced
/// (via `let _ = <name>;` for functions, or `use` aliases for traits)
/// to prevent dead-import elision.
#[allow(dead_code, unused_imports)]
fn _rmcp_api_surface_imports_compile() {
    // `ServiceExt` is the extension trait that carries `serve(transport)`.
    // It's NOT dyn-compatible (it has generic methods), so we just
    // reference the path — the `use` alone proves the trait exists.
    use rmcp::ServiceExt as _;
    // Stdio transport — server-side, gated by the `transport-io` feature.
    use rmcp::transport::stdio;
    let _stdio_fn = stdio;
}

// =============================================================================
// 2. tokio::io::duplex pair works as rmcp transport input shape
// =============================================================================

/// Construct a duplex pair and verify both halves implement
/// `AsyncRead + AsyncWrite + Send + 'static` — the bound rmcp's
/// `IntoTransport` blanket impl requires for `(R, W)` and for unified
/// AsyncRead+AsyncWrite types per docs.rs/rmcp/1.5.0/transport.
#[tokio::test]
async fn duplex_pair_satisfies_rmcp_transport_bounds() {
    use tokio::io::{AsyncRead, AsyncWrite};

    let (client, server) = tokio::io::duplex(64 * 1024);

    fn assert_async_read_write<T: AsyncRead + AsyncWrite + Send + 'static>(_: &T) {}
    assert_async_read_write(&client);
    assert_async_read_write(&server);
}

// =============================================================================
// 3. StdioServer constructs cleanly with the trusted-slice contract
// =============================================================================

/// Phase 1 boot smoke: `StdioServer::new` accepts a stub adapter +
/// trusted-boundary slice and returns a `Clone`-able server. Phase 2
/// will replace this assertion with a real `serve(duplex_half).await`
/// that drives the JSON-RPC `initialize` handshake.
#[tokio::test]
async fn stdio_server_constructs_with_trusted_boundary_slice() {
    let server = make_test_server(vec!["work", "personal"]);
    // The trusted slice survives construction unchanged. This is the
    // load-bearing precondition for ADR-025 — every tool dispatch reads
    // from this slice, NEVER from request data.
    assert_eq!(server.authorized_boundaries().len(), 2);
    assert_eq!(server.authorized_boundaries()[0].as_str(), "work");
    assert_eq!(server.authorized_boundaries()[1].as_str(), "personal");
}

// =============================================================================
// 4. The trusted-slice contract holds across an empty-slice boot
// =============================================================================

/// Empty trusted slice is a legitimate construction path (e.g. a vault
/// session with no boundaries unlocked yet). Tool dispatches against an
/// empty slice return empty result on search and `AccessDenied` on
/// write/update/delete — Phase 2 wires this end-to-end; Phase 1 just
/// verifies the empty slice doesn't panic at construction.
#[test]
fn empty_trusted_slice_is_valid_construction() {
    let server = make_test_server(vec![]);
    assert_eq!(server.authorized_boundaries().len(), 0);
}
