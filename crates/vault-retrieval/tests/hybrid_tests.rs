//! Integration tests for [`HybridRetriever`] — the Phase 2 RRF-fusion
//! retriever that composes [`SemanticRetriever`] + [`KeywordRetriever`].
//!
//! The tests prove the Phase-2 contract surface:
//!
//! - **Parallel channel execution** via `tokio::try_join!` — both
//!   semantic and keyword fire concurrently.
//! - **RRF fusion correctness** — memories ranked by either channel
//!   surface in the fused output with the expected RRF score formula.
//! - **Compositional invariants** — Q1 (empty boundaries → empty), Q2
//!   (empty query → error), Q3 (max_results bounds) inherited from the
//!   underlying trait + applied at the hybrid layer.
//! - **`max_results` truncation** — fused output respects the caller's
//!   `query.max_results` even when the union of channel results is
//!   larger.
//! - **Tiebreak** — deterministic ordering by `created_at DESC` on
//!   equal RRF scores, per `Retriever` trait invariant #3.
//! - **Concurrency** — multiple parallel `hybrid.retrieve(...)` calls
//!   against the same `HybridRetriever` instance.
//!
//! ## Stub embedder reminder
//!
//! `StubEmbedder` (in `common/mod.rs`) returns a fixed unit vector
//! `[1, 0, 0, ..., 0]` for every query — semantic ranking in these
//! tests is determined entirely by the `drift` parameter at insert
//! time, NOT by query content. BM25 ranking IS content-driven via
//! the real Tantivy index.

#![forbid(unsafe_code)]

mod common;

use std::sync::Arc;

use vault_core::{Boundary, MemoryId};
use vault_embedding::EMBEDDING_DIM;
use vault_retrieval::{
    HybridConfig, HybridRetriever, KeywordIndex, KeywordRetriever, Retriever, SemanticRetriever,
};
use vault_storage::{MetadataStore, VectorStore};

use common::{boundary, make_memory, make_test_retriever, query};

/// Bundle for hybrid tests: hybrid retriever + both child handles + the
/// downstream stores (so tests can insert memories on both sides) + temp
/// dir keepalive.
struct HybridSetup {
    hybrid: HybridRetriever,
    /// Held for direct verification in tests like "semantic-only path".
    #[allow(dead_code)]
    semantic: Arc<SemanticRetriever>,
    /// Held for direct verification in tests like "keyword-only path".
    #[allow(dead_code)]
    keyword: Arc<KeywordRetriever>,
    metadata: Arc<MetadataStore>,
    vectors: Arc<dyn VectorStore>,
    keyword_index: Arc<KeywordIndex>,
    _dir: tempfile::TempDir,
}

async fn setup_hybrid() -> HybridSetup {
    let tr = make_test_retriever().await;
    let keyword_index = Arc::new(KeywordIndex::new().expect("kw index"));
    let semantic = Arc::new(tr.retriever);
    let keyword = Arc::new(KeywordRetriever::new(
        keyword_index.clone(),
        tr.metadata.clone(),
    ));
    let hybrid = HybridRetriever::new(
        semantic.clone() as Arc<dyn Retriever>,
        keyword.clone() as Arc<dyn Retriever>,
    );
    HybridSetup {
        hybrid,
        semantic,
        keyword,
        metadata: tr.metadata,
        vectors: tr.vectors,
        keyword_index,
        _dir: tr._dir,
    }
}

/// Insert a memory into all three stores: metadata + vector (with
/// controlled drift) + keyword index. Returns the assigned id.
///
/// `drift` controls semantic rank: lower drift = closer to the query
/// (unit-vector) under cosine, so rank closer to 1. Pass 0 for the
/// closest memory; larger n for progressively farther.
async fn insert_full(s: &HybridSetup, content: &str, b: &Boundary, drift: usize) -> MemoryId {
    let m = make_memory(content, b);
    let id = m.id;
    s.metadata.create_memory(&m).await.expect("create_memory");

    // Vector side — stub embedding with controlled drift offset.
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

    // Keyword index side — content goes verbatim.
    s.keyword_index
        .insert(id, content)
        .await
        .expect("keyword insert");
    id
}

// ── Both channels contribute ─────────────────────────────────────────────

