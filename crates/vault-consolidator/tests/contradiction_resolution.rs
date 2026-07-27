//! T0.3.x A5 — nearest-neighbor contradiction detection + auto-invalidation.
//!
//! The V0.2 ship-gate: when a newer fact contradicts an older one on the same
//! subject, the nightly consolidator must detect it, `invalidate()` the stale
//! fact, so reads return only the current truth.
//!
//! ## What this pins (the bug, exactly)
//!
//! Phase-1 clustering (`phases/cluster.rs`) only forms edges at or above the
//! merge gate. A knowledge-update contradiction ("works at Vega" → "moved to
//! Atlas, left Vega") is semantically *related* but was below that gate, so the
//! pair never clustered, so `decide_merge` never saw it, so the contradiction
//! was never detected. Confirmed live in Claude Desktop on 2026-05-29
//! (`testeval` boundary, `consolidate run` → `contradictions queued: 0`, a
//! `memory_read` returned BOTH Vega and Atlas).
//!
//! The fix decouples contradiction detection from the merge gate and generates
//! candidate pairs by **nearest neighbor** (ADR-065): each fact's top-K closest
//! cosine neighbors above a floor are judged pairwise. The conflicting pair are
//! each other's nearest neighbor, so they are always surfaced — unlike K-means
//! topic grouping (the prior ADR-060 design), which split the pair across
//! groups and never judged it (proven in the §7 dogfood, 2026-06-01).
//!
//! ## ⚠️ ADR-097 changed which PATH the Vega/Atlas pair takes
//!
//! These tests originally asserted a premise — *"Vega/Atlas sits below the
//! merge gate"* — as a hardcoded `cos < 0.92`. When ADR-097 lowered the shipped
//! gate to **0.84**, that premise became false: the pair measures **0.9049**,
//! so it now **clusters** and Phase 2 (`decide_merge`) sees it BEFORE the
//! nearest-neighbour pass does.
//!
//! That is a routing change, not a weakening — `decide_merge` is
//! contradiction-aware (`MergeOutcome::{Merge, KeepSeparate, Contradiction}`)
//! and anything it leaves active is still re-examined by Phase 2b. The
//! user-visible guarantee these tests defend is unchanged and unchanged in
//! strength: **the stale fact ends up invalidated and the current one does
//! not.** The assertions therefore stay exactly as they were.
//!
//! Two consequences are handled deliberately:
//!
//! 1. **Premises are now computed against the shipped gate, never hardcoded.**
//!    A literal copy of the threshold is what let this drift go unnoticed until
//!    a diagnostic sweep caught it; the same mistake cannot silently recur.
//! 2. **The LLM stand-in must answer BOTH phases.** A single-canned-response
//!    mock returns contradiction-judge JSON to `decide_merge`'s merge schema,
//!    which fails to parse — the cluster is then logged-and-skipped and Phase 2b
//!    picks the pair up anyway, so the test would still pass, but *by accident
//!    of a parse failure*. [`PhaseAwareMock`] dispatches on the requested JSON
//!    schema so each phase gets a valid answer and the pass is real.
//!
//! Because Vega/Atlas no longer exercises the below-gate route,
//! [`below_gate_contradiction_is_caught_only_by_the_nearest_neighbour_pass`]
//! restores that coverage explicitly with a pair measured below the gate — the
//! majority of contradictions (measured range 0.7199–0.9979) still reach
//! detection only there.
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
//! are exercised (so each pair's cosine relative to the gate is proven, not
//! asserted away) and ONNX Runtime has a known macOS process-exit SIGABRT.
//! Linux + Windows CI covers it.

#![cfg(not(target_os = "macos"))]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use vault_consolidator::{Consolidator, ConsolidatorConfig};
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_llm::{CompletionParams, LlmProvider, MockLlmProvider, VaultLlmResult};
use vault_storage::{MemoryFilter, StorageBackend};

use common::{insert_and_drain, open_bge_provider, open_sealed_storage_for_test};

