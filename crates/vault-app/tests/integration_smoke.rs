//! T0.1.10 Phase 1 integration-risk spike — **compile-and-run methodology**
//! per session-open Decision 3 (HANDOFF.md, 2026-05-04).
//!
//! Wires the full V0.1 dependency graph end-to-end against real LanceDB,
//! SQLCipher, and ort. Each test exercises ONE of the four pre-declared
//! stop-and-escalate triggers:
//!
//! - **(a)** contract mismatch — *runtime panic OR trait-bound compile failure
//!   when the composed dep graph is first instantiated.*
//! - **(b)** audit-chain hash discontinuity — *end-to-end `verify_audit_chain()`
//!   call after the spike's write+retrieve sequence (BRD §11.9.2).*
//! - **(c)** `spawn_blocking` deadlock or starvation — *wall-clock-vs-baseline
//!   divergence on the spike's perf instrumentation.*
//! - **(d)** boundary-validation gap — *deliberate cross-boundary write+retrieve
//!   probe (BRD §11.4.3 mandatory access control).*
//!
//! All tests are `#[ignore]`-by-default. Phase 2 close promotes to non-
//! ignored once stable on 5 consecutive runs (per session-open Decision 3
//! promotion gate).
//!
//! ## Running
//!
//! ```text
//! cargo test -p vault-app --test integration_smoke -- --ignored --nocapture
//! ```
//!
//! Note: `--ignored` and `--nocapture` are both test-runner flags so they
//! go AFTER the `--` separator, not before it. Cargo rejects `--ignored`
//! as a top-level flag.
//!
//! Requires the bge-small-en-v1.5 fixtures at
//! `crates/vault-embedding/test-fixtures/bge-small-en-v1.5/`. Run
//! `scripts/setup-dev-env.{sh,ps1}` once per checkout to provision them.
//! Tests panic loudly on missing fixtures — never silently skip.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tempfile::TempDir;

use vault_app::{Application, VaultAdapter};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_mcp::{Adapter, ToolInvokeDetails};
use vault_retrieval::{RetrievalOptions, RetrievalQuery};
use vault_storage::{MetadataStore, SqlCipherKey};

// ----------------------------------------------------------------------
// Fixture path resolution
// ----------------------------------------------------------------------
//
// vault-embedding's test fixtures live at
// `crates/vault-embedding/test-fixtures/bge-small-en-v1.5/`. From
// vault-app's CARGO_MANIFEST_DIR (= `crates/vault-app`) they're at
// `../vault-embedding/test-fixtures/bge-small-en-v1.5/`.

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
         (or .ps1 on Windows) first; see T0.1.7_PLAN.md test fixture section"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib_path() -> PathBuf {
    require_fixture("onnxruntime.dll")
}

#[cfg(target_os = "macos")]
fn ort_lib_path() -> PathBuf {
    require_fixture("libonnxruntime.dylib")
}

#[cfg(target_os = "linux")]
fn ort_lib_path() -> PathBuf {
    require_fixture("libonnxruntime.so")
}

// ----------------------------------------------------------------------
// Application setup helper
// ----------------------------------------------------------------------

struct TestApp {
    application: Application,
    /// Held to keep tempdir alive for test duration.
    _tmp: TempDir,
    /// Fourth `MetadataStore` handle (separate from Application's three)
    /// used for read-back assertions in trigger (b) and elsewhere.
    metadata_for_assert: MetadataStore,
}

async fn setup_application() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let metadata_path = tmp.path().join("vault.db");
    let vector_dir = tmp.path().join("lance");
    let graph_path = tmp.path().join("graph.duckdb");
    let key = SqlCipherKey::new("integration-smoke-test-key");

    let application = Application::new(
        &metadata_path,
        &vector_dir,
        &graph_path,
        key.clone(),
        &require_fixture("model.onnx"),
        &require_fixture("tokenizer.json"),
        &ort_lib_path(),
    )
    .await
    .expect(
        "Application::new MUST compose the dep graph successfully. If this fails, \
         trigger (a) has fired - stop, surface, root-cause investigate. \
         Do NOT paper over with #[cfg] skips.",
    );

    let metadata_for_assert = MetadataStore::open(&metadata_path, key)
        .await
        .expect("open assert handle");

    TestApp {
        application,
        _tmp: tmp,
        metadata_for_assert,
    }
}

