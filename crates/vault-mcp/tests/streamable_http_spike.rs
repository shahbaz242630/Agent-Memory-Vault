//! Spike (compile-and-run) — concurrent multi-agent access over rmcp
//! streamable-HTTP on localhost.
//!
//! ## Why this spike exists (locked next arc, founder 2026-06-21)
//!
//! V0.2 ships an MCP **stdio** server: each agent spawns its OWN
//! `vault-cli mcp serve` subprocess (MCP stdio is 1:1 — one client spawns
//! one server). Real users run several agents at once (Claude on the
//! frontend + Codex on the backend, or an always-on agent), and multiple
//! server processes hitting the same vault files have NO cross-process
//! coordination → failed opens / corruption ([[cross-agent-mcp-connection]]).
//! The locked fix is the database-server pattern: ONE local daemon owns the
//! vault and every agent connects to it over a multi-client transport
//! (streamable-HTTP/SSE on localhost) instead of stdio-per-agent.
//!
//! ## What this spike PROVES (Stage A — transport + concurrent dispatch)
//!
//! 1. rmcp 1.5.0's `StreamableHttpService` stands up on a localhost TCP
//!    port, hosted by `hyper-util` (already in our tree — no axum needed).
//! 2. TWO independent rmcp HTTP clients connect to the ONE running daemon
//!    process CONCURRENTLY and both complete the MCP `initialize`
//!    handshake — the thing stdio structurally cannot do.
//! 3. Both clients issue tool calls (write + read) concurrently and all
//!    succeed; the shared `Arc<MockAdapter>` records BOTH clients' calls,
//!    proving every agent funnels through the ONE backend instance — the
//!    single serialization gate the real daemon will rely on.
//! 4. The default `StreamableHttpServerConfig` enforces loopback-only
//!    `allowed_hosts` at runtime: a request with a spoofed `Host` header is
//!    rejected `403` BEFORE any MCP dispatch (the DNS-rebinding guard that
//!    backs our localhost-only security posture).
//!
//! ## What this spike does NOT prove (deferred — see HANDOFF.md §1)
//!
//! - Real three-store (SQLite + LanceDB + DuckDB) concurrency safety under
//!   simultaneous WRITERS — that needs the real adapter wired at the
//!   vault-app/vault-cli layer (Stage B). `MockAdapter` records calls but
//!   never touches the stores.
//! - Per-agent auth tokens + per-connection boundary scoping (BRD §11.4.4)
//!   — the production-daemon step, behind a policy + ADR-SEC decision.
//! - Daemon lifecycle: single-instance guard, graceful shutdown.
//!
//! This file is throwaway executable-docs for the spike; the production
//! daemon lands behind an architecture + ADR-SEC decision (BRD §11 re-read)
//! per the handoff.

mod common;

use std::sync::Arc;

