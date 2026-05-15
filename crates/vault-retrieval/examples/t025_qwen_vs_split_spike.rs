//! T0.2.3 read-time architecture spike — Pipeline A (Phi-4 split) vs Pipeline B (Qwen standalone).
//!
//! **Question this spike answers:**
//!
//! Following the t024 spike which established that Phi-4-mini-instruct fails to
//! reliably surface contradictions in synthesis (1 of 8 structural passes) and
//! cannot gate hard-negatives confidently, this spike measures whether splitting
//! the read pipeline so Phi-4 handles only the binary-classification work
//! (relevance gate + pairwise contradiction detection) and Qwen2.5-14B handles
//! the open-ended synthesis work delivers the differentiator at acceptable
//! latency.
//!
//! **Two pipelines, same 8 query candidate sets:**
//!
//! - **Pipeline A — Phi-4 + Qwen split:**
//!   1. BGE semantic retrieval top-20 (existing SemanticRetriever)
//!   2. Phi-4 Variant A (single-call relevance gate) filters to ~0-8 candidates
//!   3. Phi-4 pairwise contradiction detection on pairs with BGE cosine ≥ 0.85
//!   4. Qwen2.5-14B synthesis with pre-flagged contradictions as input
//!
//! - **Pipeline B — Qwen standalone:**
//!   1. BGE semantic retrieval top-20
//!   2. Single Qwen2.5-14B call: filter + flag contradictions + synthesize in one shot
//!
//! **Latency budget (partner-locked, 2026-05-14):** hard ceiling 2 min per query.
//! Above that, broken — agent waits too long, can't ship. Both pipelines measured
//! against this gate.
//!
//! **8 query candidate sets** (drawn from existing 26-query t023 fixture):
//! - Q11, Q13: lexical-direct contradiction pairs
//! - Q25, Q26: oblique-phrased contradiction pairs
//! - Q17, Q19: multi-cluster narrative
//! - Q21, Q22: hard-negatives
//!
//! **Discipline.** Spike-grade throwaway. No production code change beyond the
//! spike-scoped `vault_llm::Qwen25_14BProvider`. No commit at run completion. No
//! architecture call in the writeup — measurement only.
//!
//! Run with (PowerShell on Windows, per standing rules):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t025_qwen_vs_split_spike
//! ```

#![allow(clippy::too_many_lines)]
#![allow(clippy::cast_precision_loss)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, ensure, Context, Result};
use serde::Deserialize;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_llm::{
    CompletionParams, LlmProvider, Phi4MiniConfig, Phi4MiniProvider, Qwen25_14BProvider,
};
use vault_retrieval::{
    RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever, SemanticRetriever,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

const TARGET_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26", "Q17", "Q19", "Q21", "Q22"];
const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];
const HARD_NEGATIVE_QUERY_IDS: &[&str] = &["Q21", "Q22"];

/// Pairs of filtered candidates with BGE cosine ≥ this threshold are checked
/// for contradictions by Phi-4 stage 2.5. Per the architectural plan.
const PAIRWISE_COSINE_THRESHOLD: f32 = 0.85;

fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        _ => None,
    }
}

// ── Prompts ──────────────────────────────────────────────────────────────

const STAGE2_VA_SYSTEM_PROMPT: &str = r#"You are the relevance-gate layer of a personal memory vault used by AI coding agents.
An agent has issued a query; we have retrieved candidate memories via semantic search.
Your job: decide which candidates actually answer the query, not just share keywords.

Rules:
- A candidate is relevant only if its content directly addresses the query's subject.
- Topical or lexical overlap alone is NOT relevance. (A "Kubernetes migration" query
  should reject "database migrations" candidates even if they share the word "migration".)
- When in doubt, exclude. False positives cost the agent more than false negatives.
- Return ONLY valid JSON matching the schema; no markdown, no commentary outside JSON."#;

const STAGE2_5_SYSTEM_PROMPT: &str = r#"You are the pairwise contradiction-detection layer of a personal memory vault.

You receive TWO memories that were both flagged as relevant to the same query. Your job:
decide whether they make conflicting claims about the same underlying fact.

Two memories contradict if they assert different values for the same property:
- "GA launch in Q1 2027" vs "GA launch moved to Q2 2027" — same fact (GA timing), different values
- "Comcast bill is $89/month" vs "Comcast bill is $109/month" — same fact (bill amount), different values

