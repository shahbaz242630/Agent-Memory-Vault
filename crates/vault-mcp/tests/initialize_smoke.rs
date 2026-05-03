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

// =============================================================================
// 5. Phase 2 Step 9 — full JSON-RPC initialize round-trip + tools/list pin
// =============================================================================

/// Phase 2 close: drives the rmcp 1.5.0 server through a real JSON-RPC
/// `initialize` handshake via `tokio::io::duplex()`, then issues
/// `tools/list` and asserts the four-tool contract. This proves the
/// macro chain — `#[tool_router]` on `impl StdioServer` (populates the
/// `tool_router: ToolRouter<Self>` field) → 4× `#[tool]` decorators on
/// the `tool_search` / `tool_write` / `tool_update` / `tool_delete`
/// methods → `#[tool_handler]` on `impl ServerHandler for StdioServer`
/// (auto-routes `tools/list` and `tools/call`) — wires up correctly
/// end-to-end.
///
/// **Set comparison on tool names** (BTreeSet) — rmcp's emit ordering
/// is internal, NOT a public contract. Pinning order would couple this
/// test to rmcp internals (1.5.0 → 1.5.1 patch could reorder without
/// semantic change and break us). The contract this test pins is "the
/// 4 tools exist with these names."
///
/// **Narrow `ServerInfo` assertion shape** — only `server_info.name`
/// (the `vault-mcp` Implementation contract from `get_info()`) and the
/// presence of the `tools` capability. Server version is
/// `env!("CARGO_PKG_VERSION")` — pinning ties tests to the bump cycle.
/// Protocol version is rmcp's choice. Instructions text is free-form.
/// All three are deliberately NOT asserted.
#[tokio::test]
async fn full_initialize_round_trip_lists_four_tools_with_expected_names() {
    use std::collections::BTreeSet;

    use rmcp::ServiceExt;

    let (client_io, server_io) = tokio::io::duplex(64 * 1024);

    // Empty trusted slice + StubAdapter is correct here — `tools/list`
    // never invokes the adapter's CRUD methods, only the macro-routed
    // tool registry.
    let server = make_test_server(vec![]);

    // We `await` the spawned server's JoinHandle below (after client
    // drop) so server-side panics — e.g. macro-chain regression,
    // get_info crash, ServerHandler routing bug — surface as hard
    // test failures with diagnostic value. Do NOT "simplify" the spawn
    // body to a fire-and-forget `let _ = server.serve(server_io).await`
    // — that swallows panics silently. The closure normalises to `()`
    // because we only care about JoinError (panic) at the outer await;
    // benign serve-startup or waiting-side errors get dropped here
    // because they would already have surfaced as client-side failures
    // earlier in the test (failed handshake / failed list_tools).
    let server_handle = tokio::spawn(async move {
        if let Ok(running) = server.serve(server_io).await {
            let _ = running.waiting().await;
        }
    });

    // Client side: `()` is a no-op `ClientHandler` (rmcp 1.5.0
    // `handler/client.rs:263 — impl ClientHandler for ()`).
    // `.serve(...).await` runs the initialize handshake; returning Ok
    // proves the handshake completed.
    let client = ().serve(client_io).await.expect("initialize handshake completes");

    // ServerInfo (= InitializeResult) arrives during initialize and is
    // stored on the peer. Narrow assertion: pin only what the public
    // contract actually requires.
    let server_info = client
        .peer_info()
        .expect("server_info populated post-initialize");
    assert_eq!(
        server_info.server_info.name, "vault-mcp",
        "ServerInfo.name pins the get_info() Implementation contract"
    );
    assert!(
        server_info.capabilities.tools.is_some(),
        "tools capability must be advertised"
    );

    // tools/list — exercises `#[tool_handler]` auto-routing through
    // the `tool_router` field that `#[tool_router]` populates from
    // the four `#[tool]` decorators in `server.rs`. End-to-end macro
    // chain verification.
    let listed = client
        .peer()
        .list_tools(Default::default())
        .await
        .expect("list_tools succeeds");

    assert_eq!(listed.tools.len(), 4, "expected exactly 4 tools advertised");

    let names: BTreeSet<&str> = listed.tools.iter().map(|t| t.name.as_ref()).collect();
    let expected: BTreeSet<&str> = [
        "memory.search",
        "memory.write",
        "memory.update",
        "memory.delete",
    ]
    .into_iter()
    .collect();
    assert_eq!(
        names, expected,
        "tool names must match the 4-tool contract — set comparison so emit order is not pinned"
    );

    // Drop the client to close the duplex; the spawned server task
    // exits on EOF.
    drop(client);

    // Server-side panic surfaces here as a `JoinError` (hard test
    // failure with diagnostic value). Clean exits give `Ok(())`. This
    // is the lower-fidelity-but-robust shape: we don't try to match
    // rmcp/tokio error-text strings (those aren't a stable contract),
    // we just guarantee that a server panic doesn't get silently
    // swallowed.
    if let Err(join_err) = server_handle.await {
        panic!("server task panicked: {join_err}");
    }
}
