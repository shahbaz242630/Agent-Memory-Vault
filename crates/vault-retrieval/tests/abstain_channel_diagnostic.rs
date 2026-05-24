//! T0.2.7 Phase 5 Step 2 — abstain channel diagnostic.
//!
//! Prints BM25 top-1 score AND semantic top-1 cosine for every
//! production query (Q11, Q13, Q25, Q26 contradictions + Q21, Q22
//! hard-negatives) against the 100-memory hand-curated fixture.
//! Diagnostic-only — gathers the calibration data we need to decide
//! whether [`AbstainingRetriever`] should consult both channels rather
//! than BM25 alone.
//!
//! # Why this test exists
//!
//! The 2026-05-21 post-stopword-fix acceptance run came back 3/4
//! contradictions + 2/2 hard-negatives — Q25 newly failing with
//! `latency=0.0s, vault_has_no_relevant_content=true`. Abstain wrongly
//! fired on a contradiction query. Diagnostic hypothesis:
//!
//! - Q25 is a task-shaped query ("help me update the product roadmap
//!   doc...") with many common words.
//! - The 2026-05-21 stopword filter strips Q25 down to a handful of
//!   content tokens that don't lexically match the "Q1 2027 GA launch"
//!   memory → BM25 top-1 score falls below the 6.0 abstain threshold.
//! - But the BGE semantic embedding bridges "product roadmap" → "GA
//!   launch" topic, so the semantic channel's top-1 cosine is high.
//! - The current `AbstainingRetriever` probes only BM25, so the
//!   semantic signal is ignored at the abstain decision.
//!
//! If the diagnostic confirms the hypothesis (Q25 BM25 low, Q25
//! semantic high), the fix is to make abstain hybrid-aware: only
//! abstain when BOTH channels are weak.
//!
//! # What this test prints
//!
//! For each of the six production queries:
//! - Top 3 BM25 hits with score + content preview
//! - Top 3 semantic hits with cosine score + content preview
//! - Side-by-side summary table
//!
//! Runs in ~3 seconds (BGE embedding pass dominates; no LLM).

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context, Result};
use serde::Deserialize;

use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_retrieval::{
    AbstainConfig, KeywordIndex, KeywordRetriever, RetrievalOptions, RetrievalQuery, Retriever,
    SemanticRetriever, MAX_RESULTS_CAP,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];
const DIAGNOSTIC_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26", "Q21", "Q22"];

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

