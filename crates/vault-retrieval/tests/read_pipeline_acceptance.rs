//! T0.2.3 close — production acceptance test for the read-time pipeline
//! (ADR-048). Runs the t026 canonical 8-query gauntlet against the real
//! Qwen2.5-7B-Instruct model with the locked V0.2 `TuningConfig` and
//! asserts the production quality contract: **4/4 contradictions surfaced
//! + 2/2 hard-negatives correctly rejected**.
//!
//! # Gating
//!
//! Cron-gated `#[ignore]` because the test loads a 4.36 GB GGUF, runs ~10
//! Qwen-7B inferences (~10-15 min on the empirically-anchored hardware),
//! and depends on the model GGUF being present at
//! `$APPDATA\com.shahbaz242630.memory-vault\models\Qwen2.5-7B-Instruct-Q4_K_M.gguf`.
//! Mirrors the gating pattern of
//! `crates/vault-llm/tests/phi4_mini_smoke.rs::phi4_mini_smoke_test`.
//!
//! Additionally `#[cfg(target_os = "windows")]` for now — the Vulkan SDK +
//! Qwen GGUF path are Windows-only in CI today. The Linux/Vulkan and
//! macOS/Metal legs need a t027c spike to unlock (ADR-042 Amendment 1).
//!
//! # Quality assertion shape
//!
//! Same structural shape as the t027b spike (`examples/t027b_qwen_7b_vulkan_spike.rs`),
//! but invoked through the production [`vault_retrieval::ReadPipeline`]
//! rather than ad-hoc retrieval+`complete_json` calls. The pipeline is the
//! contract; this test is what proves the contract.

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, ensure, Context, Result};
use serde::Deserialize;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_llm::{LlmProvider, Qwen25_14BProvider, TuningConfig};
use vault_retrieval::{
    ReadPipeline, ReadQuery, ReadResponse, SemanticRetriever, DEFAULT_MAX_CANDIDATES,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

const PRODUCTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26", "Q17", "Q19", "Q21", "Q22"];
const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];
const HARD_NEGATIVE_QUERY_IDS: &[&str] = &["Q21", "Q22"];

fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        _ => None,
    }
}

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