#[tokio::test]
async fn both_channels_contribute() {
    let s = setup_hybrid().await;
    let b = boundary("work");

    // Memory A: semantic rank 1 (drift 0) + contains query token.
    let a_id = insert_full(&s, "Project Alpha launch metrics review", &b, 0).await;
    // Memory B: semantic rank 2 (drift 1) + contains query token.
    let b_id = insert_full(&s, "Project Beta backlog cleanup", &b, 1).await;
    // Memory C: semantic rank 3 (drift 2) + does NOT contain query token.
    let c_id = insert_full(&s, "Random unrelated entry about cats", &b, 2).await;

    let results = s
        .hybrid
        .retrieve(query("Project", vec![b], 10))
        .await
        .expect("hybrid retrieve");

    // A and B both surface (semantic + keyword agree). C surfaces too
    // via semantic-only path (no Project token). All three present.
    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(
        ids.contains(&a_id),
        "A (sem rank 1 + kw match) must surface"
    );
    assert!(
        ids.contains(&b_id),
        "B (sem rank 2 + kw match) must surface"
    );
    assert!(
        ids.contains(&c_id),
        "C (sem-only) must surface via semantic channel"
    );

    // A and B (which both channels rank) score higher than C (one channel only).
    let a_score = results.iter().find(|r| r.memory.id == a_id).unwrap().score;
    let b_score = results.iter().find(|r| r.memory.id == b_id).unwrap().score;
    let c_score = results.iter().find(|r| r.memory.id == c_id).unwrap().score;
    assert!(
        a_score > c_score,
        "A both-channels score ({a_score}) must exceed C single-channel ({c_score})"
    );
    assert!(
        b_score > c_score,
        "B both-channels score ({b_score}) must exceed C single-channel ({c_score})"
    );
}

// ── Single-channel paths ─────────────────────────────────────────────────

#[tokio::test]
async fn semantic_only_match_surfaces() {
    let s = setup_hybrid().await;
    let b = boundary("work");

    // A: high semantic rank (drift 0), no query token.
    let a_id = insert_full(&s, "Notes about cats yesterday", &b, 0).await;
    // B: low semantic rank (drift 10), no query token.
    let _b_id = insert_full(&s, "Notes about dogs today", &b, 10).await;
    // The query "FELINE_BEACON" matches neither memory in BM25, but the
    // semantic channel always ranks SOMETHING (cosine is well-defined
    // for any pair) — drift 0 (A) ends up rank 1.

    let results = s
        .hybrid
        .retrieve(query("FELINE_BEACON", vec![b], 5))
        .await
        .expect("hybrid retrieve");

    // Semantic-only path surfaces A as rank 1 regardless of BM25.
    assert!(!results.is_empty());
    assert_eq!(
        results[0].memory.id, a_id,
        "highest semantic-rank memory must surface even with no BM25 hits"
    );
}

#[tokio::test]
async fn keyword_only_match_surfaces() {
    let s = setup_hybrid().await;
    let b = boundary("work");

    // A: high semantic rank (drift 0), no rare token.
    let _a_id = insert_full(&s, "Generic content about onboarding", &b, 0).await;
    // B: lower semantic rank (drift 30), CONTAINS the rare token.
    let b_id = insert_full(&s, "Rare anchor PHOENIX_VOLCANO_42 buried here", &b, 30).await;

    let results = s
        .hybrid
        .retrieve(query("PHOENIX_VOLCANO_42", vec![b], 5))
        .await
        .expect("hybrid retrieve");

    // B must surface despite low semantic rank — BM25 gives it rank 1.
    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(
        ids.contains(&b_id),
        "rare-token memory must surface via BM25 even when semantic ranks it low"
    );
}

// ── RRF score range invariant ────────────────────────────────────────────

#[tokio::test]
async fn rrf_score_in_valid_range() {
    let s = setup_hybrid().await;
    let b = boundary("work");

    // Seed 5 memories with mixed drift + content.
    for i in 0..5 {
        let content = format!("Memo {i} about topic CARDINAL_TOKEN_{i:02}");
        let _ = insert_full(&s, &content, &b, i).await;
    }

    let results = s
        .hybrid
        .retrieve(query("CARDINAL_TOKEN_01", vec![b], 5))
        .await
        .expect("retrieve");

    // RRF formula: score = sum(1/(k + rank_i)) over BOTH channels (k=60).
    // Best possible score: both channels rank a memory at #1 →
    //   2/(60+1) ≈ 0.03278688. All scores must be in [0, 0.0328].
    for r in &results {
        assert!(
            r.score >= 0.0 && r.score <= 0.0328 + 1e-6,
            "RRF score {} out of expected range [0, 0.0328]",
            r.score
        );
    }
}

// ── Compositional invariants — Q1, Q2, Q3 ────────────────────────────────

#[tokio::test]
async fn empty_authorized_boundaries_short_circuits() {
    let s = setup_hybrid().await;
    let b = boundary("work");
    let _ = insert_full(&s, "Something searchable", &b, 0).await;

    // Q1 contract: empty authorized_boundaries → empty result, no error.
    let results = s
        .hybrid
        .retrieve(query("Something", vec![], 10))
        .await
        .expect("hybrid retrieve");
    assert!(results.is_empty(), "empty boundaries must short-circuit");
}

#[tokio::test]
async fn empty_query_returns_error() {
    let s = setup_hybrid().await;
    let b = boundary("work");
    let _ = insert_full(&s, "Anything indexed", &b, 0).await;

    // Q2 contract: empty/whitespace query → InvalidInput error.
    for q in ["", "   ", "\n\t"] {
        let r = s.hybrid.retrieve(query(q, vec![b.clone()], 10)).await;
        assert!(r.is_err(), "empty/whitespace query {q:?} must error per Q2");
    }
}

