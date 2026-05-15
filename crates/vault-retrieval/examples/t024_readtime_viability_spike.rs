//! T0.2.x read-time viability spike (2026-05-14, spike iteration 2 lock).
//!
//! **Architectural context.** Following the t023 retrieval-diagnostic spike
//! (2026-05-14) which established that query-anchored retrieval recovers
//! the agent-shaped read workload at recall@20=1.00 across every real-query
//! shape — including long-form contradiction pairs — the next architectural
//! question is whether Phi-4-mini-instruct can deliver the load-bearing
//! differentiator: a read-time relevance gate that rejects hard-negative
//! false positives + a synthesis layer that surfaces contradictions and
//! produces coherent context for agents.
//!
//! Phi-4-mini was selected at ADR-042 for the merge-classifier task (100%
//! precision in the t022/t023 cron run). The merge-classifier task is
//! constrained JSON output on a narrow contract. Open-ended synthesis with
//! contradiction surfacing is a different model demand. This spike measures
//! whether the same model size + same prompt-engineering envelope can
//! deliver the new contract.
//!
//! **Three axes measured, side-by-side, on the same 8 query candidate sets:**
//!
//! 1. **Stage-2 relevance gate prompt shape.** Variant A = single Phi-4 call
//!    judges all K=20 candidates at once. Variant B = K Phi-4 calls, one per
//!    candidate. Per-query measurement (not global): partner picks the
//!    production variant during architecture decision based on full data.
//!
//! 2. **Stage-3 synthesis viability.** GBNF-constrained JSON with
//!    synthesis_markdown narrative + contradictions_flagged structured field
//!    + vault_has_no_relevant_content boolean. Stage-3 runs TWICE per query
//!      (once per stage-2 variant's filtered output). Manual review of the
//!      16 synthesis outputs against three criteria: contradiction surfacing,
//!      coherent narrative across clusters, confident "I don't know."
//!
//! 3. **Latency at K=20.** Real Phi-4 inference, real fixture, real content.
//!    Reference point: T0.2.1 spike-2 measured p50=9.8s for merge-classifier
//!    shape; synthesis prompts have larger context (more candidate content)
//!    so expect higher.
//!
//! **8 query candidate sets** (drawn from the existing 26-query t023 fixture):
//! - Q11, Q13: lexical-direct contradiction baselines (GA Q1 vs Q2, Comcast $89 vs $109)
//! - Q25, Q26: oblique-phrasing contradictions (roadmap-milestones, budget-review)
//! - Q17, Q19: multi-cluster narrative (exercise routine, launch timeline-with-embedded-contradiction)
//! - Q21, Q22: hard-negatives (Kubernetes, dental insurance)
//!
//! **Discipline.** Example-grade throwaway. No production code change. No
//! commit at run completion. No architecture call in the writeup —
//! measurement and verdicts only. Partner picks the V0.2 architecture from
//! the data.
//!
//! Run with (PowerShell on Windows, per standing rules):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t024_readtime_viability_spike
//! ```
//!
//! Expected wall time: ~22-30 min. 184 Phi-4 inference calls
//! (8 × 1 Variant-A + 8 × 20 Variant-B + 16 × stage-3).

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
use vault_llm::{CompletionParams, LlmProvider, Phi4MiniConfig, Phi4MiniProvider};
use vault_retrieval::{
    RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever, SemanticRetriever,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

const TARGET_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26", "Q17", "Q19", "Q21", "Q22"];
const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];
const HARD_NEGATIVE_QUERY_IDS: &[&str] = &["Q21", "Q22"];

/// Discriminating substrings for the structural assertion on each
/// contradiction query's synthesis output.
fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        _ => None,
    }
}

// ── Phi-4 prompts (system + user template fragments) ─────────────────────

const VARIANT_A_SYSTEM_PROMPT: &str = r#"You are the relevance-gate layer of a personal memory vault used by AI coding agents.
An agent has issued a query; we have retrieved candidate memories via semantic search.
Your job: decide which candidates actually answer the query, not just share keywords.

Rules:
- A candidate is relevant only if its content directly addresses the query's subject.
- Topical or lexical overlap alone is NOT relevance. (A "Kubernetes migration" query
  should reject "database migrations" candidates even if they share the word "migration".)
- When in doubt, exclude. False positives cost the agent more than false negatives —
  the agent can ask the user if the vault returns nothing; it cannot recover from
  acting confidently on wrong context.
- Return ONLY valid JSON matching the schema; no markdown, no commentary outside JSON."#;

const VARIANT_B_SYSTEM_PROMPT: &str = VARIANT_A_SYSTEM_PROMPT;

const STAGE3_SYSTEM_PROMPT: &str = r#"You are the synthesis layer of a personal memory vault used by AI coding agents.
The relevance gate has filtered candidates down to the ones that actually answer
the agent's query. Your job: produce a coherent synthesis the agent can use directly
as context, not a list of fragments to interpret.

Requirements:
- If the filtered memories make conflicting claims about the same fact, you MUST
  explicitly flag the contradiction with both positions and dates if available.
  Agents cannot work around contradictions they don't know exist.
