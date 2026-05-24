//! Integration tests for [`AbstainingRetriever`] — the Phase 3
//! abstain-gate decorator that short-circuits to an empty result when
//! the BM25 channel's top score falls below a calibrated threshold.
//!
//! The tests prove the Phase-3 contract surface:
//!
//! - **Abstain triggers on hard negatives** — queries whose tokens
//!   don't appear in any indexed memory short-circuit to empty.
//! - **Abstain skips on strong anchors** — queries with real BM25
//!   matches above threshold pass through to the inner retriever.
//! - **Threshold tunable** — `AbstainConfig::bm25_top_score_threshold`
//!   is honored; custom thresholds change behaviour deterministically.
//! - **Compositional invariants** — Q1 / Q2 / Q3 contracts pass
//!   through; boundary isolation honored at the abstain probe + the
//!   inner retriever level.
//! - **Score-range pass-through** — when not abstaining, the inner
//!   retriever's score range is preserved unchanged (e.g., RRF scores
//!   in [0, 0.0328] when inner is `HybridRetriever`).
//!
//! ## Threshold note for tests
//!
//! Tantivy BM25 scores are corpus-size + token-rarity sensitive. In a
//! tiny test corpus (2–5 docs), a single rare anchor token scores ~0.5
//! to ~4.0. The V0.2 production default is `1.0` (see
//! `src/strategies/abstain.rs` for the rationale — at hand-curated
//! scale the gate's job is reduced to catching genuine-zero-signal
//! queries). Hard-negative tests use the production default because
//! their queries produce ZERO BM25 hits → max score = 0 → abstain
//! fires at any positive threshold. "Should-not-abstain" tests use an
//! even-lower explicit threshold OR seed a larger corpus to push the
//! anchor BM25 score above the test threshold.

#![forbid(unsafe_code)]

mod common;

use std::sync::Arc;

use vault_core::{Boundary, MemoryId};
use vault_embedding::EMBEDDING_DIM;
use vault_retrieval::{
    AbstainConfig, AbstainingRetriever, HybridRetriever, KeywordIndex, KeywordRetriever, Retriever,
};
use vault_storage::{MetadataStore, VectorStore};

use common::{boundary, make_memory, make_test_retriever, query};

/// Bundle for abstain tests. Holds:
/// - `abstain`: the Retriever-under-test (wraps `inner` + `keyword`).
/// - `inner`: a HybridRetriever (sem + kw) — the typical production
///   wiring the abstain wraps.
/// - `keyword_index`, `metadata`, `vectors`: the underlying stores so
///   tests can insert memories.
struct AbstainSetup {
    abstain: AbstainingRetriever,
    metadata: Arc<MetadataStore>,
    vectors: Arc<dyn VectorStore>,
    keyword_index: Arc<KeywordIndex>,
    _dir: tempfile::TempDir,
}

async fn setup_abstain(cfg: AbstainConfig) -> AbstainSetup {
    let tr = make_test_retriever().await;
    let keyword_index = Arc::new(KeywordIndex::new().expect("kw index"));
    let semantic: Arc<dyn Retriever> = Arc::new(tr.retriever);
    let keyword: Arc<dyn Retriever> = Arc::new(KeywordRetriever::new(
        keyword_index.clone(),
        tr.metadata.clone(),
    ));
    let hybrid: Arc<dyn Retriever> =
        Arc::new(HybridRetriever::new(semantic.clone(), keyword.clone()));
    // Production wiring: abstain wraps hybrid (inner) and probes keyword
    // (the BM25 channel) for the threshold check.
    let abstain = AbstainingRetriever::with_config(hybrid, keyword, cfg);
    AbstainSetup {
        abstain,
        metadata: tr.metadata,
        vectors: tr.vectors,
        keyword_index,
        _dir: tr._dir,
    }
}

/// Insert a memory into all three stores: metadata + vector (with
/// drift) + keyword index. Returns the assigned id.
async fn insert_full(s: &AbstainSetup, content: &str, b: &Boundary, drift: usize) -> MemoryId {
    let m = make_memory(content, b);
    let id = m.id;
    s.metadata.create_memory(&m).await.expect("create_memory");

    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    emb[0] = 1.0;
    if drift > 0 && drift + 1 < EMBEDDING_DIM {
        emb[drift + 1] = (drift as f32) * 1e-3;
    }
    let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
    for x in &mut emb {
        *x /= norm;
    }
    s.vectors.upsert(&id, &emb, b).await.expect("vector upsert");

    s.keyword_index
        .insert(id, content)
        .await
        .expect("keyword insert");
    id
}

// ── Abstain triggers on hard negatives ───────────────────────────────────

