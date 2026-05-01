//! Integration tests for `vault-retrieval` (Heavy classification per
//! BRD §7.1). Coverage maps directly to T0.1.8_PLAN.md §5 v1.2 — 16
//! integration tests + 1 ignored perf gate.
//!
//! ## Phase 1 / Phase 2 / Phase 3 split
//!
//! Phase 1 (this commit) scaffolds every test. Tests whose body only
//! exercises the public `Retriever::retrieve` surface use
//! `#[should_panic(expected = "T0.1.8 Phase 2")]` so the unimplemented
//! body's panic is the failure signal — Phase 2 implements
//! `retrieve()` and removes the `should_panic` attributes one by one.
//!
//! Tests with a Phase 2 dependency that goes beyond `retrieve()` body
//! (e.g., `MetadataStore::get_memories_batch` for tests 13/14, the
//! `AuditEventType::RetrievalQuery` variant for tests 15/16) are
//! `#[ignore]`-d with a clear reason; Phase 2 lands the dependency
//! and removes the ignore.
//!
//! Phase 3 lands tests 2 (boundary-leak proptest), 3 (empty vault),
//! 10 / 11 (adversarial inputs), 12 (orphan-row warn), and unignores
//! 16 (chain integrity).

mod common;

use common::{boundary, insert_memory_with_drift, make_memory, make_test_retriever, query};
use vault_retrieval::{RetrievalOptions, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP};

// =============================================================================
// 1. Happy path
// =============================================================================

/// 5-memory fixture, all in `work`. Retrieve "tell me about cats" with
/// `max_results = 2`. The two highest-similarity hits are returned;
/// `RetrievedMemory.score` is in `[-1, 1]` and `explanation` follows
/// the Q6 format. Phase 2 turns this green.
#[tokio::test]
#[should_panic(expected = "T0.1.8 Phase 2")]
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
// the leak proof.

// =============================================================================
// 3. Empty vault returns empty result, audit records result_count = 0
// =============================================================================

#[tokio::test]
#[should_panic(expected = "T0.1.8 Phase 2")]
async fn empty_vault_returns_empty_result_not_error() {
    let t = make_test_retriever().await;
    let res = t
        .retriever
        .retrieve(query("anything", vec![boundary("work")], 10))
        .await
        .expect("retrieve");
    assert!(res.is_empty(), "empty vault should yield no results");
}

// =============================================================================
// 4. Empty authorized_boundaries — no round-trip to embedder / vector store
// =============================================================================

#[tokio::test]
#[should_panic(expected = "T0.1.8 Phase 2")]
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
        "empty boundaries must not invoke the embedder"
    );
}

// =============================================================================
// 5. Determinism — same inputs → byte-identical results across N runs
// =============================================================================