NOT contradictions:
- Different facts (one about rent, one about Netflix)
- Elaboration (short note + paragraph saying the same thing in more detail)
- Sequential events that don't dispute prior state

Return ONLY valid JSON matching the schema."#;

const STAGE3_WITH_FLAGS_SYSTEM_PROMPT: &str = r#"You are the synthesis layer of a personal memory vault used by AI coding agents.

The relevance gate has filtered candidates to the ones that answer the query. A separate
contradiction-detection layer has pre-flagged any pairs with conflicting claims. Your job:
produce a coherent synthesis the agent can use directly as context.

Requirements:
- If pre-flagged contradictions are listed below, you MUST explicitly surface each one in
  synthesis_markdown with BOTH positions stated, including any dates/dollar amounts/
  identifiers that distinguish them. Agents cannot work around contradictions they
  don't know exist. Populate contradictions_flagged in the output to match.
- If the filtered memories cover multiple facets of a topic, write a coherent narrative
  capturing the state of work — not a concatenated list of fragments.
- If the filtered set is empty, set vault_has_no_relevant_content=true AND state explicitly
  in synthesis_markdown that no relevant memories exist on this topic. Do NOT fabricate.
- Cite memory IDs when claiming facts ("per [mem-7], ...").
- Keep synthesis_markdown under 250 words.
- Return ONLY valid JSON matching the schema."#;

const STAGE_B_STANDALONE_SYSTEM_PROMPT: &str = r#"You are the read layer of a personal memory vault used by AI coding agents.

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

// ── Schemas ──────────────────────────────────────────────────────────────

const STAGE2_VA_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["relevant_ids", "reasoning"],
  "properties": {
    "relevant_ids": {"type": "array", "items": {"type": "string"}},
    "reasoning": {"type": "string"}
  }
}"#;

const STAGE2_5_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["contradicts", "conflicting_field"],
  "properties": {
    "contradicts": {"type": "boolean"},
    "conflicting_field": {"type": "string"}
  }
}"#;

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
    expected_memory_ids: Vec<String>,
    notes: String,
}

// ── Response types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct Stage2VaResponse {
    relevant_ids: Vec<String>,
    reasoning: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Stage25Response {
    contradicts: bool,
    conflicting_field: String,
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

// ── Spike result types ───────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct ContradictionFlag {
    pair: (String, String),
    cosine: f32,
    contradicts: bool,
    conflicting_field: String,
    latency: Duration,
}

#[derive(Debug, Clone)]
struct SynthesisResult {
    response: Option<SynthesisResponse>,
    raw_json: String,
    word_count: usize,
    latency: Duration,
    structural_assertion_passed: Option<bool>,
    structural_detail: Option<String>,
    parse_error: Option<String>,
}

#[derive(Debug, Clone)]
struct PipelineAResult {
    stage2_filtered: Vec<String>,
    stage2_reasoning: String,
    stage2_latency: Duration,
    stage2_parse_error: Option<String>,
    stage2_5_flags: Vec<ContradictionFlag>,
    stage2_5_latency: Duration,
    synthesis: SynthesisResult,
    total_latency: Duration,
}

#[derive(Debug, Clone)]
struct PipelineBResult {
    synthesis: SynthesisResult,
    total_latency: Duration,
}

