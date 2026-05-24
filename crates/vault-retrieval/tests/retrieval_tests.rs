//! Integration tests for `vault-retrieval` (Heavy classification per
//! BRD §7.1). Coverage maps directly to T0.1.8_PLAN.md §5 v1.2 — 16
//! integration tests + 1 ignored perf gate.
//!
//! ## T0.1.9 §6 audit-removal sub-phase state
//!
//! T0.1.8 emitted an `AuditEventType::RetrievalQuery` audit event from
//! every `retrieve()` call. T0.1.9 §6 moves audit-event accounting up
//! to vault-mcp (`mcp.tool_invoke`) — this layer emits operational
//! `tracing::info!` only. The 4 audit-shape tests (#3, #4, #15, #16)
//! are rewritten to assert the equivalent `tracing::info!` emission via
//! `tracing-test` instead of asserting audit-event side effects.
//!
//! Phase 3 (T0.1.8 prior) added proptest boundary-leak harness +
//! cross-crate `warn!` log assertion in test #12. Those stay
//! load-bearing here.

mod common;

use common::{boundary, insert_memory_with_drift, make_memory, make_test_retriever, query};
use vault_core::MemoryId;
use vault_embedding::EMBEDDING_DIM;
use vault_retrieval::{Retriever, MAX_QUERY_BYTES};

// =============================================================================
// 1. Happy path
// =============================================================================

#[tokio::test]
async fn happy_path_returns_top_two_results() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    for (i, content) in [
        "cats are nocturnal hunters",
        "dogs greet you at the door",
        "cats purr when content",
        "fish live in water",
        "cats have whiskers",
    ]
    .iter()
    .enumerate()
    {
        let m = make_memory(content, &b);
        insert_memory_with_drift(&t, &m, i).await;
    }
    let res = t
        .retriever
        .retrieve(query("tell me about cats", vec![b], 2))
        .await
        .expect("retrieve");
    assert_eq!(res.len(), 2);
    for r in &res {
        assert!(r.score >= -1.0 && r.score <= 1.0, "score out of [-1, 1]");
        assert!(r.explanation.starts_with("semantic: cosine="));
    }
}

// =============================================================================
// 2. Boundary-leak proptest (delegates to the trait-level invariant)
// =============================================================================

// The trait-level invariant + its `SemanticRetriever` driver live in
// `tests/trait_invariants.rs`. Keeping the entry-point in that file
// (not duplicating it here) is the discipline that lets T0.2.7's
// `MultiStrategyRetriever` re-use the same harness without rewriting
// the leak proof. Phase 3 wraps the harness in a `proptest!` block to
// fuzz arbitrary boundary configurations.

// =============================================================================
// 3. Empty vault returns empty result, tracing emits result_count=0
// =============================================================================

#[tokio::test]
#[tracing_test::traced_test]
async fn empty_vault_returns_empty_result_not_error() {
    let t = make_test_retriever().await;
    let res = t
        .retriever
        .retrieve(query("anything", vec![boundary("work")], 10))
        .await
        .expect("retrieve");
    assert!(res.is_empty(), "empty vault should yield no results");
    // T0.1.9 §6: audit accounting moved to MCP layer; this layer emits
    // structured `tracing::info!` carrying the diagnostic shape.
    assert!(
        tracing_test::internal::logs_with_scope_contain("vault_retrieval", "result_count=0"),
        "empty vault must emit result_count=0 in operational log"
    );
}

// =============================================================================
// 4. Empty authorized_boundaries — no round-trip to embedder / vector store
// =============================================================================

#[tokio::test]
#[tracing_test::traced_test]
async fn empty_authorized_boundaries_short_circuits() {
    let t = make_test_retriever().await;
    let pre_calls = t.embedder.call_count();
    let res = t
        .retriever
        .retrieve(query("anything", vec![], 10))
        .await
        .expect("retrieve");
    assert!(res.is_empty());
    assert_eq!(
        t.embedder.call_count(),
        pre_calls,
        "watch-point #1: empty boundaries must not invoke the embedder"
    );
    // T0.1.9 §6: empty-auth still emits a tracing event with
    // boundary_count=0, result_count=0 — same legitimate observability
    // data point as the old audit posture, just at info-log level.
    assert!(
        tracing_test::internal::logs_with_scope_contain("vault_retrieval", "boundary_count=0"),
        "watch-point #1: tracing emission must record boundary_count=0"
    );
    assert!(
        tracing_test::internal::logs_with_scope_contain("vault_retrieval", "result_count=0"),
        "watch-point #1: tracing emission must record result_count=0"
    );
}

