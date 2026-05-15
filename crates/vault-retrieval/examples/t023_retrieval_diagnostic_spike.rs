//! T0.2.3 retrieval-diagnostic spike (2026-05-14).
//!
//! **Architectural reframe context.** The 2026-05-14 cron acceptance run for
//! T0.2.3 commit 3 showed 24 % memory-level merge recall and 0 contradictions
//! detected on the 100-memory realism-rewritten fixture. Root cause is
//! upstream of Phi-4 (which scored 100 % precision on what it saw): BGE-small
//! pairwise cosine at the 0.92 threshold fails on length-variance and on
//! long-form content. The original fix-plan iterated threshold / chunking /
//! model-swap options — all framed against pairwise clustering.
//!
//! The reframe: retrieval IS the product surface. Consolidation matters
//! because it shapes what retrieval can find. The unanswered question is
//! whether BGE retrieval (query-anchored) holds up on the same content shape
//! where BGE pairwise clustering (no third anchor) doesn't. If query-anchored
//! retrieval is healthy, the V0.2 architecture becomes "best-effort
//! consolidation + Phi-4 read-time re-rank." If retrieval also degrades, the
//! embedding-layer problem is bigger and threshold tweaks were
//! deck-chair-rearranging.
//!
//! This spike measures one thing: against the realism-rewritten 100-memory
//! fixture, with 22 hand-curated agent-realistic queries (5 catch-me-up +
//! 5 specific-fact + 5 decision-history + 5 topic-synthesis + 2
//! hard-negative), what is the recall@K / precision@K / MRR profile per
//! query shape and per content-length tier?
//!
//! Three decision scenarios fall out of the numbers:
//! - **A** — recall@20 ≥ 0.85 across shapes: read-time re-rank is viable.
//! - **B** — recall@20 between 0.50 and 0.85: both surfaces need work.
//! - **C** — recall@20 < 0.50: embedding-layer overhaul, not threshold tweaks.
//!
//! Architectural decision is a separate conversation with Shahbaz based on
//! these numbers — this binary measures and reports, nothing else.
//!
//! **Discipline.** This file is example-grade throwaway, not production code.
//! No tests, no commit at run completion, no architectural conclusions in the
//! output. Spike artefacts (this file + the query JSON + the markdown
//! writeup) ride with whichever commit closes the architectural call.
//!
//! Run with (PowerShell on Windows, per standing rules):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t023_retrieval_diagnostic_spike
//! ```

#![allow(clippy::too_many_lines)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_retrieval::{
    RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever, SemanticRetriever,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

// Test-only at-rest key. Matches the cross-crate convention
// (`vault-consolidator/tests/common/mod.rs:26`).
const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

// Console table column widths, factored so the header and rows stay aligned.
const COL_SHAPE: usize = 18;
const COL_TIER: usize = 22;
const SEP_WIDE: usize = 120;

// ── Fixture types ────────────────────────────────────────────────────────

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
struct GroundTruth {
    #[allow(dead_code)]
    outcome: String,
    #[allow(dead_code)]
    cluster: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuerySet {
    queries: Vec<QueryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct QueryEntry {
    id: String,
    shape: String,
    length_tier: String,
    query_text: String,
    authorized_boundaries: Vec<String>,
    expected_memory_ids: Vec<String>,
    notes: String,
}

// ── Result types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct TopHit {
    rank: usize,
    fixture_id: Option<String>,
    score: f32,
    is_expected: bool,
    content_snippet: String,
}

#[derive(Debug, Clone)]
struct QueryResult {
    id: String,
    shape: String,
    length_tier: String,
    query_text: String,
    notes: String,
    expected_count: usize,
    expected_hits_at_5: usize,
    expected_hits_at_10: usize,
    expected_hits_at_20: usize,
    recall_at_5: f64,
    recall_at_10: f64,
    recall_at_20: f64,
    precision_at_5: f64,
    precision_at_10: f64,
    precision_at_20: f64,
    mrr: f64,
    first_expected_rank: Option<usize>,
    top_returned: Vec<TopHit>,
}

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    println!("{}", "=".repeat(SEP_WIDE));
    println!("T0.2.3 retrieval-diagnostic spike — bge-small + default RetrievalOptions");
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Host:    {}", std::env::consts::OS);
    println!("{}", "=".repeat(SEP_WIDE));

