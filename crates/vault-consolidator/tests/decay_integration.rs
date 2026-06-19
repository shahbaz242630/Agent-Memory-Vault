//! T0.2.4 — Phase 4 confidence decay end-to-end (real bge-small).
//!
//! Proves the decay pass fires through the real orchestrator: a fact left
//! untouched past `decay_after_days` has its confidence multiplied by 0.9 by
//! `run_consolidation`, is reported in `memories_decayed`, is **never lost**
//! (BRD §5.6 line 1023 — still present, just lower-confidence), and a
//! back-to-back run does **not** re-decay it (idempotency, BRD §5.6 line 1022).
//!
//! `#![cfg(not(target_os = "macos"))]` per ADR-033 — real BGE embeddings are
//! exercised by the orchestrator's clustering pass. Linux + Windows CI covers
//! it (matches the other `*_integration.rs` suites).

#![cfg(not(target_os = "macos"))]

mod common;

use std::sync::Arc;

use chrono::{Duration, Utc};
use vault_consolidator::{Consolidator, ConsolidatorConfig};
use vault_core::Boundary;
use vault_llm::MockLlmProvider;
use vault_storage::MemoryFilter;

use common::{
    insert_and_drain, make_memory_with_content, open_bge_provider, open_sealed_storage_for_test,
};

/// A single cold fact decays through a real consolidation run, survives
/// (no memory ever lost), and is idempotent across an immediate second run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_fact_decays_through_consolidation_and_is_never_lost() {
    let (storage, _dir) = open_sealed_storage_for_test("decay-e2e").await;
    let storage = Arc::new(storage);
    let embedder = open_bge_provider();
    let boundary = Boundary::new("personal").expect("valid boundary");

    // One fact, last accessed 200 days ago (> the default decay_after_days of
    // 180). `last_accessed` is persisted from the struct on write, so setting
    // it before insert backdates the row.
    let mut cold = make_memory_with_content("The user prefers tea over coffee.", &boundary);
    cold.last_accessed = Utc::now() - Duration::days(200);
    let cold_id = cold.id;
    let starting_confidence = cold.confidence; // 0.9 from the helper
    let emb = embedder.embed(&cold.content).await.expect("embed");
    insert_and_drain(&storage, vec![(cold, emb)]).await;

    // One fact in its own boundary → no clusters, no contradiction pairs → the
    // LLM is never called. A malformed canned response is the sentinel: if the
    // merge path WERE reached it would surface as a skipped cluster.
    let llm = Arc::new(MockLlmProvider::new(
        "phi-4-mini-test",
        "MALFORMED-NOT-JSON",
    ));
    let consolidator = Consolidator::new(
        storage.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    // ── First run: the cold fact decays ──────────────────────────────────
    let report = consolidator
        .run_consolidation(None)
        .await
        .expect("decay run must complete");
    assert_eq!(
        report.memories_decayed, 1,
        "the cold fact must be decayed in the first run"
    );
    assert_eq!(
        report.clusters_skipped, 0,
        "the LLM merge path must never be reached (single fact, no clusters)"
    );

    // No memory is ever lost — the fact is still present, confidence × 0.9.
    let after = storage
        .list_memories(MemoryFilter::default(), None)
        .await
        .expect("list after decay");
    let decayed = after
        .iter()
        .find(|m| m.id == cold_id)
        .expect("the decayed fact must still exist (no memory ever lost)");
    let expected = starting_confidence * 0.9;
    assert!(
        (decayed.confidence - expected).abs() < 1e-4,
        "confidence must decay by ×0.9: expected {expected}, got {}",
        decayed.confidence
    );

    // ── Second run, immediately: idempotent, no re-decay ──────────────────
    let report2 = consolidator
        .run_consolidation(None)
        .await
        .expect("second run must complete");
    assert_eq!(
        report2.memories_decayed, 0,
        "a back-to-back run must NOT re-decay (idempotency marker)"
    );
    let after2 = storage
        .list_memories(MemoryFilter::default(), None)
        .await
        .expect("list after second run");
    let still = after2
        .iter()
        .find(|m| m.id == cold_id)
        .expect("fact still present after second run");
    assert!(
        (still.confidence - expected).abs() < 1e-4,
        "confidence must be unchanged on the second run: expected {expected}, got {}",
        still.confidence
    );
}
