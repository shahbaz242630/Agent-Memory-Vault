//! Focused regression tests for the V0.2 [`AbstainingRetriever`] contract:
//! the gate fires ONLY on genuinely-empty BM25 signal.
//!
//! # Background
//!
//! Iteration history (T0.2.7 Phase 5 Step 2, 2026-05-21):
//!
//! 1. Initial design (spike-calibrated): threshold 6.0, abstain on weak-
//!    but-present signal. Worked on the spike's synthetic corpus where
//!    hard-neg query tokens had ZERO lexical overlap with planted
//!    memories (hard-negs scored 0–5, contradictions scored 8–15).
//! 2. Stopword filter added to the Tantivy tokenizer (`KeywordIndex`)
//!    to remove noise-floor BM25 scoring from common words like "the",
//!    "we", "did". Fixed Q21's "family-reunion-memory-scoring-6.30"
//!    bug.
//! 3. Post-stopword cron run surfaced Q25 regression: the task-shaped
//!    query ("help me update the product roadmap doc...") scored 5.09
//!    BM25 → abstain wrongly fired → no LLM call → no contradiction
//!    detection.
//! 4. Channel diagnostic (`tests/abstain_channel_diagnostic.rs`) showed
//!    BM25 and semantic distributions BOTH overlap between hard-negs
//!    and contradictions on the hand-curated fixture. No clean
//!    statistical separator exists.
//! 5. Threshold dialed down 6.0 → 1.0. The gate now catches only
//!    genuine-zero-signal queries (gibberish, no token matches anywhere).
//!    The LLM, per its explicit relevance rule in the read-time system
//!    prompt, is the relevance judge for everything else — including
//!    hard-negs.
//!
//! # What these tests assert
//!
//! - **`q21_proceeds_past_abstain_under_v0_2_threshold`**: Q21 hard-neg
//!   query against the 100-memory hand-curated fixture has BM25 top-1
//!   well above 1.0 (residual content-word matches always exist on
//!   natural prose). Abstain MUST NOT fire — Q21 candidates proceed
//!   to the inner retriever (and to the LLM in production), which is
//!   responsible for the "no relevant content" judgment per the system
//!   prompt's Kubernetes-vs-database-migration example.
//! - **`gibberish_query_abstains_at_v0_2_threshold`**: a query composed
//!   of pure nonsense tokens that match nothing in the fixture
//!   yields empty BM25 hits → top score 0 → abstain fires correctly
//!   (the gate's one remaining responsibility under V0.2).
//!
//! Both tests run in well under a second — no LLM, no Lance, no BGE.
//! Iterate freely on the abstain layer without waiting on the 28-minute
//! Qwen-7B cron gauntlet.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, ensure, Result};
use serde::Deserialize;

use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_retrieval::{
    AbstainingRetriever, KeywordIndex, KeywordRetriever, RetrievalOptions, RetrievalQuery,
    Retriever, MAX_RESULTS_CAP,
};
use vault_storage::{MetadataStore, SqlCipherKey};

