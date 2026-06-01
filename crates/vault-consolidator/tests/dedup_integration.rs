//! T0.3.x — ADR-063 deterministic-dedup end-to-end (real bge-small).
//!
//! Proves the Phase 2-pre dedup path fires through the real orchestrator with
//! real embeddings: two near-identical facts cluster, the dedup gate fires,
//! one is superseded into a canonical survivor (aggregates rolled), and the
//! LLM merge path is **never reached** — the case that previously overflowed
//! Phi-4's token budget and skipped forever.
//!
//! `#![cfg(not(target_os = "macos"))]` per ADR-033 — real BGE embeddings are
//! exercised so the facts genuinely cluster (cosine ≥ 0.92). Linux + Windows
//! CI covers it. (The skip/resilience path is covered platform-independently
//! by `merge_resilience.rs` via a constant-vector mock embedder.)

#![cfg(not(target_os = "macos"))]

mod common;

use std::sync::Arc;

use chrono::Utc;
use vault_consolidator::{Consolidator, ConsolidatorConfig};
use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_llm::MockLlmProvider;
use vault_storage::MemoryFilter;

use common::{insert_and_drain, open_bge_provider, open_sealed_storage_for_test};

fn fact(content: &str, boundary: &Boundary) -> Memory {
    Memory::try_new(NewMemory {
        content: content.into(),
        memory_type: MemoryType::Semantic,
        boundary: boundary.clone(),
        source_agent: Some("claude-opus-4-8".into()),
        confidence: 0.95,
        valid_from: Some(Utc::now()),
        valid_until: None,
        metadata: serde_json::json!({}),
    })
    .expect("valid memory")
}

/// Two near-identical facts must be resolved by deterministic dedup — no LLM
/// merge, no new merged row — leaving one canonical survivor and one
/// superseded member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn near_identical_cluster_is_deduped_without_llm() {
    let (storage, _dir) = open_sealed_storage_for_test("dedup-near-identical").await;
    let storage = Arc::new(storage);
    let embedder = open_bge_provider();
    let boundary = Boundary::new("testeval").expect("valid boundary");

    // Identical content → cosine ~1.0 ≥ 0.93 AND containment 1.0 ≥ 0.80 →
    // near-identical gate fires → deterministic dedup (no LLM).
    let a = fact(
        "The user's project codename is Helios and it ships in Q3.",
        &boundary,
    );
    let b = fact(
        "The user's project codename is Helios and it ships in Q3.",
        &boundary,
    );
    let a_id = a.id;
    let b_id = b.id;
    let a_emb = embedder.embed(&a.content).await.expect("embed");
    let b_emb = embedder.embed(&b.content).await.expect("embed");
    insert_and_drain(&storage, vec![(a, a_emb), (b, b_emb)]).await;

    // A malformed LLM response is the sentinel: if the dedup path FAILED to
    // intercept and the cluster reached decide_merge, the parse error would
    // make clusters_skipped == 1. Dedup firing means the LLM is never called.
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

    let report = consolidator
        .run_consolidation()
        .await
        .expect("dedup run must complete");

    // The dedup fired; the LLM merge path was never reached.
    assert_eq!(
        report.clusters_deduped, 1,
        "near-identical cluster must be deduped"
    );
    assert_eq!(
        report.memories_deduped, 1,
        "one member superseded into the survivor"
    );
    assert_eq!(
        report.clusters_skipped, 0,
        "no LLM merge skip — dedup intercepted before decide_merge (sentinel)"
    );

    // Structural: exactly two rows total (NO new merged row — dedup keeps an
    // existing survivor); exactly one superseded, pointing at the other.
    let all = storage
        .list_memories(
            MemoryFilter {
                include_superseded: true,
                ..MemoryFilter::default()
            },
            None,
        )
        .await
        .expect("list memories");
    assert_eq!(all.len(), 2, "dedup must NOT write a new merged row");

    let superseded: Vec<&Memory> = all.iter().filter(|m| m.superseded_by.is_some()).collect();
    let active: Vec<&Memory> = all.iter().filter(|m| m.superseded_by.is_none()).collect();
    assert_eq!(superseded.len(), 1, "exactly one loser superseded");
    assert_eq!(
        active.len(),
        1,
        "exactly one canonical survivor remains active"
    );

    let survivor = active[0];
    let loser = superseded[0];
    assert_eq!(
        loser.superseded_by,
        Some(survivor.id),
        "the loser must be superseded → the surviving member"
    );
    assert!(
        [a_id, b_id].contains(&survivor.id) && [a_id, b_id].contains(&loser.id),
        "survivor and loser must both be original members (no synthetic merged id)"
    );
}
