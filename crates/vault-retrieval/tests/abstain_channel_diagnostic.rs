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
    AbstainConfig, KeywordIndex, KeywordRetriever, RetrievalOptions, RetrievalQuery,
    RetrievedMemory, Retriever, SemanticRetriever, MAX_RESULTS_CAP,
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
    let mut query_set: QuerySet = serde_json::from_slice(&std::fs::read(&query_fixture_path)?)?;

    // Calibration add (2026-05-28, cosine-floor design / n=4 go-no-go):
    // inject synthetic ZERO-SIGNAL queries (the A6 ship-gate case). Q21/Q22
    // are topical-noise hard-negs; these are the genuine no-signal cases the
    // cosine floor must catch. One clean-distant probe (zero corpus overlap)
    // plus four near-topic probes (an adjacent cluster exists but the
    // specific fact does not) — the near-topic ones are the adversarial worst
    // case for a cosine floor. Authorize every fixture boundary so each
    // searches the whole corpus.
    {
        let mut boundaries = std::collections::BTreeSet::new();
        for e in &memory_fixture {
            boundaries.insert(e.boundary.clone());
        }
        let nosig_boundaries: Vec<String> = boundaries.into_iter().collect();
        let nosig_probes: &[(&str, &str)] = &[
            // near-topic: blood-PRESSURE cluster exists, blood TYPE does not
            ("NOSIG-blood", "what is the user's blood type"),
            // clean-distant: no corpus overlap at all
            ("NOSIG-mercury", "what is the boiling point of mercury"),
            // near-topic: household/rent/bills exist, no address
            ("NOSIG-address", "what is the user's home street address"),
            // near-topic: health/dental/physical exist, no gym
            ("NOSIG-gym", "what is the user's gym membership number"),
            // near-topic: family-reunion travel exists, no airline fact
            ("NOSIG-airline", "what airline does the user usually fly"),
        ];
        for (id, text) in nosig_probes {
            query_set.queries.push(QueryEntry {
                id: (*id).to_string(),
                query_text: (*text).to_string(),
                authorized_boundaries: nosig_boundaries.clone(),
            });
        }
    }

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

    // Tuple: (query id, kind, BM25 top-1, sem top-1 cosine, sem top-3 mean,
    // sem top-5 mean). The three semantic signals let iteration 2 pick the
    // most-separated one (top-1 vs top-k aggregate).
    let mut summary: Vec<(String, String, f32, f32, f32, f32)> = Vec::new();

    const NOSIG_IDS: &[&str] = &[
        "NOSIG-blood",
        "NOSIG-mercury",
        "NOSIG-address",
        "NOSIG-gym",
        "NOSIG-airline",
    ];
    let diagnostic_query_ids: Vec<&str> = DIAGNOSTIC_QUERY_IDS
        .iter()
        .copied()
        .chain(NOSIG_IDS.iter().copied())
        .collect();
    for qid in &diagnostic_query_ids {
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
        let sem_top3_mean = mean_top_k(&sem_hits, 3);
        let sem_top5_mean = mean_top_k(&sem_hits, 5);

        let kind = if ["Q11", "Q13", "Q25", "Q26"].contains(qid) {
            "contradiction"
        } else if qid.starts_with("NOSIG") {
            "no-signal"
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

        summary.push((
            qid.to_string(),
            kind.to_string(),
            bm25_top1,
            sem_top1,
            sem_top3_mean,
            sem_top5_mean,
        ));
    }

    println!("{:=^140}", " SUMMARY (cosine-floor calibration, ADR-057) ");
    println!(
        "{:<14} {:<14} {:>12} {:>14} {:>14} {:>14}",
        "Query", "Kind", "BM25 top-1", "Sem top-1", "Sem top-3 mean", "Sem top-5 mean",
    );
    for (qid, kind, bm25, sem1, sem3, sem5) in &summary {
        println!(
            "{:<14} {:<14} {:>12.4} {:>14.4} {:>14.4} {:>14.4}",
            qid, kind, bm25, sem1, sem3, sem5,
        );
    }

    // ── Pre-declared go/no-go decision rule (cosine-floor calibration) ──
    // The floor must sit in a gap: STRICTLY BELOW the lowest must-proceed
    // contradiction AND STRICTLY ABOVE every no-signal probe. If any
    // no-signal probe lands at/above the lowest contradiction, the signal
    // cannot carry no-signal abstention and we STOP (rethink: top-k aggregate
    // vs top-1, or move the cross-encoder up). Evaluated for each candidate
    // signal so iteration 2 can pick the most-separated one.
    for (label, sel) in [
        ("top-1", 3usize),
        ("top-3 mean", 4usize),
        ("top-5 mean", 5usize),
    ] {
        let pick = |s: &(String, String, f32, f32, f32, f32)| -> f32 {
            match sel {
                3 => s.3,
                4 => s.4,
                _ => s.5,
            }
        };
        let mut lowest_contradiction = f32::INFINITY;
        let mut highest_nosignal = f32::NEG_INFINITY;
        for s in &summary {
            let v = pick(s);
            if s.1 == "contradiction" && v < lowest_contradiction {
                lowest_contradiction = v;
            }
            if s.1 == "no-signal" && v > highest_nosignal {
                highest_nosignal = v;
            }
        }
        let gap = lowest_contradiction - highest_nosignal;
        let verdict = if gap > 0.0 {
            "GO — separable; lock floor in the gap"
        } else {
            "NO-GO — a no-signal probe sits in the proceed band; rethink the signal"
        };
        println!(
            "\n[{label}] lowest must-proceed contradiction = {lowest_contradiction:.4} · \
             highest no-signal = {highest_nosignal:.4} · gap = {gap:.4}  → {verdict}"
        );
        if gap > 0.0 {
            println!(
                "        suggested floor (gap midpoint) = {:.4}",
                highest_nosignal + gap / 2.0
            );
        }
    }

    Ok(())
}

/// Mean of the top-`k` semantic cosine scores (fewer if the result set is
/// smaller). Returns 0.0 for an empty set. Lets the diagnostic compare a
/// fragile top-1 gate signal against a top-k aggregate in the compressed
/// BGE-small cosine band (iteration-2 mechanism question).
fn mean_top_k(hits: &[RetrievedMemory], k: usize) -> f32 {
    if hits.is_empty() || k == 0 {
        return 0.0;
    }
    let n = k.min(hits.len());
    let sum: f32 = hits.iter().take(n).map(|h| h.score).sum();
    sum / n as f32
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
