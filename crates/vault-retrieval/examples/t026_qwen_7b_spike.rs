//! T0.2.3 read-time architecture spike — Qwen2.5-7B-Instruct standalone.
//!
//! **Question this spike answers:**
//!
//! Following t025 which established Qwen2.5-14B-Instruct delivers acceptable
//! quality on contradiction surfacing (3 of 4 PASS) and hard-negative "I don't
//! know" behavior (2 of 2) but at unshippable CPU latency (4.5-11 min/query —
//! 3-5× over the partner-locked 2-min ceiling), this spike measures whether the
//! smaller Qwen2.5-7B sibling — same family, same chat template, ~50% the
//! parameter count — retains enough quality at materially faster latency.
//!
//! If Qwen-7B holds quality at acceptable latency: free local tier ships.
//! If Qwen-7B drops quality below acceptable: free local tier requires cloud
//! API access (paid-only V0.2) or remains broken until GPU support lands.
//!
//! **Pipeline:** identical to t025 Pipeline B (Qwen standalone, single call).
//! - BGE semantic retrieval top-20 (existing SemanticRetriever)
//! - Single Qwen2.5-7B call: filter + flag contradictions + synthesize
//!
//! Reuses `Qwen25_14BProvider` (misleading name — actually works for any
//! Qwen2.5-family GGUF since the chat template is shared). The "_14B" in the
//! struct name is a label, not a size constraint.
//!
//! **Same 8 query candidate sets as t024 + t025:**
//! - Q11, Q13: lexical-direct contradiction pairs
//! - Q25, Q26: oblique-phrased contradiction pairs
//! - Q17, Q19: multi-cluster narrative
//! - Q21, Q22: hard-negatives
//!
//! Run with (PowerShell on Windows):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t026_qwen_7b_spike
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
use vault_llm::{CompletionParams, LlmProvider, Qwen25_14BProvider};
use vault_retrieval::{RetrievalOptions, RetrievalQuery, Retriever, SemanticRetriever};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

const TARGET_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26", "Q17", "Q19", "Q21", "Q22"];
const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];

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
struct QueryResult {
    query: QueryEntry,
    response: Option<SynthesisResponse>,
    raw_json: String,
    word_count: usize,
    latency: Duration,
    structural_assertion_passed: Option<bool>,
    structural_detail: Option<String>,
    parse_error: Option<String>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    let sep = "=".repeat(120);
    println!("{sep}");
    println!(
        "T0.2.3 read-time spike — Qwen2.5-7B-Instruct standalone (smaller model viability test)"
    );
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("{sep}");

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