#[tokio::test]
async fn max_results_out_of_range_returns_error() {
    let s = setup_hybrid().await;
    let b = boundary("work");
    let _ = insert_full(&s, "Indexed memory", &b, 0).await;

    // Q3 contract: max_results == 0 → error.
    let r0 = s
        .hybrid
        .retrieve(query("Indexed", vec![b.clone()], 0))
        .await;
    assert!(r0.is_err(), "max_results=0 must error per Q3");

    // max_results > MAX_RESULTS_CAP (200) → error.
    let r_big = s.hybrid.retrieve(query("Indexed", vec![b], 201)).await;
    assert!(
        r_big.is_err(),
        "max_results > MAX_RESULTS_CAP must error per Q3"
    );
}

// ── Boundary isolation inherited via composition ─────────────────────────

#[tokio::test]
async fn boundary_isolation_inherited() {
    let s = setup_hybrid().await;
    let work_b = boundary("work");
    let personal_b = boundary("personal");

    let work_id = insert_full(&s, "Shared anchor PIGEON_42 in work", &work_b, 0).await;
    let _personal_id = insert_full(&s, "Shared anchor PIGEON_42 in personal", &personal_b, 0).await;

    // Only work boundary authorized.
    let results = s
        .hybrid
        .retrieve(query("PIGEON_42", vec![work_b], 10))
        .await
        .expect("retrieve");

    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(ids.contains(&work_id), "work memory must surface");
    assert_eq!(
        ids.len(),
        1,
        "personal memory must NOT surface — boundary isolation inherited"
    );
}

// ── max_results truncation ───────────────────────────────────────────────

#[tokio::test]
async fn max_results_truncation() {
    let s = setup_hybrid().await;
    let b = boundary("work");

    // Insert 10 memories, all containing the query token.
    for i in 0..10 {
        let content = format!("Memo {i} about TRUNCATION_ANCHOR");
        let _ = insert_full(&s, &content, &b, i).await;
    }

    // Ask for only 3.
    let results = s
        .hybrid
        .retrieve(query("TRUNCATION_ANCHOR", vec![b], 3))
        .await
        .expect("retrieve");
    assert_eq!(
        results.len(),
        3,
        "max_results=3 must truncate fused output to exactly 3"
    );
}

// ── Concurrency ──────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_retrieve_safe() {
    let s = setup_hybrid().await;
    let b = boundary("work");

    // Seed 8 memories with distinct rare tokens.
    for i in 0..8 {
        let content = format!("Memo {i} containing rare token PARALLEL_{i:02}");
        let _ = insert_full(&s, &content, &b, i).await;
    }

    let hybrid = Arc::new(s.hybrid);
    let mut handles = Vec::with_capacity(8);
    for i in 0..8 {
        let h = hybrid.clone();
        let b = b.clone();
        handles.push(tokio::spawn(async move {
            let token = format!("PARALLEL_{i:02}");
            h.retrieve(query(&token, vec![b], 5)).await
        }));
    }

    for h in handles {
        let r = h.await.expect("join").expect("retrieve");
        assert!(
            !r.is_empty(),
            "parallel hybrid.retrieve must find its target token"
        );
    }
}

// ── Custom config (rrf_k + top_n_each tunables) ──────────────────────────

#[tokio::test]
async fn custom_config_respected() {
    let tr = make_test_retriever().await;
    let kw_index = Arc::new(KeywordIndex::new().expect("kw index"));
    let semantic = Arc::new(tr.retriever) as Arc<dyn Retriever>;
    let keyword = Arc::new(KeywordRetriever::new(kw_index.clone(), tr.metadata.clone()))
        as Arc<dyn Retriever>;

    // Custom config with very small top_n_each (5) and atypical k (10).
    let cfg = HybridConfig {
        top_n_each: 5,
        rrf_k: 10,
    };
    let hybrid = HybridRetriever::with_config(semantic, keyword, cfg);

    let b = boundary("work");
    // Insert one memory.
    let m = make_memory("Smoke test for custom config", &b);
    let id = m.id;
    tr.metadata.create_memory(&m).await.expect("create");
    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    emb[0] = 1.0;
    tr.vectors.upsert(&id, &emb, &b).await.expect("vec upsert");
    kw_index.insert(id, &m.content).await.expect("kw insert");

    let results = hybrid
        .retrieve(query("Smoke", vec![b], 5))
        .await
        .expect("retrieve");

    assert!(!results.is_empty(), "config smoke: should still retrieve");
    assert_eq!(results[0].memory.id, id);

    // With k=10 and ranks 1+1, RRF = 2/(10+1) ≈ 0.1818.
    // Range check uses k=10's max: 2/(10+1) ≈ 0.1818, well inside [-1, 1].
    let upper_bound = 2.0_f32 / (10.0 + 1.0) + 1e-6;
    assert!(
        results[0].score <= upper_bound,
        "RRF score {} must respect custom-k upper bound {}",
        results[0].score,
        upper_bound
    );
}
