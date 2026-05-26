//! T0.1.10 Phase 1 integration-risk spike — **compile-and-run methodology**
//! per session-open Decision 3 (HANDOFF.md, 2026-05-04).
//!
//! ## macOS deferral (ADR-033, T0.1.11 Phase 3 fix-forward, 2026-05-05)
//!
//! This test binary is **disabled on macOS** via the `#![cfg(...)]`
//! attribute below per ADR-033 (same upstream ORT 1.21+ static-destructor
//! mutex-race bug as `vault-embedding/tests/embedding_tests.rs`). Both
//! test binaries instantiate `BgeSmallProvider` which loads
//! `libonnxruntime.dylib`; both crash at process exit on macOS with
//! `libc++abi mutex lock failed: Invalid argument` SIGABRT after all
//! tests pass. Because integration_smoke.rs is `#[ignore]`-by-default,
//! the cfg here is defensive — a `cargo test ... -- --ignored` run on
//! macOS would crash the same way. See ADR-033 for full context +
//! revisit triggers.

#![cfg(not(target_os = "macos"))]
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

use vault_app::{AppConfig, Application, VaultAdapter};
use vault_core::{Boundary, MemoryType, NewMemory, VaultError};
use vault_mcp::{Adapter, ToolInvokeDetails};
use vault_retrieval::{RetrievalOptions, RetrievalQuery, MAX_QUERY_BYTES, MAX_RESULTS_CAP};
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
    /// Phase 1b: Sender returned by `Application::start`. Held so the
    /// spawned `RetryWorker` lives for the test's duration; on `TestApp`
    /// drop the Sender drops, `cancel.changed()` returns `Err` in the
    /// worker's `select!`, and the worker exits cleanly. Phase 2 will
    /// replace this with an await-aware `Application::shutdown()`.
    _shutdown: tokio::sync::watch::Sender<bool>,
}