- If the filtered memories cover multiple aspects of a topic, write a coherent
  narrative capturing the state of work, not a concatenated list.
- If the filtered set is empty, set vault_has_no_relevant_content to true and
  state explicitly in synthesis_markdown that no relevant memories exist on this
  topic. Do NOT fabricate context. Do NOT return empty narrative silently.
- Cite specific memory IDs in the narrative when claiming facts ("per [mem-7], ...").
- Keep synthesis_markdown under 250 words. Prioritize completeness over length —
  say everything important, then stop.
- Return ONLY valid JSON matching the schema; no markdown wrapping the JSON."#;

// ── JSON schemas (converted to GBNF by vault-llm via llama_cpp_2) ────────

const VARIANT_A_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["relevant_ids", "reasoning"],
  "properties": {
    "relevant_ids": {
      "type": "array",
      "items": {"type": "string"}
    },
    "reasoning": {"type": "string"}
  }
}"#;

const VARIANT_B_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["relevant", "reasoning"],
  "properties": {
    "relevant": {"type": "boolean"},
    "reasoning": {"type": "string"}
  }
}"#;

const STAGE3_SCHEMA: &str = r#"{
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

// ── Phi-4 response types ─────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct VariantAResponse {
    relevant_ids: Vec<String>,
    reasoning: String,
}