#[derive(Debug, Clone, Deserialize)]
struct MemoryFixtureEntry {
    id: String,
    boundary: String,
    #[allow(dead_code)]
    topic_label: String,
    content: String,
    #[allow(dead_code)]
    ground_truth: GroundTruth,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GroundTruth {
    outcome: String,
    cluster: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuerySet {
    queries: Vec<QueryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct QueryEntry {
    id: String,
    query_text: String,
    authorized_boundaries: Vec<String>,
}

/// V0.2 contract: a real hard-negative query against the hand-curated
/// fixture produces non-trivial BM25 hits (residual content-word matches
/// on natural prose). At threshold 1.0, abstain MUST NOT fire — the LLM
/// is the relevance judge for non-trivial signal.
#[tokio::test(flavor = "multi_thread")]
async fn q21_proceeds_past_abstain_under_v0_2_threshold() -> Result<()> {
    let (keyword, q21_query, q21_boundaries) = setup(Some("Q21")).await?;
    let probe = RetrievalQuery {
        query_text: q21_query.clone(),
        authorized_boundaries: q21_boundaries.clone(),
        max_results: MAX_RESULTS_CAP,
        options: RetrievalOptions::default(),
    };
    let hits = keyword.retrieve(probe.clone()).await?;
    let top_score = hits.first().map(|h| h.score).unwrap_or(0.0);
    println!("\nQ21 query: {q21_query:?}");
    println!("Q21 BM25 top-1 score = {top_score:.4} (V0.2 AbstainConfig threshold = 1.0)");

    // The full abstain wrapper exercise: wrap the keyword retriever as
    // both inner and probe so the AbstainingRetriever's pass-through
    // path returns the inner's hits when abstain doesn't fire.
    let inner = keyword.clone();
    let abstain: Arc<dyn Retriever> = Arc::new(AbstainingRetriever::new(inner, keyword));
    let abstain_results = abstain.retrieve(probe).await?;

    ensure!(
        top_score >= 1.0,
        "Q21 BM25 top-1 = {top_score:.4} < 1.0 — unexpected; the hand-curated fixture \
         always has residual content-word overlap. Investigate fixture or tokenizer \
         change before assuming abstain behaviour."
    );
    ensure!(
        !abstain_results.is_empty(),
        "AbstainingRetriever wrongly returned empty for Q21 (top BM25 = {top_score:.4}). \
         The V0.2 contract is: abstain fires only on genuine-zero-signal queries; Q21's \
         residual signal must proceed to the LLM."
    );
    println!(
        "Abstain delegated to inner → {} hits returned. LLM is the relevance judge. ✓",
        abstain_results.len()
    );
    Ok(())
}

/// V0.2 contract: a query composed entirely of nonsense tokens that
/// match nothing in the corpus yields zero BM25 hits → top score 0 →
/// abstain fires (this is the gate's one remaining job under V0.2).
#[tokio::test(flavor = "multi_thread")]
async fn gibberish_query_abstains_at_v0_2_threshold() -> Result<()> {
    let (keyword, _q21_query, _q21_boundaries) = setup(None).await?;

    // Tokens chosen to be lexically absent from the hand-curated fixture
    // AND not stripped by the stopword filter (so the parser sees them).
    let gibberish = "fhqwhgads zqwertypoiu mxckvjnsdkfh".to_string();
    let probe = RetrievalQuery {
        query_text: gibberish.clone(),
        // Use any valid boundary; the test is about token-match, not
        // boundary filtering.
        authorized_boundaries: vec![Boundary::new("work")?, Boundary::new("personal")?],
        max_results: MAX_RESULTS_CAP,
        options: RetrievalOptions::default(),
    };
    let raw_hits = keyword.retrieve(probe.clone()).await?;
    let raw_top = raw_hits.first().map(|h| h.score).unwrap_or(0.0);
    println!("\nGibberish query: {gibberish:?}");
    println!(
        "Raw BM25 hits: {} (top score = {raw_top:.4})",
        raw_hits.len()
    );

    let inner = keyword.clone();
    let abstain: Arc<dyn Retriever> = Arc::new(AbstainingRetriever::new(inner, keyword));
    let abstain_results = abstain.retrieve(probe).await?;

    ensure!(
        raw_hits.is_empty() || raw_top < 1.0,
        "Gibberish query unexpectedly yielded BM25 score {raw_top:.4} ≥ 1.0 — the test \
         tokens may need updating, or the fixture changed."
    );
    ensure!(
        abstain_results.is_empty(),
        "AbstainingRetriever failed to fire on a genuine-zero-signal query (raw top \
         BM25 = {raw_top:.4}). The V0.2 gate must short-circuit gibberish queries to \
         avoid wasting LLM cost on irrelevant context."
    );
    println!("Abstain fired on gibberry signal (raw_top={raw_top:.4} < 1.0). ✓");
    Ok(())
}

// ── Shared setup ────────────────────────────────────────────────────────

/// Load fixture (deterministic id-sorted order), index into Tantivy,
/// return the keyword retriever + (optionally) a query's text +
/// boundaries.
async fn setup(query_id: Option<&str>) -> Result<(Arc<dyn Retriever>, String, Vec<Boundary>)> {
    let dir = tempfile::tempdir()?;
    let key = SqlCipherKey::new("abstain-focused-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key).await?;
    let metadata = Arc::new(metadata);

    let memory_fixture_path = repo_root()?
        .join("crates")
        .join("vault-consolidator")
        .join("tests")
        .join("fixtures")
        .join("merge_acceptance_100.json");
    let mut memory_fixture: Vec<MemoryFixtureEntry> =
        serde_json::from_slice(&std::fs::read(&memory_fixture_path)?)?;
    memory_fixture.sort_by(|a, b| a.id.cmp(&b.id));

    let mut memories: Vec<Memory> = Vec::with_capacity(memory_fixture.len());
    for entry in &memory_fixture {
        let boundary = Boundary::new(&entry.boundary)?;
        let memory = Memory::try_new(NewMemory {
            content: entry.content.clone(),
            memory_type: MemoryType::Semantic,
            boundary,
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })?;
        metadata.create_memory(&memory).await?;
        memories.push(memory);
    }

    let keyword_index = Arc::new(KeywordIndex::new()?);
    keyword_index.bulk_insert(&memories).await?;
    let keyword: Arc<dyn Retriever> =
        Arc::new(KeywordRetriever::new(keyword_index, metadata.clone()));

    let (text, boundaries) = if let Some(qid) = query_id {
        let query_fixture_path = vault_retrieval_root()
            .join("test-fixtures")
            .join("merge_acceptance_100_queries.json");
        let query_set: QuerySet = serde_json::from_slice(&std::fs::read(&query_fixture_path)?)?;
        let q = query_set
            .queries
            .iter()
            .find(|q| q.id == qid)
            .ok_or_else(|| anyhow!("{qid} missing from query fixture"))?;
        let mut boundaries = Vec::with_capacity(q.authorized_boundaries.len());
        for b in &q.authorized_boundaries {
            boundaries.push(Boundary::new(b)?);
        }
        (q.query_text.clone(), boundaries)
    } else {
        (String::new(), Vec::new())
    };

    Ok((keyword, text, boundaries))
}

fn vault_retrieval_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> Result<PathBuf> {
    vault_retrieval_root()
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("no grandparent for vault-retrieval"))
}