async fn setup_application() -> TestApp {
    let tmp = TempDir::new().expect("tempdir");
    let metadata_path = tmp.path().join("vault.db");
    let vector_dir = tmp.path().join("lance");
    let graph_path = tmp.path().join("graph.duckdb");
    let key = SqlCipherKey::new("integration-smoke-test-key");

    // Phase 2b: AppConfig migration. metadata_path + key are cloned
    // into config because they're reused below for the assert-handle
    // open. vector_dir + graph_path are moved (single-use). Path
    // fixture lookups happen inline.
    let config = AppConfig {
        metadata_path: metadata_path.clone(),
        vector_dir,
        graph_path,
        key: key.clone(),
        model_path: require_fixture("model.onnx"),
        tokenizer_path: require_fixture("tokenizer.json"),
        ort_lib_path: ort_lib_path(),
        // T0.2.0 Phase 1: at_rest_key staged on AppConfig per ADR-040
        // amendment + iteration-1.5 amendment Discovery 4 (option (a)).
        // Phase 2/3 wire actual consumption into LanceVectorStore::
        // open_with_at_rest_key. Tests here pre-date that path; a fixed
        // 32-byte sentinel preserves the smoke-test surface without
        // exercising the at-rest sealing path.
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        // T0.2.7 Phase 4: integration smoke does not exercise the read
        // pipeline (no 4.36 GB GGUF on disk in CI). `None` leaves the
        // read pipeline unwired; memory.read calls return
        // VaultError::Config("not configured") which is the correct
        // surface for the absent-model scenario.
        qwen_model_path: None,
        // T0.3.x Batch A: same rationale as `qwen_model_path` above —
        // integration smoke does not load Phi-4-mini (no 2.49 GB GGUF
        // on disk in CI). `None` leaves the consolidator unwired;
        // `Application::run_consolidation_with_safety` returns
        // `VaultError::Config("consolidator not configured")` which is
        // the graceful absent-model surface per locked-next-arc Thread 3.
        phi4_model_path: None,
    };

    let application = Application::new(&config).await.expect(
        "Application::new MUST compose the dep graph successfully. If this fails, \
         trigger (a) has fired - stop, surface, root-cause investigate. \
         Do NOT paper over with #[cfg] skips.",
    );

    // Phase 1b: spawn the cascading retry worker. Without this, writes
    // through VaultAdapter::write land in SQLite + retry_queue but never
    // reach the vector store - SemanticRetriever queries return empty
    // (the integration finding Phase 1 spike surfaced).
    let shutdown = application.start();

    let metadata_for_assert = MetadataStore::open(&metadata_path, key)
        .await
        .expect("open assert handle");

    TestApp {
        application,
        _tmp: tmp,
        metadata_for_assert,
        _shutdown: shutdown,
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

/// Phase 1b: wait for the cascading retry worker to drain queued entries
/// to the vector store. `StorageBackend::write_memory` is async w.r.t. the
/// vector store — it commits to SQLite + retry_queue and returns; the
/// worker drains retry_queue → `vector.upsert` on its next poll cycle
/// (default 1s per `retry_worker.rs:59`). 3s margin covers up to two
/// poll-interval cycles plus processing time. Phase 2 can replace this
/// with a deterministic `Application::flush()` if/when such an API is
/// justified by a concrete consumer.
async fn wait_for_cascade_drain() {
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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

    // Phase 1b empirical H1 refutation (cross-handle SQLite visibility):
    // confirm the write IS visible through a separate MetadataStore
    // connection (`metadata_for_assert`, opened independently in
    // `setup_application`). Refutes the multi-handle WAL visibility
    // hypothesis at integration-test scope, not just by analogy to
    // the existing vault-app unit test. Doubles as a regression check:
    // if `cascading_write` ever stops writing to SQLite metadata, this
    // line catches it before downstream retrievability assertions.
    let stored = app
        .metadata_for_assert
        .get_memory(&id)
        .await
        .expect("get_memory must not error")
        .expect(
            "SQLite write through VaultAdapter::write MUST be visible to a \
             separate MetadataStore handle (cross-connection visibility \
             refutation; if this fails, multi-handle WAL visibility IS the \
             root cause and the retry-worker hypothesis is misattributed)",
        );
    assert_eq!(
        stored.content, "trigger-b probe content",
        "round-tripped memory content must match the write payload"
    );

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

    // Phase 1b: wait for cascading worker to drain retry_queue → vector
    // store. Without this wait, the retrievability check below races the
    // worker.
    wait_for_cascade_drain().await;

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

    // Phase 1b: wait for cascading worker to drain retry_queue → vector
    // store. Without this wait, the negative-control search below races
    // the worker and would fire spuriously.
    wait_for_cascade_drain().await;

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

// ======================================================================
// Phase 2c — Adversarial integration coverage (T0.1.9 Phase 3 Step 1
// forward-pointer payoff)
// ======================================================================
//
// The four tests below trace adversarial inputs end-to-end through the
// **real composed adapter** — `Application::adapter().search(...)` —
// rather than through fixture-bypass adapters (which don't reach
// `SemanticRetriever::retrieve_inner` and thus can't exercise its
// validation chain).
//
// Each test pins ONE of the four distinct defenses at
// `crates/vault-retrieval/src/strategies/semantic.rs:118-142`. The
// 1:1 test-to-defense mapping was located by source-reading 2026-05-04
// per the `feedback_identify_test_source_before_framing.md` discipline
// (an earlier Phase 2 plan paragraph mis-enumerated NUL-in-query as a
// separate test from ASCII-control; the source-read corrected the
// mapping to the four actual defenses).
//
// Original deferred-coverage block — verbatim quote from
// `crates/vault-mcp/tests/adversarial.rs:24-46` (the T0.1.9 Phase 3
// Step 1 architecture-review escalation that surfaced the bypass-
// fixture false-confidence problem):
//
//     "Adversarial coverage of `query_text` validation (oversized,
//      whitespace-only, ASCII control chars) and `max_results` bounds —
//      these defenses live inside `vault_retrieval::SemanticRetriever::
//      retrieve()` at `crates/vault-retrieval/src/strategies/semantic.rs:
//      118-133`, NOT in vault-mcp. With any vault-mcp test fixture
//      (SuccessAdapter / MockAdapter / DimMismatchAdapter), those
//      validations never fire — the adapter bypasses SemanticRetriever.
//      vault-retrieval's own adversarial suite already pins these cases
//      at `semantic.rs:482` (...). Each crate owns its own adversarial
//      coverage. Adversarial integration coverage that traces vault-mcp
//      dispatch → vault-retrieval validation end-to-end against a real
//      composed system lands at T0.1.10 alongside the integration-risk
//      spike, NOT here at Phase 3."
//
// Phase 2c lands that integration coverage. All four tests are
// `#[ignore]`-by-default per Phase 1 spike pattern.

/// **Adversarial defense:** whitespace-only `query_text` (after `.trim()`).
///
/// Defense location: `semantic.rs:119-123` —
/// `if trimmed.is_empty() { return Err(VaultError::InvalidInput("query text empty after trim".into())); }`.
///
/// Forward-pointer source: T0.1.9 Phase 3 Step 1 deferred-coverage
/// block at `crates/vault-mcp/tests/adversarial.rs:24-46` ("whitespace-
/// only" entry).
///
/// **Routing through real composed adapter** (NOT fixture-bypassed):
/// the search call dispatches `Application::adapter()` →
/// `VaultAdapter::search` → `SemanticRetriever::retrieve` →
/// `retrieve_inner` where the defense fires.
#[tokio::test]
#[ignore]
async fn adversarial_whitespace_only_query_rejected_through_composed_adapter() {
    let app = setup_application().await;
    let query = make_query("   \t\n   ", "work", 10);

    let err =
        app.application.adapter().search(query).await.expect_err(
            "whitespace-only query MUST be rejected at SemanticRetriever::retrieve_inner",
        );

    let VaultError::InvalidInput(msg) = &err else {
        panic!("expected VaultError::InvalidInput, got {err:?}");
    };
    assert!(
        msg.contains("query text empty after trim"),
        "InvalidInput message MUST cite the trim-empty defense (semantic.rs:119-123); got: {msg}"
    );
}

/// **Adversarial defense:** ASCII control characters in `query_text`
/// (caught by `b.is_ascii_control()` over the full 0x00–0x1F + 0x7F class).
///
/// Defense location: `semantic.rs:124-128`. Forward-pointer source:
/// `crates/vault-mcp/tests/adversarial.rs:24-46` ("ASCII control chars"
/// entry).
///
/// **NUL (0x00) is the canonical edge case** for this class but the
/// defense is the broader `is_ascii_control()` check, NOT a NUL-specific
/// check. A test using NUL exercises the defense via its most common
/// entry point; assertion on the class-level error message pins the
/// broader defense.
#[tokio::test]
#[ignore]
async fn adversarial_ascii_control_chars_in_query_rejected_through_composed_adapter() {
    let app = setup_application().await;
    let query = make_query("hello\x00world", "work", 10);

    let err = app.application.adapter().search(query).await.expect_err(
        "query containing ASCII control char (NUL) MUST be rejected at \
             SemanticRetriever::retrieve_inner",
    );

    let VaultError::InvalidInput(msg) = &err else {
        panic!("expected VaultError::InvalidInput, got {err:?}");
    };
    assert!(
        msg.contains("query text contains ASCII control characters"),
        "InvalidInput message MUST cite the ASCII-control-class defense (semantic.rs:124-128); got: {msg}"
    );
}

/// **Adversarial defense:** `query_text` length exceeds [`MAX_QUERY_BYTES`]
/// (= 2048) after trim.
///
/// Defense location: `semantic.rs:129-133`. Forward-pointer source:
/// `crates/vault-mcp/tests/adversarial.rs:24-46` ("oversized" entry).
///
/// **Test uses `MAX_QUERY_BYTES + 1`** (= 2049) — exactly one byte over
/// the boundary, the canonical edge case. Pins off-by-one threshold
/// semantics.
#[tokio::test]
#[ignore]
async fn adversarial_oversized_query_rejected_through_composed_adapter() {
    let app = setup_application().await;
    let oversized = "x".repeat(MAX_QUERY_BYTES + 1);
    let query = make_query(&oversized, "work", 10);

    let err = app.application.adapter().search(query).await.expect_err(
        "query_text > MAX_QUERY_BYTES MUST be rejected at SemanticRetriever::retrieve_inner",
    );

    let VaultError::InvalidInput(msg) = &err else {
        panic!("expected VaultError::InvalidInput, got {err:?}");
    };
    assert!(
        msg.contains("query length") && msg.contains("MAX_QUERY_BYTES"),
        "InvalidInput message MUST cite the length / MAX_QUERY_BYTES defense (semantic.rs:129-133); got: {msg}"
    );
}

/// **Adversarial defense:** `max_results` outside `1..=`[`MAX_RESULTS_CAP`]
/// (= 200). Both edges (`0` below the floor, `MAX_RESULTS_CAP + 1` above
/// the cap) collapse into one defense at one code path; this single
/// test exercises both edges via two sub-assertions on the same defense.
///
/// Defense location: `semantic.rs:137-141`. Forward-pointer source:
/// `crates/vault-mcp/tests/adversarial.rs:24-46` ("max_results bounds"
/// entry).
///
/// **Single-test-two-edges pattern** approved at Phase 2c plan-paragraph
/// review (single defense, single code path).
#[tokio::test]
#[ignore]
async fn adversarial_max_results_out_of_range_rejected_through_composed_adapter() {
    let app = setup_application().await;
    let adapter = app.application.adapter();

    // Below-floor edge: max_results = 0.
    let query_zero = make_query("anything", "work", 0);
    let err_zero = adapter
        .search(query_zero)
        .await
        .expect_err("max_results=0 MUST be rejected at SemanticRetriever::retrieve_inner");
    let VaultError::InvalidInput(msg_zero) = &err_zero else {
        panic!("expected VaultError::InvalidInput for zero edge, got {err_zero:?}");
    };
    assert!(
        msg_zero.contains("max_results") && msg_zero.contains("not in"),
        "InvalidInput message for zero edge MUST cite the max_results-range defense (semantic.rs:137-141); got: {msg_zero}"
    );

    // Above-cap edge: max_results = MAX_RESULTS_CAP + 1.
    let query_over = make_query("anything", "work", MAX_RESULTS_CAP + 1);
    let err_over = adapter.search(query_over).await.expect_err(
        "max_results > MAX_RESULTS_CAP MUST be rejected at SemanticRetriever::retrieve_inner",
    );
    let VaultError::InvalidInput(msg_over) = &err_over else {
        panic!("expected VaultError::InvalidInput for over-cap edge, got {err_over:?}");
    };
    assert!(
        msg_over.contains("max_results") && msg_over.contains("not in"),
        "InvalidInput message for over-cap edge MUST cite the max_results-range defense (semantic.rs:137-141); got: {msg_over}"
    );
}