#[derive(Debug, Clone)]
struct QueryResult {
    query: QueryEntry,
    #[allow(dead_code)]
    candidate_fixture_ids: Vec<String>,
    #[allow(dead_code)]
    retrieval_latency: Duration,
    pipeline_a: PipelineAResult,
    pipeline_b: PipelineBResult,
}

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    let sep_wide = "=".repeat(120);
    println!("{sep_wide}");
    println!("T0.2.3 read-time architecture spike — Pipeline A (Phi-4 + Qwen split) vs Pipeline B (Qwen standalone)");
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Host:    {}", std::env::consts::OS);
    println!("{sep_wide}");

    // ── Setup: storage + BGE + Phi-4 + Qwen ──────────────────────────────
    let dir = tempfile::tempdir().context("tempdir")?;
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

    println!("Opening Phi4MiniProvider...");
    let phi4_start = Instant::now();
    let phi4_config = Phi4MiniConfig::v0_2_default(models_dir()?);
    let phi4 = Phi4MiniProvider::new(phi4_config).await?;
    println!(
        "Phi-4 ready in {:.1}s — {}",
        phi4_start.elapsed().as_secs_f64(),
        phi4.model_id()
    );

    println!("Opening Qwen25_14BProvider (8.37 GB load, expect 10-30s)...");
    let qwen_start = Instant::now();
    let qwen_path = models_dir()?.join("Qwen2.5-14B-Instruct-Q4_K_M.gguf");
    ensure!(qwen_path.exists(), "Qwen GGUF missing at {qwen_path:?}");
    let qwen = Qwen25_14BProvider::open(&qwen_path).await?;
    println!(
        "Qwen ready in {:.1}s — {}",
        qwen_start.elapsed().as_secs_f64(),
        qwen.model_id()
    );

    // ── Load fixtures ────────────────────────────────────────────────────
    let memory_fixture_path = repo_root()?
        .join("crates")
        .join("vault-consolidator")
        .join("tests")
        .join("fixtures")
        .join("merge_acceptance_100.json");
    let memory_fixture: Vec<MemoryFixtureEntry> = {
        let bytes = std::fs::read(&memory_fixture_path)?;
        serde_json::from_slice(&bytes)?
    };
    println!("\nLoaded {} memories from fixture", memory_fixture.len());

    let query_fixture_path = vault_retrieval_root()
        .join("test-fixtures")
        .join("merge_acceptance_100_queries.json");
    let query_set: QuerySet = {
        let bytes = std::fs::read(&query_fixture_path)?;
        serde_json::from_slice(&bytes)?
    };
    let target_queries: Vec<QueryEntry> = TARGET_QUERY_IDS
        .iter()
        .map(|wanted| {
            query_set
                .queries
                .iter()
                .find(|q| q.id == *wanted)
                .cloned()
                .with_context(|| format!("target query {wanted} missing"))
        })
        .collect::<Result<Vec<_>>>()?;
    println!(
        "Filtered {} queries to 8 target queries: {:?}",
        query_set.queries.len(),
        TARGET_QUERY_IDS
    );

    // ── Insert memories ──────────────────────────────────────────────────
    println!(
        "\nInserting {} memories with BGE embeddings...",
        memory_fixture.len()
    );
    let mut fixture_id_to_memory_id: HashMap<String, MemoryId> = HashMap::new();
    // Cache content + embedding per fixture id for stage 2.5 pairwise cosine
    // and prompt construction.
    let mut fixture_id_to_embedding: HashMap<String, Vec<f32>> = HashMap::new();
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
        fixture_id_to_embedding.insert(entry.id.clone(), embedding);
        if (i + 1) % 20 == 0 {
            println!("  inserted {}/{}", i + 1, memory_fixture.len());
        }
    }
    let insert_secs = insert_start.elapsed().as_secs_f64();
    println!(
        "Inserted {} memories in {:.1}s",
        memory_fixture.len(),
        insert_secs
    );

    let memory_id_to_fixture_id: HashMap<MemoryId, String> = fixture_id_to_memory_id
        .iter()
        .map(|(fid, mid)| (*mid, fid.clone()))
        .collect();
    let fixture_id_to_content: HashMap<String, String> = memory_fixture
        .iter()
        .map(|e| (e.id.clone(), e.content.clone()))
        .collect();

    let retriever = SemanticRetriever::new(metadata.clone(), bge, vectors.clone());

    // ── Per-query: both pipelines ────────────────────────────────────────
    println!("\n{sep_wide}");
    println!("Running 8 queries × (Pipeline A: Phi-4 V-A + Phi-4 stage 2.5 + Qwen stage 3) + (Pipeline B: Qwen standalone)");
    println!("{sep_wide}\n");

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

        // Retrieval
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
                    .unwrap_or_else(|| "<unknown>".to_string())
            })
            .collect();
        println!(
            "   retrieved {} candidates in {:.0}ms",
            hits.len(),
            retrieval_latency.as_secs_f64() * 1000.0
        );

        let expected_set: HashSet<String> = query.expected_memory_ids.iter().cloned().collect();
        let is_hard_negative = HARD_NEGATIVE_QUERY_IDS.contains(&query.id.as_str());

        // Pipeline A — Phi-4 V-A + Phi-4 stage 2.5 + Qwen synth
        let pipeline_a = run_pipeline_a(
            &phi4,
            &qwen,
            query,
            &hits,
            &candidate_fixture_ids,
            &fixture_id_to_embedding,
            &fixture_id_to_content,
            &expected_set,
            is_hard_negative,
        )
        .await
        .with_context(|| format!("Pipeline A on {}", query.id))?;

        // Pipeline B — Qwen standalone
        let pipeline_b = run_pipeline_b(
            &qwen,
            query,
            &hits,
            &candidate_fixture_ids,
            &expected_set,
            is_hard_negative,
        )
        .await
        .with_context(|| format!("Pipeline B on {}", query.id))?;

        println!(
            "   A: stage2 {} filtered ({:.1}s) · stage2.5 {} pairs ({:.1}s) · synth {}w struct={} ({:.1}s) · TOTAL {:.1}s",
            pipeline_a.stage2_filtered.len(),
            pipeline_a.stage2_latency.as_secs_f64(),
            pipeline_a.stage2_5_flags.len(),
            pipeline_a.stage2_5_latency.as_secs_f64(),
            pipeline_a.synthesis.word_count,
            pipeline_a.synthesis.structural_assertion_passed
                .map_or("N/A".to_string(), |p| if p { "PASS".into() } else { "FAIL".into() }),
            pipeline_a.synthesis.latency.as_secs_f64(),
            pipeline_a.total_latency.as_secs_f64()
        );
        println!(
            "   B: synth {}w contradictions={} vault_empty={} struct={} · TOTAL {:.1}s",
            pipeline_b.synthesis.word_count,
            pipeline_b
                .synthesis
                .response
                .as_ref()
                .map_or(0, |r| r.contradictions_flagged.len()),
            pipeline_b
                .synthesis
                .response
                .as_ref()
                .is_some_and(|r| r.vault_has_no_relevant_content),
            pipeline_b
                .synthesis
                .structural_assertion_passed
                .map_or("N/A".to_string(), |p| if p {
                    "PASS".into()
                } else {
                    "FAIL".into()
                }),
            pipeline_b.total_latency.as_secs_f64()
        );

        results.push(QueryResult {
            query: query.clone(),
            candidate_fixture_ids,
            retrieval_latency,
            pipeline_a,
            pipeline_b,
        });
        println!();
    }

    // ── Aggregate output ─────────────────────────────────────────────────
    print_latency_summary(&results);

    let md_path = vault_retrieval_root()
        .join("examples")
        .join("t025_qwen_vs_split_results.md");
    let md = build_markdown_report(&results, &run_started, memory_fixture.len(), insert_secs);
    std::fs::write(&md_path, md)?;
    println!("\nMarkdown writeup: {}", md_path.display());
    println!(
        "Run completed: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    Ok(())
}