    let dir = tempfile::tempdir().context("tempdir")?;
    println!("Tempdir: {}", dir.path().display());

    let key = SqlCipherKey::new("spike-only-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key)
        .await
        .context("MetadataStore::open")?;
    let metadata = Arc::new(metadata);

    let vectors_raw = LanceVectorStore::open_with_at_rest_key(
        &dir.path().join("vectors"),
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
    .await
    .context("LanceVectorStore::open_with_at_rest_key")?;
    let vectors: Arc<dyn VectorStore> = Arc::new(vectors_raw);

    println!("Opening BgeSmallProvider against bundled fixtures...");
    let bge = open_bge_provider().context("open BgeSmallProvider")?;

    // ── Load fixtures ────────────────────────────────────────────────────
    let memory_fixture_path = repo_root()?
        .join("crates")
        .join("vault-consolidator")
        .join("tests")
        .join("fixtures")
        .join("merge_acceptance_100.json");
    let memory_fixture: Vec<MemoryFixtureEntry> = {
        let bytes = std::fs::read(&memory_fixture_path)
            .with_context(|| format!("read memory fixture {memory_fixture_path:?}"))?;
        serde_json::from_slice(&bytes).context("parse memory fixture JSON")?
    };
    println!(
        "Loaded {} memories from {:?}",
        memory_fixture.len(),
        memory_fixture_path
    );

    let query_fixture_path = vault_retrieval_root()
        .join("test-fixtures")
        .join("merge_acceptance_100_queries.json");
    let query_set: QuerySet = {
        let bytes = std::fs::read(&query_fixture_path)
            .with_context(|| format!("read query fixture {query_fixture_path:?}"))?;
        serde_json::from_slice(&bytes).context("parse query fixture JSON")?
    };
    println!(
        "Loaded {} queries from {:?}",
        query_set.queries.len(),
        query_fixture_path
    );

    // ── Insert all fixture memories ──────────────────────────────────────
    //
    // Spike pattern: hit MetadataStore + VectorStore directly (the
    // semantic.rs::tests::insert convention). No cascading write, no retry
    // worker — the retriever doesn't care how memories got into the stores
    // as long as both have them. Skipping the cascade keeps the spike
    // setup ~3-4× faster than the cron-test's insert_and_drain path.
    println!(
        "\nInserting {} memories with BGE-computed embeddings...",
        memory_fixture.len()
    );
    let mut fixture_id_to_memory_id: HashMap<String, MemoryId> = HashMap::new();
    let insert_start = Instant::now();
    for (i, entry) in memory_fixture.iter().enumerate() {
        let boundary = Boundary::new(&entry.boundary).context("Boundary::new")?;
        let memory = Memory::try_new(NewMemory {
            content: entry.content.clone(),
            memory_type: MemoryType::Semantic,
            boundary,
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .context("Memory::try_new")?;

        let embedding = bge.embed(&entry.content).await.context("bge.embed")?;
        metadata
            .create_memory(&memory)
            .await
            .context("create_memory")?;
        vectors
            .upsert(&memory.id, &embedding, &memory.boundary)
            .await
            .context("vectors.upsert")?;

        fixture_id_to_memory_id.insert(entry.id.clone(), memory.id);

        if (i + 1) % 20 == 0 {
            println!("  inserted {}/{}", i + 1, memory_fixture.len());
        }
    }
    let insert_secs = insert_start.elapsed().as_secs_f64();
    println!(
        "Inserted {} memories in {:.1}s ({:.0} ms/memory)",
        memory_fixture.len(),
        insert_secs,
        (insert_secs * 1000.0) / memory_fixture.len() as f64
    );

    // ── Build retriever + reverse lookup ─────────────────────────────────
    let retriever = SemanticRetriever::new(metadata.clone(), bge, vectors.clone());

    let memory_id_to_fixture_id: HashMap<MemoryId, String> = fixture_id_to_memory_id
        .iter()
        .map(|(fid, mid)| (*mid, fid.clone()))
        .collect();
    let fixture_id_to_content: HashMap<String, String> = memory_fixture
        .iter()
        .map(|e| (e.id.clone(), e.content.clone()))
        .collect();

    // ── Run queries ──────────────────────────────────────────────────────
    println!(
        "\nRunning {} queries × max_results=20 (recall/precision computed at K∈{{5,10,20}})...\n",
        query_set.queries.len()
    );
    print_per_query_header();

    let mut results: Vec<QueryResult> = Vec::with_capacity(query_set.queries.len());
    let query_loop_start = Instant::now();
    for query in &query_set.queries {
        let mut boundaries = Vec::with_capacity(query.authorized_boundaries.len());
        for b in &query.authorized_boundaries {
            boundaries.push(Boundary::new(b).context("Boundary::new for query")?);
        }
        let q = RetrievalQuery {
            query_text: query.query_text.clone(),
            authorized_boundaries: boundaries,
            max_results: 20,
            options: RetrievalOptions::default(),
        };
        let hits = retriever.retrieve(q).await.context("retriever.retrieve")?;

        let expected_real_ids: HashSet<MemoryId> = query
            .expected_memory_ids
            .iter()
            .filter_map(|fid| fixture_id_to_memory_id.get(fid).copied())
            .collect();
        if expected_real_ids.len() != query.expected_memory_ids.len() {
            anyhow::bail!(
                "query {} references {} expected_memory_ids but only {} resolved against the loaded fixture",
                query.id,
                query.expected_memory_ids.len(),
                expected_real_ids.len()
            );
        }

        let qr = compute_query_result(
            query,
            &hits,
            &expected_real_ids,
            &memory_id_to_fixture_id,
            &fixture_id_to_content,
        );
        print_query_row(&qr);
        results.push(qr);
    }
    let query_loop_secs = query_loop_start.elapsed().as_secs_f64();
    println!(
        "\n22 queries completed in {:.2}s ({:.0} ms/query)",
        query_loop_secs,
        (query_loop_secs * 1000.0) / 22.0
    );

    // ── Aggregates ───────────────────────────────────────────────────────
    println!("\n{}", "=".repeat(SEP_WIDE));
    println!("AGGREGATE BY QUERY SHAPE");
    println!("{}", "=".repeat(SEP_WIDE));
    print_aggregate(&results, |r| r.shape.clone());

    println!("\n{}", "=".repeat(SEP_WIDE));
    println!("AGGREGATE BY CONTENT-LENGTH TIER");
    println!("{}", "=".repeat(SEP_WIDE));
    print_aggregate(&results, |r| r.length_tier.clone());

    // ── Contradiction-surfacing verdict ──────────────────────────────────
    println!("\n{}", "=".repeat(SEP_WIDE));
    println!("CONTRADICTION-SURFACING VERDICT (queries Q11 + Q13)");
    println!("{}", "=".repeat(SEP_WIDE));
    let q11 = results
        .iter()
        .find(|r| r.id == "Q11")
        .context("Q11 missing from results")?;
    let q13 = results
        .iter()
        .find(|r| r.id == "Q13")
        .context("Q13 missing from results")?;
    print_pair_verdict("Q11 (GA launch Q1 vs Q2)        ", q11);
    print_pair_verdict("Q13 (Comcast $89 vs $109)       ", q13);
    println!("\n  Verdict legend: PASS = both contradiction-pair memories in top-20;");
    println!("                  PARTIAL = exactly one in top-20; FAIL = neither.");

    // ── Hard-negative inspection ─────────────────────────────────────────
    println!("\n{}", "=".repeat(SEP_WIDE));
    println!("HARD-NEGATIVE INSPECTION (queries Q21 + Q22)");
    println!("{}", "=".repeat(SEP_WIDE));
    let q21 = results
        .iter()
        .find(|r| r.id == "Q21")
        .context("Q21 missing from results")?;
    let q22 = results
        .iter()
        .find(|r| r.id == "Q22")
        .context("Q22 missing from results")?;
    print_hard_negative(q21);
    print_hard_negative(q22);

    // ── Write markdown writeup ───────────────────────────────────────────
    let md_path = vault_retrieval_root()
        .join("examples")
        .join("t023_retrieval_diagnostic_results.md");
    let md = build_markdown_report(
        &results,
        &run_started,
        memory_fixture.len(),
        query_set.queries.len(),
        insert_secs,
        query_loop_secs,
    );
    std::fs::write(&md_path, md).context("write markdown report")?;
    println!("\n{}", "=".repeat(SEP_WIDE));
    println!("Markdown writeup: {}", md_path.display());
    println!(
        "Run completed:    {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("{}", "=".repeat(SEP_WIDE));

    Ok(())
}

// ── BGE provider setup ───────────────────────────────────────────────────

fn open_bge_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let fixture_root = vault_embedding_test_fixtures()?;
    let model = fixture_root.join("model.onnx");
    let tokenizer = fixture_root.join("tokenizer.json");
    let ort_lib = fixture_root.join(ort_lib_name());
    for p in [&model, &tokenizer, &ort_lib] {
        ensure!(
            p.exists(),
            "missing BGE fixture {p:?} — run scripts/setup-dev-env.ps1 from repo root"
        );
    }
    let provider = BgeSmallProvider::open(&model, &tokenizer, &ort_lib)
        .context("BgeSmallProvider::open against bge-small-en-v1.5 fixtures")?;
    Ok(Arc::new(provider))
}

#[cfg(target_os = "windows")]
fn ort_lib_name() -> &'static str {
    "onnxruntime.dll"
}

#[cfg(target_os = "linux")]
fn ort_lib_name() -> &'static str {
    "libonnxruntime.so"
}

#[cfg(target_os = "macos")]
fn ort_lib_name() -> &'static str {
    "libonnxruntime.dylib"
}

fn vault_retrieval_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> Result<PathBuf> {
    vault_retrieval_root()
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .context("vault-retrieval dir has no grandparent (repo root)")
}

fn vault_embedding_test_fixtures() -> Result<PathBuf> {
    let p = repo_root()?
        .join("crates")
        .join("vault-embedding")
        .join("test-fixtures")
        .join("bge-small-en-v1.5");
    ensure!(p.exists(), "bge-small-en-v1.5 fixture dir missing at {p:?}");
    Ok(p)
}

// ── Per-query computation ────────────────────────────────────────────────

fn compute_query_result(
    query: &QueryEntry,
    hits: &[RetrievedMemory],
    expected_real_ids: &HashSet<MemoryId>,
    memory_id_to_fixture_id: &HashMap<MemoryId, String>,
    fixture_id_to_content: &HashMap<String, String>,
) -> QueryResult {
    let n_expected = expected_real_ids.len();
    let mut hits_at_5 = 0_usize;
    let mut hits_at_10 = 0_usize;
    let mut hits_at_20 = 0_usize;
    let mut first_rank: Option<usize> = None;
    let mut top_returned: Vec<TopHit> = Vec::with_capacity(hits.len());

    for (i, h) in hits.iter().enumerate() {
        let rank = i + 1;
        let is_expected = expected_real_ids.contains(&h.memory.id);
        let fixture_id = memory_id_to_fixture_id.get(&h.memory.id).cloned();
        let content_snippet = fixture_id
            .as_ref()
            .and_then(|fid| fixture_id_to_content.get(fid).cloned())
            .map(|c| {
                let max_chars = 60_usize;
                let s: String = c.chars().take(max_chars).collect();
                if c.chars().count() > max_chars {
                    format!("{s}…")
                } else {
                    s
                }
            })
            .unwrap_or_else(|| "<unknown>".to_string());

        if is_expected {
            if rank <= 5 {
                hits_at_5 += 1;
            }
            if rank <= 10 {
                hits_at_10 += 1;
            }
            if rank <= 20 {
                hits_at_20 += 1;
            }
            if first_rank.is_none() {
                first_rank = Some(rank);
            }
        }
        top_returned.push(TopHit {
            rank,
            fixture_id,
            score: h.score,
            is_expected,
            content_snippet,
        });
    }

    // Recall is undefined when expected_count = 0 (hard-negatives). Use NaN
    // so the aggregate path can filter those out without conflating with 0.
    let recall_or_nan = |hits: usize| -> f64 {
        if n_expected == 0 {
            f64::NAN
        } else {
            hits as f64 / n_expected as f64
        }
    };
    let recall_at_5 = recall_or_nan(hits_at_5);
    let recall_at_10 = recall_or_nan(hits_at_10);
    let recall_at_20 = recall_or_nan(hits_at_20);

    let precision_at_5 = hits_at_5 as f64 / 5.0;
    let precision_at_10 = hits_at_10 as f64 / 10.0;
    let precision_at_20 = hits_at_20 as f64 / 20.0;

    let mrr = first_rank.map_or(0.0, |r| 1.0 / r as f64);

    QueryResult {
        id: query.id.clone(),
        shape: query.shape.clone(),
        length_tier: query.length_tier.clone(),
        query_text: query.query_text.clone(),
        notes: query.notes.clone(),
        expected_count: n_expected,
        expected_hits_at_5: hits_at_5,
        expected_hits_at_10: hits_at_10,
        expected_hits_at_20: hits_at_20,
        recall_at_5,
        recall_at_10,
        recall_at_20,
        precision_at_5,
        precision_at_10,
        precision_at_20,
        mrr,
        first_expected_rank: first_rank,
        top_returned,
    }
}

// ── Console formatters ───────────────────────────────────────────────────

fn print_per_query_header() {
    println!(
        "{:<5} {:<width_shape$} {:<width_tier$} | r@5  r@10 r@20 | p@5  p@10 p@20 |  MRR  | first-hit",
        "Q",
        "shape",
        "length tier",
        width_shape = COL_SHAPE,
        width_tier = COL_TIER,
    );
    println!("{}", "-".repeat(SEP_WIDE));
}

fn print_query_row(qr: &QueryResult) {
    let first_rank = qr
        .first_expected_rank
        .map_or_else(|| "—".to_string(), |r| r.to_string());
    let fmt_recall = |v: f64| -> String {
        if v.is_nan() {
            " —  ".to_string()
        } else {
            format!("{v:>4.2}")
        }
    };
    println!(
        "{:<5} {:<width_shape$} {:<width_tier$} | {} {} {} | {:>4.2} {:>4.2} {:>4.2} | {:>5.3} | {}",
        qr.id,
        qr.shape,
        qr.length_tier,
        fmt_recall(qr.recall_at_5),
        fmt_recall(qr.recall_at_10),
        fmt_recall(qr.recall_at_20),
        qr.precision_at_5,
        qr.precision_at_10,
        qr.precision_at_20,
        qr.mrr,
        first_rank,
        width_shape = COL_SHAPE,
        width_tier = COL_TIER,
    );
}

fn print_aggregate<F>(results: &[QueryResult], group_by: F)
where
    F: Fn(&QueryResult) -> String,
{
    let mut groups: HashMap<String, Vec<&QueryResult>> = HashMap::new();
    for r in results {
        groups.entry(group_by(r)).or_default().push(r);
    }
    let mut keys: Vec<String> = groups.keys().cloned().collect();
    keys.sort();
    println!(
        "{:<24}  n  | avg r@5  avg r@10  avg r@20 | avg MRR",
        "group"
    );
    println!("{}", "-".repeat(80));
    for k in &keys {
        let group = &groups[k];
        let recall_eligible: Vec<&&QueryResult> =
            group.iter().filter(|r| r.expected_count > 0).collect();
        let n = group.len();
        if recall_eligible.is_empty() {
            println!(
                "{:<24} {:>2}  | (no recall-eligible queries in this group)",
                k, n
            );
            continue;
        }
        let avg = |f: fn(&QueryResult) -> f64| -> f64 {
            recall_eligible.iter().map(|r| f(r)).sum::<f64>() / recall_eligible.len() as f64
        };
        println!(
            "{:<24} {:>2}  |   {:>4.2}     {:>4.2}      {:>4.2}    |  {:>5.3}",
            k,
            n,
            avg(|r| r.recall_at_5),
            avg(|r| r.recall_at_10),
            avg(|r| r.recall_at_20),
            avg(|r| r.mrr),
        );
    }
}

fn pair_verdict(qr: &QueryResult) -> &'static str {
    if qr.expected_count == 0 {
        return "N/A";
    }
    if qr.expected_hits_at_20 == qr.expected_count {
        "PASS"
    } else if qr.expected_hits_at_20 > 0 {
        "PARTIAL"
    } else {
        "FAIL"
    }
}