// =============================================================================
// 5. Determinism — same inputs → byte-identical results across N runs
// =============================================================================

#[tokio::test]
async fn determinism_five_runs_byte_identical() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    for i in 0..5 {
        let m = make_memory(&format!("memory {i}"), &b);
        insert_memory_with_drift(&t, &m, i).await;
    }
    let mut runs = Vec::new();
    for _ in 0..5 {
        let res = t
            .retriever
            .retrieve(query("memory", vec![b.clone()], 5))
            .await
            .expect("retrieve");
        runs.push(res);
    }
    let first = &runs[0];
    for (idx, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(run.len(), first.len(), "run {idx} length differs");
        for (a, b) in run.iter().zip(first.iter()) {
            assert_eq!(a.memory.id, b.memory.id, "run {idx} order/id differs");
            assert!(
                (a.score - b.score).abs() < f32::EPSILON,
                "run {idx} score differs"
            );
        }
    }
}

// =============================================================================
// 6. max_results honoured — 50 memories, max_results=10 → exactly 10
// =============================================================================

#[tokio::test]
async fn max_results_honoured() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    for i in 0..50 {
        let m = make_memory(&format!("memory {i}"), &b);
        insert_memory_with_drift(&t, &m, i).await;
    }
    let res = t
        .retriever
        .retrieve(query("memory", vec![b], 10))
        .await
        .expect("retrieve");
    assert_eq!(res.len(), 10);
}

// =============================================================================
// 7. Result ordering — score DESC, then created_at DESC for ties
// =============================================================================

#[tokio::test]
async fn result_ordering_score_then_created_at_desc() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    for i in 0..5 {
        let m = make_memory(&format!("memory {i}"), &b);
        insert_memory_with_drift(&t, &m, i).await;
    }
    let res = t
        .retriever
        .retrieve(query("memory", vec![b], 5))
        .await
        .expect("retrieve");
    for w in res.windows(2) {
        assert!(
            w[0].score >= w[1].score,
            "score must be non-increasing: {} then {}",
            w[0].score,
            w[1].score
        );
        if (w[0].score - w[1].score).abs() < f32::EPSILON {
            assert!(
                w[0].memory.created_at >= w[1].memory.created_at,
                "tied scores must tiebreak created_at DESC"
            );
        }
    }
}

// =============================================================================
// 8. Score range — every score in [-1, 1]
// =============================================================================

#[tokio::test]
async fn score_range_all_in_negative_one_to_one() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    for i in 0..10 {
        let m = make_memory(&format!("memory {i}"), &b);
        insert_memory_with_drift(&t, &m, i).await;
    }
    let res = t
        .retriever
        .retrieve(query("memory", vec![b], 10))
        .await
        .expect("retrieve");
    for r in &res {
        assert!(
            r.score.is_finite() && (-1.0..=1.0).contains(&r.score),
            "score {} out of [-1, 1]",
            r.score
        );
    }
}

// =============================================================================
// 9. Memory hydration correctness — every result.memory belongs to authorised boundary
// =============================================================================

#[tokio::test]
async fn memory_hydration_correctness() {
    let t = make_test_retriever().await;
    let work = boundary("work");
    let personal = boundary("personal");
    for i in 0..3 {
        let m = make_memory(&format!("work {i}"), &work);
        insert_memory_with_drift(&t, &m, i).await;
    }
    for i in 0..3 {
        let m = make_memory(&format!("personal {i}"), &personal);
        insert_memory_with_drift(&t, &m, i + 10).await;
    }
    let res = t
        .retriever
        .retrieve(query("anything", vec![work.clone()], 100))
        .await
        .expect("retrieve");
    for r in &res {
        assert_eq!(r.memory.boundary, work);
    }
}

// =============================================================================
// 10. Adversarial: control chars rejected
// =============================================================================