#[derive(Debug, Clone, Deserialize)]
struct VariantBResponse {
    relevant: bool,
    reasoning: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Stage3Response {
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
struct VariantResult {
    /// Spike-IDs (the `mem-w-...` fixture IDs) Phi-4 marked relevant.
    filtered_fixture_ids: Vec<String>,
    /// Per-candidate decisions for Variant B; empty for Variant A.
    #[allow(dead_code)]
    per_candidate_decisions: Vec<(String, bool, String)>,
    /// Phi-4 reasoning text (Variant A only; Variant B aggregates per-candidate).
    reasoning: String,
    /// Wall-time for the whole stage-2 invocation (single call for A;
    /// summed K calls for B).
    latency: Duration,
    /// Number of Phi-4 calls made (1 for A; K for B).
    n_calls: usize,
    /// Computed metrics against ground truth.
    precision: f64,
    recall: f64,
    /// For HN queries: true if filter is empty (correct rejection); for
    /// non-HN queries: not meaningful (set to false).
    hard_negative_rejected: bool,
    /// `Some(msg)` if Phi-4's output failed to parse against the schema (degeneracy
    /// loop hitting max_tokens, malformed JSON, etc). Downstream uses empty filter
    /// as the conservative default. For Variant B, lists the candidate(s) whose
    /// parse failed.
    parse_errors: Vec<String>,
    /// Raw Phi-4 output(s) when parse failed — preserved for partner review.
    /// Single-element for Variant A; one entry per failing candidate for Variant B.
    raw_failed_outputs: Vec<String>,
}

#[derive(Debug, Clone)]
struct Stage3Result {
    /// `Some(parsed)` on parse success; `None` if Phi-4's output failed schema parse.
    response: Option<Stage3Response>,
    raw_json: String,
    word_count: usize,
    latency: Duration,
    /// `Some(true|false)` for contradiction queries (only computed if parse OK);
    /// `None` for non-contradiction queries OR when parse failed.
    structural_assertion_passed: Option<bool>,
    structural_detail: Option<String>,
    /// `Some(msg)` if parse failed — partner review still sees raw_json verbatim.
    parse_error: Option<String>,
}

#[derive(Debug, Clone)]
struct QueryResult {
    query: QueryEntry,
    #[allow(dead_code)]
    candidate_fixture_ids: Vec<String>,
    #[allow(dead_code)]
    candidate_scores: Vec<f32>,
    retrieval_latency: Duration,
    variant_a: VariantResult,
    variant_b: VariantResult,
    stage3_from_a: Stage3Result,
    stage3_from_b: Stage3Result,
}

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    let sep_wide = "=".repeat(120);
    println!("{sep_wide}");
    println!("T0.2.x read-time viability spike — bge-small retrieval + Phi-4-mini stage-2/3");
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Host:    {}", std::env::consts::OS);
    println!("{sep_wide}");

    // ── Setup: storage + BGE + Phi-4 ─────────────────────────────────────
    let dir = tempfile::tempdir().context("tempdir")?;
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

    println!("Opening Phi4MiniProvider (cache-resolved against %APPDATA%)...");
    let phi4_start = Instant::now();
    let phi4_config = Phi4MiniConfig::v0_2_default(models_dir()?);
    let phi4 = Phi4MiniProvider::new(phi4_config)
        .await
        .context("Phi4MiniProvider::new")?;
    println!(
        "Phi-4 ready in {:.1}s — model_id={}",
        phi4_start.elapsed().as_secs_f64(),
        phi4.model_id()
    );

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
    println!("Loaded {} memories from fixture", memory_fixture.len());

    let query_fixture_path = vault_retrieval_root()
        .join("test-fixtures")
        .join("merge_acceptance_100_queries.json");
    let query_set: QuerySet = {
        let bytes = std::fs::read(&query_fixture_path)
            .with_context(|| format!("read query fixture {query_fixture_path:?}"))?;
        serde_json::from_slice(&bytes).context("parse query fixture JSON")?
    };
    let target_queries: Vec<QueryEntry> = TARGET_QUERY_IDS
        .iter()
        .map(|wanted| {
            query_set
                .queries
                .iter()
                .find(|q| q.id == *wanted)
                .cloned()
                .with_context(|| format!("target query {wanted} missing from fixture"))
        })
        .collect::<Result<Vec<_>>>()?;
    println!(
        "Loaded {} queries; filtered to {} target queries: {:?}",
        query_set.queries.len(),
        target_queries.len(),
        TARGET_QUERY_IDS
    );

    // ── Insert memories ──────────────────────────────────────────────────
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

    // ── Per-query: retrieve + stage-2 (both variants) + stage-3 (both inputs) ──
    println!("\n{sep_wide}");
    println!("Running 8 target queries × (stage-2 Variant A + Variant B + stage-3 × 2)...");
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

        // -- Retrieval --
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
        let hits = retriever.retrieve(rq).await.context("retrieve")?;
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
        let candidate_scores: Vec<f32> = hits.iter().map(|h| h.score).collect();
        println!(
            "   retrieved {} candidates in {:.0}ms",
            hits.len(),
            retrieval_latency.as_secs_f64() * 1000.0
        );

        let expected_set: HashSet<String> = query.expected_memory_ids.iter().cloned().collect();
        let is_hard_negative = HARD_NEGATIVE_QUERY_IDS.contains(&query.id.as_str());

        // -- Stage-2 Variant A: single call --
        let variant_a = run_variant_a(
            &phi4,
            query,
            &hits,
            &candidate_fixture_ids,
            &expected_set,
            is_hard_negative,
        )
        .await
        .with_context(|| format!("Variant A on {}", query.id))?;
        println!(
            "   V-A: filtered {} / {} candidates · prec={:.2} rec={:.2} HN-reject={} · {:.1}s",
            variant_a.filtered_fixture_ids.len(),
            hits.len(),
            variant_a.precision,
            variant_a.recall,
            variant_a.hard_negative_rejected,
            variant_a.latency.as_secs_f64()
        );

        // -- Stage-2 Variant B: per-candidate (K calls) --
        let variant_b = run_variant_b(
            &phi4,
            query,
            &hits,
            &candidate_fixture_ids,
            &expected_set,
            is_hard_negative,
        )
        .await
        .with_context(|| format!("Variant B on {}", query.id))?;
        println!(
            "   V-B: filtered {} / {} candidates · prec={:.2} rec={:.2} HN-reject={} · {:.1}s ({} calls)",
            variant_b.filtered_fixture_ids.len(),
            hits.len(),
            variant_b.precision,
            variant_b.recall,
            variant_b.hard_negative_rejected,
            variant_b.latency.as_secs_f64(),
            variant_b.n_calls
        );

        // -- Stage-3 from Variant A filter --
        let stage3_from_a = run_stage3(
            &phi4,
            query,
            &variant_a.filtered_fixture_ids,
            &candidate_fixture_ids,
            &fixture_id_to_content,
        )
        .await
        .with_context(|| format!("Stage-3 from V-A on {}", query.id))?;
        let s3a_summary = stage3_from_a
            .response
            .as_ref()
            .map(|r| {
                format!(
                    "contradictions={} · vault_empty={}",
                    r.contradictions_flagged.len(),
                    r.vault_has_no_relevant_content
                )
            })
            .unwrap_or_else(|| "PARSE_FAILED".to_string());
        println!(
            "   S3-A: {} words · {} · structural={} · {:.1}s",
            stage3_from_a.word_count,
            s3a_summary,
            stage3_from_a
                .structural_assertion_passed
                .map_or("N/A".to_string(), |p| if p {
                    "PASS".into()
                } else {
                    "FAIL".into()
                }),
            stage3_from_a.latency.as_secs_f64()
        );

        // -- Stage-3 from Variant B filter --
        let stage3_from_b = run_stage3(
            &phi4,
            query,
            &variant_b.filtered_fixture_ids,
            &candidate_fixture_ids,
            &fixture_id_to_content,
        )
        .await
        .with_context(|| format!("Stage-3 from V-B on {}", query.id))?;
        let s3b_summary = stage3_from_b
            .response
            .as_ref()
            .map(|r| {
                format!(
                    "contradictions={} · vault_empty={}",
                    r.contradictions_flagged.len(),
                    r.vault_has_no_relevant_content
                )
            })
            .unwrap_or_else(|| "PARSE_FAILED".to_string());
        println!(
            "   S3-B: {} words · {} · structural={} · {:.1}s",
            stage3_from_b.word_count,
            s3b_summary,
            stage3_from_b
                .structural_assertion_passed
                .map_or("N/A".to_string(), |p| if p {
                    "PASS".into()
                } else {
                    "FAIL".into()
                }),
            stage3_from_b.latency.as_secs_f64()
        );

        results.push(QueryResult {
            query: query.clone(),
            candidate_fixture_ids,
            candidate_scores,
            retrieval_latency,
            variant_a,
            variant_b,
            stage3_from_a,
            stage3_from_b,
        });
        println!();
    }

