//! T0.2.3 t027a extension — n_threads = 14 and 16 sanity check.
//!
//! **Question this spike answers:** does the diminishing-returns curve from
//! t027a (4→8 saved 52s, 8→10 saved 19s, 10→12 saved 5s) flatten further
//! at 14/16 threads, or does hyperthreading contention reverse the gain?
//!
//! **Hypothesis:** on i7-13620H (10 P-cores / 16 logical), going past the
//! physical-core count typically regresses by 5-15% for AVX2 LLM inference
//! due to SIMD-unit contention between hyperthreaded siblings. We expect
//! t14 ≈ t12 (within noise) and t16 ≥ t12 (slightly slower).
//!
//! **KV cache: stays default f16.** t027a proved Q8_0 hurt latency 34% on
//! this AVX2-without-VNNI CPU. Locked out of the menu.
//!
//! Same 2 queries as t027a (Q19 multi-cluster narrative + Q26 oblique
//! Comcast contradiction). Same quality assertion on Q26.
//!
//! Run with (PowerShell on Windows):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t027a_ext_t14_t16_spike
//! ```

#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_precision_loss)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_llm::{
    framework_defaults_probe, CompletionParams, LlmProvider, Qwen25_14BProvider, TuningConfig,
};
use vault_retrieval::{RetrievalOptions, RetrievalQuery, Retriever, SemanticRetriever};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

const TARGET_QUERY_IDS: &[&str] = &["Q19", "Q26"];

const QUALITY_QUERY_ID: &str = "Q26";
const QUALITY_SUBSTRINGS: [&str; 2] = ["89", "109"];

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
struct ConfigVariant {
    label: String,
    description: String,
    tuning: TuningConfig,
}

#[derive(Debug, Clone)]
struct QueryRun {
    query_id: String,
    response: Option<SynthesisResponse>,
    #[allow(dead_code)]
    raw_json: String,
    latency: Duration,
    quality_pass: Option<bool>,
    quality_detail: Option<String>,
    parse_error: Option<String>,
}

#[derive(Debug, Clone)]
struct ConfigResult {
    variant: ConfigVariant,
    runs: Vec<QueryRun>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    let sep = "=".repeat(120);
    println!("{sep}");
    println!("T0.2.3 t027a extension — n_threads = 14 + 16 sanity check");
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Reference data from t027a:");
    println!("  t10 -> 139.6s mean, t12 -> 134.2s mean (winner so far)");
    println!("  Hypothesis: t14 ~= t12, t16 >= t12 (HT contention regresses)");
    println!("{sep}");

    let (df_threads, df_threads_batch, df_n_batch, df_n_ubatch) = framework_defaults_probe();
    println!(
        "\nllama.cpp framework defaults — n_threads={df_threads}, n_threads_batch={df_threads_batch}, n_batch={df_n_batch}, n_ubatch={df_n_ubatch}"
    );
    let logical_cores = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(0);
    println!(
        "Host CPU — std::thread::available_parallelism()={logical_cores} (logical threads). i7-13620H has 10 P-cores / 16 logical."
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

    println!("Opening Qwen2.5-7B...");
    let qwen_start = Instant::now();
    let qwen_path = models_dir()?.join("Qwen2.5-7B-Instruct-Q4_K_M.gguf");
    ensure!(qwen_path.exists(), "Qwen-7B GGUF missing at {qwen_path:?}");
    let qwen = Qwen25_14BProvider::open(&qwen_path).await?;
    println!(
        "Qwen-7B ready in {:.1}s — {}",
        qwen_start.elapsed().as_secs_f64(),
        qwen.model_id()
    );

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
    let target_queries: Vec<QueryEntry> = TARGET_QUERY_IDS
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
        fixture_id_to_memory_id.insert(entry.id.clone(), memory.id);
    }
    let insert_secs = insert_start.elapsed().as_secs_f64();
    println!("Inserted in {insert_secs:.1}s");

    let memory_id_to_fixture_id: HashMap<MemoryId, String> = fixture_id_to_memory_id
        .iter()
        .map(|(fid, mid)| (*mid, fid.clone()))
        .collect();

    let retriever = SemanticRetriever::new(metadata.clone(), bge, vectors.clone());

    println!(
        "\nPre-retrieving candidates for {} queries...",
        target_queries.len()
    );
    let mut prepared_prompts: Vec<(QueryEntry, String)> = Vec::new();
    for query in &target_queries {
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
        println!(
            "  {} — {} candidates, prompt ~{} chars",
            query.id,
            hits.len(),
            user_prompt.len()
        );
        prepared_prompts.push((query.clone(), user_prompt));
    }