// ── Pipeline A ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_pipeline_a(
    phi4: &Phi4MiniProvider,
    qwen: &Qwen25_14BProvider,
    query: &QueryEntry,
    candidates: &[RetrievedMemory],
    candidate_fixture_ids: &[String],
    fixture_id_to_embedding: &HashMap<String, Vec<f32>>,
    fixture_id_to_content: &HashMap<String, String>,
    _expected_set: &HashSet<String>,
    _is_hard_negative: bool,
) -> Result<PipelineAResult> {
    let total_start = Instant::now();

    // Stage 2 V-A: single Phi-4 call relevance gate
    let mut user_prompt = format!("QUERY: {}\n\nCANDIDATES:\n", query.query_text);
    for (i, c) in candidates.iter().enumerate() {
        user_prompt.push_str(&format!(
            "[{}] {}\n",
            candidate_fixture_ids[i], c.memory.content
        ));
    }
    user_prompt.push_str("\nDecide which candidates are relevant. Return JSON.");

    let stage2_params = CompletionParams {
        max_tokens: 768,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(STAGE2_VA_SYSTEM_PROMPT.to_string()),
    };
    let stage2_start = Instant::now();
    let stage2_raw = phi4
        .complete_json(&user_prompt, STAGE2_VA_SCHEMA, &stage2_params)
        .await?;
    let stage2_latency = stage2_start.elapsed();

    let (stage2_filtered, stage2_reasoning, stage2_parse_error) =
        match serde_json::from_str::<Stage2VaResponse>(&stage2_raw) {
            Ok(parsed) => {
                let valid_ids: HashSet<String> = candidate_fixture_ids.iter().cloned().collect();
                let filtered: Vec<String> = parsed
                    .relevant_ids
                    .iter()
                    .filter(|id| valid_ids.contains(*id))
                    .cloned()
                    .collect();
                (filtered, parsed.reasoning, None)
            }
            Err(e) => (
                Vec::new(),
                format!("PARSE_ERROR: {e}"),
                Some(format!("{e}")),
            ),
        };

    // Stage 2.5: pairwise contradiction detection for pairs above cosine threshold
    let stage2_5_start = Instant::now();
    let mut stage2_5_flags: Vec<ContradictionFlag> = Vec::new();
    for i in 0..stage2_filtered.len() {
        for j in (i + 1)..stage2_filtered.len() {
            let a = &stage2_filtered[i];
            let b = &stage2_filtered[j];
            let cos = match (
                fixture_id_to_embedding.get(a),
                fixture_id_to_embedding.get(b),
            ) {
                (Some(ea), Some(eb)) => cosine_similarity(ea, eb),
                _ => continue,
            };
            if cos < PAIRWISE_COSINE_THRESHOLD {
                continue;
            }
            let content_a = fixture_id_to_content
                .get(a)
                .map_or("<missing>", String::as_str);
            let content_b = fixture_id_to_content
                .get(b)
                .map_or("<missing>", String::as_str);
            let pair_prompt = format!(
                "QUERY CONTEXT: {}\n\nMEMORY 1 [{a}]: {content_a}\n\nMEMORY 2 [{b}]: {content_b}\n\nDo these contradict? Return JSON.",
                query.query_text
            );
            let pair_params = CompletionParams {
                max_tokens: 256,
                temperature: 0.0,
                top_p: 1.0,
                seed: Some(42),
                system_prompt: Some(STAGE2_5_SYSTEM_PROMPT.to_string()),
            };
            let pair_start = Instant::now();
            let pair_raw = phi4
                .complete_json(&pair_prompt, STAGE2_5_SCHEMA, &pair_params)
                .await?;
            let pair_latency = pair_start.elapsed();
            let parsed: Result<Stage25Response, _> = serde_json::from_str(&pair_raw);
            match parsed {
                Ok(r) => {
                    stage2_5_flags.push(ContradictionFlag {
                        pair: (a.clone(), b.clone()),
                        cosine: cos,
                        contradicts: r.contradicts,
                        conflicting_field: r.conflicting_field,
                        latency: pair_latency,
                    });
                }
                Err(_) => {
                    stage2_5_flags.push(ContradictionFlag {
                        pair: (a.clone(), b.clone()),
                        cosine: cos,
                        contradicts: false,
                        conflicting_field: format!("PARSE_ERROR: {pair_raw}"),
                        latency: pair_latency,
                    });
                }
            }
        }
    }
    let stage2_5_latency = stage2_5_start.elapsed();

    // Stage 3: Qwen synthesis with pre-flagged contradictions
    let mut synth_user_prompt = format!("QUERY: {}\n\nFILTERED MEMORIES:\n", query.query_text);
    if stage2_filtered.is_empty() {
        synth_user_prompt.push_str("(empty — relevance gate rejected all candidates)\n");
    } else {
        for fid in &stage2_filtered {
            let content = fixture_id_to_content
                .get(fid)
                .map_or("<missing>", String::as_str);
            synth_user_prompt.push_str(&format!("[{fid}] {content}\n"));
        }
    }
    synth_user_prompt.push_str("\nPRE-FLAGGED CONTRADICTIONS:\n");
    let confirmed: Vec<&ContradictionFlag> =
        stage2_5_flags.iter().filter(|f| f.contradicts).collect();
    if confirmed.is_empty() {
        synth_user_prompt.push_str("(none — pairwise check found no contradictions)\n");
    } else {
        for f in &confirmed {
            synth_user_prompt.push_str(&format!(
                "- [{}] and [{}] disagree on: {}\n",
                f.pair.0, f.pair.1, f.conflicting_field
            ));
        }
    }
    synth_user_prompt.push_str("\nSynthesize. Return JSON.");

    let synth_params = CompletionParams {
        max_tokens: 1024,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(STAGE3_WITH_FLAGS_SYSTEM_PROMPT.to_string()),
    };
    let synthesis = run_synthesis(qwen, query, &synth_user_prompt, &synth_params).await?;

    Ok(PipelineAResult {
        stage2_filtered,
        stage2_reasoning,
        stage2_latency,
        stage2_parse_error,
        stage2_5_flags,
        stage2_5_latency,
        synthesis,
        total_latency: total_start.elapsed(),
    })
}