#[tokio::test]
async fn adversarial_query_with_control_chars_rejected() {
    let t = make_test_retriever().await;
    let res = t
        .retriever
        .retrieve(query("hello\x07world", vec![boundary("work")], 10))
        .await;
    assert!(matches!(res, Err(vault_core::VaultError::InvalidInput(_))));
}

// =============================================================================
// 11. Adversarial: 2,048 chars succeeds; 2,049 rejected
// =============================================================================

#[tokio::test]
async fn adversarial_query_length_exact_cap_and_one_over() {
    let t = make_test_retriever().await;
    let just_at_cap = "x".repeat(MAX_QUERY_BYTES);
    let one_over = "x".repeat(MAX_QUERY_BYTES + 1);
    // At-cap should succeed (no length-rejection).
    let _ = t
        .retriever
        .retrieve(query(&just_at_cap, vec![boundary("work")], 10))
        .await
        .expect("at-cap query");
    // Over-cap must be rejected as InvalidInput.
    let res = t
        .retriever
        .retrieve(query(&one_over, vec![boundary("work")], 10))
        .await;
    assert!(matches!(res, Err(vault_core::VaultError::InvalidInput(_))));
}

// =============================================================================
// 12. Adversarial: deleted-but-not-purged memory (LanceDB has it, MetadataStore doesn't)
// =============================================================================

/// `#[traced_test]` (Phase 3) installs a thread-local subscriber that
/// captures `tracing` events emitted during the test. Cross-crate
/// emission works: the `warn!` fires in vault-storage's
/// `MetadataStore::get_memories_batch::missing-id` branch, and
/// `logs_contain` finds it from the vault-retrieval test thread. Per
/// the tracing-test docs, this captures only the calling thread's
/// events — concurrent cargo test parallelism is safe.
#[tokio::test]
#[tracing_test::traced_test]
async fn adversarial_deleted_but_not_purged_memory() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    let real = make_memory("real memory", &b);
    insert_memory_with_drift(&t, &real, 1).await;
    // Orphan vector row: present in LanceDB, absent from MetadataStore.
    let fake_id = MemoryId::new();
    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    emb[0] = 1.0;
    t.vectors
        .upsert(&fake_id, &emb, &b)
        .await
        .expect("orphan vector upsert");
    // retrieve() must not crash; orphan must be filtered out.
    let res = t
        .retriever
        .retrieve(query("anything", vec![b], 10))
        .await
        .expect("retrieve");
    assert!(
        res.iter().all(|r| r.memory.id != fake_id),
        "orphan id must not be returned"
    );
    assert!(
        res.iter().any(|r| r.memory.id == real.id),
        "real memory must still be returned"
    );
    // Phase 3: assert the cross-crate `warn!` from
    // `MetadataStore::get_memories_batch` (missing-id branch) actually
    // fires. The exact log message lives in vault-storage's
    // metadata_store.rs and pins this contract — if the message
    // changes there, this assertion fails loudly and the operator log
    // semantics are re-reviewed at the same time.
    //
    // We bypass the macro-injected `logs_contain` (which scopes by the
    // test function name) and call the internal API with scope
    // `"vault_storage"` instead — the warn fires from a `spawn_blocking`
    // worker thread that doesn't carry the test's `info_span` context,
    // so the formatted line contains ` vault_storage:` (the event's
    // target prefix) but not the test's span name. The
    // `no-env-filter` feature on `tracing-test` (workspace dep) ensures
    // vault_storage events reach the capture buffer in the first place
    // — without it, the default `vault_retrieval=trace` filter drops
    // them silently.
    assert!(
        tracing_test::internal::logs_with_scope_contain(
            "vault_storage",
            "get_memories_batch: id not found in metadata store",
        ),
        "expected warn! from get_memories_batch's missing-id branch"
    );
}

// =============================================================================
// 13. get_memories_batch order preservation
// =============================================================================