#[tokio::test]
#[should_panic(expected = "T0.1.8 Phase 2")]
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
#[should_panic(expected = "T0.1.8 Phase 2")]
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
#[should_panic(expected = "T0.1.8 Phase 2")]
async fn result_ordering_score_then_created_at_desc() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    // Insert 5 memories with monotonically increasing drift → distinct
    // scores. The retrieval order should be drift-DESC equivalent
    // (closer-to-query first).
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
#[should_panic(expected = "T0.1.8 Phase 2")]
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
#[should_panic(expected = "T0.1.8 Phase 2")]
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
#[should_panic(expected = "T0.1.8 Phase 2")]
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
#[should_panic(expected = "T0.1.8 Phase 2")]
async fn adversarial_query_length_exact_cap_and_one_over() {
    let t = make_test_retriever().await;
    let just_at_cap = "x".repeat(MAX_QUERY_BYTES);
    let one_over = "x".repeat(MAX_QUERY_BYTES + 1);
    // At-cap should succeed (or at least not be rejected for length).
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

/// Phase 2/3 behaviour: orphan vector rows (present in LanceDB,
/// absent from MetadataStore) should produce a `warn!` log and be
/// silently omitted from the result. The retriever does NOT crash and
/// returns whatever real memories *did* hydrate.
#[tokio::test]
#[should_panic(expected = "T0.1.8 Phase 2")]
async fn adversarial_deleted_but_not_purged_memory() {
    let t = make_test_retriever().await;
    let b = boundary("work");
    // Insert a real memory, then upsert a fake vector row whose ID has
    // no corresponding metadata row — simulates the "delete from
    // metadata succeeded, vector cascade failed" partial-state.
    let real = make_memory("real memory", &b);
    insert_memory_with_drift(&t, &real, 1).await;
    let fake_id = vault_core::MemoryId::new();
    let mut emb = vec![0.0_f32; vault_embedding::EMBEDDING_DIM];
    emb[0] = 1.0;
    t.vectors
        .upsert(&fake_id, &emb, &b)
        .await
        .expect("orphan vector upsert");
    // Retrieval must not crash; it should return the one real memory
    // (and warn about the orphan).
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
}

// =============================================================================
// 13. get_memories_batch order preservation (Phase 2 dep)
// =============================================================================

/// `MetadataStore::get_memories_batch(&[a, b, c])` returns memories in
/// that exact input order. Phase 2 ships `get_memories_batch`; until
/// then this test compiles cleanly because the body doesn't reference
/// the not-yet-existing method.
#[tokio::test]
#[ignore = "T0.1.8 Phase 2: depends on MetadataStore::get_memories_batch"]
async fn get_memories_batch_preserves_input_order() {
    unimplemented!("T0.1.8 Phase 2");
}

// =============================================================================
// 14. get_memories_batch partial-hit (Phase 2 dep)
// =============================================================================

/// `MetadataStore::get_memories_batch(&[a, b_missing, c])` returns
/// `[a, c]` and emits a `warn!` for `b_missing`. Phase 2 implementation.
#[tokio::test]
#[ignore = "T0.1.8 Phase 2: depends on MetadataStore::get_memories_batch"]
async fn get_memories_batch_partial_hit_warns_and_omits() {
    unimplemented!("T0.1.8 Phase 2");
}

// =============================================================================
// 15. Audit-event round-trip on success (Phase 2 dep)
// =============================================================================

/// One retrieve() → one new audit_log row with `action =
/// "retrieval.query"` and details_json containing the v1.2 fields:
/// query_length, boundary_count, result_count, max_results,
/// score_threshold, include_archived, latency_ms. Critically: NO
/// `query_hash` (v1.2 dropped that under ADR-021 reversal).
#[tokio::test]
#[ignore = "T0.1.8 Phase 2: depends on AuditEventType::RetrievalQuery variant"]
async fn audit_event_round_trip_v1_2_shape() {
    unimplemented!("T0.1.8 Phase 2");
}

// =============================================================================
// 16. Audit-event chain integrity after retrieve()
// =============================================================================

/// After a retrieve(), `MetadataStore::verify_audit_chain()` must
/// return `Ok(())`. The new audit event participates in the chain
/// (BRD §11.9.2 / T0.1.3) without breaking it.
#[tokio::test]
#[ignore = "T0.1.8 Phase 2: depends on AuditEventType::RetrievalQuery variant"]
async fn audit_event_chain_integrity_after_retrieve() {
    unimplemented!("T0.1.8 Phase 2");
}

// =============================================================================
// 17. Perf gate (BRD §5.5: end-to-end retrieval < 200ms over 1k memories)
// =============================================================================

/// `#[ignore]`-d so it runs only via `cargo test -- --ignored`. Mirrors
/// the T0.1.7 perf-gate pattern (test 6 in vault-embedding).
#[tokio::test]
#[ignore = "perf gate: run via `cargo test -p vault-retrieval -- --ignored`"]
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

// Suppress the "unused" warning on RetrievalOptions / MAX_RESULTS_CAP
// imports in Phase 1. Phase 2/3 lights them up via the threshold +
// over-cap tests.
#[allow(dead_code)]
fn _phase_1_keepalive() {
    let _ = RetrievalOptions::default();
    let _ = MAX_RESULTS_CAP;
}
