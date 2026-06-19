//! T0.3.x — Phase-2 merge resilience (ADR-062 iteration 2) + skip accounting
//! (ADR-063).
//!
//! **Failing-first** for the fix to a live crash (2026-05-30): merging the
//! CAP_OK content-ceiling probes made Phi-4's `merged_text` exceed the default
//! 256-token budget, truncating the JSON mid-string. The parse error
//! propagated through `decide_merge(...).await?` and **aborted the entire
//! consolidation run** (`error: consolidation run failed`, exit 1).
//!
//! The fix makes the orchestrator's Phase-2 loop log-and-skip a failed cluster
//! (mirroring the topic contradiction pass) instead of propagating. This test
//! pins the resilience: a malformed merge response must NOT abort the run, the
//! cluster's members must survive (active, unmerged), and the skip must now be
//! COUNTED in `ConsolidationReport.clusters_skipped` (ADR-063 — closes the
//! "skips are invisible" gap).
//!
//! **Why a constant-vector mock embedder (not real BGE).** ADR-063's
//! deterministic dedup now intercepts near-identical clusters BEFORE the LLM,
//! so two *identical* facts (the original fixture) would dedup, not reach
//! `decide_merge`. To still exercise the LLM-merge skip path we need a cluster
//! that forms but is NOT near-identical: the mock returns one constant unit
//! vector for every input (so any two facts cluster at cosine 1.0), while the
//! two facts are chosen with near-disjoint wording (lexical containment well
//! below the 0.80 gate) so the dedup gate declines and the cluster falls
//! through to the LLM. This makes the test platform-independent (no model
//! load) — real-BGE clustering is covered separately by `dedup_integration.rs`.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use vault_consolidator::{Consolidator, ConsolidatorConfig};
use vault_core::{Boundary, Memory, MemoryType, NewMemory, VaultResult};
use vault_embedding::{EmbeddingProvider, EMBEDDING_DIM};
use vault_llm::MockLlmProvider;
use vault_storage::MemoryFilter;

use common::{insert_and_drain, open_sealed_storage_for_test};

/// Returns one constant L2-normalised unit vector for every input, so any two
/// memories cluster (cosine 1.0). Lexical containment — computed from content,
/// not vectors — is what then keeps non-near-identical pairs out of dedup.
struct ConstantEmbedder;

fn constant_vector() -> Vec<f32> {
    let mut v = vec![0.0_f32; EMBEDDING_DIM];
    v[0] = 1.0; // unit norm
    v
}

#[async_trait]
impl EmbeddingProvider for ConstantEmbedder {
    async fn embed(&self, _text: &str) -> VaultResult<Vec<f32>> {
        Ok(constant_vector())
    }
}

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

/// A malformed Phase-2 response (truncated/non-JSON, as a token-budget overflow
/// produces) must be logged-and-skipped, NOT abort the run; the skip is counted
/// in the report; the two members remain active and unmerged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn malformed_merge_response_does_not_abort_the_run() {
    let (storage, _dir) = open_sealed_storage_for_test("merge-resilience-skip").await;
    let storage = Arc::new(storage);
    let embedder = Arc::new(ConstantEmbedder);
    let boundary = Boundary::new("testeval").expect("valid boundary");

    // Two near-disjoint facts → constant embedder makes them cluster (cosine
    // 1.0 ≥ 0.92) but lexical containment is far below 0.80, so ADR-063 dedup
    // declines and the cluster reaches decide_merge.
    let a = fact(
        "The user's project codename is Helios and it ships in the third quarter.",
        &boundary,
    );
    let b = fact(
        "Aurora is the laptop the user bought last winter for frequent travel.",
        &boundary,
    );
    let a_id = a.id;
    let b_id = b.id;
    insert_and_drain(
        &storage,
        vec![(a, constant_vector()), (b, constant_vector())],
    )
    .await;

    // Mock returns malformed JSON for every call — simulating the truncated
    // merged_text that overflowed the token budget live. decide_merge fails to
    // parse; the orchestrator must skip the cluster and finish the run.
    let llm = Arc::new(MockLlmProvider::new(
        "phi-4-mini-test",
        "{\"decision\":\"merge\",\"merged_te",
    ));

    let consolidator = Consolidator::new(
        storage.clone(),
        llm,
        embedder,
        ConsolidatorConfig::default(),
    );

    // THE assertion: the run completes (Ok), it does NOT abort.
    let report = consolidator
        .run_consolidation(None)
        .await
        .expect("a malformed merge response MUST NOT abort the consolidation run");

    // ADR-063: the skip is COUNTED, not silent. The cluster reached the LLM
    // (not dedup), so it is a skip — not a dedup.
    assert_eq!(
        report.clusters_skipped, 1,
        "the failed merge cluster must be counted as skipped"
    );
    assert_eq!(
        report.clusters_deduped, 0,
        "a low-containment cluster must NOT be deduped"
    );

    // Both members survive, active and unmerged (no supersession).
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
    for id in [a_id, b_id] {
        let row = all
            .iter()
            .find(|m| m.id == id)
            .expect("row must still exist");
        assert!(
            row.valid_until.is_none() && row.superseded_by.is_none(),
            "a skipped-merge member {id} must stay active and unmerged"
        );
    }
}