fn print_pair_verdict(label: &str, qr: &QueryResult) {
    println!(
        "  {}  {:<7}  recall@20 = {:.2}  ({}/{} expected in top-20)  first-hit-rank = {}",
        label,
        pair_verdict(qr),
        qr.recall_at_20,
        qr.expected_hits_at_20,
        qr.expected_count,
        qr.first_expected_rank
            .map_or_else(|| "—".to_string(), |r| r.to_string()),
    );
    for t in qr.top_returned.iter().take(5) {
        let marker = if t.is_expected { "✓" } else { " " };
        let fid = t.fixture_id.as_deref().unwrap_or("<unknown>");
        println!(
            "      {} rank {:>2}  score={:.4}  {:<22}  {}",
            marker, t.rank, t.score, fid, t.content_snippet
        );
    }
}

fn print_hard_negative(qr: &QueryResult) {
    let top_score = qr.top_returned.first().map_or(0.0, |t| t.score);
    let n_above_05 = qr.top_returned.iter().filter(|t| t.score >= 0.5).count();
    let n_above_07 = qr.top_returned.iter().filter(|t| t.score >= 0.7).count();
    println!(
        "  {}: top-1 score = {:.4}, # results with score≥0.5 = {}, # with score≥0.7 = {}",
        qr.id, top_score, n_above_05, n_above_07
    );
    println!("    query: \"{}\"", qr.query_text);
    println!("    top-5 returned (none of these are 'expected' since the vault has no matching memories):");
    for t in qr.top_returned.iter().take(5) {
        let fid = t.fixture_id.as_deref().unwrap_or("<unknown>");
        println!(
            "      rank {:>2}  score={:.4}  {:<22}  {}",
            t.rank, t.score, fid, t.content_snippet
        );
    }
}