    // ── Aggregate latency stats ──────────────────────────────────────────
    print_latency_summary(&results);

    // ── Write markdown writeup ───────────────────────────────────────────
    let md_path = vault_retrieval_root()
        .join("examples")
        .join("t024_readtime_viability_results.md");
    let md = build_markdown_report(&results, &run_started, memory_fixture.len(), insert_secs);
    std::fs::write(&md_path, md).context("write markdown report")?;
    println!("\nMarkdown writeup: {}", md_path.display());
    println!(
        "Run completed: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );

    Ok(())
}

// ── Stage-2 Variant A (single call) ──────────────────────────────────────

async fn run_variant_a(
    phi4: &Phi4MiniProvider,
    query: &QueryEntry,
    candidates: &[RetrievedMemory],
    candidate_fixture_ids: &[String],
    expected_set: &HashSet<String>,
    is_hard_negative: bool,
) -> Result<VariantResult> {
    let mut user_prompt = String::new();
    user_prompt.push_str(&format!("QUERY: {}\n\nCANDIDATES:\n", query.query_text));
    for (i, c) in candidates.iter().enumerate() {
        let fid = &candidate_fixture_ids[i];
        user_prompt.push_str(&format!("[{}] {}\n", fid, c.memory.content));
    }
    user_prompt.push_str("\nDecide which candidates are relevant. Return JSON.");

    let params = CompletionParams {
        max_tokens: 768,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(VARIANT_A_SYSTEM_PROMPT.to_string()),
    };

    let start = Instant::now();
    let raw = phi4
        .complete_json(&user_prompt, VARIANT_A_SCHEMA, &params)
        .await
        .context("Variant A complete_json")?;
    let latency = start.elapsed();

    let parsed_result: Result<VariantAResponse, _> = serde_json::from_str(&raw);
    match parsed_result {
        Ok(parsed) => {
            let valid_ids: HashSet<String> = candidate_fixture_ids.iter().cloned().collect();
            let filtered: Vec<String> = parsed
                .relevant_ids
                .iter()
                .filter(|id| valid_ids.contains(*id))
                .cloned()
                .collect();
            let (precision, recall) = compute_pr(&filtered, expected_set);
            let hn_rejected = is_hard_negative && filtered.is_empty();
            Ok(VariantResult {
                filtered_fixture_ids: filtered,
                per_candidate_decisions: Vec::new(),
                reasoning: parsed.reasoning,
                latency,
                n_calls: 1,
                precision,
                recall,
                hard_negative_rejected: hn_rejected,
                parse_errors: Vec::new(),
                raw_failed_outputs: Vec::new(),
            })
        }
        Err(e) => {
            // Phi-4 produced output that doesn't parse — typically a degeneracy loop
            // truncated at max_tokens (Q22 V-A pattern from the 2026-05-14 crash).
            // Record the failure as data; treat downstream as empty filter
            // (conservative — Phi-4 didn't successfully recommend anything).
            let err_msg = format!("Variant A JSON parse failed: {e}");
            let (precision, recall) = compute_pr(&[], expected_set);
            // hn_rejected is TRUE iff filter is empty on a HN query — parse-failure
            // empty filter is technically a "rejected all" but for diagnostic clarity
            // we keep it false because the rejection wasn't intentional.
            Ok(VariantResult {
                filtered_fixture_ids: Vec::new(),
                per_candidate_decisions: Vec::new(),
                reasoning: format!("PARSE_ERROR: {err_msg}"),
                latency,
                n_calls: 1,
                precision,
                recall,
                hard_negative_rejected: false,
                parse_errors: vec![err_msg],
                raw_failed_outputs: vec![raw],
            })
        }
    }
}

// ── Stage-2 Variant B (per-candidate, K calls) ───────────────────────────

async fn run_variant_b(
    phi4: &Phi4MiniProvider,
    query: &QueryEntry,
    candidates: &[RetrievedMemory],
    candidate_fixture_ids: &[String],
    expected_set: &HashSet<String>,
    is_hard_negative: bool,
) -> Result<VariantResult> {
    let mut filtered: Vec<String> = Vec::new();
    let mut decisions: Vec<(String, bool, String)> = Vec::with_capacity(candidates.len());
    let mut parse_errors: Vec<String> = Vec::new();
    let mut raw_failed: Vec<String> = Vec::new();
    let total_start = Instant::now();

    for (i, c) in candidates.iter().enumerate() {
        let fid = &candidate_fixture_ids[i];
        let user_prompt = format!(
            "QUERY: {}\n\nCANDIDATE: {}\n\nIs this candidate relevant to the query? Return JSON.",
            query.query_text, c.memory.content
        );
        let params = CompletionParams {
            max_tokens: 256,
            temperature: 0.0,
            top_p: 1.0,
            seed: Some(42),
            system_prompt: Some(VARIANT_B_SYSTEM_PROMPT.to_string()),
        };
        let raw = phi4
            .complete_json(&user_prompt, VARIANT_B_SCHEMA, &params)
            .await
            .with_context(|| format!("Variant B complete_json on candidate {fid}"))?;
        let parsed_result: Result<VariantBResponse, _> = serde_json::from_str(&raw);
        match parsed_result {
            Ok(parsed) => {
                if parsed.relevant {
                    filtered.push(fid.clone());
                }
                decisions.push((fid.clone(), parsed.relevant, parsed.reasoning));
            }
            Err(e) => {
                // Conservative default: parse failure → treat as relevant=false
                // (don't promote a candidate we couldn't get a decision on).
                parse_errors.push(format!("candidate {fid}: {e}"));
                raw_failed.push(format!("[{fid}] {raw}"));
                decisions.push((fid.clone(), false, format!("PARSE_ERROR: {e}")));
            }
        }
    }

    let latency = total_start.elapsed();
    let (precision, recall) = compute_pr(&filtered, expected_set);
    let hn_rejected = is_hard_negative && filtered.is_empty();

    Ok(VariantResult {
        filtered_fixture_ids: filtered,
        per_candidate_decisions: decisions,
        reasoning: String::new(),
        latency,
        n_calls: candidates.len(),
        precision,
        recall,
        hard_negative_rejected: hn_rejected,
        parse_errors,
        raw_failed_outputs: raw_failed,
    })
}

// ── Stage-3 (synthesis) ──────────────────────────────────────────────────

async fn run_stage3(
    phi4: &Phi4MiniProvider,
    query: &QueryEntry,
    filtered_fixture_ids: &[String],
    candidate_fixture_ids: &[String],
    fixture_id_to_content: &HashMap<String, String>,
) -> Result<Stage3Result> {
    // Preserve BGE-cosine ordering: walk the candidate list in retrieval
    // order, keep only those in the filtered set. Per spike scope —
    // Phi-4-driven re-rank order is a separate measurement deferred.
    let filtered_set: HashSet<&str> = filtered_fixture_ids.iter().map(String::as_str).collect();
    let ordered: Vec<&String> = candidate_fixture_ids
        .iter()
        .filter(|fid| filtered_set.contains(fid.as_str()))
        .collect();

    let mut user_prompt = String::new();
    user_prompt.push_str(&format!(
        "QUERY: {}\n\nFILTERED MEMORIES (relevance-gated):\n",
        query.query_text
    ));
    if ordered.is_empty() {
        user_prompt.push_str("(empty — relevance gate rejected all candidates)\n");
    } else {
        for fid in &ordered {
            let content = fixture_id_to_content
                .get(fid.as_str())
                .map_or("<missing>", String::as_str);
            user_prompt.push_str(&format!("[{fid}] {content}\n"));
        }
    }
    user_prompt.push_str("\nSynthesize. Return JSON.");

    let params = CompletionParams {
        max_tokens: 1024,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(STAGE3_SYSTEM_PROMPT.to_string()),
    };

    let start = Instant::now();
    let raw = phi4
        .complete_json(&user_prompt, STAGE3_SCHEMA, &params)
        .await
        .context("Stage-3 complete_json")?;
    let latency = start.elapsed();

    let parsed_result: Result<Stage3Response, _> = serde_json::from_str(&raw);
    match parsed_result {
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
                        "contradictions_flagged.len()={} · synthesis contains '{}'={} AND '{}'={}",
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
            Ok(Stage3Result {
                response: Some(parsed),
                raw_json: raw,
                word_count,
                latency,
                structural_assertion_passed: structural_passed,
                structural_detail,
                parse_error: None,
            })
        }
        Err(e) => {
            let err_msg = format!("Stage-3 JSON parse failed: {e}");
            Ok(Stage3Result {
                response: None,
                raw_json: raw,
                word_count: 0,
                latency,
                structural_assertion_passed: None,
                structural_detail: None,
                parse_error: Some(err_msg),
            })
        }
    }
}

