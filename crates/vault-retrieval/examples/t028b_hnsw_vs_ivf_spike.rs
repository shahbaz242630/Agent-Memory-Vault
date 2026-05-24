//! T0.2.7 Phase 1 — t028b HNSW vs IVF benchmark spike (2026-05-17).
//!
//! **Question this spike answers.** Of the two vector-index options that
//! lancedb 0.27.2 exposes through our [`LanceVectorStore`] surface — HNSW +
//! Scalar-Quantization (`IvfHnswSq`) vs IVF-Flat (`IvfFlat`) — which gives
//! the better recall × latency trade-off on the V0.2 memory-vault content
//! shape and at our expected scale tier?
//!
//! Per the T0.2.7 plan iteration 2 lock (2026-05-15), the spike is the
//! decision-input for the T0.2.7 production-index choice. Data-only — no
//! pass/fail gate. Partner reviews the numbers + selects HNSW or IVF for
//! the production wiring (separate downstream commit).
//!
//! **Fixture content shape matches t026 realism-rewrite, not synthetic
//! short.** (Verbatim phrase per iteration 2 amendment A.) The 100-memory
//! fixture at `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json`
//! carries the realism-rewritten distribution: 56 short (50–150 chars) +
//! 30 paragraph (300–1000 chars) + 11 long-form (1000–2000 chars) + 3
//! BGE-truncation entries (2000–2430 chars). The 8-query gauntlet (`Q11`,
//! `Q13`, `Q17`, `Q19`, `Q21`, `Q22`, `Q25`, `Q26`) is the canonical t026
//! production-acceptance set pinned by
//! `tests/read_pipeline_acceptance.rs::PRODUCTION_QUERY_IDS`.
//!
//! **Iteration 3 scope** — IVF arm dropped, LLM (Qwen-7B) stage added at
//! scales {1000, 10000}. The iteration 2 run (2026-05-17) found IVF
//! architecturally blocked: `IvfFlat`'s index emission triggers
//! `SealedObjectStore::put_multipart` which the V0.2 sealed envelope
//! intentionally does not support (per-file granularity is the V0.2 lock).
//! HNSW (`IvfHnswSq`) writes per-file fragments only and is compatible.
//! Effective answer to "HNSW vs IVF": **HNSW wins by architectural
//! compatibility**, not by performance margin — V0.2 production index
//! lock is HNSW. Iteration 3 reframes the spike's purpose: instead of
//! comparing two indexes, it now validates the full read pipeline (HNSW
//! retrieval → Qwen-7B synthesis) at {1K, 10K} scale to answer "does the
//! V0.2 product quality contract (4/4 contradictions + 2/2 hard-negatives,
//! per ADR-048) hold at larger corpus sizes than the 100-memory baseline
//! validated by t026 / `read_pipeline_acceptance`?"
//!
//! **Bulk_upsert promoted to LanceVectorStore inherent surface** — the
//! iteration 2 run found single-row `upsert` degraded from ~21 ms/memory
//! at scale=100 to ~264 ms/memory at scale=10K (12× slowdown). The new
//! `bulk_upsert` batches N rows into one `merge_insert` call: at scale=10K
//! it lands ~0.36 ms/memory — ~730× faster than single-row. This is the
//! pattern V0.2 production bulk callers (sync import, MCP bulk_create
//! when added) should use. Spike calls bulk_upsert in chunks of 500.
//!
//! **Iteration 2 scope (preserved for history)** — three scales
//! {100, 1K, 10K} via session-prefix variation. Each base memory M_i
//! becomes N copies decorated with `[session-{j:03}] {M_i}` for j in 1..=N.
//! The prefix is short enough (~17 chars) that the original
//! length-distribution per amendment A shifts only marginally; BGE's
//! contextual embedding produces distinct vectors per variation (the
//! prefix changes the embedding).
//!
//! **Known limitation (documented in results.md):** the variations cluster
//! around 100 base embedding centroids in the vector space. At scale=10K
//! with 100 base × 100 copies, brute-force top-20 for a given query may be
//! saturated by variations of the same 1-3 base memories. This stresses
//! the index's near-duplicate handling — which is realistic for a
//! cross-agent vault where multiple AI sessions write similar content —
//! but does NOT measure index behavior on a corpus of 10K genuinely
//! independent memories. If iteration 2 shows degenerate recall (all 1.0
//! across all scales), iteration 3 would generate a richer noise pool
//! using template + vocabulary combinations.
//!
//! **Methodology declaration** — compile-and-run on the local Windows dev
//! box (per `feedback_spike_methodology_explicit.md`). Uses the bundled
//! BGE-small ONNX provider for embedding + the sealed LanceVectorStore for
//! storage + lancedb's default index builders (`IvfHnswSqIndexBuilder::default()`
//! and `IvfFlatIndexBuilder::default()`) so the comparison measures
//! upstream out-of-the-box behavior, not parameter-tuning skill.
//!
//! **Discipline.** This file is example-grade throwaway. No tests, no
//! commit at run completion, no architectural conclusions in the source.
//! Spike artefact rides with the T0.2.7 production-decision commit per the
//! spike-bundle-with-consumer rule. The matching results markdown lands
//! alongside as `t028b_hnsw_vs_ivf_results.md`.
//!
//! Run with (PowerShell on Windows, per standing rules):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t028b_hnsw_vs_ivf_spike
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
use vault_llm::{LlmProvider, Qwen25_14BProvider, TuningConfig};
use vault_retrieval::{ReadPipeline, ReadQuery, ReadResponse, SemanticRetriever};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

