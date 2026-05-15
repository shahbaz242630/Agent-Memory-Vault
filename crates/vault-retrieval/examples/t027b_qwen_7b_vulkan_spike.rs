//! T0.2.3 t027b — Qwen2.5-7B Vulkan iGPU offload spike.
//!
//! **Question this spike answers:** does offloading all Qwen-7B layers to
//! the Intel UHD Graphics iGPU via llama.cpp's Vulkan backend close the
//! ~14s gap from t12 CPU-only (134.2s mean per t027a + t14/t16 extension)
//! to the 120s hard ceiling, **without regressing quality** against the
//! t026 baseline (4/4 contradictions + 2/2 hard-negatives)?
//!
//! **Architecture:** identical pipeline to t026 / t027a — BGE retrieval
//! top-20 → single Qwen-7B synthesis call with GBNF JSON output. The only
//! change is `n_gpu_layers=99` on `LlamaModelParams` (offload all), plus
//! the `vulkan` Cargo feature enabled on `llama-cpp-2` so the Vulkan
//! backend is linked. CPU fallback happens automatically per layer if
//! GPU memory is insufficient — the spike captures llama.cpp's startup
//! offload report so partial-offload is visible.
//!
//! **Single config:**
//! - `n_gpu_layers = 99` (model param, set at provider open)
//! - `n_threads = 12, n_threads_batch = 12` (locked winner from t027a)
//! - KV cache: f16 default (Q8_0 hurt 34% on AVX2-without-VNNI per
//!   t027a evidence; do not override)
//!
//! **2-query warmup → 8-query full gauntlet:**
//! - Warmup (Q26 + Q19): latencies discarded; pages model + warms iGPU
//!   shaders / command buffers.
//! - Production: 8 queries matching t026's canonical set
//!   - Contradictions (Q11, Q13, Q25, Q26): substring + flagged-nonempty
//!     hard assertions.
//!   - Multi-cluster narrative (Q17, Q19): observability only.
//!   - Hard-negatives (Q21, Q22): `vault_has_no_relevant_content=true`
//!     hard assertion.
//!
//! **Quality gate (unchanged from t026):** any latency win that breaks
//! 4/4 contradictions OR 2/2 hard-negatives is a regression, not a win.
//! The results.md surfaces both side-by-side.
//!
//! **Memory headroom warning:** i7-13620H ships with 16 GB system RAM
//! (15.73 GB visible). Vulkan iGPU shares system RAM. If llama.cpp logs
//! anything less than full layer offload (e.g. "offloaded 20/28"), the
//! resulting CPU-GPU split will likely be SLOWER than pure CPU per
//! research playbook. The spike does not auto-detect this; the operator
//! must check stdout for the offload report before trusting the latency
//! numbers.
//!
//! Run with (PowerShell on Windows, AFTER enabling vulkan feature on
//! llama-cpp-2 in vault-llm/Cargo.toml):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t027b_qwen_7b_vulkan_spike
//! ```

#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, ensure, Context, Result};
use serde::Deserialize;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_llm::{
    framework_defaults_probe, CompletionParams, LlmProvider, Qwen25_14BProvider, TuningConfig,
};
use vault_retrieval::{RetrievalOptions, RetrievalQuery, Retriever, SemanticRetriever};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

// 8-query canonical gauntlet from t026 — same order.
const PRODUCTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26", "Q17", "Q19", "Q21", "Q22"];

// Warmup queries (latencies discarded). Q26 + Q19 are the longest/hardest
// pair from t027a, so they exercise the full inference pipeline.
const WARMUP_QUERY_IDS: &[&str] = &["Q26", "Q19"];

const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];
const HARD_NEGATIVE_QUERY_IDS: &[&str] = &["Q21", "Q22"];

/// Contradiction substrings for the structural assertion. Same as t026.
fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        _ => None,
    }
}

const STANDALONE_SYSTEM_PROMPT: &str = r#"You are the read layer of a personal memory vault used by AI coding agents.

You receive a query and a set of candidate memories retrieved via semantic similarity.
In ONE pass you must: (a) filter to actually-relevant candidates, (b) detect any
contradictions among the filtered set, and (c) produce a coherent synthesis the agent
can use directly as context.

Rules:
- A candidate is relevant only if its content directly addresses the query's subject.
  Topical overlap alone is NOT relevance.