#[tokio::test(flavor = "multi_thread")]
async fn abstain_channel_diagnostic_top_scores_per_query() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let key = SqlCipherKey::new("abstain-channel-diag-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key).await?;
    let metadata = Arc::new(metadata);
    let vectors_raw = LanceVectorStore::open_with_at_rest_key(
        &dir.path().join("vectors"),
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
    .await?;
    let vectors: Arc<dyn VectorStore> = Arc::new(vectors_raw);

    let bge = open_bge_provider()?;

    // Load + sort fixture deterministically (same discipline as
    // abstain_q21_focused.rs).
    let memory_fixture_path = repo_root()?
        .join("crates")
        .join("vault-consolidator")
        .join("tests")
        .join("fixtures")
        .join("merge_acceptance_100.json");
    let mut memory_fixture: Vec<MemoryFixtureEntry> =
        serde_json::from_slice(&std::fs::read(&memory_fixture_path)?)?;
    memory_fixture.sort_by(|a, b| a.id.cmp(&b.id));

    let query_fixture_path = vault_retrieval_root()
        .join("test-fixtures")
        .join("merge_acceptance_100_queries.json");
    let query_set: QuerySet = serde_json::from_slice(&std::fs::read(&query_fixture_path)?)?;

    // Insert memories into BOTH stores.
    let mut all_memories: Vec<Memory> = Vec::with_capacity(memory_fixture.len());
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
        let embedding = bge.embed(&entry.content).await?;
        metadata.create_memory(&memory).await?;
        vectors
            .upsert(&memory.id, &embedding, &memory.boundary)
            .await?;
        all_memories.push(memory);
    }

    let keyword_index = Arc::new(KeywordIndex::new()?);
    keyword_index.bulk_insert(&all_memories).await?;

    let keyword: Arc<dyn Retriever> = Arc::new(KeywordRetriever::new(
        keyword_index.clone(),
        metadata.clone(),
    ));
    let semantic: Arc<dyn Retriever> = Arc::new(SemanticRetriever::new(
        metadata.clone(),
        bge,
        vectors.clone(),
    ));

    println!(
        "\n{:=^140}",
        " Abstain channel diagnostic — BM25 vs semantic top-1 per query "
    );
    println!(
        "Corpus: {} memories from merge_acceptance_100.json (fixture-id sorted, deterministic insertion)",
        all_memories.len()
    );
    let threshold = AbstainConfig::default().bm25_top_score_threshold;
    println!("Stopword filter: ON (StopWordFilter(English) registered for `vault_text` analyzer)");
    println!("Current AbstainConfig default threshold: {threshold} (BM25 top-1)");
    println!();

    let mut summary: Vec<(String, String, f32, f32, &'static str)> = Vec::new();

    for qid in DIAGNOSTIC_QUERY_IDS {
        let q = query_set
            .queries
            .iter()
            .find(|q| q.id == *qid)
            .with_context(|| format!("query {qid} missing from fixture"))?;
        let mut boundaries = Vec::with_capacity(q.authorized_boundaries.len());
        for b in &q.authorized_boundaries {
            boundaries.push(Boundary::new(b)?);
        }
        let probe = RetrievalQuery {
            query_text: q.query_text.clone(),
            authorized_boundaries: boundaries,
            max_results: MAX_RESULTS_CAP,
            options: RetrievalOptions::default(),
        };

        let bm25_hits = keyword.retrieve(probe.clone()).await?;
        let sem_hits = semantic.retrieve(probe).await?;

        let bm25_top1 = bm25_hits.first().map(|h| h.score).unwrap_or(0.0);
        let sem_top1 = sem_hits.first().map(|h| h.score).unwrap_or(0.0);

        let kind = if ["Q11", "Q13", "Q25", "Q26"].contains(qid) {
            "contradiction"
        } else {
            "hard-negative"
        };

        println!("{:-^140}", format!(" {qid} [{kind}] "));
        println!("Query: {:?}", q.query_text);
        println!(
            "BOUNDARIES: {:?} · BM25 hits returned (post-filter): {} · Semantic hits returned: {}",
            q.authorized_boundaries,
            bm25_hits.len(),
            sem_hits.len()
        );
        println!();
        println!("BM25 top 3:");
        for (i, h) in bm25_hits.iter().take(3).enumerate() {
            let preview: String = h.memory.content.chars().take(95).collect();
            println!("  [{i}] score={:>7.4}  {preview}", h.score);
        }
        println!("Semantic top 3 (cosine):");
        for (i, h) in sem_hits.iter().take(3).enumerate() {
            let preview: String = h.memory.content.chars().take(95).collect();
            println!("  [{i}] score={:>7.4}  {preview}", h.score);
        }
        println!();

        summary.push((qid.to_string(), kind.to_string(), bm25_top1, sem_top1, kind));
    }

    println!("{:=^140}", " SUMMARY ");
    println!(
        "{:<5} {:<14} {:>14} {:>14} {:>14}    Gate decision",
        "Query",
        "Kind",
        "BM25 top-1",
        "Sem top-1 cos",
        format!("BM25 vs {threshold}"),
    );
    for (qid, kind, bm25, sem, _) in &summary {
        let gate = if *bm25 < threshold {
            "ABSTAIN (BM25 below threshold → no LLM call)"
        } else {
            "PROCEED (LLM judges relevance)"
        };
        println!(
            "{:<5} {:<14} {:>14.4} {:>14.4} {:>14}    {gate}",
            qid,
            kind,
            bm25,
            sem,
            if *bm25 < threshold { "<" } else { ">=" }
        );
    }
    println!(
        "\nV0.2 architectural note: at threshold {threshold} the abstain gate catches only \
         genuine-zero-signal queries (gibberish). All non-trivial queries proceed to the \
         LLM, which judges relevance per the read-time system prompt's explicit rules."
    );

    Ok(())
}

fn open_bge_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let fixture_root = vault_embedding_test_fixtures()?;
    let model = fixture_root.join("model.onnx");
    let tokenizer = fixture_root.join("tokenizer.json");
    let ort_lib = fixture_root.join("onnxruntime.dll");
    for p in [&model, &tokenizer, &ort_lib] {
        ensure!(p.exists(), "missing BGE fixture {p:?}");
    }
    let provider = BgeSmallProvider::open(&model, &tokenizer, &ort_lib)?;
    Ok(Arc::new(provider))
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

fn vault_embedding_test_fixtures() -> Result<PathBuf> {
    let p = repo_root()?
        .join("crates")
        .join("vault-embedding")
        .join("test-fixtures")
        .join("bge-small-en-v1.5");
    ensure!(p.exists(), "bge fixtures missing at {p:?}");
    Ok(p)
}
