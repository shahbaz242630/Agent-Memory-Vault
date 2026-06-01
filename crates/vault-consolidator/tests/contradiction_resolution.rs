//! T0.3.x A5 — nearest-neighbor contradiction detection + auto-invalidation.
//!
//! The V0.2 ship-gate: when a newer fact contradicts an older one on the same
//! subject, the nightly consolidator must detect it, `invalidate()` the stale
//! fact, so reads return only the current truth.
//!
//! ## What this pins (the bug, exactly)
//!
//! Phase-1 clustering (`phases/cluster.rs`) only forms edges at cosine
//! **≥ 0.92** (the merge gate). A knowledge-update contradiction
//! ("works at Vega" → "moved to Atlas, left Vega") is semantically
//! *related* but sits **below** 0.92, so the pair never clusters, so
//! `decide_merge` never sees it, so the contradiction is never detected.
//! Confirmed live in Claude Desktop on 2026-05-29 (`testeval` boundary,
//! `consolidate run` → `contradictions queued: 0`, a `memory_read` returned
//! BOTH Vega and Atlas).
//!
//! The fix decouples contradiction detection from the 0.92 merge gate and
//! generates candidate pairs by **nearest neighbor** (ADR-065): each fact's
//! top-K closest cosine neighbors above a floor are judged pairwise. The
//! conflicting pair are each other's nearest neighbor, so they are always
//! surfaced — unlike K-means topic grouping (the prior ADR-060 design), which
//! split the pair across groups and never judged it (proven in the §7 dogfood,
//! 2026-06-01). The 0.92 gate stays as-is for merging near-duplicates.
//!
//! ## Fixture provenance
//!
//! The Vega→Atlas pair + their content dates (2026-01-10 → 2026-04-01) are
//! the exact strings from the 2026-05-29 Claude Desktop server log so the
//! test mirrors the live failure rather than a synthetic approximation.
//!
//! ## macOS deferral
//!
//! `#![cfg(not(target_os = "macos"))]` per ADR-033 — real BGE embeddings
//! are exercised (so the sub-0.92 cosine is proven, not asserted away) and
//! ONNX Runtime has a known macOS process-exit SIGABRT. Linux + Windows CI
//! covers it.

#![cfg(not(target_os = "macos"))]

mod common;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use vault_consolidator::{Consolidator, ConsolidatorConfig};
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_llm::MockLlmProvider;
use vault_storage::{MemoryFilter, StorageBackend};

use common::{insert_and_drain, open_bge_provider, open_sealed_storage_for_test};

/// Cosine similarity for L2-normalised vectors (the `EmbeddingProvider`
/// contract guarantees normalisation) reduces to a dot product.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Build a `Memory` with an explicit fact-time (`valid_from`) so the test
/// reflects the real-world dates in the content rather than write-time.
fn fact(content: &str, boundary: &Boundary, valid_from: DateTime<Utc>) -> Memory {
    Memory::try_new(NewMemory {
        content: content.into(),
        memory_type: MemoryType::Semantic,
        boundary: boundary.clone(),
        source_agent: Some("claude-opus-4-8".into()),
        confidence: 0.95,
        valid_from: Some(valid_from),
        valid_until: None,
        metadata: serde_json::json!({}),
    })
    .expect("valid memory")
}