// ── Markdown writeup ─────────────────────────────────────────────────────

fn build_markdown_report(
    results: &[QueryResult],
    run_started: &chrono::DateTime<chrono::Utc>,
    n_memories: usize,
    n_queries: usize,
    insert_secs: f64,
    query_loop_secs: f64,
) -> String {
    let mut s = String::new();
    s.push_str("# T0.2.3 Retrieval-Diagnostic Spike — Results\n\n");
    s.push_str(&format!(
        "**Run started:** {}  \n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    s.push_str(&format!("**Host OS:** {}  \n", std::env::consts::OS));
    s.push_str("**Embedding model:** bge-small-en-v1.5 (384-dim, BgeSmallProvider)  \n");
    s.push_str("**Retriever:** SemanticRetriever with `RetrievalOptions::default()` (no score threshold, `include_archived = false`)  \n");
    s.push_str("**Storage:** sealed LanceVectorStore + MetadataStore against a tempdir (single-process, single-run)  \n");
    s.push_str(&format!(
        "**Fixture:** {} memories from `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json`  \n",
        n_memories
    ));
    s.push_str(&format!(
        "**Queries:** {} from `crates/vault-retrieval/test-fixtures/merge_acceptance_100_queries.json`  \n",
        n_queries
    ));
    s.push_str(&format!(
        "**Setup wall time:** {:.1}s memory insertion ({:.0} ms/memory, includes BGE embed + LanceDB upsert + SQLite write)  \n",
        insert_secs,
        (insert_secs * 1000.0) / n_memories as f64
    ));
    s.push_str(&format!(
        "**Query wall time:** {:.2}s for {} queries ({:.0} ms/query at max_results=20)  \n\n",
        query_loop_secs,
        n_queries,
        (query_loop_secs * 1000.0) / n_queries as f64
    ));

    s.push_str("> **Discipline note.** This document reports measurements only. Architectural conclusions ");
    s.push_str("(scenario A / B / C, read-time re-rank viability, embedding-layer overhaul) live in a separate ");
    s.push_str(
        "conversation with Shahbaz once the data is in. No ADR amendments, no plan changes, no ",
    );
    s.push_str("threshold tuning, no code changes are derived in this file.\n\n");

    s.push_str("---\n\n");

    // ── Aggregate by shape ───────────────────────────────────────────────
    s.push_str("## Aggregate by query shape\n\n");
    s.push_str(&markdown_aggregate(results, |r| r.shape.clone()));
    s.push('\n');

    // ── Aggregate by length tier ─────────────────────────────────────────
    s.push_str("## Aggregate by content-length tier\n\n");
    s.push_str("This is the load-bearing breakdown for the architectural question. If recall is healthy on ");
    s.push_str("`short-only` but degraded on `mixed-length` and `long-form-dominant`, the same length-variance ");
    s.push_str("pattern that broke pairwise clustering is also present in query-anchored retrieval (Scenario B/C). ");
    s.push_str("If recall is uniform across tiers, the query anchor compensates for what pairwise cosine couldn't ");
    s.push_str("(Scenario A).\n\n");
    s.push_str(&markdown_aggregate(results, |r| r.length_tier.clone()));
    s.push('\n');

    // ── Contradiction verdict ────────────────────────────────────────────
    s.push_str("## Contradiction-surfacing verdict (Q11, Q13)\n\n");
    s.push_str(
        "**The single most product-critical output of this spike.** Read-time re-rank with Phi-4 ",
    );
    s.push_str(
        "can only flag a contradiction to the agent if BGE retrieval surfaces BOTH halves of the ",
    );
    s.push_str("conflicting pair in the top-K. If retrieval drops one, Phi-4 has nothing to compare against.\n\n");
    s.push_str(
        "| Query | Pair | Verdict | recall@20 | top-20 hits / expected | first-hit rank |\n",
    );
    s.push_str("|---|---|---|---|---|---|\n");
    for id in ["Q11", "Q13"] {
        if let Some(qr) = results.iter().find(|r| r.id == id) {
            let label = match id {
                "Q11" => "GA launch Q1 vs Q2",
                "Q13" => "Comcast $89 vs $109",
                _ => "",
            };
            s.push_str(&format!(
                "| {} | {} | **{}** | {:.2} | {} / {} | {} |\n",
                id,
                label,
                pair_verdict(qr),
                qr.recall_at_20,
                qr.expected_hits_at_20,
                qr.expected_count,
                qr.first_expected_rank
                    .map_or_else(|| "—".to_string(), |r| r.to_string()),
            ));
        }
    }
    s.push_str(
        "\n**Verdict legend.** PASS = both contradiction-pair memories appeared in top-20. ",
    );
    s.push_str("PARTIAL = exactly one in top-20 (read-time re-rank cannot surface the conflict). ");
    s.push_str("FAIL = neither in top-20 (retrieval is blind to the contradiction).\n\n");

    // ── Hard-negative inspection ─────────────────────────────────────────
    s.push_str("## Hard-negative inspection (Q21, Q22)\n\n");
    s.push_str(
        "Are BGE scores absolute (low score → no good match) or relative (top-K always returned ",
    );
    s.push_str(
        "even if scores are bad)? Low top-1 scores on hard-negatives → score-threshold gating ",
    );
    s.push_str("would let the retriever say \"I don't know\" cleanly. High top-1 scores → false ");
    s.push_str(
        "positives are a precision problem we'd need to handle elsewhere (read-time re-rank can ",
    );
    s.push_str("filter, or absolute thresholds must be applied at the retriever).\n\n");
    for id in ["Q21", "Q22"] {
        if let Some(qr) = results.iter().find(|r| r.id == id) {
            s.push_str(&format!("### {} — \"{}\"\n\n", id, qr.query_text));
            let top_score = qr.top_returned.first().map_or(0.0, |t| t.score);
            let n_above_05 = qr.top_returned.iter().filter(|t| t.score >= 0.5).count();
            let n_above_07 = qr.top_returned.iter().filter(|t| t.score >= 0.7).count();
            s.push_str(&format!(
                "- Top-1 score: **{:.4}**  \n- Results with score ≥ 0.5: {}  \n- Results with score ≥ 0.7: {}  \n\n",
                top_score, n_above_05, n_above_07
            ));
            s.push_str(
                "Top-5 returned (all false positives — vault has no matching memories):\n\n",
            );
            s.push_str("| Rank | Score | Fixture ID | Content snippet |\n");
            s.push_str("|---|---|---|---|\n");
            for t in qr.top_returned.iter().take(5) {
                s.push_str(&format!(
                    "| {} | {:.4} | {} | {} |\n",
                    t.rank,
                    t.score,
                    t.fixture_id.as_deref().unwrap_or("<unknown>"),
                    escape_md(&t.content_snippet),
                ));
            }
            s.push('\n');
        }
    }

    // ── Per-query detail ─────────────────────────────────────────────────
    s.push_str("---\n\n## Per-query detail (all 22 queries)\n\n");
    for qr in results {
        s.push_str(&format!(
            "### {} — {} / {}\n\n",
            qr.id, qr.shape, qr.length_tier
        ));
        s.push_str(&format!("**Query:** \"{}\"\n\n", qr.query_text));
        s.push_str(&format!("**Notes:** {}\n\n", qr.notes));
        s.push_str(&format!(
            "**Expected memories:** {}  \n**Top-20 hits / expected:** {} / {}  \n",
            qr.expected_count, qr.expected_hits_at_20, qr.expected_count
        ));
        let fmt_recall = |v: f64| -> String {
            if v.is_nan() {
                "—".to_string()
            } else {
                format!("{v:.2}")
            }
        };
        let fmt_hits = |hits: usize| -> String {
            if qr.expected_count == 0 {
                "—".to_string()
            } else {
                format!("{hits}/{}", qr.expected_count)
            }
        };
        s.push_str(&format!(
            "**Recall:** @5={} ({}) · @10={} ({}) · @20={} ({})  \n",
            fmt_recall(qr.recall_at_5),
            fmt_hits(qr.expected_hits_at_5),
            fmt_recall(qr.recall_at_10),
            fmt_hits(qr.expected_hits_at_10),
            fmt_recall(qr.recall_at_20),
            fmt_hits(qr.expected_hits_at_20),
        ));
        s.push_str(&format!(
            "**Precision:** @5={:.2} · @10={:.2} · @20={:.2}  \n",
            qr.precision_at_5, qr.precision_at_10, qr.precision_at_20
        ));
        s.push_str(&format!("**MRR:** {:.3}  \n", qr.mrr));
        s.push_str(&format!(
            "**First-hit rank:** {}\n\n",
            qr.first_expected_rank.map_or_else(
                || "— (no expected hit in top-20)".to_string(),
                |r| r.to_string()
            ),
        ));
        s.push_str("Top-10 returned:\n\n");
        s.push_str("| Rank | Score | Expected? | Fixture ID | Snippet |\n");
        s.push_str("|---|---|---|---|---|\n");
        for t in qr.top_returned.iter().take(10) {
            s.push_str(&format!(
                "| {} | {:.4} | {} | {} | {} |\n",
                t.rank,
                t.score,
                if t.is_expected { "✓" } else { "" },
                t.fixture_id.as_deref().unwrap_or("<unknown>"),
                escape_md(&t.content_snippet),
            ));
        }
        s.push('\n');
    }

    s.push_str("---\n\n");
    s.push_str("## Architectural decision — DEFERRED\n\n");
    s.push_str("This document is data, not decision. The architectural call (scenario A read-time re-rank ");
    s.push_str(
        "/ scenario B both-surfaces / scenario C embedding-layer overhaul) happens in a separate ",
    );
    s.push_str(
        "conversation with Shahbaz once these numbers are read. When that decision is made, the ",
    );
    s.push_str(
        "relevant ADR (e.g. ADR-048) will land alongside the production code that implements it, ",
    );
    s.push_str("and this spike's artefacts will ride with that commit per ");
    s.push_str("`feedback_spike_examples_bundle_with_consumer_code.md`.\n");

    s
}

fn markdown_aggregate<F>(results: &[QueryResult], group_by: F) -> String
where
    F: Fn(&QueryResult) -> String,
{
    let mut groups: HashMap<String, Vec<&QueryResult>> = HashMap::new();
    for r in results {
        groups.entry(group_by(r)).or_default().push(r);
    }
    let mut keys: Vec<String> = groups.keys().cloned().collect();
    keys.sort();
    let mut s = String::new();
    s.push_str("| Group | n queries | avg recall@5 | avg recall@10 | avg recall@20 | avg MRR |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    for k in &keys {
        let group = &groups[k];
        let recall_eligible: Vec<&&QueryResult> =
            group.iter().filter(|r| r.expected_count > 0).collect();
        let n = group.len();
        if recall_eligible.is_empty() {
            s.push_str(&format!("| `{}` | {} | — | — | — | — |\n", k, n));
            continue;
        }
        let avg = |f: fn(&QueryResult) -> f64| -> f64 {
            recall_eligible.iter().map(|r| f(r)).sum::<f64>() / recall_eligible.len() as f64
        };
        s.push_str(&format!(
            "| `{}` | {} | {:.2} | {:.2} | {:.2} | {:.3} |\n",
            k,
            n,
            avg(|r| r.recall_at_5),
            avg(|r| r.recall_at_10),
            avg(|r| r.recall_at_20),
            avg(|r| r.mrr),
        ));
    }
    s
}

fn escape_md(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}