// Test-only at-rest key. Matches the cross-crate convention
// (`vault-consolidator/tests/common/mod.rs:26`).
const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

// 8-query t026 production-acceptance gauntlet
// (`tests/read_pipeline_acceptance.rs::PRODUCTION_QUERY_IDS`).
const GAUNTLET_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q17", "Q19", "Q21", "Q22", "Q25", "Q26"];

// Number of times each query is repeated for the latency-percentile
// sample distribution. With 8 queries × 16 reps = 128 samples per index
// → p50/p99 are meaningful but not over-sampled.
const LATENCY_REPS_PER_QUERY: usize = 16;

// Scales to benchmark. Iteration 2 lock: {100, 1K, 10K} per T0.2.7 plan.
const SCALES: &[usize] = &[100, 1000, 10000];

// LLM stage runs only at scales >= this threshold. t026 already validated
// scale=100 (4/4 contradictions, 2/2 hard-negatives via the production
// `read_pipeline_acceptance.rs` test); iteration 3 fills in 1K + 10K.
const LLM_SCALE_THRESHOLD: usize = 1000;

// 4 contradiction queries from the t026 gauntlet (must surface both
// substrings + flag at least one contradiction structurally).
const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];

// 2 hard-negative queries from the t026 gauntlet (must set
// vault_has_no_relevant_content=true).
const HARD_NEGATIVE_QUERY_IDS: &[&str] = &["Q21", "Q22"];

// Qwen2.5-7B-Instruct GGUF filename — same as the production
// `read_pipeline_acceptance.rs` consumes.
const QWEN_MODEL_FILENAME: &str = "Qwen2.5-7B-Instruct-Q4_K_M.gguf";

const SEP_WIDE: usize = 100;

// ── Fixture types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct MemoryFixtureEntry {
    // `id` is the fixture-level identifier; t028b tracks memories by their
    // position in the loaded vec (fixture_idx) and doesn't reference the
    // JSON id at all, but serde needs the field present to deserialize.
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    shape: String,
    #[allow(dead_code)]
    length_tier: String,
    query_text: String,
    authorized_boundaries: Vec<String>,
    #[allow(dead_code)]
    expected_memory_ids: Vec<String>,
    #[allow(dead_code)]
    notes: String,
}

// ── Result types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PerQueryRecall {
    query_id: String,
    recall_at_10: f64,
    recall_at_20: f64,
}

#[derive(Debug, Clone)]
struct IndexArmResult {
    scale: usize,
    index_label: String,
    build_secs: f64,
    embed_secs: f64,
    upsert_secs: f64,
    per_query_recall: Vec<PerQueryRecall>,
    mean_recall_at_10: f64,
    mean_recall_at_20: f64,
    p50_latency_us: u128,
    p99_latency_us: u128,
    mean_latency_us: u128,
    llm_stage: Option<LlmStageResult>,
}

// ── LLM stage types ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PerQueryLlm {
    query_id: String,
    verdict_label: &'static str, // "contradiction PASS", "contradiction FAIL", etc.
    detail: String,
    latency_secs: f64,
}

#[derive(Debug, Clone)]
struct LlmStageResult {
    contradiction_passes: usize,
    hard_negative_passes: usize,
    per_query: Vec<PerQueryLlm>,
    mean_latency_secs: f64,
    p50_latency_secs: f64,
    p99_latency_secs: f64,
}