    println!("Opening Qwen2.5-7B (Qwen25_14BProvider with 7B path)...");
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
        if (i + 1) % 20 == 0 {
            println!("  inserted {}/{}", i + 1, memory_fixture.len());
        }
    }
    let insert_secs = insert_start.elapsed().as_secs_f64();
    println!("Inserted in {:.1}s", insert_secs);

    let memory_id_to_fixture_id: HashMap<MemoryId, String> = fixture_id_to_memory_id
        .iter()
        .map(|(fid, mid)| (*mid, fid.clone()))
        .collect();

    let retriever = SemanticRetriever::new(metadata.clone(), bge, vectors.clone());

    println!("\n{sep}");
    println!("Running 8 queries × Qwen2.5-7B standalone...");
    println!("{sep}\n");

    let mut results: Vec<QueryResult> = Vec::with_capacity(target_queries.len());
    for (qi, query) in target_queries.iter().enumerate() {
        println!(
            "[{}/{}] {} — {} — \"{}\"",
            qi + 1,
            target_queries.len(),
            query.id,
            query.shape,
            query.query_text
        );

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
        let retrieval_start = Instant::now();
        let hits = retriever.retrieve(rq).await?;
        let retrieval_latency = retrieval_start.elapsed();

        let candidate_fixture_ids: Vec<String> = hits
            .iter()
            .map(|h| {
                memory_id_to_fixture_id
                    .get(&h.memory.id)
                    .cloned()
                    .unwrap_or_else(|| "<unknown>".into())
            })
            .collect();
        println!(
            "   retrieved {} candidates in {:.0}ms",
            hits.len(),
            retrieval_latency.as_secs_f64() * 1000.0
        );

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

        let qr = match serde_json::from_str::<SynthesisResponse>(&raw) {
            Ok(parsed) => {
                let word_count = parsed.synthesis_markdown.split_whitespace().count();
                let (structural_passed, structural_detail) =
                    if CONTRADICTION_QUERY_IDS.contains(&query.id.as_str()) {
                        let (sub_a, sub_b) = structural_substrings(&query.id)
                            .ok_or_else(|| anyhow!("missing substrings for {}", query.id))?;
                        let contains_a = parsed.synthesis_markdown.contains(sub_a);
                        let contains_b = parsed.synthesis_markdown.contains(sub_b);
                        let flagged_nonempty = !parsed.contradictions_flagged.is_empty();
                        let pass = flagged_nonempty && contains_a && contains_b;
                        let detail = format!(
                            "contradictions_flagged.len()={} · contains '{}'={} AND '{}'={}",
                            parsed.contradictions_flagged.len(),
                            sub_a,
                            contains_a,
                            sub_b,
                            contains_b
                        );
                        (Some(pass), Some(detail))
                    } else {
                        (None, None)
                    };
                QueryResult {
                    query: query.clone(),
                    response: Some(parsed),
                    raw_json: raw.clone(),
                    word_count,
                    latency,
                    structural_assertion_passed: structural_passed,
                    structural_detail,
                    parse_error: None,
                }
            }
            Err(e) => QueryResult {
                query: query.clone(),
                response: None,
                raw_json: raw,
                word_count: 0,
                latency,
                structural_assertion_passed: None,
                structural_detail: None,
                parse_error: Some(format!("{e}")),
            },
        };

        let struct_str = qr
            .structural_assertion_passed
            .map_or("N/A".to_string(), |p| {
                if p {
                    "PASS".into()
                } else {
                    "FAIL".into()
                }
            });
        let contra_count = qr
            .response
            .as_ref()
            .map_or(0, |r| r.contradictions_flagged.len());
        let vault_empty = qr
            .response
            .as_ref()
            .is_some_and(|r| r.vault_has_no_relevant_content);
        println!(
            "   synth {}w · contradictions={} · vault_empty={} · struct={} · {:.1}s",
            qr.word_count,
            contra_count,
            vault_empty,
            struct_str,
            latency.as_secs_f64()
        );
        results.push(qr);
        println!();
    }

    // Latency summary
    let lats: Vec<f64> = results.iter().map(|r| r.latency.as_secs_f64()).collect();
    let stats = latency_stats(&lats);
    println!("\n{sep}");
    println!("LATENCY SUMMARY (Qwen-7B standalone, n=8)");
    println!("{sep}");
    println!(
        "min={:.1}s · p50={:.1}s · p99={:.1}s · max={:.1}s · mean={:.1}s",
        stats.min, stats.p50, stats.p99, stats.max, stats.mean
    );
    println!("2-min ceiling: 120s. Pipeline shippable iff p99 < 120s.");

    let md_path = vault_retrieval_root()
        .join("examples")
        .join("t026_qwen_7b_results.md");
    let md = build_markdown_report(&results, &run_started, memory_fixture.len(), insert_secs);
    std::fs::write(&md_path, md)?;
    println!("\nMarkdown writeup: {}", md_path.display());
    println!(
        "Run completed: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
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
    results: &[QueryResult],
    run_started: &chrono::DateTime<chrono::Utc>,
    n_memories: usize,
    insert_secs: f64,
) -> String {
    let mut s = String::new();
    s.push_str("# T0.2.3 Qwen2.5-7B Standalone Spike — Results\n\n");
    s.push_str(&format!(
        "**Run started:** {}  \n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    s.push_str("**Model:** Qwen2.5-7B-Instruct Q4_K_M (4.36 GB GGUF, Apache 2.0)  \n");
    s.push_str("**Pipeline:** Single Qwen-7B call — filter + flag contradictions + synthesize  \n");
    s.push_str("**Latency ceiling:** 2 min hard cap per partner product framing  \n");
    s.push_str(&format!(
        "**Fixture:** {} memories from merge_acceptance_100.json, 8 target queries  \n",
        n_memories
    ));
    s.push_str(&format!(
        "**Setup:** {:.1}s memory insertion\n\n",
        insert_secs
    ));

    let lats: Vec<f64> = results.iter().map(|r| r.latency.as_secs_f64()).collect();
    let stats = latency_stats(&lats);
    s.push_str("## Latency summary\n\n");
    s.push_str(&format!("| min | p50 | p99 | max | mean |\n|---|---|---|---|---|\n| {:.1}s | {:.1}s | {:.1}s | {:.1}s | {:.1}s |\n\n",
        stats.min, stats.p50, stats.p99, stats.max, stats.mean));
    s.push_str(&format!(
        "**2-min ceiling check:** p99 = {:.1}s — {}\n\n",
        stats.p99,
        if stats.p99 < 120.0 {
            "**WITHIN budget**"
        } else {
            "**OVER budget**"
        }
    ));

    s.push_str("## Structural assertion (contradiction queries)\n\n");
    s.push_str("| Query | Verdict | Detail |\n|---|---|---|\n");
    for r in results {
        if !CONTRADICTION_QUERY_IDS.contains(&r.query.id.as_str()) {
            continue;
        }
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            r.query.id,
            r.structural_assertion_passed
                .map_or("N/A".to_string(), |p| if p {
                    "**PASS**".into()
                } else {
                    "FAIL".into()
                }),
            r.structural_detail.as_deref().unwrap_or("—")
        ));
    }
    s.push('\n');

    s.push_str("## Per-query detail\n\n");
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
            s.push_str(&format!(
                "**PARSE_FAILURE:** {} · latency {:.1}s\n\n```\n{}\n```\n\n",
                err,
                r.latency.as_secs_f64(),
                r.raw_json
            ));
            continue;
        }
        let response = r.response.as_ref().unwrap();
        s.push_str(&format!("- **word_count:** {} · **latency:** {:.1}s · **contradictions_flagged.len():** {} · **vault_has_no_relevant_content:** {}\n",
            r.word_count, r.latency.as_secs_f64(),
            response.contradictions_flagged.len(),
            response.vault_has_no_relevant_content));
        if let (Some(passed), Some(detail)) = (r.structural_assertion_passed, &r.structural_detail)
        {
            s.push_str(&format!(
                "- **structural assertion:** {} — {}\n",
                if passed { "PASS" } else { "FAIL" },
                detail
            ));
        }
        s.push_str("\n**synthesis_markdown:**\n\n```\n");
        s.push_str(&response.synthesis_markdown);
        if !response.synthesis_markdown.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("```\n\n");
        if !response.contradictions_flagged.is_empty() {
            s.push_str("**contradictions_flagged:**\n\n```json\n");
            let pretty = serde_json::to_string_pretty(&serde_json::json!(response
                .contradictions_flagged
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "memory_ids": c.memory_ids,
                        "positions": c.positions,
                        "current_position_if_determinable": c.current_position_if_determinable,
                    })
                })
                .collect::<Vec<_>>()))
            .unwrap_or_else(|_| "[error]".to_string());
            s.push_str(&pretty);
            s.push_str("\n```\n\n");
        }
        s.push_str("---\n\n");
    }

    s.push_str("## Architectural decision — DEFERRED\n\nData only.\n");
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
