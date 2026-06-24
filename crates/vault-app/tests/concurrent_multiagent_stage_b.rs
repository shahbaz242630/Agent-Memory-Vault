//! Stage B spike — REAL three-store concurrency under two agents over the
//! localhost daemon.
//!
//! ## macOS deferral (ADR-033) — same as `integration_smoke.rs`
//!
//! Disabled on macOS via the file-level `cfg` below: this binary loads the
//! real `BgeSmallProvider` (`libonnxruntime.dylib`) and would hit the same
//! ORT static-destructor SIGABRT at process exit. See `integration_smoke.rs`.

#![cfg(not(target_os = "macos"))]
//!
//! ## What Stage A proved (in `vault-mcp/tests/streamable_http_spike.rs`)
//!
//! Two rmcp HTTP clients connect to ONE localhost daemon concurrently and both
//! read/write — but against a MOCK adapter that never touches real storage. So
//! Stage A proved the **transport + concurrent dispatch**, not data safety.
//!
//! ## What Stage B proves (this file) — the real-store safety gap
//!
//! The handoff flagged the corruption risk precisely: SQLite metadata is
//! WAL/multi-process-tolerant, but **LanceDB** vectors use optimistic
//! concurrency (corruption risk under concurrent writers) and **DuckDB** graph
//! takes a single-process exclusive lock. The daemon fix is "ONE process owns
//! all three stores; every write serializes through the existing
//! `Mutex<Connection>` gate." This test is the empirical proof of THAT claim:
//!
//! - Stand up the localhost daemon over the **real** `VaultAdapter`
//!   (`Application::new` composes real BGE + SQLCipher + LanceDB + DuckDB).
//! - Two agents (separate rmcp HTTP clients) each write a batch of distinct
//!   facts CONCURRENTLY through the one daemon → the one adapter → the one gate.
//! - Assert the stores survived: (1) the BLAKE3 **audit chain is intact**
//!   across the interleaved multi-connection appends — the canonical
//!   "did concurrency corrupt SQLite" check (BRD §11.9.2); (2) **every** write
//!   persisted (no lost writes) — `list_memories` count equals the total
//!   written; (3) the writes **cascaded to the vector store** and are
//!   retrievable through the real retriever (proves LanceDB wasn't corrupted).
//!
//! ## Not covered here (deferred to the daemon build / hardening)
//!
//! - Daemon lifecycle (single-instance guard, graceful shutdown).
//! - Per-agent auth tokens + per-connection boundary scoping (BRD §11.4.4).
//! - Adversarial interleaving fuzz / sustained-load soak.
//!
//! Throwaway executable-docs for the spike; production daemon lands behind the
//! architecture + ADR-SEC decision (BRD §11 re-read) per the handoff.
//!
//! ## Running
//!
//! ```text
//! cargo test -p vault-app --test concurrent_multiagent_stage_b -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`-by-default: needs the bge-small-en-v1.5 fixtures at
//! `crates/vault-embedding/test-fixtures/` and is heavy (~15s, real BGE).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{
    StreamableHttpClientTransport, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::ServiceExt;
use tempfile::TempDir;
use tokio::net::TcpListener;

use vault_app::{AppConfig, Application};
use vault_core::Boundary;
use vault_mcp::{Adapter, StdioServer};
use vault_retrieval::{RetrievalOptions, RetrievalQuery};
use vault_storage::{MemoryFilter, MetadataStore, SqlCipherKey};

// ----------------------------------------------------------------------
// Fixture path resolution (mirrors integration_smoke.rs)
// ----------------------------------------------------------------------

const FIXTURE_REL: &str = "../vault-embedding/test-fixtures/bge-small-en-v1.5";

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(FIXTURE_REL);
    p
}

fn require_fixture(name: &str) -> PathBuf {
    let p = fixture_root().join(name);
    assert!(
        p.exists(),
        "missing test fixture {p:?} - run scripts/setup-dev-env.sh \
         (or .ps1 on Windows) first"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib_path() -> PathBuf {
    require_fixture("onnxruntime.dll")
}

#[cfg(target_os = "linux")]
fn ort_lib_path() -> PathBuf {
    require_fixture("libonnxruntime.so")
}

// ----------------------------------------------------------------------
// Daemon over the real adapter (mirrors the Stage A spawn helper)
// ----------------------------------------------------------------------

/// Mount `server` as a streamable-HTTP daemon on a loopback ephemeral port.
/// The per-session factory clones the handler, but every clone shares the same
/// inner `Arc<dyn Adapter>` — so all agents funnel through the ONE real backend.
async fn spawn_localhost_daemon(server: StdioServer) -> std::net::SocketAddr {
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
                let _ = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(io, hyper_service)
                    .await;
            });
        }
    });

    addr
}

/// A `memory_write` call into `boundary`. `CallToolRequestParams` is
/// `#[non_exhaustive]` → construct via `::new` + set the public `arguments`.
fn write_call(content: &str, boundary: &str) -> CallToolRequestParams {
    let mut params = CallToolRequestParams::new("memory_write");
    params.arguments = serde_json::json!({
        "content": content,
        "boundary": boundary,
    })
    .as_object()
    .cloned();
    params
}