fn make_new_memory(content: &str, boundary: &str) -> NewMemory {
    NewMemory {
        content: content.to_string(),
        memory_type: MemoryType::Semantic,
        boundary: Boundary::new(boundary).expect("valid boundary in test"),
        source_agent: Some("integration-smoke".to_string()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    }
}

fn make_query(text: &str, boundary: &str, max_results: usize) -> RetrievalQuery {
    RetrievalQuery {
        query_text: text.to_string(),
        authorized_boundaries: vec![Boundary::new(boundary).expect("valid boundary in test")],
        max_results,
        options: RetrievalOptions::default(),
    }
}

// ======================================================================
// Trigger (a) — contract mismatch
// ======================================================================

/// **Trigger (a) detection method:** runtime panic OR trait-bound compile
/// failure when the composed dep graph is first instantiated.
///
/// Pass = `Application::new` returns Ok and we can dispatch one method
/// through the `dyn Trait` chain (Adapter -> Retriever -> EmbeddingProvider
/// + VectorStore). Failure = compile error (won't reach runtime) or
/// `expect()` panic at `setup_application`.
#[tokio::test]
#[ignore]
async fn trigger_a_dep_graph_composes_without_contract_mismatch() {
    let app = setup_application().await;

    // Force one round-trip through every dyn-Trait boundary in the
    // composed chain: Adapter -> Retriever -> EmbeddingProvider +
    // VectorStore. Empty vault returns empty results; what we're
    // really pinning is that the dispatch chain is well-typed end-to-
    // end, including the trait-object vtables.
    let q = make_query("smoke probe", "work", 5);
    let results = app
        .application
        .adapter()
        .search(q)
        .await
        .expect("Adapter::search through composed chain MUST return Ok on empty vault");
    assert_eq!(
        results.len(),
        0,
        "empty vault must return empty results - trigger (a) variant: \
         retriever returned non-empty Vec for empty store"
    );
}

// ======================================================================
// Trigger (b) — audit-chain hash discontinuity
// ======================================================================

/// **Trigger (b) detection method:** end-to-end `verify_audit_chain()`
/// call after the spike's write+retrieve sequence.
///
/// Setup:
/// 1. `VaultAdapter::write` -> `StorageBackend::write_memory` appends a
///    `MemoryCreate` audit row via storage-internal `MetadataStore`
///    handle (handle #1).
/// 2. `VaultAdapter::append_tool_invoke_audit` appends a `McpToolInvoke`
///    audit row via the adapter's separate `MetadataStore` handle
///    (handle #2).
/// 3. `verify_audit_chain` reads the chain through the assert-handle
///    (handle #4) and validates BLAKE3 prev_hash links across BOTH rows.
///
/// If multi-connection writes interleave incorrectly (e.g., transaction
/// isolation fails to serialize the SELECT MAX(seq) ... INSERT pattern),
/// prev_hash links break. BRD §11.9.2 mandates consistency must hold
/// across composition as it does in isolation.
///
/// Pass = `verify_audit_chain` returns Ok AND the written memory is
/// retrievable through the composed Application chain.
#[tokio::test]
#[ignore]
async fn trigger_b_audit_chain_consistent_across_composition() {
    let app = setup_application().await;
    let adapter = app.application.adapter();

    // Write through VaultAdapter::write -> StorageBackend::write_memory.
    // This appends a `MemoryCreate` row via storage's internal
    // MetadataStore handle.
    let id = adapter
        .write(make_new_memory("trigger-b probe content", "work"))
        .await
        .expect("write through composed chain MUST succeed");

    // Append a tool-invoke audit row via the adapter - exercises the
    // SECOND MetadataStore handle.
    let details = ToolInvokeDetails {
        tool: "memory.search",
        duration_ms: 5,
        result_count: 0,
        boundary_count: 1,
        max_results: Some(10),
        score_threshold: None,
        include_archived: Some(false),
        query_length: Some(7),
        error: None,
    };
    adapter
        .append_tool_invoke_audit(details)
        .await
        .expect("append_tool_invoke_audit MUST succeed across separate connection");

    // Verify the audit chain end-to-end via the assert-handle (a fourth,
    // independent connection). BRD §11.9.2: BLAKE3 hash chain must hold;
    // first inconsistency surfaces as VaultError::Storage pinpointing
    // the breaking row.
    app.metadata_for_assert.verify_audit_chain().await.expect(
        "audit chain MUST be consistent across composition (BRD §11.9.2). \
         If this fails, the spike has surfaced an interleaving bug between \
         StorageBackend's internal MetadataStore handle (which appended \
         MemoryCreate) and VaultAdapter's append_tool_invoke_audit handle \
         (which appended McpToolInvoke). Stop, surface, do not paper over.",
    );

    // Also confirm the written memory is visible from the retriever path
    // through Application's wiring - proves StorageBackend writes are
    // visible to SemanticRetriever's read.
    let q = make_query("trigger-b probe content", "work", 10);
    let results = adapter
        .search(q)
        .await
        .expect("search through composed chain MUST succeed");
    assert!(
        results.iter().any(|r| r.memory.id == id),
        "memory written through VaultAdapter::write MUST be retrievable through \
         Adapter::search via SemanticRetriever - if not, trigger (b) variant: \
         storage write not visible to retriever read"
    );
}

// ======================================================================
// Trigger (c) — spawn_blocking deadlock or starvation
// ======================================================================

/// **Trigger (c) detection method:** wall-clock-vs-baseline divergence on
/// the spike's perf instrumentation.
///
/// Methodology choice (the alternative was `tokio-console`): wall-clock
/// measurement is single-binary and works in standard `cargo test` -
/// `tokio-console` requires the `tracing` feature on tokio + a separate
/// console process. For Phase 1's go/no-go signal, single-binary wall-
/// clock is sufficient.
///
/// Setup:
/// 1. Sequential baseline: 3 single writes back-to-back -> per-call time.
/// 2. Concurrent burst: 8 writes via `tokio::spawn`.
/// 3. Healthy: concurrent_elapsed within ~5x of (per_call * 8).
///    BgeSmallProvider serializes inference internally (Mutex<Session>)
///    so concurrent throughput approaches sequential; 5x headroom
///    accommodates spawn_blocking pool scheduling overhead.
/// 4. Pathological: deadlock or starvation -> concurrent_elapsed >> 5x.
///    The test fails fast if it reaches this threshold.
///
/// Pass = concurrent_elapsed < 5 * per_call_baseline * 8 AND every
/// spawned write returned Ok.
///
/// # Green definition for Phase 1's 5-consecutive-runs spike gate
///
/// Trigger (c) is the ONE trigger here that's statistically-shaped, not
/// binary. (a), (b), (d) are flake-immune by assertion structure: they
/// either hold or don't, deterministically. (c)'s wall-clock measurement
/// can superficially look like a flake while actually being signal.
///
/// **Pre-declared rule: ALL 5 runs must complete under the deadlock
/// threshold. ANY single run over threshold = stop-and-escalate
/// (intermittent deadlock signature), NO flake-retries on this trigger.**
/// An intermittent threshold breach is exactly the failure mode pre-flight
/// detection is supposed to catch — papering over it with retries defeats
/// the purpose.
///
/// # Escalation path if this trigger fires
///
/// `tokio-console` is the right diagnostic tool for the investigation
/// phase (NOT this test's wall-clock check, which is pre-flight detection
/// only). Add `console-subscriber` as a dev-dep, gate behind a cargo
/// feature, attach to a running instance of this test or a derived
/// long-form spike binary, and analyse blocking-call traces / task wake
/// latency / runtime-thread saturation. The wall-clock signal here is
/// purely "is there a deadlock pattern?"; `tokio-console` answers
/// "where is the deadlock?". Don't re-derive this escalation under
/// pressure when the trigger fires.
#[tokio::test]
#[ignore]
async fn trigger_c_spawn_blocking_no_deadlock_under_concurrent_writes() {
    let app = setup_application().await;
    let adapter: Arc<VaultAdapter> = app.application.adapter().clone();

    // Sequential baseline.
    let seq_start = Instant::now();
    for i in 0..3 {
        adapter
            .write(make_new_memory(&format!("seq-baseline-{i}"), "work"))
            .await
            .expect("baseline write MUST succeed");
    }
    let seq_elapsed = seq_start.elapsed();
    let per_call_baseline = seq_elapsed / 3;

    // Concurrent burst.
    let concurrent_start = Instant::now();
    let mut handles = Vec::with_capacity(8);
    for i in 0..8 {
        let a = adapter.clone();
        handles.push(tokio::spawn(async move {
            a.write(make_new_memory(&format!("concurrent-burst-{i}"), "work"))
                .await
        }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        h.await
            .expect("spawned task MUST not panic")
            .unwrap_or_else(|e| panic!("concurrent write {i} returned Err: {e:?}"));
    }
    let concurrent_elapsed = concurrent_start.elapsed();

    // Healthy threshold: concurrent throughput should be no worse than
    // 5x linear extrapolation of the sequential baseline. Mutex-
    // serialized inference puts the realistic upper bound at roughly
    // 1x baseline * 8 calls; 5x absorbs scheduler jitter without
    // permitting actual deadlock signatures.
    let deadlock_threshold = per_call_baseline * 8 * 5;
    assert!(
        concurrent_elapsed < deadlock_threshold,
        "spawn_blocking deadlock or starvation pattern detected: \
         concurrent_elapsed={concurrent_elapsed:?} >= deadlock_threshold={deadlock_threshold:?} \
         (per_call_baseline={per_call_baseline:?}, seq_elapsed={seq_elapsed:?}). \
         Stop, surface, root-cause investigate - do NOT raise the threshold."
    );
}

// ======================================================================
// Trigger (d) — boundary-validation gap
// ======================================================================

/// **Trigger (d) detection method:** deliberate cross-boundary write+
/// retrieve probe.
///
/// Write a memory in `personal` boundary; query from `work` boundary
/// scope. BRD §11.4.3 mandates mandatory access control - the personal
/// memory MUST NOT appear in work-boundary results. Defense-in-depth
/// proven in isolation at vault-storage; this test confirms the
/// invariant holds end-to-end through the composed VaultAdapter ->
/// SemanticRetriever -> MetadataStore boundary filter chain.
///
/// Pass = work-boundary query returns ZERO results despite the embedder
/// computing a high-similarity vector for the cross-boundary content.
#[tokio::test]
#[ignore]
async fn trigger_d_cross_boundary_write_invisible_in_other_boundary() {
    let app = setup_application().await;
    let adapter = app.application.adapter();

    // Write to personal.
    adapter
        .write(make_new_memory(
            "personal note that must not leak into work",
            "personal",
        ))
        .await
        .expect("personal-boundary write MUST succeed");

    // Confirm the write landed by querying from personal scope - this
    // proves the embedder + retriever path is functional, so a
    // zero-result work-boundary query below is not a false positive
    // from a broken retriever.
    let personal_results = adapter
        .search(make_query(
            "personal note that must not leak",
            "personal",
            10,
        ))
        .await
        .expect("personal-scope search MUST succeed");
    assert!(
        !personal_results.is_empty(),
        "personal-boundary search MUST find the personal-boundary memory \
         (negative-control invariant for trigger d) - if this is empty the \
         retriever pathway itself is broken, not the boundary filter"
    );

    // Query from work - MUST be zero.
    let work_results = adapter
        .search(make_query("personal note that must not leak", "work", 100))
        .await
        .expect("work-scope search MUST succeed");
    assert_eq!(
        work_results.len(),
        0,
        "BRD §11.4.3 violation: personal-boundary memory leaked into work- \
         boundary query results. Stop, surface, root-cause investigate. \
         Defense-in-depth must hold end-to-end through the composed \
         VaultAdapter -> SemanticRetriever -> MetadataStore boundary filter \
         chain. Got {} result(s).",
        work_results.len()
    );
}