- If filtered memories contradict each other (different dates/values for the same fact),
  you MUST surface each contradiction in synthesis_markdown with BOTH positions stated
  AND populate contradictions_flagged to match.
- Write a coherent narrative; cite memory IDs.
- If no candidates are relevant, set vault_has_no_relevant_content=true AND state this
  in synthesis_markdown explicitly. Do NOT fabricate.
- Keep synthesis_markdown under 250 words.
- Return ONLY valid JSON matching the schema."#;

const SYNTHESIS_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["synthesis_markdown", "contradictions_flagged", "vault_has_no_relevant_content"],
  "properties": {
    "synthesis_markdown": {"type": "string"},
    "contradictions_flagged": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["memory_ids", "positions"],
        "properties": {
          "memory_ids": {"type": "array", "items": {"type": "string"}},
          "positions": {"type": "array", "items": {"type": "string"}},
          "current_position_if_determinable": {"type": "string"}
        }
      }
    },
    "vault_has_no_relevant_content": {"type": "boolean"}
  }
}"#;

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
    shape: String,
    #[allow(dead_code)]
    length_tier: String,
    query_text: String,
    authorized_boundaries: Vec<String>,
    #[allow(dead_code)]
    expected_memory_ids: Vec<String>,
    notes: String,
}

#[derive(Debug, Clone, Deserialize)]
struct SynthesisResponse {
    synthesis_markdown: String,
    contradictions_flagged: Vec<ContradictionEntry>,
    vault_has_no_relevant_content: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct ContradictionEntry {
    memory_ids: Vec<String>,
    positions: Vec<String>,
    #[serde(default)]
    current_position_if_determinable: String,
}

#[derive(Debug, Clone)]
enum QualityVerdict {
    Pass(String),
    Fail(String),
    Observational,
}

#[derive(Debug, Clone)]
struct QueryRun {
    query: QueryEntry,
    response: Option<SynthesisResponse>,
    #[allow(dead_code)]
    raw_json: String,
    word_count: usize,
    latency: Duration,
    quality: QualityVerdict,
    parse_error: Option<String>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    let sep = "=".repeat(120);
    println!("{sep}");
    println!("T0.2.3 t027b — Qwen-7B Vulkan iGPU offload spike");
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Goal: close 14.2s gap from t12 CPU baseline (134.2s) to 120s ceiling");
    println!("Quality gate: 4/4 contradictions + 2/2 hard-negatives (t026 baseline)");
    println!("{sep}");

    let (df_threads, df_threads_batch, df_n_batch, df_n_ubatch) = framework_defaults_probe();
    println!(
        "\nllama.cpp framework defaults — n_threads={df_threads}, n_threads_batch={df_threads_batch}, n_batch={df_n_batch}, n_ubatch={df_n_ubatch}"
    );
    let logical_cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(0);
    println!(
        "Host CPU — std::thread::available_parallelism()={logical_cores} (logical threads). i7-13620H: 10 P-cores / 16 logical."
    );
    println!(
        "\n>>> Watch the next ~20 lines of llama.cpp startup output for \
         'offloaded N/M layers to GPU'. If N < M, partial offload — likely SLOWER than pure CPU."
    );