// ── Metrics ──────────────────────────────────────────────────────────────

fn compute_pr(filtered: &[String], expected: &HashSet<String>) -> (f64, f64) {
    if filtered.is_empty() {
        let precision = 1.0; // vacuously: of zero things returned, all were correct
        let recall = if expected.is_empty() { 1.0 } else { 0.0 };
        return (precision, recall);
    }
    let filtered_set: HashSet<&str> = filtered.iter().map(String::as_str).collect();
    let true_positives = expected
        .iter()
        .filter(|fid| filtered_set.contains(fid.as_str()))
        .count();
    let precision = if expected.is_empty() {
        // Hard-negative case: any prediction is a false positive
        0.0
    } else {
        true_positives as f64 / filtered.len() as f64
    };
    let recall = if expected.is_empty() {
        // Hard-negative: recall is "did we correctly return nothing?"
        // Filtered is non-empty here, so recall is N/A; report 0.
        0.0
    } else {
        true_positives as f64 / expected.len() as f64
    };
    (precision, recall)
}

// ── Latency aggregation ──────────────────────────────────────────────────

fn print_latency_summary(results: &[QueryResult]) {
    let sep = "=".repeat(120);
    println!("\n{sep}");
    println!("LATENCY SUMMARY");
    println!("{sep}");
    let v_a_lats: Vec<f64> = results
        .iter()
        .map(|r| r.variant_a.latency.as_secs_f64())
        .collect();
    let v_b_lats: Vec<f64> = results
        .iter()
        .map(|r| r.variant_b.latency.as_secs_f64())
        .collect();
    let s3_a_lats: Vec<f64> = results
        .iter()
        .map(|r| r.stage3_from_a.latency.as_secs_f64())
        .collect();
    let s3_b_lats: Vec<f64> = results
        .iter()
        .map(|r| r.stage3_from_b.latency.as_secs_f64())
        .collect();

    println!("{:<28} | min   p50   p99   max   mean", "stage");
    println!("{}", "-".repeat(80));
    print_latency_row("Variant A (1 call)", &v_a_lats);
    print_latency_row("Variant B (K=20 calls total)", &v_b_lats);
    print_latency_row("Stage-3 from V-A", &s3_a_lats);
    print_latency_row("Stage-3 from V-B", &s3_b_lats);

    let end_to_end_a: Vec<f64> = results
        .iter()
        .map(|r| {
            r.retrieval_latency.as_secs_f64()
                + r.variant_a.latency.as_secs_f64()
                + r.stage3_from_a.latency.as_secs_f64()
        })
        .collect();
    let end_to_end_b: Vec<f64> = results
        .iter()
        .map(|r| {
            r.retrieval_latency.as_secs_f64()
                + r.variant_b.latency.as_secs_f64()
                + r.stage3_from_b.latency.as_secs_f64()
        })
        .collect();
    print_latency_row("End-to-end (retrieve+V-A+S3)", &end_to_end_a);
    print_latency_row("End-to-end (retrieve+V-B+S3)", &end_to_end_b);
}