    let configs = [
        ConfigVariant {
            label: "t14".into(),
            description: "n_threads=14, n_threads_batch=14 (above P-core count, into HT)".into(),
            tuning: TuningConfig {
                n_threads: Some(14),
                n_threads_batch: Some(14),
                ..TuningConfig::default()
            },
        },
        ConfigVariant {
            label: "t16".into(),
            description: "n_threads=16 (= full logical-thread count)".into(),
            tuning: TuningConfig {
                n_threads: Some(16),
                n_threads_batch: Some(16),
                ..TuningConfig::default()
            },
        },
    ];

    println!("\n{sep}");
    println!(
        "Running {} configs x {} queries",
        configs.len(),
        target_queries.len()
    );
    println!("{sep}");

    let mut results: Vec<ConfigResult> = Vec::new();
    for (ci, config) in configs.iter().enumerate() {
        println!(
            "\n[{}/{}] CONFIG: {} — {}",
            ci + 1,
            configs.len(),
            config.label,
            config.description
        );
        let runs = run_one_config(&qwen, config, &prepared_prompts).await?;
        for r in &runs {
            let quality_str = r.quality_pass.map_or("N/A".to_string(), |p| {
                if p {
                    "PASS".into()
                } else {
                    "FAIL".into()
                }
            });
            println!(
                "   {} — {:.1}s · quality={} · contradictions={}",
                r.query_id,
                r.latency.as_secs_f64(),
                quality_str,
                r.response
                    .as_ref()
                    .map_or(0, |x| x.contradictions_flagged.len())
            );
        }
        results.push(ConfigResult {
            variant: config.clone(),
            runs,
        });
    }

    println!("\n{sep}");
    println!("SUMMARY (with reference data from t027a in parens)");
    println!("{sep}");
    println!(
        "{:<10} {:>10} {:>10} {:>10} {:>10}",
        "config", "mean(s)", "Q19(s)", "Q26(s)", "Q26 qual"
    );
    println!(
        "{:<10} {:>10} {:>10} {:>10} {:>10}",
        "t10 (ref)", "139.6", "142.0", "137.2", "PASS"
    );
    println!(
        "{:<10} {:>10} {:>10} {:>10} {:>10}",
        "t12 (ref)", "134.2", "132.4", "136.0", "PASS"
    );
    for r in &results {
        let lats: Vec<f64> = r.runs.iter().map(|x| x.latency.as_secs_f64()).collect();
        let mean = lats.iter().sum::<f64>() / lats.len() as f64;
        let q19 = r
            .runs
            .iter()
            .find(|x| x.query_id == "Q19")
            .map_or(0.0, |x| x.latency.as_secs_f64());
        let q26 = r
            .runs
            .iter()
            .find(|x| x.query_id == "Q26")
            .map_or(0.0, |x| x.latency.as_secs_f64());
        let q26_qual = r
            .runs
            .iter()
            .find(|x| x.query_id == "Q26")
            .and_then(|x| x.quality_pass)
            .map_or("N/A".to_string(), |p| {
                if p {
                    "PASS".into()
                } else {
                    "FAIL".into()
                }
            });
        println!(
            "{:<10} {:>10.1} {:>10.1} {:>10.1} {:>10}",
            r.variant.label, mean, q19, q26, q26_qual
        );
    }