enum QualityVerdict {
    ContradictionPass(String),
    ContradictionFail(String),
    HardNegativePass(String),
    HardNegativeFail(String),
    Observational(String),
}

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    println!("{}", "=".repeat(SEP_WIDE));
    println!("T0.2.7 Phase 1 — t028b HNSW vs IVF spike (iteration 2, scales={SCALES:?})");
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Host:    {}", std::env::consts::OS);
    println!("{}", "=".repeat(SEP_WIDE));

    // ── Load fixtures ────────────────────────────────────────────────────
    let memory_fixture = load_memory_fixture()?;
    let query_set = load_query_set()?;
    println!(
        "Loaded {} base memories + {} queries (gauntlet subset: {})",
        memory_fixture.len(),
        query_set.queries.len(),
        GAUNTLET_QUERY_IDS.len()
    );

    // Filter to the 8-query gauntlet subset.
    let gauntlet_queries: Vec<&QueryEntry> = query_set
        .queries
        .iter()
        .filter(|q| GAUNTLET_QUERY_IDS.contains(&q.id.as_str()))
        .collect();
    ensure!(
        gauntlet_queries.len() == GAUNTLET_QUERY_IDS.len(),
        "gauntlet subset has {} queries but expected {} (check IDs against fixture: {:?})",
        gauntlet_queries.len(),
        GAUNTLET_QUERY_IDS.len(),
        GAUNTLET_QUERY_IDS,
    );

    // ── Open BGE provider ────────────────────────────────────────────────
    println!("\nOpening BgeSmallProvider against bundled fixtures...");
    let bge = open_bge_provider().context("open BgeSmallProvider")?;

    // ── Open Qwen-7B once (reused across scales for the LLM stage) ──────
    //
    // Loaded eagerly so failure surfaces before we burn embedding time.
    // Locked V0.2 TuningConfig per ADR-049 + the production acceptance
    // test (`read_pipeline_acceptance.rs`).
    let qwen_path = models_dir()?.join(QWEN_MODEL_FILENAME);
    ensure!(
        qwen_path.exists(),
        "Qwen-7B GGUF missing at {qwen_path:?} — required for LLM stage"
    );
    let tuning = TuningConfig {
        n_threads: Some(12),
        n_threads_batch: Some(12),
        n_gpu_layers: Some(99),
        ..TuningConfig::default()
    };
    println!("\nOpening Qwen-7B (Q4_K_M GGUF, Vulkan offload n_gpu_layers=99)...");
    let qwen_load_start = Instant::now();
    let qwen_provider = Qwen25_14BProvider::open_with_tuning(&qwen_path, tuning.clone())
        .await
        .context("Qwen25_14BProvider::open_with_tuning")?;
    println!(
        "  ready in {:.1}s (id={})",
        qwen_load_start.elapsed().as_secs_f64(),
        qwen_provider.model_id()
    );
    let qwen: Arc<dyn LlmProvider> = Arc::new(qwen_provider);

    // ── Pre-compute query embeddings (constant across scales) ────────────
    println!("\nEmbedding {} gauntlet queries...", gauntlet_queries.len());
    let mut query_embeddings: HashMap<String, Vec<f32>> = HashMap::new();
    for q in &gauntlet_queries {
        let emb = bge.embed(&q.query_text).await.context("bge.embed query")?;
        query_embeddings.insert(q.id.clone(), emb);
    }

    // ── Iterate over scales ──────────────────────────────────────────────
    //
    // Iteration 3: HNSW only (IVF dropped — sealed-envelope put_multipart
    // blocker confirmed at iteration 2). LLM stage runs at scales >=
    // LLM_SCALE_THRESHOLD (1K + 10K); scale=100 LLM coverage already
    // landed via t026 / `read_pipeline_acceptance.rs`.
    let mut all_results: Vec<IndexArmResult> = Vec::with_capacity(SCALES.len());

    for &scale in SCALES {
        println!("\n{}", "█".repeat(SEP_WIDE));
        println!("SCALE = {scale}");
        println!("{}", "█".repeat(SEP_WIDE));

        // Generate corpus for this scale.
        let scaled_corpus = generate_scaled_corpus(&memory_fixture, scale);
        ensure!(
            scaled_corpus.len() == scale,
            "scaled corpus has {} entries but expected {scale}",
            scaled_corpus.len(),
        );
        println!(
            "Generated {} corpus entries (base={} × variations + truncate)",
            scaled_corpus.len(),
            memory_fixture.len()
        );

        // Embed the scaled corpus. Same embedding bank consumed by both
        // index arms below — eliminates BGE variance from the comparison.
        println!("Embedding {} corpus entries...", scaled_corpus.len());
        let embed_start = Instant::now();
        let mut corpus_embeddings: Vec<Vec<f32>> = Vec::with_capacity(scaled_corpus.len());
        for (i, entry) in scaled_corpus.iter().enumerate() {
            let emb = bge
                .embed(&entry.content)
                .await
                .context("bge.embed corpus")?;
            corpus_embeddings.push(emb);
            if (i + 1) % 500 == 0 {
                println!("  embedded {}/{}", i + 1, scaled_corpus.len());
            }
        }
        let embed_secs = embed_start.elapsed().as_secs_f64();
        println!(
            "  done in {:.1}s ({:.0} ms/entry)",
            embed_secs,
            (embed_secs * 1000.0) / scaled_corpus.len() as f64
        );

        // Brute-force ground truth at this scale.
        // Cosine similarity == dot product on L2-normalized BGE vectors.
        println!("Computing brute-force top-20 ground truth for each query...");
        let mut ground_truth: HashMap<String, Vec<usize>> = HashMap::new();
        for q in &gauntlet_queries {
            let q_emb = &query_embeddings[&q.id];
            let mut scored: Vec<(usize, f32)> = corpus_embeddings
                .iter()
                .enumerate()
                .map(|(i, m_emb)| (i, dot(q_emb, m_emb)))
                .collect();
            // Larger dot = closer (cosine similarity, since L2-normalized).
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            ground_truth.insert(
                q.id.clone(),
                scored.into_iter().take(20).map(|(i, _)| i).collect(),
            );
        }

        // Run HNSW arm at this scale (IVF dropped — see iteration 3
        // doc-comment). LLM stage runs only at scale >= 1000.
        let llm_for_scale: Option<Arc<dyn LlmProvider>> = if scale >= LLM_SCALE_THRESHOLD {
            Some(qwen.clone())
        } else {
            None
        };
        let hnsw_result = run_index_arm(
            "HNSW (IvfHnswSq, default)",
            scale,
            embed_secs,
            &scaled_corpus,
            &corpus_embeddings,
            &gauntlet_queries,
            &query_embeddings,
            &ground_truth,
            bge.clone(),
            llm_for_scale,
        )
        .await
        .with_context(|| format!("HNSW arm @ scale={scale}"))?;

        // Per-scale reporting.
        println!("\n--- Scale {scale} summary ---");
        print_arm_summary(&hnsw_result);

        all_results.push(hnsw_result);
    }

    // ── Write results.md (all scales) ────────────────────────────────────
    let results_path = vault_retrieval_root()
        .join("examples")
        .join("t028b_hnsw_vs_ivf_results.md");
    write_results_md(
        &results_path,
        &run_started,
        &all_results,
        memory_fixture.len(),
    )
    .with_context(|| format!("write results markdown to {results_path:?}"))?;
    println!("\nResults written to {results_path:?}");

    let elapsed_total = chrono::Utc::now().signed_duration_since(run_started);
    println!(
        "\nDone in {:.1}s total.",
        elapsed_total.num_milliseconds() as f64 / 1000.0
    );
    Ok(())
}