    let dir = tempfile::tempdir()?;
    let key = SqlCipherKey::new("spike-only-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key).await?;
    let metadata = Arc::new(metadata);
    let vectors_raw = LanceVectorStore::open_with_at_rest_key(
        &dir.path().join("vectors"),
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
    .await?;
    let vectors: Arc<dyn VectorStore> = Arc::new(vectors_raw);

    println!("\nOpening BgeSmallProvider...");
    let bge = open_bge_provider()?;

    println!("\nOpening Qwen2.5-7B with Vulkan iGPU offload (n_gpu_layers=99) + t12 threading...");
    let qwen_start = Instant::now();
    let qwen_path = models_dir()?.join("Qwen2.5-7B-Instruct-Q4_K_M.gguf");
    ensure!(qwen_path.exists(), "Qwen-7B GGUF missing at {qwen_path:?}");
    let tuning = TuningConfig {
        n_threads: Some(12),
        n_threads_batch: Some(12),
        n_gpu_layers: Some(99),
        ..TuningConfig::default()
    };
    let qwen = Qwen25_14BProvider::open_with_tuning(&qwen_path, tuning.clone()).await?;
    println!(
        "Qwen-7B ready in {:.1}s — {}",
        qwen_start.elapsed().as_secs_f64(),
        qwen.model_id()
    );
    println!("Tuning applied: {tuning:#?}");

    let memory_fixture_path = repo_root()?
        .join("crates")
        .join("vault-consolidator")
        .join("tests")
        .join("fixtures")
        .join("merge_acceptance_100.json");
    let memory_fixture: Vec<MemoryFixtureEntry> =
        serde_json::from_slice(&std::fs::read(&memory_fixture_path)?)?;
    println!("Loaded {} memories from fixture", memory_fixture.len());

    let query_fixture_path = vault_retrieval_root()
        .join("test-fixtures")
        .join("merge_acceptance_100_queries.json");
    let query_set: QuerySet = serde_json::from_slice(&std::fs::read(&query_fixture_path)?)?;

    let warmup_queries: Vec<QueryEntry> = WARMUP_QUERY_IDS
        .iter()
        .map(|wanted| {
            query_set
                .queries
                .iter()
                .find(|q| q.id == *wanted)
                .cloned()
                .with_context(|| format!("warmup {wanted} missing"))
        })
        .collect::<Result<Vec<_>>>()?;
    let production_queries: Vec<QueryEntry> = PRODUCTION_QUERY_IDS
        .iter()
        .map(|wanted| {
            query_set
                .queries
                .iter()
                .find(|q| q.id == *wanted)
                .cloned()
                .with_context(|| format!("target {wanted} missing"))
        })
        .collect::<Result<Vec<_>>>()?;

    println!("\nInserting 100 memories...");
    let mut fixture_id_to_memory_id: HashMap<String, MemoryId> = HashMap::new();
    let insert_start = Instant::now();
    for (i, entry) in memory_fixture.iter().enumerate() {
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
        fixture_id_to_memory_id.insert(entry.id.clone(), memory.id);
        if (i + 1) % 25 == 0 {
            println!("  inserted {}/{}", i + 1, memory_fixture.len());
        }
    }
    let insert_secs = insert_start.elapsed().as_secs_f64();
    println!("Inserted in {insert_secs:.1}s");

    let memory_id_to_fixture_id: HashMap<MemoryId, String> = fixture_id_to_memory_id
        .iter()
        .map(|(fid, mid)| (*mid, fid.clone()))
        .collect();

    let retriever = SemanticRetriever::new(metadata.clone(), bge, vectors.clone());

    // --- WARMUP ---
    println!("\n{sep}");
    println!(
        "WARMUP — {} queries (latencies discarded)",
        warmup_queries.len()
    );
    println!("{sep}");
    for (wi, q) in warmup_queries.iter().enumerate() {
        println!("[warmup {}/{}] {}", wi + 1, warmup_queries.len(), q.id);
        let _ = run_query(&qwen, &retriever, q, &memory_id_to_fixture_id).await?;
    }
    println!("Warmup complete. Beginning production run.\n");

    // --- PRODUCTION RUN ---
    println!("{sep}");
    println!(
        "PRODUCTION — {} queries (8-query t026 gauntlet)",
        production_queries.len()
    );
    println!("{sep}\n");

    let mut results: Vec<QueryRun> = Vec::with_capacity(production_queries.len());
    for (qi, q) in production_queries.iter().enumerate() {
        println!(
            "[{}/{}] {} — {} — \"{}\"",
            qi + 1,
            production_queries.len(),
            q.id,
            q.shape,
            q.query_text
        );
        let run = run_query(&qwen, &retriever, q, &memory_id_to_fixture_id).await?;

        let quality_str = match &run.quality {
            QualityVerdict::Pass(d) => format!("PASS — {d}"),
            QualityVerdict::Fail(d) => format!("FAIL — {d}"),
            QualityVerdict::Observational => "N/A (observational)".to_string(),
        };
        let contra_count = run
            .response
            .as_ref()
            .map_or(0, |r| r.contradictions_flagged.len());
        let vault_empty = run
            .response
            .as_ref()
            .is_some_and(|r| r.vault_has_no_relevant_content);
        println!(
            "   synth {}w · contradictions={} · vault_empty={} · {:.1}s",
            run.word_count,
            contra_count,
            vault_empty,
            run.latency.as_secs_f64()
        );
        println!("   quality: {quality_str}");
        results.push(run);
        println!();
    }

    // --- LATENCY SUMMARY ---
    let lats: Vec<f64> = results.iter().map(|r| r.latency.as_secs_f64()).collect();
    let stats = latency_stats(&lats);

    // --- QUALITY ROLLUP ---
    let contradiction_passes = results
        .iter()
        .filter(|r| CONTRADICTION_QUERY_IDS.contains(&r.query.id.as_str()))
        .filter(|r| matches!(r.quality, QualityVerdict::Pass(_)))
        .count();
    let hardneg_passes = results
        .iter()
        .filter(|r| HARD_NEGATIVE_QUERY_IDS.contains(&r.query.id.as_str()))
        .filter(|r| matches!(r.quality, QualityVerdict::Pass(_)))
        .count();

    println!("\n{sep}");
    println!("LATENCY SUMMARY (Qwen-7B + Vulkan iGPU, n={})", lats.len());
    println!("{sep}");
    println!(
        "min={:.1}s · p50={:.1}s · p99={:.1}s · max={:.1}s · mean={:.1}s",
        stats.min, stats.p50, stats.p99, stats.max, stats.mean
    );
    println!(
        "vs t12 CPU baseline (t027a): 134.2s mean · gap to ceiling: 120s · current p99 vs ceiling: {:.1}s",
        stats.p99
    );
    println!("\n{sep}");
    println!("QUALITY ROLLUP (t026 baseline: 4/4 + 2/2)");
    println!("{sep}");
    println!(
        "Contradictions: {}/4 (Q11, Q13, Q25, Q26)",
        contradiction_passes
    );
    println!("Hard-negatives: {}/2 (Q21, Q22)", hardneg_passes);
    let regression = contradiction_passes < 4 || hardneg_passes < 2;
    if regression {
        println!("\n** QUALITY REGRESSION vs t026 — latency win does NOT count. **");
    } else {
        println!("\nQuality preserved vs t026 baseline.");
    }

    let md_path = vault_retrieval_root()
        .join("examples")
        .join("t027b_qwen_7b_vulkan_results.md");
    let md = build_markdown_report(
        &results,
        &run_started,
        memory_fixture.len(),
        insert_secs,
        contradiction_passes,
        hardneg_passes,
    );
    std::fs::write(&md_path, md)?;
    println!("\nMarkdown writeup: {}", md_path.display());
    println!(
        "Run completed: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
}

async fn run_query(
    qwen: &Qwen25_14BProvider,
    retriever: &SemanticRetriever,
    query: &QueryEntry,
    memory_id_to_fixture_id: &HashMap<MemoryId, String>,
) -> Result<QueryRun> {
    let mut boundaries = Vec::with_capacity(query.authorized_boundaries.len());
    for b in &query.authorized_boundaries {
        boundaries.push(Boundary::new(b)?);
    }
    let rq = RetrievalQuery {
        query_text: query.query_text.clone(),
        authorized_boundaries: boundaries,
        max_results: 20,
        options: RetrievalOptions::default(),
    };
    let hits = retriever.retrieve(rq).await?;
    let candidate_fixture_ids: Vec<String> = hits
        .iter()
        .map(|h| {
            memory_id_to_fixture_id
                .get(&h.memory.id)
                .cloned()
                .unwrap_or_else(|| "<unknown>".into())
        })
        .collect();
    let mut user_prompt = format!("QUERY: {}\n\nCANDIDATES:\n", query.query_text);
    for (i, c) in hits.iter().enumerate() {
        user_prompt.push_str(&format!(
            "[{}] {}\n",
            candidate_fixture_ids[i], c.memory.content
        ));
    }
    user_prompt.push_str("\nFilter, flag contradictions, synthesize. Return JSON.");

    let params = CompletionParams {
        max_tokens: 1024,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(STANDALONE_SYSTEM_PROMPT.to_string()),
    };
    let start = Instant::now();
    let raw = qwen
        .complete_json(&user_prompt, SYNTHESIS_SCHEMA, &params)
        .await?;
    let latency = start.elapsed();

    let run = match serde_json::from_str::<SynthesisResponse>(&raw) {
        Ok(parsed) => {
            let word_count = parsed.synthesis_markdown.split_whitespace().count();
            let quality = assess_quality(&query.id, &parsed)?;
            QueryRun {
                query: query.clone(),
                response: Some(parsed),
                raw_json: raw.clone(),
                word_count,
                latency,
                quality,
                parse_error: None,
            }
        }
        Err(e) => QueryRun {
            query: query.clone(),
            response: None,
            raw_json: raw,
            word_count: 0,
            latency,
            quality: QualityVerdict::Fail(format!("parse error: {e}")),
            parse_error: Some(format!("{e}")),
        },
    };
    Ok(run)
}

fn assess_quality(query_id: &str, parsed: &SynthesisResponse) -> Result<QualityVerdict> {
    if CONTRADICTION_QUERY_IDS.contains(&query_id) {
        let (sub_a, sub_b) = structural_substrings(query_id)
            .ok_or_else(|| anyhow!("missing substrings for {query_id}"))?;
        let contains_a = parsed.synthesis_markdown.contains(sub_a);
        let contains_b = parsed.synthesis_markdown.contains(sub_b);
        let flagged_nonempty = !parsed.contradictions_flagged.is_empty();
        let pass = flagged_nonempty && contains_a && contains_b;
        let detail = format!(
            "contradictions_flagged.len()={} · '{sub_a}'={contains_a} AND '{sub_b}'={contains_b}",
            parsed.contradictions_flagged.len()
        );
        Ok(if pass {
            QualityVerdict::Pass(detail)
        } else {
            QualityVerdict::Fail(detail)
        })
    } else if HARD_NEGATIVE_QUERY_IDS.contains(&query_id) {
        let pass = parsed.vault_has_no_relevant_content;
        let detail = format!("vault_has_no_relevant_content={pass}");
        Ok(if pass {
            QualityVerdict::Pass(detail)
        } else {
            QualityVerdict::Fail(detail)
        })
    } else {
        Ok(QualityVerdict::Observational)
    }
}

struct LatencyStats {
    min: f64,
    p50: f64,
    p99: f64,
    max: f64,
    mean: f64,
}

fn latency_stats(samples: &[f64]) -> LatencyStats {
    if samples.is_empty() {
        return LatencyStats {
            min: 0.0,
            p50: 0.0,
            p99: 0.0,
            max: 0.0,
            mean: 0.0,
        };
    }
    let mut sorted: Vec<f64> = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let pct = |p: f64| -> f64 {
        let idx = ((p * (sorted.len() as f64 - 1.0)).round() as usize).min(sorted.len() - 1);
        sorted[idx]
    };
    let sum: f64 = sorted.iter().sum();
    LatencyStats {
        min: sorted[0],
        p50: pct(0.50),
        p99: pct(0.99),
        max: sorted[sorted.len() - 1],
        mean: sum / sorted.len() as f64,
    }
}

fn build_markdown_report(
    results: &[QueryRun],
    run_started: &chrono::DateTime<chrono::Utc>,
    n_memories: usize,
    insert_secs: f64,
    contradiction_passes: usize,
    hardneg_passes: usize,
) -> String {
    let mut s = String::new();
    s.push_str("# T0.2.3 t027b — Qwen-7B + Vulkan iGPU Offload Spike Results\n\n");
    s.push_str(&format!(
        "**Run started:** {}  \n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    s.push_str("**Model:** Qwen2.5-7B-Instruct Q4_K_M  \n");
    s.push_str("**Hardware:** i7-13620H (10P / 16T, AVX2) + Intel UHD Graphics iGPU (Vulkan)  \n");
    s.push_str("**CPU reference (t027a t12):** 134.2s mean (Q19+Q26)  \n");
    s.push_str("**Hard ceiling:** 120s per query  \n");
    s.push_str("**Quality gate:** t026 baseline — 4/4 contradictions + 2/2 hard-negatives  \n");
    s.push_str("**Tuning:** `n_gpu_layers=99, n_threads=12, n_threads_batch=12, KV K/V = f16`  \n");
    s.push_str(&format!(
        "**Fixture:** {n_memories} memories, {insert_secs:.1}s insertion\n\n"
    ));

    let lats: Vec<f64> = results.iter().map(|r| r.latency.as_secs_f64()).collect();
    let stats = latency_stats(&lats);
    s.push_str("## Latency summary\n\n");
    s.push_str("| min | p50 | p99 | max | mean |\n|---|---|---|---|---|\n");
    s.push_str(&format!(
        "| {:.1}s | {:.1}s | {:.1}s | {:.1}s | {:.1}s |\n\n",
        stats.min, stats.p50, stats.p99, stats.max, stats.mean
    ));
    s.push_str(&format!(
        "**Mean vs t12 CPU baseline (134.2s):** {:+.1}s ({:+.1}%)  \n",
        stats.mean - 134.2,
        (stats.mean - 134.2) / 134.2 * 100.0
    ));
    s.push_str(&format!(
        "**p99 vs 120s hard ceiling:** {:+.1}s ({})\n\n",
        stats.p99 - 120.0,
        if stats.p99 < 120.0 {
            "**WITHIN budget**"
        } else {
            "**OVER budget**"
        }
    ));

    s.push_str("## Quality rollup (t026 baseline: 4/4 + 2/2)\n\n");
    s.push_str(&format!(
        "- Contradictions: **{contradiction_passes}/4** (Q11, Q13, Q25, Q26)\n"
    ));
    s.push_str(&format!(
        "- Hard-negatives: **{hardneg_passes}/2** (Q21, Q22)\n"
    ));
    let regression = contradiction_passes < 4 || hardneg_passes < 2;
    s.push_str(if regression {
        "- **VERDICT: REGRESSION vs t026 — latency win does NOT count.**\n\n"
    } else {
        "- **VERDICT: quality preserved vs t026 baseline.**\n\n"
    });

    s.push_str("## Per-query detail\n\n");
    s.push_str("| Query | Shape | Quality | Contradictions | Vault empty | Latency |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    for r in results {
        let q = match &r.quality {
            QualityVerdict::Pass(_) => "**PASS**",
            QualityVerdict::Fail(_) => "**FAIL**",
            QualityVerdict::Observational => "—",
        };
        let contra = r
            .response
            .as_ref()
            .map_or(0, |x| x.contradictions_flagged.len());
        let empty = r
            .response
            .as_ref()
            .is_some_and(|x| x.vault_has_no_relevant_content);
        s.push_str(&format!(
            "| {} | {} | {q} | {contra} | {empty} | {:.1}s |\n",
            r.query.id,
            r.query.shape,
            r.latency.as_secs_f64()
        ));
    }
    s.push('\n');

    s.push_str("## Per-query synthesis\n\n");
    for r in results {
        s.push_str(&format!(
            "### {} — \"{}\"\n\n",
            r.query.id, r.query.query_text
        ));
        s.push_str(&format!(
            "**Shape:** {} · **Notes:** {}\n\n",
            r.query.shape, r.query.notes
        ));
        if let Some(err) = &r.parse_error {
            s.push_str(&format!("**PARSE_FAILURE:** {err}\n\n"));
            continue;
        }
        let resp = r.response.as_ref().unwrap();
        let quality = match &r.quality {
            QualityVerdict::Pass(d) => format!("PASS — {d}"),
            QualityVerdict::Fail(d) => format!("FAIL — {d}"),
            QualityVerdict::Observational => "N/A (observational)".to_string(),
        };
        s.push_str(&format!(
            "- word_count: {} · latency: {:.1}s · contradictions_flagged: {} · vault_has_no_relevant_content: {}\n",
            r.word_count,
            r.latency.as_secs_f64(),
            resp.contradictions_flagged.len(),
            resp.vault_has_no_relevant_content
        ));
        s.push_str(&format!("- **quality: {quality}**\n\n"));
        s.push_str("  synthesis_markdown:\n\n  ```\n  ");
        s.push_str(&resp.synthesis_markdown.replace('\n', "\n  "));
        s.push_str("\n  ```\n\n");
        s.push_str("---\n\n");
    }

    s.push_str("## Decision\n\nData only — partner reviews before promoting.\n");
    s
}

fn open_bge_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let fixture_root = vault_embedding_test_fixtures()?;
    let model = fixture_root.join("model.onnx");
    let tokenizer = fixture_root.join("tokenizer.json");
    let ort_lib = fixture_root.join(ort_lib_name());
    for p in [&model, &tokenizer, &ort_lib] {
        ensure!(p.exists(), "missing BGE fixture {p:?}");
    }
    let provider = BgeSmallProvider::open(&model, &tokenizer, &ort_lib)?;
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
        .context("no grandparent")
}
fn vault_embedding_test_fixtures() -> Result<PathBuf> {
    let p = repo_root()?
        .join("crates")
        .join("vault-embedding")
        .join("test-fixtures")
        .join("bge-small-en-v1.5");
    ensure!(p.exists(), "bge fixtures missing");
    Ok(p)
}
fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}