use common::{make_mock_server_with_adapter, MockAdapter};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{
    StreamableHttpClientTransport, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServiceExt;
use tokio::net::TcpListener;
use vault_mcp::StdioServer;

/// Stand up the vault MCP handler as a streamable-HTTP daemon on a loopback
/// ephemeral port and return its address. The accept loop runs in a detached
/// task for the lifetime of the test runtime.
async fn spawn_localhost_daemon(server: StdioServer) -> std::net::SocketAddr {
    // Per-session factory: each inbound connection gets its OWN handler clone,
    // but every clone shares the SAME inner `Arc<dyn Adapter>` (StdioServer's
    // Clone is an Arc-clone of the adapter) — so ALL agents funnel through one
    // backend, which is the daemon model's whole point. `StreamableHttpService`
    // is the rmcp `tower::Service`.
    let service = StreamableHttpService::new(
        move || Ok::<_, std::io::Error>(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback ephemeral port");
    let addr = listener.local_addr().expect("daemon local addr");

    tokio::spawn(async move {
        loop {
            let (stream, _peer) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let io = TokioIo::new(stream);
            let hyper_service = TowerToHyperService::new(service.clone());
            tokio::spawn(async move {
                // Per-connection errors (a client dropping mid-SSE) are normal
                // teardown, not a daemon fault — ignore them.
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(io, hyper_service)
                    .await;
            });
        }
    });

    addr
}

/// One `memory_write` tool-call into the `work` boundary.
/// `CallToolRequestParams` is `#[non_exhaustive]`, so we use the `::new`
/// constructor + set the public `arguments` field (a struct literal is
/// illegal cross-crate for a non-exhaustive type).
fn write_call(content: &str) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new("memory_write");
    params.arguments = serde_json::json!({
        "content": content,
        "boundary": "work",
    })
    .as_object()
    .cloned();
    params
}

/// One `memory_read` tool-call.
fn read_call(query: &str) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new("memory_read");
    params.arguments = serde_json::json!({ "query": query }).as_object().cloned();
    params
}

/// Stage-A core proof: two agents, one daemon, concurrent read + write, all
/// funneling through the single shared backend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_agents_one_daemon_concurrent_read_write() {
    // ONE daemon over ONE shared backend adapter. The `work`/`personal`
    // boundaries are the trusted slice; `memory_write` targets `work`.
    let (server, adapter): (StdioServer, Arc<MockAdapter>) =
        make_mock_server_with_adapter(vec!["work", "personal"]);
    let addr = spawn_localhost_daemon(server).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    // Each agent: connect (MCP initialize over HTTP) → write → read.
    let agent = |label: &'static str, fact: &'static str, question: &'static str, url: String| async move {
        let client = ()
            .serve(StreamableHttpClientTransport::from_uri(url))
            .await
            .unwrap_or_else(|e| panic!("{label} initialize handshake failed: {e}"));
        let write = client
            .peer()
            .call_tool(write_call(fact))
            .await
            .unwrap_or_else(|e| panic!("{label} memory_write failed: {e}"));
        let read = client
            .peer()
            .call_tool(read_call(question))
            .await
            .unwrap_or_else(|e| panic!("{label} memory_read failed: {e}"));
        // Client drops at scope end → closes its session; daemon stays up.
        (write, read)
    };

    // Both agents run AT ONCE against the same daemon URL — the concurrent
    // multi-client access stdio cannot do.
    let ((a_write, a_read), (b_write, b_read)) = tokio::join!(
        agent(
            "agent-A",
            "The user prefers dark mode in their editor.",
            "what editor theme does the user prefer",
            url.clone(),
        ),
        agent(
            "agent-B",
            "The user works in the Pacific timezone.",
            "what timezone is the user in",
            url.clone(),
        ),
    );

    // Every tool call returned a non-error CallToolResult.
    assert_ne!(a_write.is_error, Some(true), "agent A write should succeed");
    assert_ne!(b_write.is_error, Some(true), "agent B write should succeed");
    assert_ne!(a_read.is_error, Some(true), "agent A read should succeed");
    assert_ne!(b_read.is_error, Some(true), "agent B read should succeed");

    // The shared backend saw BOTH agents' writes AND reads → every agent
    // funneled through the ONE adapter instance (the single serialization
    // gate the real daemon serializes every store mutation behind).
    assert_eq!(
        adapter.write_calls().len(),
        2,
        "both agents' writes must reach the one shared backend, got {:?}",
        adapter.write_calls().len()
    );
    assert_eq!(
        adapter.read_calls().len(),
        2,
        "both agents' reads must reach the one shared backend, got {:?}",
        adapter.read_calls().len()
    );
}

/// Stage-A security proof: the default config's loopback-only `allowed_hosts`
/// rejects a spoofed `Host` header at runtime (DNS-rebinding guard), 403,
/// BEFORE any MCP dispatch reaches the vault. Hand-rolled HTTP/1.1 over a
/// std `TcpStream` so the negative path needs no extra HTTP-client dep.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spoofed_host_header_rejected_by_loopback_guard() {
    let (server, _adapter) = make_mock_server_with_adapter(vec!["work"]);
    let addr = spawn_localhost_daemon(server).await;

    let status_line = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Write};

        let mut stream = std::net::TcpStream::connect(addr).expect("connect loopback");
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .ok();

        let body = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        // `Host: evil.example.com` is NOT in the default allowed set
        // (localhost / 127.0.0.1 / ::1) → the guard must reject it.
        let head = format!(
            "POST /mcp HTTP/1.1\r\n\
             Host: evil.example.com\r\n\
             Content-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(head.as_bytes())
            .expect("write request head");
        stream.write_all(body).expect("write request body");
        stream.flush().ok();

        // The status line arrives in the first chunk; one read is enough.
        let mut buf = [0u8; 256];
        let n = stream.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n])
            .lines()
            .next()
            .unwrap_or_default()
            .to_string()
    })
    .await
    .expect("raw-socket task joins");

    assert!(
        status_line.contains("403"),
        "spoofed Host must be rejected 403 by the loopback DNS-rebinding guard; \
         got status line: {status_line:?}"
    );
}