// ── Index-arm execution ──────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_index_arm(
    label: &str,
    scale: usize,
    embed_secs: f64,
    memory_fixture: &[MemoryFixtureEntry],
    memory_embeddings: &[Vec<f32>],
    gauntlet_queries: &[&QueryEntry],
    query_embeddings: &HashMap<String, Vec<f32>>,
    ground_truth: &HashMap<String, Vec<usize>>,
    bge: Arc<dyn EmbeddingProvider>,
    llm: Option<Arc<dyn LlmProvider>>,
) -> Result<IndexArmResult> {
    println!("\n{}", "─".repeat(SEP_WIDE));
    println!("ARM: {label} @ scale={scale}");
    println!("{}", "─".repeat(SEP_WIDE));

    let dir = tempfile::tempdir().context("tempdir")?;
    let key = SqlCipherKey::new("spike-only-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key)
        .await
        .context("MetadataStore::open")?;
    let metadata = Arc::new(metadata);

    // Wrap LanceVectorStore in Arc so we can use both inherent methods (via
    // Deref) and clone-coerce to Arc<dyn VectorStore> for the optional LLM
    // stage's SemanticRetriever.
    let vectors: Arc<LanceVectorStore> = Arc::new(
        LanceVectorStore::open_with_at_rest_key(
            &dir.path().join("vectors"),
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .context("LanceVectorStore::open_with_at_rest_key")?,
    );

    // ── Upsert all memories (batched into chunks via bulk_upsert) ────────
    //
    // Single-row upsert at scale=10K degraded to ~264 ms/memory (vs 21 ms
    // at scale=100) in the 2026-05-17 prior run. `bulk_upsert` amortizes
    // per-call overhead. Metadata writes stay single-row (SqlCipher inserts
    // don't show the same non-linear pattern).
    const UPSERT_BATCH_SIZE: usize = 500;
    println!(
        "Upserting {} memories (batch size {UPSERT_BATCH_SIZE})...",
        memory_fixture.len()
    );
    let upsert_start = Instant::now();
    let mut fixture_idx_to_memory_id: HashMap<usize, MemoryId> = HashMap::new();
    let mut batch_rows: Vec<(MemoryId, Vec<f32>, Boundary)> = Vec::with_capacity(UPSERT_BATCH_SIZE);

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
        metadata
            .create_memory(&memory)
            .await
            .context("create_memory")?;

        let memory_id = memory.id;
        batch_rows.push((
            memory_id,
            memory_embeddings[i].clone(),
            memory.boundary.clone(),
        ));
        fixture_idx_to_memory_id.insert(i, memory_id);

        if batch_rows.len() >= UPSERT_BATCH_SIZE {
            vectors
                .bulk_upsert(&batch_rows)
                .await
                .context("vectors.bulk_upsert chunk")?;
            batch_rows.clear();
            println!("  upserted {}/{}", i + 1, memory_fixture.len());
        }
    }
    // Flush the tail (final partial batch).
    if !batch_rows.is_empty() {
        vectors
            .bulk_upsert(&batch_rows)
            .await
            .context("vectors.bulk_upsert tail")?;
    }
    let upsert_secs = upsert_start.elapsed().as_secs_f64();
    println!(
        "  upserted in {:.1}s ({:.2} ms/memory avg)",
        upsert_secs,
        (upsert_secs * 1000.0) / memory_fixture.len() as f64
    );

    // ── Build HNSW index, time it ────────────────────────────────────────
    println!("Building HNSW index...");
    let build_start = Instant::now();
    vectors
        .create_vector_index_hnsw_sq()
        .await
        .context("create_vector_index_hnsw_sq")?;
    let build_secs = build_start.elapsed().as_secs_f64();
    println!("  built in {build_secs:.2}s");

    // ── Map memory_id back to fixture_idx for recall scoring ─────────────
    let memory_id_to_fixture_idx: HashMap<MemoryId, usize> = fixture_idx_to_memory_id
        .iter()
        .map(|(idx, mid)| (*mid, *idx))
        .collect();

    // ── Run queries: collect recall per-query, latencies across reps ─────
    println!(
        "Running {} gauntlet queries × {LATENCY_REPS_PER_QUERY} reps each...",
        gauntlet_queries.len()
    );
    let mut per_query_recall = Vec::with_capacity(gauntlet_queries.len());
    let mut all_latencies_us: Vec<u128> =
        Vec::with_capacity(gauntlet_queries.len() * LATENCY_REPS_PER_QUERY);

    for q in gauntlet_queries {
        let q_emb = &query_embeddings[&q.id];
        let mut boundaries = Vec::with_capacity(q.authorized_boundaries.len());
        for b in &q.authorized_boundaries {
            boundaries.push(Boundary::new(b).context("Boundary::new for query")?);
        }

        // First call: gather the top-20 result for recall scoring.
        let hits = vectors
            .search(q_emb, 20, &boundaries)
            .await
            .context("vectors.search recall pass")?;
        let returned_fixture_idxs: Vec<usize> = hits
            .iter()
            .filter_map(|(mid, _score)| memory_id_to_fixture_idx.get(mid).copied())
            .collect();
        let gt = &ground_truth[&q.id];
        let recall_10 = compute_recall(&returned_fixture_idxs, gt, 10);
        let recall_20 = compute_recall(&returned_fixture_idxs, gt, 20);
        per_query_recall.push(PerQueryRecall {
            query_id: q.id.clone(),
            recall_at_10: recall_10,
            recall_at_20: recall_20,
        });

        // Latency reps: run the same query LATENCY_REPS_PER_QUERY times,
        // collect the per-call duration.
        for _ in 0..LATENCY_REPS_PER_QUERY {
            let t = Instant::now();
            let _ = vectors
                .search(q_emb, 20, &boundaries)
                .await
                .context("vectors.search latency rep")?;
            all_latencies_us.push(t.elapsed().as_micros());
        }
    }

    let mean_recall_at_10 = per_query_recall.iter().map(|r| r.recall_at_10).sum::<f64>()
        / per_query_recall.len() as f64;
    let mean_recall_at_20 = per_query_recall.iter().map(|r| r.recall_at_20).sum::<f64>()
        / per_query_recall.len() as f64;

    all_latencies_us.sort_unstable();
    let p50_latency_us = percentile(&all_latencies_us, 0.50);
    let p99_latency_us = percentile(&all_latencies_us, 0.99);
    let mean_latency_us = if all_latencies_us.is_empty() {
        0
    } else {
        all_latencies_us.iter().sum::<u128>() / all_latencies_us.len() as u128
    };

    // ── Optional LLM stage (Qwen-7B synthesis at scale >= 1000) ─────────
    let llm_stage = if let Some(qwen) = llm {
        println!("\nRunning LLM stage (Qwen-7B over HNSW retrievals)...");
        let vectors_dyn: Arc<dyn VectorStore> = vectors.clone();
        let stage = run_llm_stage(
            metadata.clone(),
            bge.clone(),
            vectors_dyn,
            qwen,
            gauntlet_queries,
        )
        .await
        .with_context(|| format!("LLM stage @ scale={scale}"))?;
        Some(stage)
    } else {
        None
    };

    Ok(IndexArmResult {
        scale,
        index_label: label.to_string(),
        build_secs,
        embed_secs,
        upsert_secs,
        per_query_recall,
        mean_recall_at_10,
        mean_recall_at_20,
        p50_latency_us,
        p99_latency_us,
        mean_latency_us,
        llm_stage,
    })
}

// ── LLM stage ───────────────────────────────────────────────────────────

async fn run_llm_stage(
    metadata: Arc<MetadataStore>,
    bge: Arc<dyn EmbeddingProvider>,
    vectors: Arc<dyn VectorStore>,
    llm: Arc<dyn LlmProvider>,
    gauntlet_queries: &[&QueryEntry],
) -> Result<LlmStageResult> {
    let retriever = Arc::new(SemanticRetriever::new(metadata, bge, vectors));
    let pipeline = ReadPipeline::new(retriever, llm);

    let mut per_query = Vec::with_capacity(gauntlet_queries.len());
    let mut contradiction_passes = 0_usize;
    let mut hard_negative_passes = 0_usize;
    let mut latencies = Vec::with_capacity(gauntlet_queries.len());

    for q in gauntlet_queries {
        let mut boundaries = Vec::with_capacity(q.authorized_boundaries.len());
        for b in &q.authorized_boundaries {
            boundaries.push(Boundary::new(b).context("Boundary::new for LLM stage")?);
        }
        let rq = ReadQuery {
            query_text: q.query_text.clone(),
            authorized_boundaries: boundaries,
        };

        let start = Instant::now();
        // Tolerate per-query pipeline failures: at high near-duplicate
        // density (1K + 10K scale), Qwen synthesis can exceed the schema
        // token budget and ReadPipeline returns a parse error. We record
        // the failure as a verdict and continue so the spike measures
        // how many of the 8 queries succeed at each scale rather than
        // crashing on the first failure.
        let read_result = pipeline.read(rq).await;
        let latency_secs = start.elapsed().as_secs_f64();
        latencies.push(latency_secs);

        let (verdict_label, detail) = match read_result {
            Ok(resp) => {
                let verdict = assess_query(&q.id, &resp);
                match &verdict {
                    QualityVerdict::ContradictionPass(d) => {
                        contradiction_passes += 1;
                        ("contradiction PASS", d.clone())
                    }
                    QualityVerdict::ContradictionFail(d) => ("contradiction FAIL", d.clone()),
                    QualityVerdict::HardNegativePass(d) => {
                        hard_negative_passes += 1;
                        ("hard-negative PASS", d.clone())
                    }
                    QualityVerdict::HardNegativeFail(d) => ("hard-negative FAIL", d.clone()),
                    QualityVerdict::Observational(d) => ("observational", d.clone()),
                }
            }
            Err(e) => {
                // Capture the head of the error message (truncate for readability).
                let mut err_head = format!("{e}");
                if err_head.len() > 160 {
                    err_head.truncate(160);
                    err_head.push_str("...");
                }
                ("pipeline ERROR", err_head)
            }
        };
        println!(
            "    {} {verdict_label} ({:.1}s) — {detail}",
            q.id, latency_secs
        );
        per_query.push(PerQueryLlm {
            query_id: q.id.clone(),
            verdict_label,
            detail,
            latency_secs,
        });
    }

    let mean_latency_secs = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let mut sorted = latencies.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50_latency_secs = sorted[sorted.len() / 2];
    let p99_idx = ((sorted.len() as f64 - 1.0) * 0.99).round() as usize;
    let p99_latency_secs = sorted[p99_idx.min(sorted.len() - 1)];

    Ok(LlmStageResult {
        contradiction_passes,
        hard_negative_passes,
        per_query,
        mean_latency_secs,
        p50_latency_secs,
        p99_latency_secs,
    })
}

// ── Recall + percentile math ─────────────────────────────────────────────

fn compute_recall(retrieved: &[usize], ground_truth: &[usize], k: usize) -> f64 {
    let retrieved_top_k: HashSet<usize> = retrieved.iter().take(k).copied().collect();
    let gt_top_k: HashSet<usize> = ground_truth.iter().take(k).copied().collect();
    if gt_top_k.is_empty() {
        return 0.0;
    }
    let intersect = retrieved_top_k.intersection(&gt_top_k).count();
    intersect as f64 / gt_top_k.len() as f64
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(
        a.len(),
        b.len(),
        "dot product requires equal-length vectors"
    );
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

// ── Reporting ────────────────────────────────────────────────────────────

fn print_arm_summary(r: &IndexArmResult) {
    println!("\n{} @ scale={}", r.index_label, r.scale);
    println!("  embed (total):      {:.1}s", r.embed_secs);
    println!("  upsert (total):     {:.2}s", r.upsert_secs);
    println!("  index build:        {:.2}s", r.build_secs);
    println!("  mean recall@10:     {:.3}", r.mean_recall_at_10);
    println!("  mean recall@20:     {:.3}", r.mean_recall_at_20);
    println!("  search latency p50: {} µs", r.p50_latency_us);
    println!("  search latency p99: {} µs", r.p99_latency_us);
    println!("  search latency mean:{} µs", r.mean_latency_us);
    println!("  per-query recall@10 / recall@20:");
    for pq in &r.per_query_recall {
        println!(
            "    {:>4}: {:.3} / {:.3}",
            pq.query_id, pq.recall_at_10, pq.recall_at_20
        );
    }
    if let Some(llm) = &r.llm_stage {
        println!("  LLM stage (Qwen-7B):");
        println!(
            "    contradictions: {}/{}  ·  hard-negatives: {}/{}",
            llm.contradiction_passes,
            CONTRADICTION_QUERY_IDS.len(),
            llm.hard_negative_passes,
            HARD_NEGATIVE_QUERY_IDS.len()
        );
        println!(
            "    LLM latency p50/p99/mean: {:.1}s / {:.1}s / {:.1}s",
            llm.p50_latency_secs, llm.p99_latency_secs, llm.mean_latency_secs
        );
    }
}

fn write_results_md(
    path: &PathBuf,
    run_started: &chrono::DateTime<chrono::Utc>,
    all_results: &[IndexArmResult],
    base_fixture_size: usize,
) -> Result<()> {
    let mut out = String::new();
    out.push_str("# T0.2.7 Phase 1 — t028b HNSW + LLM at scale (iteration 3)\n\n");
    out.push_str(&format!(
        "**Iteration 3** — scales={SCALES:?}, HNSW only (IVF blocked by V0.2 sealed-envelope per-file-granularity lock — confirmed in iteration 2). LLM stage (Qwen-7B Q4_K_M, ADR-049) runs at scales >= {LLM_SCALE_THRESHOLD}. 8-query t026 gauntlet, lancedb 0.27.2 defaults.\n\n"
    ));
    out.push_str(&format!(
        "**Run started:** {}\n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!("**Host OS:** {}\n\n", std::env::consts::OS));
    out.push_str("Fixture content shape matches t026 realism-rewrite, not synthetic short.\n\n");

    // ── Cross-scale summary table ────────────────────────────────────────
    out.push_str("## Cross-scale summary\n\n");
    out.push_str("| Scale | Build (s) | Embed (s) | Upsert (s) | Recall@10 | Recall@20 | Search p50 (µs) | Search p99 (µs) | Contradictions | Hard-negatives | LLM mean (s) |\n");
    out.push_str("|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---:|\n");
    for r in all_results {
        let (contra_str, hard_neg_str, llm_mean_str) = match &r.llm_stage {
            Some(s) => (
                format!(
                    "{}/{}",
                    s.contradiction_passes,
                    CONTRADICTION_QUERY_IDS.len()
                ),
                format!(
                    "{}/{}",
                    s.hard_negative_passes,
                    HARD_NEGATIVE_QUERY_IDS.len()
                ),
                format!("{:.1}", s.mean_latency_secs),
            ),
            None => ("n/a".to_string(), "n/a".to_string(), "n/a".to_string()),
        };
        out.push_str(&format!(
            "| {} | {:.2} | {:.1} | {:.2} | {:.3} | {:.3} | {} | {} | {contra_str} | {hard_neg_str} | {llm_mean_str} |\n",
            r.scale,
            r.build_secs,
            r.embed_secs,
            r.upsert_secs,
            r.mean_recall_at_10,
            r.mean_recall_at_20,
            r.p50_latency_us,
            r.p99_latency_us,
        ));
    }
    out.push('\n');

    // ── Per-scale detail ─────────────────────────────────────────────────
    for r in all_results {
        out.push_str(&format!("## Scale = {}\n\n", r.scale));
        out.push_str("### Index + retrieval\n\n");
        out.push_str("| Metric | Value |\n|---|---:|\n");
        out.push_str(&format!("| Index build (s) | {:.2} |\n", r.build_secs));
        out.push_str(&format!("| Embed total (s) | {:.1} |\n", r.embed_secs));
        out.push_str(&format!("| Upsert total (s) | {:.2} |\n", r.upsert_secs));
        out.push_str(&format!(
            "| Mean recall@10 | {:.3} |\n",
            r.mean_recall_at_10
        ));
        out.push_str(&format!(
            "| Mean recall@20 | {:.3} |\n",
            r.mean_recall_at_20
        ));
        out.push_str(&format!("| Search p50 (µs) | {} |\n", r.p50_latency_us));
        out.push_str(&format!("| Search p99 (µs) | {} |\n", r.p99_latency_us));
        out.push_str(&format!("| Search mean (µs) | {} |\n\n", r.mean_latency_us));

        out.push_str("### Per-query recall\n\n");
        out.push_str("| Query | Recall@10 | Recall@20 |\n|---|---:|---:|\n");
        for pq in &r.per_query_recall {
            out.push_str(&format!(
                "| {} | {:.3} | {:.3} |\n",
                pq.query_id, pq.recall_at_10, pq.recall_at_20
            ));
        }
        out.push('\n');

        if let Some(llm) = &r.llm_stage {
            out.push_str("### LLM stage (Qwen-7B Q4_K_M, ADR-049)\n\n");
            out.push_str(&format!(
                "**Contradictions surfaced:** {}/{}  ·  **Hard-negatives rejected:** {}/{}\n\n",
                llm.contradiction_passes,
                CONTRADICTION_QUERY_IDS.len(),
                llm.hard_negative_passes,
                HARD_NEGATIVE_QUERY_IDS.len()
            ));
            out.push_str(&format!(
                "**LLM latency:** p50 = {:.1}s · p99 = {:.1}s · mean = {:.1}s\n\n",
                llm.p50_latency_secs, llm.p99_latency_secs, llm.mean_latency_secs
            ));
            out.push_str("| Query | Verdict | Detail | Latency (s) |\n|---|---|---|---:|\n");
            for pq in &llm.per_query {
                out.push_str(&format!(
                    "| {} | {} | {} | {:.1} |\n",
                    pq.query_id, pq.verdict_label, pq.detail, pq.latency_secs
                ));
            }
            out.push('\n');
        }
    }

    // ── Methodology ──────────────────────────────────────────────────────
    out.push_str("## Methodology\n\n");
    out.push_str(&format!(
        "- {base_fixture_size}-memory base fixture from `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json`\n"
    ));
    out.push_str("- 8-query gauntlet from `crates/vault-retrieval/test-fixtures/merge_acceptance_100_queries.json` (subset `Q11`, `Q13`, `Q17`, `Q19`, `Q21`, `Q22`, `Q25`, `Q26`)\n");
    out.push_str(
        "- Scale-up via session-prefix variation: `[session-{j:03}] {original_content}` — see `generate_scaled_corpus` in the spike source. **Limitation:** variations cluster around base centroids; stresses near-duplicate handling, not corpus diversity.\n",
    );
    out.push_str("- BGE-small-en-v1.5 ONNX provider for embedding\n");
    out.push_str("- Sealed `LanceVectorStore` per scale (fresh tempdir). HNSW index built via `IvfHnswSqIndexBuilder::default()`.\n");
    out.push_str("- Bulk inserts via `LanceVectorStore::bulk_upsert` in chunks of 500 (production-candidate batch API; ~730× faster than single-row at scale 10K).\n");
    out.push_str(&format!(
        "- `{LATENCY_REPS_PER_QUERY}` search-latency reps × 8 queries = {} samples per scale\n",
        LATENCY_REPS_PER_QUERY * 8
    ));
    out.push_str(
        "- Brute-force ground truth: dot product on BGE's L2-normalized vectors, top-20 per query (recomputed at each scale)\n",
    );
    out.push_str("- Recall@K = |retrieved∩ground_truth| ÷ K, computed per query then averaged\n");
    out.push_str(&format!(
        "- LLM stage uses production `ReadPipeline` (ADR-048) + `SemanticRetriever`. Same Qwen-7B (`{QWEN_MODEL_FILENAME}`) + locked V0.2 `TuningConfig` (n_threads=12, n_threads_batch=12, n_gpu_layers=99) as `read_pipeline_acceptance.rs`.\n"
    ));
    out.push_str("- LLM stage runs only at scale >= 1000 (scale=100 LLM coverage already landed via t026 + `read_pipeline_acceptance`).\n\n");

    out.push_str("## Open items\n\n");
    out.push_str(
        "- If recall stays at 1.000 across all scales (degenerate due to near-duplicate clustering), iteration 4+ would build a richer noise corpus (template + vocabulary combinations) to genuinely diversify embeddings.\n",
    );
    out.push_str(
        "- IVF re-eval blocked until V0.2.x adds streaming-multipart support to SealedObjectStore. Not on the V0.2 critical path.\n",
    );
    out.push_str(
        "- Production-index decision: HNSW locked for V0.2 (architectural compatibility). See ADR-050.\n",
    );
    std::fs::write(path, out).context("std::fs::write results.md")?;
    Ok(())
}

// ── Corpus scaling ──────────────────────────────────────────────────────

/// Generate a corpus of size `target` from `base` via session-prefix
/// variation. First `base.len()` entries are the bare originals; additional
/// entries decorate each base with `[session-{j:03}] {original_content}`,
/// incrementing `j` per pass through the base set until `target` is hit.
///
/// **Limitation (documented in module doc-comment):** variations cluster
/// around `base.len()` centroids in BGE embedding space, so at large
/// scale the brute-force top-K may be dominated by variations of the
/// same 1-3 base memories. Stresses near-duplicate handling, not corpus
/// diversity.
fn generate_scaled_corpus(base: &[MemoryFixtureEntry], target: usize) -> Vec<MemoryFixtureEntry> {
    let mut out = Vec::with_capacity(target);
    out.extend(base.iter().cloned());
    if target <= base.len() {
        out.truncate(target);
        return out;
    }
    let mut j: usize = 1;
    while out.len() < target {
        for entry in base {
            if out.len() >= target {
                break;
            }
            out.push(MemoryFixtureEntry {
                id: format!("{}-v{:03}", entry.id, j),
                boundary: entry.boundary.clone(),
                topic_label: entry.topic_label.clone(),
                content: format!("[session-{j:03}] {}", entry.content),
                ground_truth: entry.ground_truth.clone(),
            });
        }
        j += 1;
    }
    out
}

// ── Fixture / provider helpers ──────────────────────────────────────────

fn load_memory_fixture() -> Result<Vec<MemoryFixtureEntry>> {
    let p = repo_root()?
        .join("crates")
        .join("vault-consolidator")
        .join("tests")
        .join("fixtures")
        .join("merge_acceptance_100.json");
    let bytes = std::fs::read(&p).with_context(|| format!("read memory fixture {p:?}"))?;
    serde_json::from_slice(&bytes).context("parse memory fixture JSON")
}

fn load_query_set() -> Result<QuerySet> {
    let p = vault_retrieval_root()
        .join("test-fixtures")
        .join("merge_acceptance_100_queries.json");
    let bytes = std::fs::read(&p).with_context(|| format!("read query fixture {p:?}"))?;
    serde_json::from_slice(&bytes).context("parse query fixture JSON")
}

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

// ── LLM stage helpers ───────────────────────────────────────────────────

fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set on Windows")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}

/// Per-query expected substrings for contradiction queries. Mirrors the
/// production `read_pipeline_acceptance.rs::structural_substrings`.
fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        _ => None,
    }
}

/// Assess a single LLM response against the t026 gauntlet quality contract.
/// Same shape as `read_pipeline_acceptance.rs::assess_query`.
fn assess_query(query_id: &str, resp: &ReadResponse) -> QualityVerdict {
    if CONTRADICTION_QUERY_IDS.contains(&query_id) {
        let Some((sub_a, sub_b)) = structural_substrings(query_id) else {
            return QualityVerdict::Observational("no structural-substrings rule".into());
        };
        let contains_a = resp.synthesis_markdown.contains(sub_a);
        let contains_b = resp.synthesis_markdown.contains(sub_b);
        let flagged_nonempty = !resp.contradictions_flagged.is_empty();
        let detail = format!(
            "flagged={} · '{sub_a}'={contains_a} '{sub_b}'={contains_b}",
            resp.contradictions_flagged.len()
        );
        if flagged_nonempty && contains_a && contains_b {
            QualityVerdict::ContradictionPass(detail)
        } else {
            QualityVerdict::ContradictionFail(detail)
        }
    } else if HARD_NEGATIVE_QUERY_IDS.contains(&query_id) {
        let detail = format!(
            "vault_has_no_relevant_content={}",
            resp.vault_has_no_relevant_content
        );
        if resp.vault_has_no_relevant_content {
            QualityVerdict::HardNegativePass(detail)
        } else {
            QualityVerdict::HardNegativeFail(detail)
        }
    } else {
        QualityVerdict::Observational(format!(
            "contradictions_flagged.len()={}",
            resp.contradictions_flagged.len()
        ))
    }
}