#[tokio::test]
async fn abstain_fires_on_hard_negative() {
    let s = setup_abstain(AbstainConfig::default()).await;
    let b = boundary("work");

    // Seed memories about cats and dogs — NONE about Kubernetes.
    insert_full(&s, "Cat photos from yesterday", &b, 0).await;
    insert_full(&s, "Dog training notes from last week", &b, 1).await;
    insert_full(&s, "Cat behaviour observations notebook", &b, 2).await;

    // Query for a token that exists in zero memories.
    let results = s
        .abstain
        .retrieve(query("KUBERNETES_K8S_HARD_NEG", vec![b], 10))
        .await
        .expect("retrieve");

    assert!(
        results.is_empty(),
        "abstain MUST fire on hard negative (zero BM25 matches → max_score=0 < V0.2 default threshold=1.0)"
    );
}

#[tokio::test]
async fn abstain_fires_when_no_matches_default_threshold() {
    let s = setup_abstain(AbstainConfig::default()).await;
    let b = boundary("work");

    // Single memory, query token absent.
    insert_full(&s, "Lorem ipsum dolor sit amet", &b, 0).await;

    let results = s
        .abstain
        .retrieve(query("ABSENT_ANCHOR_TOKEN_X9Z", vec![b], 5))
        .await
        .expect("retrieve");

    assert!(results.is_empty(), "no BM25 matches → abstain fires");
}

// ── Abstain skips on strong anchors ──────────────────────────────────────

#[tokio::test]
async fn abstain_skips_on_strong_anchor_low_threshold() {
    // Calibrated low threshold so a tiny-corpus BM25 hit clears it.
    // V0.2 production default is 1.0; tests with 2-3 docs may not even
    // reach 1.0 on a perfect match because IDF is tiny. Using an
    // explicitly-lower threshold here keeps the test focused on the
    // "abstain does not fire when signal is present" contract,
    // independent of whatever the production default happens to be.
    let s = setup_abstain(AbstainConfig {
        bm25_top_score_threshold: 0.1,
    })
    .await;
    let b = boundary("work");

    let target_id = insert_full(&s, "COMCAST_ANCHOR_42 monthly bill review", &b, 0).await;
    insert_full(&s, "Unrelated grocery list memo", &b, 1).await;

    let results = s
        .abstain
        .retrieve(query("COMCAST_ANCHOR_42", vec![b], 10))
        .await
        .expect("retrieve");

    assert!(
        !results.is_empty(),
        "strong-anchor match must NOT abstain at threshold=0.1"
    );
    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(ids.contains(&target_id));
}

// ── Threshold tunable + respected ────────────────────────────────────────

#[tokio::test]
async fn threshold_respected() {
    let b = boundary("work");

    // Build two setups with the SAME content but DIFFERENT thresholds.
    // Low threshold passes; impossibly-high threshold abstains.
    let s_low = setup_abstain(AbstainConfig {
        bm25_top_score_threshold: 0.1,
    })
    .await;
    let s_high = setup_abstain(AbstainConfig {
        bm25_top_score_threshold: 1000.0,
    })
    .await;

    for s in [&s_low, &s_high] {
        insert_full(s, "RARE_TOKEN_OMEGA_99 in some content", &b, 0).await;
        insert_full(s, "Other unrelated content", &b, 1).await;
    }

    let q = || query("RARE_TOKEN_OMEGA_99", vec![b.clone()], 5);

    let low = s_low.abstain.retrieve(q()).await.expect("low");
    assert!(
        !low.is_empty(),
        "threshold=0.1 must not abstain on real match"
    );

    let high = s_high.abstain.retrieve(q()).await.expect("high");
    assert!(
        high.is_empty(),
        "threshold=1000.0 must abstain even on real match (score never reaches threshold)"
    );
}

#[tokio::test]
async fn custom_config_respected() {
    // Construct via with_config and verify the threshold takes effect.
    let cfg = AbstainConfig {
        bm25_top_score_threshold: 0.05,
    };
    let s = setup_abstain(cfg).await;
    let b = boundary("work");

    insert_full(&s, "SMOKE_CONFIG_TOKEN ABC123", &b, 0).await;

    let results = s
        .abstain
        .retrieve(query("SMOKE_CONFIG_TOKEN", vec![b], 5))
        .await
        .expect("retrieve");
    assert!(
        !results.is_empty(),
        "very-low custom threshold (0.05) must permit pass-through"
    );
}

// ── Compositional invariants ─────────────────────────────────────────────

#[tokio::test]
async fn empty_authorized_boundaries_short_circuits() {
    let s = setup_abstain(AbstainConfig::default()).await;
    let b = boundary("work");
    insert_full(&s, "Anything searchable here", &b, 0).await;

    // Q1: empty authorized_boundaries → Ok(empty), no error, no BM25
    // probe round-trip (the wrapper short-circuits before the keyword
    // call).
    let results = s
        .abstain
        .retrieve(query("Anything", vec![], 5))
        .await
        .expect("retrieve");
    assert!(results.is_empty());
}