/// THE A5 ship-gate. Vega (older) + Atlas (newer) on one boundary, below the
/// 0.92 merge gate. After consolidation the stale Vega fact MUST be
/// invalidated and the current Atlas fact MUST stay valid. The pair is
/// surfaced by nearest-neighbor candidate generation (ADR-065), not K-means
/// topic grouping.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nearest_neighbor_contradiction_retires_stale_employment_fact() {
    let (storage, _dir) = open_sealed_storage_for_test("a5-contradiction-vega-atlas").await;
    let storage = Arc::new(storage);
    let embedder = open_bge_provider();
    let boundary = Boundary::new("testeval").expect("valid boundary");

    let jan = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let apr = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();

    let vega = fact(
        "As of 2026-01-10 the user worked as a structural engineer at Vega Bridgeworks.",
        &boundary,
        jan,
    );
    let atlas = fact(
        "As of 2026-04-01 the user works as a structural engineer at Atlas Structures, \
         having left Vega Bridgeworks.",
        &boundary,
        apr,
    );
    let vega_id = vega.id;
    let atlas_id = atlas.id;

    let vega_emb = embedder.embed(&vega.content).await.expect("embed vega");
    let atlas_emb = embedder.embed(&atlas.content).await.expect("embed atlas");

    // Premise check: the pair MUST sit below the 0.92 merge gate — that is
    // the whole reason Phase-1 clustering can't catch this. If this ever
    // fails, the bug's premise changed and the rest of the test is moot.
    let cos = cosine(&vega_emb, &atlas_emb);
    assert!(
        cos < 0.92,
        "A5 premise: Vega/Atlas must be below the 0.92 merge gate (measured cosine {cos:.4}); \
         if they cluster, Phase-1 merge would handle them and topic-level detection is unneeded"
    );

    insert_and_drain(&storage, vec![(vega, vega_emb), (atlas, atlas_emb)]).await;

    // Phi-4 stand-in (pairwise judge, ADR-062 iter 2): the Vega/Atlas group is
    // a single pair → exactly one `complete_json` call. The model only DETECTS
    // the contradiction (contradiction=true + shared_attribute); CODE then
    // retires the OLDER fact by recency (the Bug-1 fix) — Vega (valid_from Jan)
    // is older than Atlas (Apr), so Vega is invalidated regardless of the
    // model's `stale` label.
    let llm = Arc::new(MockLlmProvider::new(
        "phi-4-mini-test",
        r#"{"shared_attribute":"employer","contradiction":true,"stale":"a","reasoning":"Atlas explicitly supersedes Vega; the user left Vega Bridgeworks"}"#,
    ));

    let consolidator = Consolidator::new(
        storage.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    consolidator
        .run_consolidation()
        .await
        .expect("consolidation run must succeed");

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

    let vega_row = all
        .iter()
        .find(|m| m.id == vega_id)
        .expect("vega row must still exist (invalidated, not deleted)");
    let atlas_row = all
        .iter()
        .find(|m| m.id == atlas_id)
        .expect("atlas row must still exist");

    assert!(
        vega_row.valid_until.is_some(),
        "A5: the stale Vega fact MUST be invalidated (valid_until set) by topic-level \
         contradiction detection. valid_until is None → contradiction was never detected \
         (this is the current bug: detection is gated behind the 0.92 merge cluster)."
    );
    assert!(
        atlas_row.valid_until.is_none(),
        "A5: the current Atlas fact MUST stay valid (valid_until None) — only the loser is retired"
    );
}

/// Adversarial guard (the false-positive risk of looser candidate pairing):
/// two related facts that are NOT contradictory must both survive. "works at
/// Atlas" + "commutes by train" are co-topical (employment) but compatible. If
/// they are close enough to become a candidate pair, Phi-4 returns
/// `contradiction=false`; the consolidator must NOT invalidate either. Guards
/// against the nearest-neighbor pairing over-invalidating.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn co_topical_but_compatible_facts_are_not_falsely_invalidated() {
    let (storage, _dir) = open_sealed_storage_for_test("a5-no-false-positive").await;
    let storage = Arc::new(storage);
    let embedder = open_bge_provider();
    let boundary = Boundary::new("testeval").expect("valid boundary");

    let now = Utc::now();
    let employer = fact(
        "As of 2026-04-01 the user works as a structural engineer at Atlas Structures.",
        &boundary,
        now,
    );
    let commute = fact(
        "The user commutes to work by train every weekday.",
        &boundary,
        now,
    );
    let employer_id = employer.id;
    let commute_id = commute.id;

    let employer_emb = embedder.embed(&employer.content).await.expect("embed");
    let commute_emb = embedder.embed(&commute.content).await.expect("embed");

    insert_and_drain(
        &storage,
        vec![(employer, employer_emb), (commute, commute_emb)],
    )
    .await;

    // Phi-4 stand-in (pairwise judge, ADR-062 iter 2): the single pair is
    // compatible (different attributes) — shared_attribute=null,
    // contradiction=false, stale="neither". No invalidation expected.
    let llm = Arc::new(MockLlmProvider::new(
        "phi-4-mini-test",
        r#"{"shared_attribute":null,"contradiction":false,"stale":"neither","reasoning":"distinct compatible facts about the same person; no contradiction"}"#,
    ));

    let consolidator = Consolidator::new(
        storage.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    consolidator
        .run_consolidation()
        .await
        .expect("consolidation run must succeed");

    assert_neither_invalidated(&storage, employer_id, commute_id).await;
}

// Mass-invalidate safety net (a runaway model can't wipe the active set):
// under pairwise judging (ADR-062) with recency-deterministic stale selection
// (the Bug-1 fix), the globally-newest fact in a conflict chain is never
// flagged (it is never the older side of any pair), so a run cannot sweep the
// entire active set. The orchestrator's whole-set-refusal guard is therefore
// belt-and-braces and is covered reliably at the unit layer:
//   - `consolidator::tests::resolve_stale_*` (the orchestrator refuses a sweep,
//     dedups, and ignores out-of-group ids), and
//   - `phases::contradiction::tests::aggregator_recency_keeps_only_the_newest_in_a_conflict_chain`
//     (recency retires the older members and keeps the newest).
// The two end-to-end tests above still exercise the orchestrator's
// `resolve_stale_ids` wiring through its Invalidate and Nothing branches.

async fn assert_neither_invalidated(storage: &StorageBackend, a: MemoryId, b: MemoryId) {
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
    for id in [a, b] {
        let row = all.iter().find(|m| m.id == id).expect("row must exist");
        assert!(
            row.valid_until.is_none(),
            "co-topical compatible fact {id} MUST NOT be invalidated — \
             keep_separate verdict means no action"
        );
    }
}