/// **The production acceptance test.** Runs the locked read-time pipeline
/// (ADR-048) against the t026 8-query gauntlet and asserts the quality
/// contract: 4/4 contradictions + 2/2 hard-negatives. Latency is logged
/// as observability (NOT asserted — CI runners may lack the iGPU that
/// empirically anchored t027b's 86s mean; quality is the canonical gate
/// in CI).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "heavy: ~10-15 min Qwen-7B inference + 4.36 GB GGUF + Vulkan iGPU; cron-only"]
async fn read_pipeline_acceptance_8_query_gauntlet() -> Result<()> {
    let run_started = chrono::Utc::now();
    println!(
        "T0.2.3 production acceptance — read-time pipeline (ADR-048) — started {}",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    );

    let dir = tempfile::tempdir()?;
    let key = SqlCipherKey::new("acceptance-test-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key).await?;
    let metadata = Arc::new(metadata);
    let vectors_raw = LanceVectorStore::open_with_at_rest_key(
        &dir.path().join("vectors"),
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
    .await?;
    let vectors: Arc<dyn VectorStore> = Arc::new(vectors_raw);

    println!("Opening BgeSmallProvider...");
    let bge = open_bge_provider()?;

    println!("Opening Qwen2.5-7B-Instruct with locked V0.2 TuningConfig...");
    let qwen_path = models_dir()?.join("Qwen2.5-7B-Instruct-Q4_K_M.gguf");
    ensure!(qwen_path.exists(), "Qwen-7B GGUF missing at {qwen_path:?}");
    let tuning = TuningConfig {
        n_threads: Some(12),
        n_threads_batch: Some(12),
        n_gpu_layers: Some(99),
        ..TuningConfig::default()
    };
    let qwen_load = Instant::now();
    let qwen = Qwen25_14BProvider::open_with_tuning(&qwen_path, tuning.clone()).await?;
    println!(
        "Qwen-7B ready in {:.1}s (id={}, tuning={:?})",
        qwen_load.elapsed().as_secs_f64(),
        qwen.model_id(),
        tuning
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
    let production_queries: Vec<QueryEntry> = PRODUCTION_QUERY_IDS
        .iter()
        .map(|wanted| {
            query_set
                .queries
                .iter()
                .find(|q| q.id == *wanted)
                .cloned()
                .with_context(|| format!("target {wanted} missing from query fixture"))
        })
        .collect::<Result<Vec<_>>>()?;

    println!("Inserting 100 memories...");
    let mut fixture_id_to_memory_id: HashMap<String, MemoryId> = HashMap::new();
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
    println!("Fixture inserted.");

    let retriever = Arc::new(SemanticRetriever::new(
        metadata.clone(),
        bge,
        vectors.clone(),
    ));
    let llm: Arc<dyn vault_llm::LlmProvider> = Arc::new(qwen);
    let pipeline = ReadPipeline::new(retriever, llm);
    println!(
        "ReadPipeline ready (max_candidates={}, system prompt = production default)",
        DEFAULT_MAX_CANDIDATES
    );

    // ---- 2-query warmup (Q26 + Q19), latencies discarded ----
    println!("\n--- Warmup (Q26 + Q19) ---");
    for warmup_id in ["Q26", "Q19"] {
        let q = production_queries
            .iter()
            .find(|qq| qq.id == warmup_id)
            .expect("warmup query in production set");
        let mut boundaries = Vec::with_capacity(q.authorized_boundaries.len());
        for b in &q.authorized_boundaries {
            boundaries.push(Boundary::new(b)?);
        }
        let rq = ReadQuery {
            query_text: q.query_text.clone(),
            authorized_boundaries: boundaries,
        };
        let start = Instant::now();
        let _ = pipeline.read(rq).await?;
        println!(
            "  warmup {warmup_id}: {:.1}s (discarded)",
            start.elapsed().as_secs_f64()
        );
    }

    // ---- Production 8-query gauntlet ----
    println!("\n--- Production run (8-query gauntlet) ---");
    let mut contradiction_passes = 0_usize;
    let mut hard_negative_passes = 0_usize;
    let mut latencies = Vec::with_capacity(production_queries.len());

    for (qi, query) in production_queries.iter().enumerate() {
        let mut boundaries = Vec::with_capacity(query.authorized_boundaries.len());
        for b in &query.authorized_boundaries {
            boundaries.push(Boundary::new(b)?);
        }
        let rq = ReadQuery {
            query_text: query.query_text.clone(),
            authorized_boundaries: boundaries,
        };
        let start = Instant::now();
        let resp: ReadResponse = pipeline.read(rq).await?;
        let latency = start.elapsed();
        latencies.push(latency.as_secs_f64());

        let verdict = assess_query(&query.id, &resp);
        match verdict {
            QualityVerdict::ContradictionPass(detail) => {
                contradiction_passes += 1;
                println!(
                    "[{}/{}] {} contradiction PASS — {:.1}s — {detail}",
                    qi + 1,
                    production_queries.len(),
                    query.id,
                    latency.as_secs_f64()
                );
            }
            QualityVerdict::ContradictionFail(detail) => {
                println!(
                    "[{}/{}] {} contradiction FAIL — {:.1}s — {detail}",
                    qi + 1,
                    production_queries.len(),
                    query.id,
                    latency.as_secs_f64()
                );
            }
            QualityVerdict::HardNegativePass(detail) => {
                hard_negative_passes += 1;
                println!(
                    "[{}/{}] {} hard-negative PASS — {:.1}s — {detail}",
                    qi + 1,
                    production_queries.len(),
                    query.id,
                    latency.as_secs_f64()
                );
            }
            QualityVerdict::HardNegativeFail(detail) => {
                println!(
                    "[{}/{}] {} hard-negative FAIL — {:.1}s — {detail}",
                    qi + 1,
                    production_queries.len(),
                    query.id,
                    latency.as_secs_f64()
                );
            }
            QualityVerdict::Observational => {
                let contra_count = resp.contradictions_flagged.len();
                println!(
                    "[{}/{}] {} observational — {:.1}s — contradictions_flagged={contra_count}",
                    qi + 1,
                    production_queries.len(),
                    query.id,
                    latency.as_secs_f64()
                );
            }
        }
    }

    // ---- Observability: latency stats ----
    let mean = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let max = latencies.iter().cloned().fold(0.0_f64, f64::max);
    let min = latencies.iter().cloned().fold(f64::INFINITY, f64::min);
    println!(
        "\nLatency observability: min={min:.1}s · mean={mean:.1}s · max={max:.1}s (NOT asserted; quality is the canonical gate)"
    );

    // ---- Quality assertions ----
    println!(
        "\nQuality rollup: contradictions {}/4, hard-negatives {}/2",
        contradiction_passes, hard_negative_passes
    );
    assert_eq!(
        contradiction_passes, 4,
        "T0.2.3 close production quality contract (ADR-048): 4/4 contradictions required, got {contradiction_passes}/4"
    );
    assert_eq!(
        hard_negative_passes, 2,
        "T0.2.3 close production quality contract (ADR-048): 2/2 hard-negatives required, got {hard_negative_passes}/2"
    );
    println!("Quality contract preserved vs t026 baseline ✓");
    Ok(())
}

#[derive(Debug)]
enum QualityVerdict {
    ContradictionPass(String),
    ContradictionFail(String),
    HardNegativePass(String),
    HardNegativeFail(String),
    Observational,
}

fn assess_query(query_id: &str, resp: &ReadResponse) -> QualityVerdict {
    if CONTRADICTION_QUERY_IDS.contains(&query_id) {
        let Some((sub_a, sub_b)) = structural_substrings(query_id) else {
            return QualityVerdict::Observational;
        };
        let contains_a = resp.synthesis_markdown.contains(sub_a);
        let contains_b = resp.synthesis_markdown.contains(sub_b);
        let flagged_nonempty = !resp.contradictions_flagged.is_empty();
        let detail = format!(
            "contradictions_flagged.len()={} · '{sub_a}'={contains_a} AND '{sub_b}'={contains_b}",
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
        QualityVerdict::Observational
    }
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
fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}