#[tokio::test]
async fn empty_query_returns_error() {
    let s = setup_abstain(AbstainConfig::default()).await;
    let b = boundary("work");
    insert_full(&s, "Indexed memory", &b, 0).await;

    // Q2: empty/whitespace query → InvalidInput.
    for q in ["", "   ", "\n\t"] {
        let r = s.abstain.retrieve(query(q, vec![b.clone()], 10)).await;
        assert!(r.is_err(), "empty query {q:?} must error per Q2");
    }
}

#[tokio::test]
async fn max_results_out_of_range_returns_error() {
    let s = setup_abstain(AbstainConfig::default()).await;
    let b = boundary("work");
    insert_full(&s, "Indexed", &b, 0).await;

    // Q3: max_results == 0 → error.
    let r0 = s
        .abstain
        .retrieve(query("Indexed", vec![b.clone()], 0))
        .await;
    assert!(r0.is_err(), "max_results=0 must error per Q3");

    // max_results > MAX_RESULTS_CAP (200) → error.
    let r_big = s.abstain.retrieve(query("Indexed", vec![b], 201)).await;
    assert!(r_big.is_err(), "max_results > 200 must error per Q3");
}

#[tokio::test]
async fn boundary_isolation_inherited() {
    let s = setup_abstain(AbstainConfig {
        bm25_top_score_threshold: 0.05,
    })
    .await;
    let work_b = boundary("work");
    let personal_b = boundary("personal");

    let work_id = insert_full(&s, "Shared anchor BOUNDARY_PIGEON in work", &work_b, 0).await;
    let _personal_id = insert_full(
        &s,
        "Shared anchor BOUNDARY_PIGEON in personal",
        &personal_b,
        0,
    )
    .await;

    // Only work boundary authorized.
    let results = s
        .abstain
        .retrieve(query("BOUNDARY_PIGEON", vec![work_b], 5))
        .await
        .expect("retrieve");

    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(ids.contains(&work_id), "work memory must surface");
    assert_eq!(
        ids.len(),
        1,
        "personal memory must NOT surface — boundary filter inherited"
    );
}

// ── Score-range pass-through (inner ranges preserved) ────────────────────

#[tokio::test]
async fn score_range_invariant_when_not_abstaining() {
    // Inner is a HybridRetriever — its scores live in RRF range
    // [0, 2/(60+1)] ≈ [0, 0.0328]. AbstainingRetriever must pass these
    // through unchanged, NOT replace with BM25 scores from the probe.
    let s = setup_abstain(AbstainConfig {
        bm25_top_score_threshold: 0.05,
    })
    .await;
    let b = boundary("work");
    insert_full(&s, "Score pass-through SHARK_77 anchor", &b, 0).await;
    insert_full(&s, "Other content", &b, 1).await;

    let results = s
        .abstain
        .retrieve(query("SHARK_77", vec![b], 5))
        .await
        .expect("retrieve");

    assert!(!results.is_empty(), "should not abstain at threshold 0.05");
    for r in &results {
        assert!(
            r.score >= 0.0 && r.score <= 0.0328 + 1e-6,
            "inner Hybrid RRF score {} preserved through abstain wrapper (must be in [0, 0.0328])",
            r.score
        );
    }
}

#[tokio::test]
async fn inner_only_smoke_works_with_semantic_alone() {
    // Verify the decorator also wraps a non-Hybrid inner (e.g.
    // SemanticRetriever directly). Confirms loose-coupling.
    let tr = make_test_retriever().await;
    let kw_index = Arc::new(KeywordIndex::new().expect("kw"));
    let semantic: Arc<dyn Retriever> = Arc::new(tr.retriever);
    let keyword: Arc<dyn Retriever> =
        Arc::new(KeywordRetriever::new(kw_index.clone(), tr.metadata.clone()));
    let abstain = AbstainingRetriever::with_config(
        semantic.clone(), // inner is SemanticRetriever directly
        keyword.clone(),
        AbstainConfig {
            bm25_top_score_threshold: 0.05,
        },
    );

    let b = boundary("work");
    let m = make_memory("LOOSE_COUPLING_TOKEN_X42 sample", &b);
    let id = m.id;
    tr.metadata.create_memory(&m).await.expect("create");
    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    emb[0] = 1.0;
    tr.vectors.upsert(&id, &emb, &b).await.expect("vec");
    kw_index.insert(id, &m.content).await.expect("kw insert");

    let results = abstain
        .retrieve(query("LOOSE_COUPLING_TOKEN_X42", vec![b], 5))
        .await
        .expect("retrieve");
    assert!(!results.is_empty());
    assert_eq!(results[0].memory.id, id);

    // Inner is SemanticRetriever — scores are cosine-similarity in
    // [-1, 1] per Q7 contract. We won't assert exact value (depends
    // on stub embedder + drift), but must respect the trait range.
    for r in &results {
        assert!(r.score >= -1.0 && r.score <= 1.0);
    }
}