fn print_latency_row(label: &str, samples: &[f64]) {
    let stats = latency_stats(samples);
    println!(
        "{:<28} | {:>5.1} {:>5.1} {:>5.1} {:>5.1} {:>5.1}",
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
    s.push_str("# T0.2.x Read-Time Viability Spike — Results\n\n");
    s.push_str(&format!(
        "**Run started:** {}  \n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    s.push_str(&format!("**Host OS:** {}  \n", std::env::consts::OS));
    s.push_str("**Embedding model:** bge-small-en-v1.5 (384-dim, BgeSmallProvider)  \n");
    s.push_str("**LLM:** Phi-4-mini-instruct (Q4_K_M GGUF, ~2.49 GB, Phi4MiniProvider)  \n");
    s.push_str("**Retriever:** SemanticRetriever, `RetrievalOptions::default()`, K=20  \n");
    s.push_str("**Phi-4 CompletionParams:** temperature=0.0, top_p=1.0, seed=42 (deterministic-greedy under GBNF)  \n");
    s.push_str(&format!(
        "**Fixture:** {} memories from `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json`  \n",
        n_memories
    ));
    s.push_str(&format!(
        "**Setup wall time:** {:.1}s memory insertion ({:.0} ms/memory)  \n\n",
        insert_secs,
        (insert_secs * 1000.0) / n_memories as f64
    ));

    s.push_str("> **Discipline note.** This document reports measurements only. Manual review of synthesis outputs ");
    s.push_str("is pending — it happens by partner reading the verbatim `synthesis_markdown` blocks below + grading ");
    s.push_str("each against the locked criteria. No architecture call, no scenario verdict, no ADR draft, no plan ");
    s.push_str("iteration in this file.\n\n");

    s.push_str("## BRD §5.5 alignment notes\n\n");
    s.push_str("- §5.5 line 869 (200ms latency property test): applies to `Retriever::retrieve`. The new read-pipeline layer above has its own latency budget; the 200ms contract is NOT inherited.\n");
    s.push_str("- §5.5 line 856 (V1 classifier is heuristic, NOT LLM): the new Phi-4 stage is DOWNSTREAM of strategy execution + reranker, not in the classifier role. ADR documentation at architectural-lock.\n");
    s.push_str("- §5.5 lines 846-852 + 873-883 (multi-strategy future): the new stage layers above `Retriever::retrieve` regardless of implementer — works with V0.1 `SemanticRetriever` today, with future `MultiStrategyRetriever`.\n");
    s.push_str("- §5.5 lines 823-829 (RetrievalQuery shape): BRD says `boundary: Option<Boundary>`; current code says `authorized_boundaries: Vec<Boundary>`. Pre-existing divergence, ADR-024 documented; spike uses the current shape.\n\n");

    s.push_str("## What this spike is NOT measuring\n\n");
    s.push_str("- Production-scale corpus (100 memories; scale concerns deferred to separate measurement)\n");
    s.push_str("- MCP wire-format integration (separate test surface at plan iteration 2)\n");
    s.push_str(
        "- Multi-strategy retrieval (semantic-only today; T0.2.7 lands `MultiStrategyRetriever`)\n",
    );
    s.push_str("- Stage-3 input ordering — BGE-cosine-rank per spike scope; Phi-4-driven re-rank order is a separate measurement deferred until basic synthesis viability lands.\n");
    s.push_str("- The 10 staged T0.2.3 commit-3 files — preserved in working tree, ride the architectural-close commit.\n\n");

    s.push_str("---\n\n");

    s.push_str("## Stage-2 results — Variant A vs Variant B per-query\n\n");
    s.push_str("| Query | Shape | Expected | V-A filtered (n) | V-A prec | V-A rec | V-A HN-rej | V-A latency | V-B filtered (n) | V-B prec | V-B rec | V-B HN-rej | V-B latency |\n");
    s.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for r in results {
        s.push_str(&format!(
            "| {} | {} | {} | {} | {:.2} | {:.2} | {} | {:.1}s | {} | {:.2} | {:.2} | {} | {:.1}s |\n",
            r.query.id,
            r.query.shape,
            r.query.expected_memory_ids.len(),
            r.variant_a.filtered_fixture_ids.len(),
            r.variant_a.precision,
            r.variant_a.recall,
            yes_no_na(r.variant_a.hard_negative_rejected, r.query.id.as_str()),
            r.variant_a.latency.as_secs_f64(),
            r.variant_b.filtered_fixture_ids.len(),
            r.variant_b.precision,
            r.variant_b.recall,
            yes_no_na(r.variant_b.hard_negative_rejected, r.query.id.as_str()),
            r.variant_b.latency.as_secs_f64(),
        ));
    }
    s.push('\n');

    s.push_str("## Stage-2 — variant filter detail (which fixture IDs each variant kept)\n\n");
    for r in results {
        s.push_str(&format!("### {}\n\n", r.query.id));
        s.push_str(&format!(
            "**Expected** ({}): `{}`  \n",
            r.query.expected_memory_ids.len(),
            if r.query.expected_memory_ids.is_empty() {
                "(empty — hard-negative)".to_string()
            } else {
                r.query.expected_memory_ids.join("`, `")
            }
        ));
        s.push_str(&format!(
            "**V-A filtered** ({}): `{}`  \n",
            r.variant_a.filtered_fixture_ids.len(),
            if r.variant_a.filtered_fixture_ids.is_empty() {
                "(empty)".to_string()
            } else {
                r.variant_a.filtered_fixture_ids.join("`, `")
            }
        ));
        s.push_str(&format!(
            "**V-A reasoning:** {}  \n",
            escape_md(&r.variant_a.reasoning)
        ));
        if !r.variant_a.parse_errors.is_empty() {
            s.push_str(&format!(
                "**V-A PARSE_ERRORS** ({}): {}  \n",
                r.variant_a.parse_errors.len(),
                escape_md(&r.variant_a.parse_errors.join(" | "))
            ));
            for raw in &r.variant_a.raw_failed_outputs {
                s.push_str("\n<details><summary>V-A raw failed output</summary>\n\n```\n");
                s.push_str(raw);
                s.push_str("\n```\n</details>\n\n");
            }
        }
        s.push_str(&format!(
            "**V-B filtered** ({}): `{}`  \n",
            r.variant_b.filtered_fixture_ids.len(),
            if r.variant_b.filtered_fixture_ids.is_empty() {
                "(empty)".to_string()
            } else {
                r.variant_b.filtered_fixture_ids.join("`, `")
            }
        ));
        if !r.variant_b.parse_errors.is_empty() {
            s.push_str(&format!(
                "**V-B PARSE_ERRORS** ({}): {}  \n",
                r.variant_b.parse_errors.len(),
                escape_md(&r.variant_b.parse_errors.join(" | "))
            ));
        }
        s.push('\n');
    }

    s.push_str("---\n\n## Stage-3 results — synthesis outputs\n\n");
    s.push_str("Per-query, two synthesis outputs are presented (from each variant's filtered set). Structural ");
    s.push_str("assertions appear inline for contradiction queries. **Manual review grades pending — append ");
    s.push_str("after partner reads the verbatim outputs.**\n\n");

    for r in results {
        s.push_str(&format!(
            "### {} — \"{}\"\n\n",
            r.query.id, r.query.query_text
        ));
        s.push_str(&format!(
            "**Shape:** {} · **Notes:** {}\n\n",
            r.query.shape, r.query.notes
        ));

        // Stage-3 from Variant A
        s.push_str("#### Stage-3 (from V-A filtered set)\n\n");
        write_stage3_detail(&mut s, &r.stage3_from_a);

        // Stage-3 from Variant B
        s.push_str("#### Stage-3 (from V-B filtered set)\n\n");
        write_stage3_detail(&mut s, &r.stage3_from_b);

        s.push_str("---\n\n");
    }

    // Latency summary
    s.push_str("## Latency summary\n\n");
    s.push_str("| Stage | min | p50 | p99 | max | mean |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    let v_a: Vec<f64> = results
        .iter()
        .map(|r| r.variant_a.latency.as_secs_f64())
        .collect();
    let v_b: Vec<f64> = results
        .iter()
        .map(|r| r.variant_b.latency.as_secs_f64())
        .collect();
    let s3_a: Vec<f64> = results
        .iter()
        .map(|r| r.stage3_from_a.latency.as_secs_f64())
        .collect();
    let s3_b: Vec<f64> = results
        .iter()
        .map(|r| r.stage3_from_b.latency.as_secs_f64())
        .collect();
    let e2e_a: Vec<f64> = results
        .iter()
        .map(|r| {
            r.retrieval_latency.as_secs_f64()
                + r.variant_a.latency.as_secs_f64()
                + r.stage3_from_a.latency.as_secs_f64()
        })
        .collect();
    let e2e_b: Vec<f64> = results
        .iter()
        .map(|r| {
            r.retrieval_latency.as_secs_f64()
                + r.variant_b.latency.as_secs_f64()
                + r.stage3_from_b.latency.as_secs_f64()
        })
        .collect();
    push_latency_md_row(&mut s, "Variant A (1 call)", &v_a);
    push_latency_md_row(&mut s, "Variant B (K=20 calls total)", &v_b);
    push_latency_md_row(&mut s, "Stage-3 from V-A", &s3_a);
    push_latency_md_row(&mut s, "Stage-3 from V-B", &s3_b);
    push_latency_md_row(&mut s, "End-to-end (retrieve + V-A + S3)", &e2e_a);
    push_latency_md_row(&mut s, "End-to-end (retrieve + V-B + S3)", &e2e_b);
    s.push('\n');

    s.push_str("---\n\n## Architectural decision — DEFERRED\n\n");
    s.push_str("Data only. The architectural call (whether Phi-4-mini delivers the differentiator on our content; ");
    s.push_str("which variant to ship; whether stage-3 viability passes the partner's manual review; whether latency ");
    s.push_str("forces an architecture fork) lives in a separate conversation between partner + Claude once these ");
    s.push_str("measurements are read.\n");

    s
}

fn write_stage3_detail(s: &mut String, r: &Stage3Result) {
    if let Some(err) = &r.parse_error {
        s.push_str(&format!(
            "- **PARSE FAILURE:** {} · **latency:** {:.1}s · raw output length: {} chars\n",
            err,
            r.latency.as_secs_f64(),
            r.raw_json.len()
        ));
        s.push_str("\n**Phi-4 raw output (verbatim, parse failed):**\n\n");
        s.push_str("```\n");
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
            s.push_str("- **(unknown state — no response and no parse error; this is a bug)**\n\n");
            return;
        }
    };
    s.push_str(&format!(
        "- **word_count:** {} · **latency:** {:.1}s · **contradictions_flagged.len():** {} · **vault_has_no_relevant_content:** {}\n",
        r.word_count,
        r.latency.as_secs_f64(),
        response.contradictions_flagged.len(),
        response.vault_has_no_relevant_content
    ));
    if let (Some(passed), Some(detail)) = (r.structural_assertion_passed, &r.structural_detail) {
        s.push_str(&format!(
            "- **structural assertion:** {} — {}\n",
            if passed { "PASS" } else { "FAIL" },
            detail
        ));
    }
    s.push_str("\n**synthesis_markdown (verbatim):**\n\n");
    s.push_str("```\n");
    s.push_str(&response.synthesis_markdown);
    if !response.synthesis_markdown.ends_with('\n') {
        s.push('\n');
    }
    s.push_str("```\n\n");
    if !response.contradictions_flagged.is_empty() {
        s.push_str("**contradictions_flagged (verbatim JSON):**\n\n");
        s.push_str("```json\n");
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
    s.push_str(&format!(
        "<details><summary>raw stage-3 JSON</summary>\n\n```json\n{}\n```\n</details>\n\n",
        escape_md(&r.raw_json)
    ));
}

fn push_latency_md_row(s: &mut String, label: &str, samples: &[f64]) {
    let stats = latency_stats(samples);
    s.push_str(&format!(
        "| {} | {:.1}s | {:.1}s | {:.1}s | {:.1}s | {:.1}s |\n",
        label, stats.min, stats.p50, stats.p99, stats.max, stats.mean
    ));
}

fn yes_no_na(value: bool, query_id: &str) -> String {
    if HARD_NEGATIVE_QUERY_IDS.contains(&query_id) {
        if value {
            "yes".to_string()
        } else {
            "NO".to_string()
        }
    } else {
        "N/A".to_string()
    }
}

fn escape_md(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

// ── BGE provider + Phi-4 cache path ──────────────────────────────────────

fn open_bge_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let fixture_root = vault_embedding_test_fixtures()?;
    let model = fixture_root.join("model.onnx");
    let tokenizer = fixture_root.join("tokenizer.json");
    let ort_lib = fixture_root.join(ort_lib_name());
    for p in [&model, &tokenizer, &ort_lib] {
        ensure!(
            p.exists(),
            "missing BGE fixture {p:?} — run scripts/setup-dev-env.ps1"
        );
    }
    let provider =
        BgeSmallProvider::open(&model, &tokenizer, &ort_lib).context("BgeSmallProvider::open")?;
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
        .context("vault-retrieval has no grandparent (repo root)")
}

fn vault_embedding_test_fixtures() -> Result<PathBuf> {
    let p = repo_root()?
        .join("crates")
        .join("vault-embedding")
        .join("test-fixtures")
        .join("bge-small-en-v1.5");
    ensure!(p.exists(), "bge-small-en-v1.5 fixtures missing at {p:?}");
    Ok(p)
}

/// Production cache location per ADR-043 / phi4_load_and_json_spike.rs convention.
/// Spike is Windows-only locked (matches t023 + vault-llm spike pattern).
fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA")
        .context("APPDATA env var must be set on Windows for Phi-4 cache resolution")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}