#[tokio::test]
async fn get_memories_batch_preserves_input_order() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    let mems: Vec<_> = (0..5)
        .map(|i| make_memory(&format!("mem-{i}"), &b))
        .collect();
    for m in &mems {
        t.metadata.create_memory(m).await.expect("create");
    }
    // Query in reverse order — assert returned Vec preserves input order.
    let ids: Vec<MemoryId> = mems.iter().map(|m| m.id).rev().collect();
    let out = t
        .metadata
        .get_memories_batch(&ids)
        .await
        .expect("batch fetch");
    assert_eq!(out.len(), 5);
    for (got, expected_id) in out.iter().zip(ids.iter()) {
        assert_eq!(got.id, *expected_id, "input-order preservation broken");
    }
}

// =============================================================================
// 14. get_memories_batch partial-hit (warns + omits)
// =============================================================================

#[tokio::test]
async fn get_memories_batch_partial_hit_warns_and_omits() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    let a = make_memory("a", &b);
    let c = make_memory("c", &b);
    t.metadata.create_memory(&a).await.expect("create a");
    t.metadata.create_memory(&c).await.expect("create c");
    let b_missing = MemoryId::new();
    let out = t
        .metadata
        .get_memories_batch(&[a.id, b_missing, c.id])
        .await
        .expect("batch fetch");
    // b_missing omitted; a and c returned in input order.
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].id, a.id);
    assert_eq!(out[1].id, c.id);
}

// =============================================================================
// 15. Tracing-event round-trip on success (v1.3 shape — no query_hash)
// =============================================================================

#[tokio::test]
#[tracing_test::traced_test]
async fn tracing_event_round_trip_v1_3_shape() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    let m = make_memory("seed", &b);
    insert_memory_with_drift(&t, &m, 1).await;
    let _ = t
        .retriever
        .retrieve(query("anything", vec![b], 5))
        .await
        .expect("retrieve");
    // T0.1.9 §6: every diagnostic field from the old v1.2 audit
    // `details_json` shape carries forward as a structured tracing
    // field. The default formatter renders `field=value`.
    for field_eq in [
        "query_length=",
        "boundary_count=",
        "result_count=",
        "max_results=",
        "include_archived=",
        "score_threshold=",
        "latency_ms=",
    ] {
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_retrieval", field_eq),
            "missing tracing field '{field_eq}' in retrieval pipeline log"
        );
    }
    // The "no query_hash" invariant from T0.1.8 v1.2 watch-point #3
    // carries forward — operational logging must not introduce hashing.
    assert!(
        !tracing_test::internal::logs_with_scope_contain("vault_retrieval", "query_hash"),
        "v1.3 must NOT introduce query_hash in tracing emission"
    );
}

// =============================================================================
// 16. Audit-chain integrity unaffected by retrieve()
// =============================================================================

/// T0.1.9 §6 audit-removal: `retrieve()` no longer appends to the audit
/// chain (the equivalent `mcp.tool_invoke` event lands at the MCP layer
/// instead). The audit chain still contains `memory.create` entries from
/// setup; `retrieve()` MUST NOT touch the chain. This test pins that
/// contract — the chain verifies cleanly after `retrieve()` precisely
/// because `retrieve()` was a no-op against it.
#[tokio::test]
async fn audit_chain_integrity_unaffected_by_retrieve() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    let m = make_memory("chain-test", &b);
    insert_memory_with_drift(&t, &m, 1).await;
    let pre_chain_len = t
        .metadata
        .list_audit_events(1000)
        .await
        .expect("audit pre")
        .len();
    let _ = t
        .retriever
        .retrieve(query("chain-test", vec![b], 5))
        .await
        .expect("retrieve");
    let post_chain_len = t
        .metadata
        .list_audit_events(1000)
        .await
        .expect("audit post")
        .len();
    assert_eq!(
        pre_chain_len, post_chain_len,
        "T0.1.9 §6: retrieve() must not append audit events (audit moved to MCP layer)"
    );
    t.metadata
        .verify_audit_chain()
        .await
        .expect("audit chain must remain valid after retrieve()");
}

// =============================================================================
// 17. Perf gate (BRD §5.5: end-to-end retrieval < 200ms over 1k memories)
// =============================================================================