/// Cosine similarity for L2-normalised vectors (the `EmbeddingProvider`
/// contract guarantees normalisation) reduces to a dot product.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// The gate the product actually ships, read live from `ConsolidatorConfig`.
///
/// Never hardcode this in a premise. A `0.92` literal is precisely what let
/// these tests keep claiming "this pair is below the merge gate" after ADR-097
/// moved the gate to 0.84 and made that false.
fn shipped_gate() -> f32 {
    ConsolidatorConfig::default().merge_similarity_threshold
}

/// LLM stand-in that answers each consolidation phase with JSON valid for
/// *that* phase, dispatching on the requested JSON schema.
///
/// Needed because a pair at or above the merge gate is judged twice on one run:
/// once by Phase 2 `decide_merge` (schema keyed on `"decision"`) and, if it
/// survives active, again by the Phase 2b pairwise contradiction judge (schema
/// keyed on `"shared_attribute"`). A single-canned-response mock can only
/// satisfy one of them; feeding the wrong shape to `decide_merge` makes it fail
/// to parse, and a parse failure is logged-and-skipped — the test would still
/// go green via Phase 2b while silently proving nothing about Phase 2.
#[derive(Debug)]
struct PhaseAwareMock {
    /// Returned to `decide_merge` (Phase 2).
    merge_response: String,
    /// Returned to the pairwise contradiction judge (Phase 2b).
    contradiction_response: String,
}

#[async_trait]
impl LlmProvider for PhaseAwareMock {
    async fn complete_json(
        &self,
        _prompt: &str,
        json_schema: &str,
        _params: &CompletionParams,
    ) -> VaultLlmResult<String> {
        if json_schema.contains("shared_attribute") {
            Ok(self.contradiction_response.clone())
        } else {
            Ok(self.merge_response.clone())
        }
    }

    fn model_id(&self) -> &str {
        "phi-4-mini-test"
    }
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

    // Premise check, computed against the SHIPPED gate — never a literal
    // (ADR-097; see the module docs). This pair measures ~0.9049, so at the
    // shipped 0.84 gate it DOES cluster and Phase 2 sees it first. Recorded as
    // an assertion rather than a comment so that if a future gate change moves
    // the pair back below the line, this says so loudly instead of letting the
    // test quietly exercise a different path than it documents.
    let cos = cosine(&vega_emb, &atlas_emb);
    let gate = shipped_gate();
    assert!(
        cos >= gate,
        "A5 premise drift: Vega/Atlas measured cosine {cos:.4} is now BELOW the shipped merge \
         gate {gate} — the pair no longer clusters, so this test now exercises the \
         nearest-neighbour route rather than the merge route it documents. Re-read the ADR-097 \
         section in this file's module docs and re-point the tests deliberately."
    );

    insert_and_drain(&storage, vec![(vega, vega_emb), (atlas, atlas_emb)]).await;

    // Phi-4 stand-in for BOTH phases the pair now passes through (ADR-097).
    //
    // Phase 2 (`decide_merge`) sees it first because it clusters. The mock
    // answers `contradiction` with NO `clear_winner` — deliberately the WEAKER
    // branch: it queues a review and leaves both facts active rather than
    // resolving them. That keeps the burden on Phase 2b, so this test still
    // proves the nearest-neighbour pass retires the stale fact, exactly as it
    // did before the gate moved.
    //
    // Phase 2b (pairwise judge, ADR-062 iter 2): the model only DETECTS the
    // contradiction (contradiction=true + shared_attribute); CODE then retires
    // the OLDER fact by recency (the Bug-1 fix) — Vega (valid_from Jan) is
    // older than Atlas (Apr), so Vega is invalidated regardless of the model's
    // `stale` label.
    let llm = Arc::new(PhaseAwareMock {
        merge_response: r#"{"decision":"contradiction","reasoning":"same employer attribute with conflicting values"}"#.to_string(),
        contradiction_response: r#"{"shared_attribute":"employer","contradiction":true,"stale":"a","reasoning":"Atlas explicitly supersedes Vega; the user left Vega Bridgeworks"}"#.to_string(),
    });