// ── Pipeline B ───────────────────────────────────────────────────────────

async fn run_pipeline_b(
    qwen: &Qwen25_14BProvider,
    query: &QueryEntry,
    candidates: &[RetrievedMemory],
    candidate_fixture_ids: &[String],
    _expected_set: &HashSet<String>,
    _is_hard_negative: bool,
) -> Result<PipelineBResult> {
    let total_start = Instant::now();
    let mut user_prompt = format!("QUERY: {}\n\nCANDIDATES:\n", query.query_text);
    for (i, c) in candidates.iter().enumerate() {
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
        system_prompt: Some(STAGE_B_STANDALONE_SYSTEM_PROMPT.to_string()),
    };
    let synthesis = run_synthesis(qwen, query, &user_prompt, &params).await?;
    Ok(PipelineBResult {
        synthesis,
        total_latency: total_start.elapsed(),
    })
}

// ── Synthesis runner (shared by A stage-3 and B standalone) ──────────────

async fn run_synthesis(
    qwen: &Qwen25_14BProvider,
    query: &QueryEntry,
    user_prompt: &str,
    params: &CompletionParams,
) -> Result<SynthesisResult> {
    let start = Instant::now();
    let raw = qwen
        .complete_json(user_prompt, SYNTHESIS_SCHEMA, params)
        .await?;
    let latency = start.elapsed();

    match serde_json::from_str::<SynthesisResponse>(&raw) {
        Ok(parsed) => {
            let word_count = parsed.synthesis_markdown.split_whitespace().count();
            let (structural_passed, structural_detail) =
                if CONTRADICTION_QUERY_IDS.contains(&query.id.as_str()) {
                    let (sub_a, sub_b) = structural_substrings(&query.id)
                        .ok_or_else(|| anyhow!("missing structural substrings for {}", query.id))?;
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
            Ok(SynthesisResult {
                response: Some(parsed),
                raw_json: raw,
                word_count,
                latency,
                structural_assertion_passed: structural_passed,
                structural_detail,
                parse_error: None,
            })
        }
        Err(e) => Ok(SynthesisResult {
            response: None,
            raw_json: raw,
            word_count: 0,
            latency,
            structural_assertion_passed: None,
            structural_detail: None,
            parse_error: Some(format!("{e}")),
        }),
    }
}

// ── Cosine similarity ────────────────────────────────────────────────────

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

// ── Latency aggregation ──────────────────────────────────────────────────

fn print_latency_summary(results: &[QueryResult]) {
    let sep = "=".repeat(120);
    println!("\n{sep}");
    println!("LATENCY SUMMARY — both pipelines, end-to-end per query");
    println!("{sep}");
    let a_total: Vec<f64> = results
        .iter()
        .map(|r| r.pipeline_a.total_latency.as_secs_f64())
        .collect();
    let b_total: Vec<f64> = results
        .iter()
        .map(|r| r.pipeline_b.total_latency.as_secs_f64())
        .collect();
    println!("{:<40} | min   p50   p99   max   mean", "stage");
    println!("{}", "-".repeat(80));
    print_latency_row("Pipeline A end-to-end", &a_total);
    print_latency_row("Pipeline B end-to-end", &b_total);
    println!("\n2-min ceiling (per partner): both pipelines must clear 120s p99 to be shippable.");
}

fn print_latency_row(label: &str, samples: &[f64]) {
    let stats = latency_stats(samples);
    println!(
        "{:<40} | {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1}",
        label, stats.min, stats.p50, stats.p99, stats.max, stats.mean
    );
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

// ── Markdown writeup ─────────────────────────────────────────────────────

fn build_markdown_report(
    results: &[QueryResult],
    run_started: &chrono::DateTime<chrono::Utc>,
    n_memories: usize,
    insert_secs: f64,
) -> String {
    let mut s = String::new();
    s.push_str("# T0.2.3 Read-Time Architecture Spike — Pipeline A vs Pipeline B Results\n\n");
    s.push_str(&format!(
        "**Run started:** {}  \n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    s.push_str(&format!("**Host OS:** {}  \n", std::env::consts::OS));
    s.push_str("**Pipeline A:** Phi-4 V-A relevance gate → Phi-4 stage-2.5 pairwise contradiction detection → Qwen2.5-14B synthesis with pre-flagged contradictions  \n");
    s.push_str("**Pipeline B:** Qwen2.5-14B standalone (single call does filter + contradiction-flag + synthesize)  \n");
    s.push_str("**Latency budget:** 2-min hard ceiling per partner-locked product framing.  \n");
    s.push_str(&format!(
        "**Fixture:** {} memories, 8 target queries from existing t023 26-query set  \n",
        n_memories
    ));
    s.push_str(&format!(
        "**Setup time:** {:.1}s for memory insertion\n\n",
        insert_secs
    ));

    s.push_str("> **Discipline note.** Measurement only. Architectural decision (which pipeline to ship, or whether either does) is partner conversation based on this data + manual review of the verbatim synthesis outputs below.\n\n");

    s.push_str("---\n\n## Latency comparison\n\n");
    s.push_str("| Pipeline | min | p50 | p99 | max | mean |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    let a_total: Vec<f64> = results
        .iter()
        .map(|r| r.pipeline_a.total_latency.as_secs_f64())
        .collect();
    let b_total: Vec<f64> = results
        .iter()
        .map(|r| r.pipeline_b.total_latency.as_secs_f64())
        .collect();
    push_lat_row(&mut s, "A end-to-end", &a_total);
    push_lat_row(&mut s, "B end-to-end", &b_total);
    s.push_str(
        "\n2-min (120s) ceiling: pipelines must clear this on every query, not just average.\n\n",
    );

    s.push_str("## Structural assertion results (contradiction queries only)\n\n");
    s.push_str("| Query | Pipeline A struct | Pipeline B struct |\n");
    s.push_str("|---|---|---|\n");
    for r in results {
        if !CONTRADICTION_QUERY_IDS.contains(&r.query.id.as_str()) {
            continue;
        }
        s.push_str(&format!(
            "| {} | {} | {} |\n",
            r.query.id,
            r.pipeline_a.synthesis.structural_assertion_passed.map_or(
                "N/A".to_string(),
                |p| if p { "**PASS**".into() } else { "FAIL".into() }
            ),
            r.pipeline_b.synthesis.structural_assertion_passed.map_or(
                "N/A".to_string(),
                |p| if p { "**PASS**".into() } else { "FAIL".into() }
            ),
        ));
    }
    s.push('\n');

    s.push_str("---\n\n## Per-query detail\n\n");
    for r in results {
        s.push_str(&format!(
            "### {} — \"{}\"\n\n",
            r.query.id, r.query.query_text
        ));
        s.push_str(&format!(
            "**Shape:** {} · **Notes:** {}\n\n",
            r.query.shape, r.query.notes
        ));
        s.push_str(&format!(
            "**Expected memories ({}):** `{}`\n\n",
            r.query.expected_memory_ids.len(),
            if r.query.expected_memory_ids.is_empty() {
                "(empty — hard-negative)".to_string()
            } else {
                r.query.expected_memory_ids.join("`, `")
            }
        ));

        // Pipeline A
        s.push_str("#### Pipeline A — Phi-4 split + Qwen synthesis\n\n");
        s.push_str(&format!(
            "- **Stage 2 filtered ({}):** `{}`\n",
            r.pipeline_a.stage2_filtered.len(),
            if r.pipeline_a.stage2_filtered.is_empty() {
                "(empty)".to_string()
            } else {
                r.pipeline_a.stage2_filtered.join("`, `")
            }
        ));
        s.push_str(&format!(
            "- **Stage 2 reasoning:** {}\n",
            escape_md(&r.pipeline_a.stage2_reasoning)
        ));
        if let Some(err) = &r.pipeline_a.stage2_parse_error {
            s.push_str(&format!("- **Stage 2 PARSE_ERROR:** {}\n", escape_md(err)));
        }
        s.push_str(&format!(
            "- **Stage 2 latency:** {:.1}s\n",
            r.pipeline_a.stage2_latency.as_secs_f64()
        ));
        s.push_str(&format!(
            "- **Stage 2.5 pairs checked ({}):** ",
            r.pipeline_a.stage2_5_flags.len()
        ));
        if r.pipeline_a.stage2_5_flags.is_empty() {
            s.push_str("(no pairs above cosine 0.85 threshold)\n");
        } else {
            s.push('\n');
            for f in &r.pipeline_a.stage2_5_flags {
                s.push_str(&format!(
                    "  - [{} ↔ {}] cos={:.3} · contradicts={} · field=\"{}\" · {:.1}s\n",
                    f.pair.0,
                    f.pair.1,
                    f.cosine,
                    f.contradicts,
                    escape_md(&f.conflicting_field),
                    f.latency.as_secs_f64()
                ));
            }
        }
        s.push_str(&format!(
            "- **Stage 2.5 total latency:** {:.1}s\n",
            r.pipeline_a.stage2_5_latency.as_secs_f64()
        ));
        s.push_str(&format!(
            "- **Total Pipeline A latency:** {:.1}s (budget 120s: {})\n\n",
            r.pipeline_a.total_latency.as_secs_f64(),
            if r.pipeline_a.total_latency.as_secs_f64() <= 120.0 {
                "**WITHIN**"
            } else {
                "**OVER**"
            }
        ));
        s.push_str("**Synthesis output (Qwen, with pre-flagged contradictions):**\n\n");
        write_synthesis_detail(&mut s, &r.pipeline_a.synthesis);

        // Pipeline B
        s.push_str("#### Pipeline B — Qwen standalone\n\n");
        s.push_str(&format!(
            "- **Total Pipeline B latency:** {:.1}s (budget 120s: {})\n\n",
            r.pipeline_b.total_latency.as_secs_f64(),
            if r.pipeline_b.total_latency.as_secs_f64() <= 120.0 {
                "**WITHIN**"
            } else {
                "**OVER**"
            }
        ));
        s.push_str("**Synthesis output (Qwen single-call):**\n\n");
        write_synthesis_detail(&mut s, &r.pipeline_b.synthesis);

        s.push_str("---\n\n");
    }

    s.push_str("## Architectural decision — DEFERRED\n\n");
    s.push_str("Data only. The architecture call (which pipeline ships, or whether either does at acceptable quality + latency) is partner conversation.\n");
    s
}

fn write_synthesis_detail(s: &mut String, r: &SynthesisResult) {
    if let Some(err) = &r.parse_error {
        s.push_str(&format!(
            "- **PARSE_FAILURE:** {} · latency {:.1}s · raw output {} chars\n\n",
            escape_md(err),
            r.latency.as_secs_f64(),
            r.raw_json.len()
        ));
        s.push_str("**Phi/Qwen raw output (parse failed):**\n\n```\n");
        s.push_str(&r.raw_json);
        if !r.raw_json.ends_with('\n') {
            s.push('\n');
        }
        s.push_str("```\n\n");
        return;
    }
    let response = match &r.response {
        Some(resp) => resp,
        None => {
            s.push_str("- (unknown state)\n\n");
            return;
        }
    };
    s.push_str(&format!("- **word_count:** {} · **latency:** {:.1}s · **contradictions_flagged.len():** {} · **vault_has_no_relevant_content:** {}\n",
        r.word_count, r.latency.as_secs_f64(),
        response.contradictions_flagged.len(),
        response.vault_has_no_relevant_content));
    if let (Some(passed), Some(detail)) = (r.structural_assertion_passed, &r.structural_detail) {
        s.push_str(&format!(
            "- **structural assertion:** {} — {}\n",
            if passed { "PASS" } else { "FAIL" },
            detail
        ));
    }
    s.push_str("\n```\n");
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
        .unwrap_or_else(|_| "[parse-error]".to_string());
        s.push_str(&pretty);
        s.push_str("\n```\n\n");
    }
}

fn push_lat_row(s: &mut String, label: &str, samples: &[f64]) {
    let stats = latency_stats(samples);
    s.push_str(&format!(
        "| {} | {:.1}s | {:.1}s | {:.1}s | {:.1}s | {:.1}s |\n",
        label, stats.min, stats.p50, stats.p99, stats.max, stats.mean
    ));
}

fn escape_md(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

// ── BGE + model paths ────────────────────────────────────────────────────

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