    let md_path = vault_retrieval_root()
        .join("examples")
        .join("t027a_ext_t14_t16_results.md");
    let md = build_markdown_report(&results, &run_started, memory_fixture.len(), insert_secs);
    std::fs::write(&md_path, md)?;
    println!("\nMarkdown writeup: {}", md_path.display());
    println!(
        "Run completed: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
}

async fn run_one_config(
    qwen: &Qwen25_14BProvider,
    config: &ConfigVariant,
    prepared_prompts: &[(QueryEntry, String)],
) -> Result<Vec<QueryRun>> {
    let mut runs = Vec::with_capacity(prepared_prompts.len());
    for (query, user_prompt) in prepared_prompts {
        let params = CompletionParams {
            max_tokens: 1024,
            temperature: 0.0,
            top_p: 1.0,
            seed: Some(42),
            system_prompt: Some(STANDALONE_SYSTEM_PROMPT.to_string()),
        };
        let start = Instant::now();
        let raw = qwen
            .complete_json_with_tuning(
                user_prompt,
                SYNTHESIS_SCHEMA,
                &params,
                config.tuning.clone(),
            )
            .await?;
        let latency = start.elapsed();

        let run = match serde_json::from_str::<SynthesisResponse>(&raw) {
            Ok(parsed) => {
                let (quality_pass, quality_detail) = if query.id == QUALITY_QUERY_ID {
                    let contains_a = parsed.synthesis_markdown.contains(QUALITY_SUBSTRINGS[0]);
                    let contains_b = parsed.synthesis_markdown.contains(QUALITY_SUBSTRINGS[1]);
                    let flagged_nonempty = !parsed.contradictions_flagged.is_empty();
                    let pass = flagged_nonempty && contains_a && contains_b;
                    let detail = format!(
                        "contradictions_flagged.len()={} · '{}'={} AND '{}'={}",
                        parsed.contradictions_flagged.len(),
                        QUALITY_SUBSTRINGS[0],
                        contains_a,
                        QUALITY_SUBSTRINGS[1],
                        contains_b
                    );
                    (Some(pass), Some(detail))
                } else {
                    (None, None)
                };
                QueryRun {
                    query_id: query.id.clone(),
                    response: Some(parsed),
                    raw_json: raw.clone(),
                    latency,
                    quality_pass,
                    quality_detail,
                    parse_error: None,
                }
            }
            Err(e) => QueryRun {
                query_id: query.id.clone(),
                response: None,
                raw_json: raw,
                latency,
                quality_pass: None,
                quality_detail: None,
                parse_error: Some(format!("{e}")),
            },
        };
        runs.push(run);
    }
    Ok(runs)
}

fn build_markdown_report(
    results: &[ConfigResult],
    run_started: &chrono::DateTime<chrono::Utc>,
    n_memories: usize,
    insert_secs: f64,
) -> String {
    let mut s = String::new();
    s.push_str("# T0.2.3 t027a extension — n_threads = 14 + 16 sanity check\n\n");
    s.push_str(&format!(
        "**Run started:** {}  \n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    s.push_str("**Model:** Qwen2.5-7B-Instruct Q4_K_M  \n");
    s.push_str("**Hardware:** i7-13620H (10P / 16T, AVX2)  \n");
    s.push_str("**Reference (t027a):** t10 = 139.6s mean, t12 = 134.2s mean (winner so far)  \n");
    s.push_str("**Hard ceiling:** 120s per query  \n");
    s.push_str(&format!(
        "**Fixture:** {n_memories} memories, {insert_secs:.1}s insertion\n\n"
    ));

    s.push_str("## Summary table (with t027a reference)\n\n");
    s.push_str("| Config | Description | Q19 (s) | Q26 (s) | Mean (s) | Q26 quality |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    s.push_str("| t10 (ref) | n_threads=10 (from t027a) | 142.0 | 137.2 | 139.6 | **PASS** |\n");
    s.push_str("| t12 (ref) | n_threads=12 (from t027a) | 132.4 | 136.0 | 134.2 | **PASS** |\n");
    for r in results {
        let lats: Vec<f64> = r.runs.iter().map(|x| x.latency.as_secs_f64()).collect();
        let mean = lats.iter().sum::<f64>() / lats.len() as f64;
        let q19 = r
            .runs
            .iter()
            .find(|x| x.query_id == "Q19")
            .map_or(0.0, |x| x.latency.as_secs_f64());
        let q26 = r
            .runs
            .iter()
            .find(|x| x.query_id == "Q26")
            .map_or(0.0, |x| x.latency.as_secs_f64());
        let q26_qual = r
            .runs
            .iter()
            .find(|x| x.query_id == "Q26")
            .and_then(|x| x.quality_pass)
            .map_or("N/A".to_string(), |p| {
                if p {
                    "**PASS**".into()
                } else {
                    "**FAIL**".into()
                }
            });
        s.push_str(&format!(
            "| {} | {} | {q19:.1} | {q26:.1} | {mean:.1} | {q26_qual} |\n",
            r.variant.label, r.variant.description
        ));
    }
    s.push('\n');

    s.push_str("## Per-config detail\n\n");
    for r in results {
        s.push_str(&format!(
            "### {} — {}\n\n",
            r.variant.label, r.variant.description
        ));
        for run in &r.runs {
            s.push_str(&format!("#### {}\n\n", run.query_id));
            s.push_str(&format!("- latency: {:.1}s\n", run.latency.as_secs_f64()));
            if let Some(err) = &run.parse_error {
                s.push_str(&format!("- **PARSE_FAILURE:** {err}\n\n"));
                continue;
            }
            let resp = run.response.as_ref().unwrap();
            s.push_str(&format!(
                "- contradictions_flagged: {} · vault_has_no_relevant_content: {}\n",
                resp.contradictions_flagged.len(),
                resp.vault_has_no_relevant_content
            ));
            if let (Some(p), Some(d)) = (run.quality_pass, &run.quality_detail) {
                s.push_str(&format!(
                    "- **Q26 quality assert: {} — {d}**\n",
                    if p { "PASS" } else { "FAIL" }
                ));
            }
            s.push_str("\n  synthesis_markdown:\n\n  ```\n  ");
            s.push_str(&resp.synthesis_markdown.replace('\n', "\n  "));
            s.push_str("\n  ```\n\n");
        }
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