    let consolidator = Consolidator::new(
        storage.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    consolidator
        .run_consolidation(None)
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

/// R2 (ADR-082) — incremental cross-corpus contradiction. The NEW fact (Atlas,
/// created after the watermark) is the ONLY seed; the OLD fact (Vega, created
/// before it) is not. Incremental Phase 2b must still surface the pair (the
/// seed's LanceDB neighbour search hits the whole corpus) and retire the stale
/// Vega — proving a nightly run scoped to today's new facts does NOT miss a
/// contradiction against yesterday's facts (the recall loss the cross-corpus
/// invariant forbids).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incremental_seed_retires_stale_old_fact() {
    let (storage, _dir) = open_sealed_storage_for_test("a5-incremental-cross-corpus").await;
    let storage = Arc::new(storage);
    let embedder = open_bge_provider();
    let boundary = Boundary::new("testeval").expect("valid boundary");

    let jan = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let apr = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();

    // OLD fact: written + drained first, so its created_at precedes the watermark.
    let vega = fact(
        "As of 2026-01-10 the user worked as a structural engineer at Vega Bridgeworks.",
        &boundary,
        jan,
    );
    let vega_id = vega.id;
    let vega_emb = embedder.embed(&vega.content).await.expect("embed vega");
    insert_and_drain(&storage, vec![(vega, vega_emb)]).await;

    // Watermark AFTER the old fact: the incremental run seeds only on facts
    // created from here on.
    let watermark = Utc::now();

    // NEW fact (the only seed): created after the watermark; contradicts the old.
    let atlas = fact(
        "As of 2026-04-01 the user works as a structural engineer at Atlas Structures, \
         having left Vega Bridgeworks.",
        &boundary,
        apr,
    );
    let atlas_id = atlas.id;
    let atlas_emb = embedder.embed(&atlas.content).await.expect("embed atlas");
    insert_and_drain(&storage, vec![(atlas, atlas_emb)]).await;

    // Phase-aware for the same reason as the test above (ADR-097): at the
    // shipped gate this pair clusters, so Phase 2 is consulted before Phase 2b.
    // `contradiction` with no `clear_winner` leaves both facts active, so the
    // cross-corpus guarantee this test exists for is still proven by Phase 2b.
    let llm = Arc::new(PhaseAwareMock {
        merge_response: r#"{"decision":"contradiction","reasoning":"same employer attribute with conflicting values"}"#.to_string(),
        contradiction_response: r#"{"shared_attribute":"employer","contradiction":true,"stale":"a","reasoning":"Atlas explicitly supersedes Vega"}"#.to_string(),
    });
    let consolidator = Consolidator::new(
        storage.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    // Incremental run: seed = {atlas} only. The old Vega is reached via the
    // seed's neighbour search, judged, and retired by recency.
    consolidator
        .run_consolidation(Some(watermark))
        .await
        .expect("incremental consolidation run must succeed");

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
        .expect("vega row exists");
    let atlas_row = all
        .iter()
        .find(|m| m.id == atlas_id)
        .expect("atlas row exists");
    assert!(
        vega_row.valid_until.is_some(),
        "R2: an incremental run seeded ONLY on the new Atlas fact must still retire the OLD Vega \
         fact (cross-corpus invariant); valid_until None → the new-vs-old contradiction was missed"
    );
    assert!(
        atlas_row.valid_until.is_none(),
        "R2: the current Atlas fact must stay valid"
    );
}

/// ADR-097 coverage restoration — a contradiction that the merge gate CANNOT
/// reach must still be caught, by the nearest-neighbour pass alone.
///
/// The A5 + R2 tests above used to carry this guarantee via their "below the
/// gate" premise. Lowering the shipped gate to 0.84 pulled their Vega/Atlas
/// fixture (0.9049) above the line, so they now exercise the merge route and
/// that coverage would have been silently lost. This restores it explicitly.
///
/// **Fixture choice is measured, not assumed** (`vault-embedding`'s
/// `paraphrase_cluster_gate` example): this pair sits at cosine **0.8196** —
/// below the 0.84 merge gate (so Phase 1 forms no edge and `decide_merge` is
/// never consulted) and well above the 0.70
/// `CONTRADICTION_NN_SIMILARITY_FLOOR` (so the pairwise judge is still offered
/// the pair). Margins: **0.0204 to the gate** and **0.1196 to the NN floor** —
/// ~20x and ~120x the sub-1e-3 cross-platform embedding noise respectively, so
/// the routing holds across the Linux + Windows CI legs.
///
/// ⚠️ The gate-side margin is the tighter one and it SHRANK when 0.84 shipped
/// (it was 0.07 at 0.89). Any further gate reduction must re-measure this
/// fixture first — at a gate of 0.82 or below this pair would cluster and this
/// test would quietly stop testing the nearest-neighbour route. The premise
/// assertion below is what makes that failure loud instead of silent.
///
/// This matters beyond bookkeeping: the measured contradiction range is
/// 0.7199–0.9979, and the majority of that mass sits below the gate. The
/// nearest-neighbour pass is the only thing standing between those facts and a
/// vault that serves stale answers forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn below_gate_contradiction_is_caught_only_by_the_nearest_neighbour_pass() {
    let (storage, _dir) = open_sealed_storage_for_test("a5-below-gate-nn-only").await;
    let storage = Arc::new(storage);
    let embedder = open_bge_provider();
    let boundary = Boundary::new("testeval").expect("valid boundary");

    let jan = Utc.with_ymd_and_hms(2026, 1, 10, 0, 0, 0).unwrap();
    let apr = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();

    let old_pref = fact(
        "The user's favourite programming language is Rust.",
        &boundary,
        jan,
    );
    let new_pref = fact(
        "The user's favourite programming language is Python.",
        &boundary,
        apr,
    );
    let old_id = old_pref.id;
    let new_id = new_pref.id;

    let old_emb = embedder.embed(&old_pref.content).await.expect("embed old");
    let new_emb = embedder.embed(&new_pref.content).await.expect("embed new");

    // Premise: BOTH sides of the routing must hold, or this test is not
    // testing what it claims. Computed against the shipped gate, never a
    // literal — that is the ADR-097 lesson.
    let cos = cosine(&old_emb, &new_emb);
    let gate = shipped_gate();
    assert!(
        cos < gate,
        "premise: this fixture must sit BELOW the shipped merge gate {gate} (measured {cos:.4}) \
         — if it clusters, Phase 2 handles it and the nearest-neighbour route is untested here"
    );
    assert!(
        cos >= 0.70,
        "premise: this fixture must sit at or above CONTRADICTION_NN_SIMILARITY_FLOOR 0.70 \
         (measured {cos:.4}) — below it the pairwise judge is never offered the pair and this \
         test would pass vacuously"
    );

    insert_and_drain(&storage, vec![(old_pref, old_emb), (new_pref, new_emb)]).await;

    // Single-response mock is correct here precisely BECAUSE the merge phase is
    // never reached — if a future change makes this pair cluster, `decide_merge`
    // would receive this contradiction-judge JSON, fail to parse, and the
    // cluster would be skipped. The premise assertion above catches that first.
    let llm = Arc::new(MockLlmProvider::new(
        "phi-4-mini-test",
        r#"{"shared_attribute":"favourite programming language","contradiction":true,"stale":"a","reasoning":"same attribute, conflicting values"}"#,
    ));

    let consolidator = Consolidator::new(
        storage.clone(),
        llm.clone(),
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    consolidator
        .run_consolidation(None)
        .await
        .expect("consolidation run must succeed");

    // Non-vacuity guard. The neighbouring co-topical test silently stopped
    // exercising its judge when its fixture drifted below the NN floor
    // (measured 0.6164); an assertion on real model consultation is what would
    // have caught that.
    assert!(
        llm.call_count() >= 1,
        "the pairwise contradiction judge was never consulted — the pair never became a \
         candidate, so this test proved nothing"
    );

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
    let old_row = all.iter().find(|m| m.id == old_id).expect("old row exists");
    let new_row = all.iter().find(|m| m.id == new_id).expect("new row exists");

    assert!(
        old_row.valid_until.is_some(),
        "a BELOW-gate contradiction must still be retired — the nearest-neighbour pass is the \
         only route that can reach it, and most contradictions live here"
    );
    assert!(
        new_row.valid_until.is_none(),
        "the current fact must stay valid — only the older side of the pair is retired"
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
        .run_consolidation(None)
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