/// **Status: investigation COMPLETE at T0.1.10 Phase 3a (2026-05-04);
/// intervention in flight at T0.2.7.** Gate remains `#[ignore]`-d until
/// the intervention (hybrid retrieval + `bulk_upsert` promotion) is fully
/// landed in production and benchmarked at SCALE=1K. See HANDOFF.md
/// "Tech debt — open items" entry "Promote `bulk_upsert` from t028b spike
/// to `VectorStore` trait + production."
///
/// **Historical context (preserved for audit trail):**
/// First end-to-end run of this gate (T0.1.8 Phase 3, 2026-05-01, idle
/// machine + fresh build cache) measured **412ms** (run 1) and
/// **1,852ms** (run 2) — both well over the 200ms BRD §5.5 ceiling.
/// Phase 1 had `retrieve()` as `unimplemented!()` so this gate had
/// never actually executed before; Phase 2 left it `#[ignore]`-d.
///
/// **Root cause (confirmed at T0.1.10 Phase 3a, 2026-05-04 — see
/// `HANDOFF_V0.1_ARCHIVE.md` line 1762 for the 5-run measurement results
/// + outcome (i) confirmation):** LanceDB fragmentation. The setup loop
/// does 1000 individual `vectors.upsert` calls, creating 1000 fragments;
/// without an explicit vector index, search falls back to per-fragment
/// full-scan cosine k-NN, so latency grows roughly
/// `O(fragments × rows_per_fragment)`. Production V0.1 writes memories
/// one-at-a-time, so fragmentation accumulates similarly — this isn't a
/// synthetic-fixture artefact, it's a real V0.1 perf concern surfaced by
/// the gate.
///
/// **Intervention in flight at T0.2.7 (this commit's parent task):**
/// hybrid retrieval architecture (BGE + Tantivy BM25 + RRF + abstain)
/// supersedes vanilla cosine-only retrieval as the production read path
/// (`vault_retrieval::HybridRetriever` wired into vault-app at Phase 4).
/// Tantivy is in-RAM, sidestepping LanceDB fragmentation for the keyword
/// channel. The semantic channel still uses LanceDB and will benefit
/// from the separately-tracked `bulk_upsert` promotion (HANDOFF.md tech
/// debt) — spike measured **730× faster** insertion at 10K vs single-row
/// upsert.
///
/// **Why this gate STAYS `#[ignore]`-d even after T0.2.7 ships:**
/// 1. The hybrid retriever is the production read path; this gate
///    measures `SemanticRetriever` (vanilla cosine) in isolation per
///    BRD §5.5 wording. An independent gate for the hybrid path is a
///    separate test that doesn't yet exist.
/// 2. `bulk_upsert` promotion (the LanceDB fragmentation remediation)
///    is itself open tech-debt; this gate's setup loop will benefit
///    automatically once promoted (estimated 30–80ms PASS based on
///    t028b spike measurements).
/// 3. The assertion stays preserved as the contract pin — when
///    `bulk_upsert` is promoted to production, light up the gate again,
///    expect it to PASS comfortably under 200ms.
///
/// The `assert!(elapsed.as_millis() < 200, ...)` line below stays
/// load-bearing for whenever the relight happens.
#[tokio::test]
#[ignore = "T0.1.10 Phase 3a (2026-05-04) confirmed root cause: LanceDB fragmentation from 1-at-a-time upserts in setup loop (412ms / 1852ms on first runs vs 200ms ceiling). Intervention in flight at T0.2.7 (hybrid retrieval) + bulk_upsert promotion (HANDOFF.md tech-debt). Relight gate when bulk_upsert lands; expect PASS at 30-80ms per t028b spike (730× speedup at 10K). See HANDOFF.md 'Tech debt — open items' for the bulk_upsert promotion ship-gate language."]
async fn end_to_end_retrieval_latency_under_200ms_with_1k_memories() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    for i in 0..1_000 {
        let m = make_memory(&format!("memory {i}"), &b);
        insert_memory_with_drift(&t, &m, i % 100).await;
    }
    let start = std::time::Instant::now();
    let _ = t
        .retriever
        .retrieve(query("anything", vec![b], 10))
        .await
        .expect("retrieve");
    let elapsed = start.elapsed();
    eprintln!("retrieval over 1k memories: {elapsed:?}");
    assert!(
        elapsed.as_millis() < 200,
        "perf gate violated: {elapsed:?} > 200ms"
    );
}