// ----------------------------------------------------------------------
// The Stage B proof
// ----------------------------------------------------------------------

/// Two agents writing distinct facts concurrently through one daemon leave the
/// real three stores intact: audit chain holds, no writes lost, all retrievable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real-model spike: needs BGE fixtures, heavy (~15s). Run with -- --ignored"]
async fn two_agents_concurrent_writes_through_daemon_keep_stores_intact() {
    const PER_AGENT: usize = 5;
    const TOTAL: usize = PER_AGENT * 2;

    // --- Real vault: full dep graph over SQLCipher + LanceDB + DuckDB ---
    let tmp = TempDir::new().expect("tempdir");
    let metadata_path = tmp.path().join("vault.db");
    let key = SqlCipherKey::new("stage-b-concurrency-key");

    let config = AppConfig {
        metadata_path: metadata_path.clone(),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: key.clone(),
        model_path: require_fixture("model.onnx"),
        tokenizer_path: require_fixture("tokenizer.json"),
        ort_lib_path: ort_lib_path(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        // No GGUF/reranker in the spike — read/consolidation paths stay unwired;
        // we only exercise the write+search+audit paths, which need none of them.
        qwen_model_path: None,
        phi4_model_path: None,
        rerank_model_path: None,
        rerank_tokenizer_path: None,
    };

    let application = Application::new(&config)
        .await
        .expect("Application::new MUST compose the real dep graph");
    // Spawns the cascading retry worker (drains SQLite retry_queue → LanceDB).
    // Held to the end of the test so the worker lives long enough to drain.
    let _shutdown = application.start();

    // Independent handle for read-back assertions (a separate SQLCipher conn).
    let metadata_for_assert = MetadataStore::open(&metadata_path, key.clone())
        .await
        .expect("open assert handle");

    // --- Daemon over the REAL adapter ---
    let dyn_adapter: Arc<dyn Adapter> = application.adapter().clone();
    let boundaries = vec![
        Boundary::new("work").expect("valid boundary"),
        Boundary::new("personal").expect("valid boundary"),
    ];
    let server = StdioServer::new(dyn_adapter, boundaries);
    let addr = spawn_localhost_daemon(server).await;
    let url = format!("http://127.0.0.1:{}/mcp", addr.port());

    // --- Two agents writing distinct facts CONCURRENTLY through the daemon ---
    let agent = |label: &'static str, brand: &'static str, url: String| async move {
        let client = ()
            .serve(StreamableHttpClientTransport::from_uri(url))
            .await
            .unwrap_or_else(|e| panic!("{label} initialize handshake failed: {e}"));
        for i in 0..PER_AGENT {
            let content = format!("The user owns a {brand} device model number {i} for testing.");
            let res = client
                .peer()
                .call_tool(write_call(&content, "work"))
                .await
                .unwrap_or_else(|e| panic!("{label} memory_write {i} failed: {e}"));
            assert_ne!(
                res.is_error,
                Some(true),
                "{label} write {i} returned an MCP error result"
            );
        }
    };

    tokio::join!(
        agent("agent-A", "alpha", url.clone()),
        agent("agent-B", "bravo", url.clone()),
    );

    // Let the cascade worker drain queued writes to the vector store before the
    // retrievability assertion (mirrors integration_smoke's wait_for_cascade_drain).
    tokio::time::sleep(Duration::from_secs(4)).await;

    // --- Assertion 1: SQLite audit chain intact across concurrent appends ---
    // Every write appended a MemoryCreate row AND the daemon appended a
    // McpToolInvoke row, interleaved across connections. If concurrency broke
    // the SELECT MAX(seq)+INSERT serialization, the BLAKE3 prev_hash links
    // break here. This is THE "did concurrency corrupt the metadata store" check.
    metadata_for_assert
        .verify_audit_chain()
        .await
        .expect("BRD §11.9.2: audit chain MUST hold across concurrent multi-agent writes");

    // --- Assertion 2: no lost writes — every fact persisted to metadata ---
    let stored = metadata_for_assert
        .list_memories(MemoryFilter::default(), None)
        .await
        .expect("list_memories succeeds");
    assert_eq!(
        stored.len(),
        TOTAL,
        "all {TOTAL} concurrent writes must persist (no lost writes); got {}",
        stored.len()
    );

    // --- Assertion 3: writes cascaded to LanceDB and are retrievable ---
    // A non-empty search per agent proves the vector store accepted the
    // concurrent upserts without corruption (a corrupted index errors or
    // returns nothing for a clearly-present term).
    let adapter = application.adapter();
    for brand in ["alpha", "bravo"] {
        let query = RetrievalQuery {
            query_text: format!("{brand} device for testing"),
            authorized_boundaries: vec![Boundary::new("work").expect("valid boundary")],
            max_results: 10,
            options: RetrievalOptions::default(),
        };
        let hits = adapter
            .search(query)
            .await
            .unwrap_or_else(|e| panic!("search for {brand} failed: {e}"));
        assert!(
            !hits.is_empty(),
            "writes for {brand:?} must be retrievable through the real retriever \
             after the concurrent run (proves the LanceDB cascade survived)"
        );
    }
}
