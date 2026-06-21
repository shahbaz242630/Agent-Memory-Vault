//! A1 (ADR-084) — Phase 4 cold archive end-to-end (real bge-small).
//!
//! Proves the cold-archive pass fires through the real orchestrator: a fact
//! left untouched past `archive_after_days` is moved OUT of default retrieval
//! by `run_consolidation` (its `archived_at` marker is set), is reported in
//! `memories_archived`, is **never lost** (BRD §5.6 line 1023 — still present
//! via an explicit archive search, just out of the default scope), and a
//! back-to-back run does **not** re-archive it (idempotency, BRD §5.6 line
//! 1022 — it is no longer in the active set).
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

/// A single cold fact is archived through a real consolidation run, survives
/// (still retrievable via the explicit archive search), drops out of default
/// retrieval, and is idempotent across an immediate second run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cold_fact_is_archived_through_consolidation_and_is_never_lost() {
    let (storage, _dir) = open_sealed_storage_for_test("archive-e2e").await;
    let storage = Arc::new(storage);
    let embedder = open_bge_provider();
    let boundary = Boundary::new("personal").expect("valid boundary");

    // One fact, last accessed 400 days ago (> the default archive_after_days of
    // 365). `last_accessed` is persisted from the struct on write, so setting
    // it before insert backdates the row.
    let mut cold = make_memory_with_content("The user once used a Nokia 3310.", &boundary);
    cold.last_accessed = Utc::now() - Duration::days(400);
    let cold_id = cold.id;
    let emb = embedder.embed(&cold.content).await.expect("embed");
    insert_and_drain(&storage, vec![(cold, emb)]).await;

    // One fact in its own boundary → no clusters, no contradiction pairs → the
    // LLM is never called (a malformed canned response is the sentinel).
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

    // ── First run: the cold fact is archived ─────────────────────────────
    let report = consolidator
        .run_consolidation(None)
        .await
        .expect("archive run must complete");
    assert_eq!(
        report.memories_archived, 1,
        "the cold fact must be archived in the first run"
    );
    assert_eq!(
        report.clusters_skipped, 0,
        "the LLM merge path must never be reached (single fact, no clusters)"
    );

    // Out of default retrieval...
    let default_listed = storage
        .list_memories(MemoryFilter::default(), None)
        .await
        .expect("default list after archive");
    assert!(
        !default_listed.iter().any(|m| m.id == cold_id),
        "archived fact must be excluded from the default (active) scope"
    );

    // ...but never lost — still present (with archived_at set) via the explicit
    // archive search.
    let with_archived = storage
        .list_memories(
            MemoryFilter {
                include_archived: true,
                ..Default::default()
            },
            None,
        )
        .await
        .expect("archive search after archive");
    let archived = with_archived
        .iter()
        .find(|m| m.id == cold_id)
        .expect("the archived fact must still exist (no memory ever lost)");
    assert!(
        archived.is_archived(),
        "the surviving fact must carry the archived_at marker"
    );

    // ── Second run, immediately: idempotent, no re-archive ────────────────
    let report2 = consolidator
        .run_consolidation(None)
        .await
        .expect("second run must complete");
    assert_eq!(
        report2.memories_archived, 0,
        "a back-to-back run must NOT re-archive (fact no longer in the active set)"
    );
}
