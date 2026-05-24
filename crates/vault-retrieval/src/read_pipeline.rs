//! Read-time pipeline — the V0.2 production read path for AI agent
//! consumption.
//!
//! See **ADR-048** (T0.2.3 close, 2026-05-15) for the architectural lock:
//! retrieval IS the product surface for agent-shaped workloads;
//! consolidation is housekeeping. This module is the load-bearing read
//! contract.
//!
//! # Two-stage pipeline
//!
//! 1. **Stage 1 — Semantic retrieval top-N** via the existing
//!    [`Retriever`] trait (V0.2 ships [`SemanticRetriever`] as the only
//!    implementer; T0.2.7 will add a multi-strategy implementer
//!    additively without changing the [`ReadPipeline`] surface).
//! 2. **Stage 2 — Single Qwen-class LLM synthesis call** via the
//!    `vault_llm::LlmProvider` trait. The pipeline builds one prompt
//!    that asks the model to (a) filter to actually-relevant candidates,
//!    (b) flag contradictions across them, (c) write a coherent narrative
//!    with inline citations to memory IDs — all in one pass under a GBNF
//!    grammar constraining the output to [`READ_TIME_JSON_SCHEMA`].
//!
//! No Phi-4 stage 2/2.5 split (the t025 spike showed splitting hurts
//! both quality and latency vs Qwen-7B alone). No multi-call orchestration.
//!
//! # Quality contract
//!
//! Pinned by the t026 8-query gauntlet: **4/4 contradictions surfaced +
//! 2/2 hard-negatives correctly rejected** on the
//! `merge_acceptance_100_queries.json` acceptance fixture. The integration
//! test at `tests/read_pipeline_acceptance.rs` exercises the production
//! pipeline against the locked Qwen-7B model + locked `TuningConfig`
//! (cron-gated `#[ignore]` per the t026 heavy-test pattern; runs in the
//! local-spike harness and on cron CI runs).
//!
//! # Latency budget
//!
//! Read-time synthesis stage has its OWN budget (NOT BRD §5.5 line 869's
//! 200ms retriever contract — that applies only to [`Retriever::retrieve`]
//! at stage 1). Empirical anchor on i7-13620H + Intel UHD Graphics +
//! Windows 11 with Vulkan iGPU offload: **mean 86.0s · p99 119.7s**
//! against the t026 8-query gauntlet — see
//! `examples/t027b_qwen_7b_vulkan_results.md` for the canonical run.
//!
//! # Producer-side configuration
//!
//! The pipeline takes any [`Retriever`] + any `vault_llm::LlmProvider` at
//! construction. Production wires:
//! - [`SemanticRetriever`] as the retriever
//! - `Qwen25_14BProvider::open_with_tuning(path, TuningConfig { n_threads:
//!   Some(12), n_threads_batch: Some(12), n_gpu_layers: Some(99), .. })`
//!   as the LLM (see the **V0.2 backend + tuning config locked** section
//!   in HANDOFF.md for the locked literal).
//!
//! Tests wire `MockLlmProvider` + a test-local mock `Retriever` to exercise
//! pipeline wiring without loading the 4.36 GB GGUF — see this module's
//! `#[cfg(test)] mod tests` block.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use vault_core::{Boundary, VaultError, VaultResult};
use vault_llm::{CompletionParams, LlmProvider};

use crate::retriever::{
    RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever, MAX_RESULTS_CAP,
};

/// Default top-N retrieved candidates fed to the synthesis stage. Per
/// ADR-048 / t026 fixture: 20 candidates is the locked V0.2 default.
/// Callers can override via [`ReadPipeline::with_max_candidates`].
pub const DEFAULT_MAX_CANDIDATES: usize = 20;

/// JSON schema for the read-time synthesis output. GBNF-compiled by the
/// underlying llama.cpp backend, so the model is guaranteed to emit
/// structurally-valid JSON matching this shape.
///
/// Required fields: `synthesis_markdown` (the narrative answer),
/// `contradictions_flagged` (zero or more contradiction records, each
/// naming the memory IDs and conflicting positions), and
/// `vault_has_no_relevant_content` (boolean — true if the model determined
/// no candidate is relevant, which is the correct hard-negative behaviour).
pub const READ_TIME_JSON_SCHEMA: &str = r#"{
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

/// Canonical V0.2 read-time system prompt. **v10** (T0.2.7 Phase 5 Step 2,
/// 2026-05-23 — companion to the rank-indexed prompt format established
/// by [`build_user_prompt`]). Supersedes v9 with two changes only:
/// the Comcast example references `{memory_ids: ["3", "7"], ...}`
/// (rank strings) instead of `{memory_ids: [M1, M2], ...}`, and the
/// OUTPUT section explicitly teaches the LLM that candidates are
/// 1-indexed and that the rank strings are what go in
/// `contradictions_flagged.memory_ids`. All other v9 instructions —
/// RELEVANCE, CONTRADICTIONS (VERBATIM RULE + TEMPORAL VALUE CHANGES),
/// NARRATIVE COMPLIANCE anti-pattern, TASK-SHAPED QUERIES — are kept
/// verbatim from v9.
///
/// **Why v10 supersedes v9 (and v9 superseded the earlier t026 prompt).**
/// v9's validation chain at SCALE=10K (diverse corpus, 9-query gauntlet,
/// v3a/v3b/v3c spike) closed the worst prose-elision patterns. v10
/// targets a different layer entirely — input determinism. The T0.2.7
/// Phase 5 Step 2 t030 byte-equality probe (2026-05-23) showed that
/// retrieval is deterministic both in-process and across-process, but
/// embedding random per-process UUIDv7 memory IDs in the prompt added
/// ~300 BPE tokens of input randomness per query — enough to flip
/// Qwen-7B's Q25 verdict at SCALE=1000 across runs even at
/// temperature=0 with seed=42. v10 plus the rank-indexed
/// [`build_user_prompt`] make the prompt a pure function of
/// `(query, ordered candidate content)`.
///
/// **Quality contract context.** Per the 2026-05-20 verdict refinement
/// ([[structured-contract-user-sees-via-agent]]), `contradictions_flagged`
/// is the production-load-bearing field — the agent consumes it
/// structurally. `synthesis_markdown` is convenience prose. v9/v10
/// tighten the prose contract but the structured-field-is-contract
/// policy makes the prose-substring gap non-blocking even if the prompt
/// doesn't fully close it.
///
/// Callers may override via [`ReadPipeline::with_system_prompt`] for
/// per-tenant customisation, but the default is the production text
/// validated against the T0.2.7 spike's 10K-corpus 9-query gauntlet.
pub const READ_TIME_SYSTEM_PROMPT: &str = r#"You are the read layer of a personal memory vault used by AI coding agents.

You receive a query and a set of candidate memories retrieved via semantic similarity.
In ONE pass you must: (a) filter to actually-relevant candidates, (b) detect any
contradictions among the filtered set, and (c) produce a coherent synthesis.

RELEVANCE:
- A candidate is relevant ONLY if its content explicitly mentions the subject of the
  query. Topical proximity is NOT relevance.
- Example: a query about "Kubernetes migration" is NOT satisfied by memories about
  database migrations, container tooling in general, or other infrastructure changes.
  The subject is specifically "Kubernetes" — if no candidate uses that word (or a
  direct synonym like "k8s"), the vault has no relevant content for this query.
- When uncertain whether a candidate addresses the query's subject, prefer
  vault_has_no_relevant_content=true over fabricating a relevance link. Conservative
  beats over-confident.

CONTRADICTIONS (load-bearing):
- If two or more memories disagree on a value for the same fact — different numbers,
  dates, amounts, names, choices, quantities — you MUST surface the disagreement.
  This holds even when many memories support one value and only one supports the
  other. Minority evidence is never optional.
- VERBATIM RULE: when you state a contradictory value in synthesis_markdown, copy
  the EXACT text from the source memory, including all modifiers (years, units,
  qualifiers). Do NOT abbreviate, round, or paraphrase. If a memory says
  "Q1 2027", write "Q1 2027" — not "Q1" alone. If a memory says "$89.99/month",
  write "$89.99/month" — not "around $90".
- For EACH contradiction detected you MUST do BOTH:
    (a) Mention BOTH literal values in synthesis_markdown (verbatim, per the rule
        above).
    (b) Add an entry to contradictions_flagged with the participating candidate
        indices (as strings, matching the [N] prefix on each candidate) and the
        conflicting positions (also verbatim).
- TEMPORAL VALUE CHANGES count as contradictions. If one memory says X has value A
  and another memory says X has value B (or "X is now B", or "X increased to B",
  or "B starting next cycle"), both memories disagree about what value X currently
  carries — the older memory implies the answer is A; the newer implies B. You MUST
  flag this in contradictions_flagged using the same dual-field rule above. A
  monthly-review or audit query is asking precisely for these flags; reporting the
  change in synthesis_markdown alone is NOT enough.
- Example: if candidate [3] says "Comcast bill is $89/month" and candidate [7] says
  "Comcast bill is now $109/month starting next cycle", BOTH values disagree about
  the current Comcast cost — populate contradictions_flagged with
  {memory_ids: ["3", "7"], positions: ["$89/month", "$109/month starting next cycle"]}.
- Reporting only the majority value in synthesis_markdown while leaving
  contradictions_flagged empty is a FAILURE. Both fields are required for every
  contradiction.
- NARRATIVE COMPLIANCE (load-bearing anti-pattern): synthesis_markdown is the
  user-facing narrative. It MUST be self-contained — a reader who sees only
  synthesis_markdown (not contradictions_flagged) must see BOTH literal values.
  When the change has a documented reason (renewal, schedule push, status
  update, evolution-with-justification), the temptation is to write only the
  new value in prose and rely on the reader to infer the old one. This is
  WRONG.
- ANTI-PATTERN: "has been moved to Q2 2027", "renewed at $4,200/mo", "now
  costs $109/month", "increased to 109" — any phrasing that mentions the
  NEW value without literally writing the OLD value is non-compliant with
  rule (a) above, even when the prose is otherwise coherent and the
  structured field is correct. Phrases like "the previous agreement", "the
  prior target", "up from before", "originally planned" do NOT satisfy the
  rule — the OLD value must appear as the same literal token sequence as
  in the source memory.
- CORRECT: "The Wi-Fi vendor cost was $2,500/mo. and renewed at $4,200/mo.
  at the 18-month mark (vendor cited square-footage expansion and SLA
  upgrade)." Both literal values are present; the reason is preserved.
- INCORRECT: "The Wi-Fi vendor renewed at $4,200/mo. (up from the previous
  agreement, due to square-footage expansion)." Only the new value is
  literal; the old value is implied — FAILS rule (a).
- CORRECT: "The GA launch target was Q1 2027 but has been moved to Q2 2027
  based on the latest beta-readiness assessment." Both Q1 2027 and Q2 2027
  appear literally.
- INCORRECT: "The GA launch timing has been moved to Q2 2027, based on the
  latest beta-readiness assessment." Only Q2 2027 is literal — FAILS rule
  (a) even though contradictions_flagged correctly lists both positions.

TASK-SHAPED QUERIES:
- Some queries are phrased as agent tasks ("help me update the X doc with the
  latest milestone dates", "doing the monthly Y review — anything I should flag?",
  "putting together Z, what should I include?").
- Ignore the action verb ("help me update", "doing", "putting together"). Focus on
  the NOUN PHRASE — what is the agent asking about? In "help me update the product
  roadmap doc", the noun phrase is "product roadmap" and the agent needs to know
  the current roadmap state and any contradictions.
- Your output is NOT the completed task. Your output is a summary of relevant
  memory content (including any contradictions), which the agent will use to
  complete the task themselves. Do NOT generate boilerplate task text.

OUTPUT:
- Each candidate is prefixed with a 1-indexed bracketed rank like [1], [2], [3] —
  use those rank strings as the values in contradictions_flagged.memory_ids and
  for any inline citations in synthesis_markdown (e.g. "[3] notes Q1 2027 GA").
- Write a coherent narrative in synthesis_markdown.
- If no candidates are relevant: set vault_has_no_relevant_content=true and state
  this in synthesis_markdown. Do NOT fabricate.
- Keep synthesis_markdown under 250 words.
- Return ONLY valid JSON matching the schema."#;

/// User-facing read query. Mirrors [`RetrievalQuery`] in spirit
/// (boundaries are mandatory; empty `Vec` = empty result without an
/// error per BRD §11.4.3) but tuned for the read-pipeline shape.
#[derive(Clone, Debug)]
pub struct ReadQuery {
    /// The raw user / agent question text. Validated the same way
    /// [`RetrievalQuery::query_text`] is (trim, reject empty / control
    /// chars / oversized) when stage 1 runs.
    pub query_text: String,

    /// The set of boundaries the caller is authorised to read from.
    /// Empty `Vec` short-circuits to a "no relevant content" response
    /// without touching the LLM. Never `Option`-al.
    pub authorized_boundaries: Vec<Boundary>,
}

/// One contradiction record from the synthesis stage. Memory IDs and
/// positions are returned as `String` (not [`vault_core::MemoryId`]) because the
/// model emits whatever short or long form the system prompt encouraged;
/// consumers parse downstream if they need to resolve back to a typed
/// [`vault_core::MemoryId`].
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContradictionRef {
    /// Memory IDs (as strings) that participate in the contradiction.
    pub memory_ids: Vec<String>,
    /// One natural-language position per participating memory (same
    /// order as `memory_ids`).
    pub positions: Vec<String>,
    /// If the model can determine the most recent / authoritative
    /// position, it surfaces it here. Empty string when undetermined.
    #[serde(default)]
    pub current_position_if_determinable: String,
}

/// The structured read-time synthesis response. Deserialised from the
/// LLM's GBNF-constrained JSON output via [`READ_TIME_JSON_SCHEMA`].
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReadResponse {
    /// Coherent narrative answer the agent can consume directly. Includes
    /// inline citations to memory IDs (the model is instructed to cite,
    /// though the citation format is not parsed structurally here).
    pub synthesis_markdown: String,
    /// Zero or more contradictions the model surfaced across the
    /// filtered candidate set.
    pub contradictions_flagged: Vec<ContradictionRef>,
    /// True when the model determined no candidate is relevant to the
    /// query (the correct hard-negative behaviour — `vault_has_no_relevant_content`
    /// short-circuiting at the retrieval boundary OR the synthesis boundary
    /// both flow through this field).
    pub vault_has_no_relevant_content: bool,
}

/// Production read-time pipeline. Pair an `Arc<dyn Retriever>` (V0.2:
/// `SemanticRetriever`) with an `Arc<dyn LlmProvider>` (V0.2:
/// `Qwen25_14BProvider` with the locked tuning config — see HANDOFF.md)
/// at construction; call [`ReadPipeline::read`] per agent query.
///
/// Concrete struct (NOT a trait) per the V0.2 forward-compat policy
/// (`feedback_forward_compat_concrete_vs_hypothetical.md`). The trait
/// surface lands when V0.3 cloud-tier becomes the imminent next task and
/// a second concrete implementation (remote synthesis) is in play.
#[derive(Clone)]
pub struct ReadPipeline {
    retriever: Arc<dyn Retriever>,
    llm: Arc<dyn LlmProvider>,
    max_candidates: usize,
    system_prompt: String,
}

impl ReadPipeline {
    /// Construct with default `DEFAULT_MAX_CANDIDATES` candidates and the
    /// production-locked [`READ_TIME_SYSTEM_PROMPT`].
    #[must_use]
    pub fn new(retriever: Arc<dyn Retriever>, llm: Arc<dyn LlmProvider>) -> Self {
        Self {
            retriever,
            llm,
            max_candidates: DEFAULT_MAX_CANDIDATES,
            system_prompt: READ_TIME_SYSTEM_PROMPT.to_string(),
        }
    }

    /// Override the top-N count passed to the retriever. Clamped to
    /// `[1, MAX_RESULTS_CAP]` at call time — values outside the band
    /// surface as `VaultError::InvalidInput` from the retriever.
    #[must_use]
    pub fn with_max_candidates(mut self, n: usize) -> Self {
        self.max_candidates = n;
        self
    }

    /// Override the system prompt for the synthesis call. Default is
    /// [`READ_TIME_SYSTEM_PROMPT`] which was validated by t026/t027b
    /// against the production quality contract — overrides void the
    /// quality guarantee until re-validated.
    #[must_use]
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Run the two-stage read pipeline.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] — query text is empty / whitespace-only
    ///   after trim, or the retriever rejected the constructed
    ///   [`RetrievalQuery`].
    /// - [`VaultError::Retrieval`] — stage-1 retrieval failed.
    /// - [`VaultError::Embedding`] — stage-1 embedder failed.
    /// - [`VaultError::Storage`] — stage-1 vector or metadata store failed.
    /// - [`VaultError::Llm`] — stage-2 LLM inference failed OR the model's
    ///   output failed to parse against [`READ_TIME_JSON_SCHEMA`]. GBNF
    ///   constraint guarantees structural validity at sample time, so a
    ///   parse failure here is a hard error (likely a model-side bug or
    ///   a grammar-construction-time issue that should have fired earlier).
    pub async fn read(&self, query: ReadQuery) -> VaultResult<ReadResponse> {
        let trimmed = query.query_text.trim();
        if trimmed.is_empty() {
            return Err(VaultError::InvalidInput(
                "read pipeline: query_text is empty after trim".into(),
            ));
        }

        // Stage 1 — semantic retrieval top-N.
        let retrieval = RetrievalQuery {
            query_text: trimmed.to_string(),
            authorized_boundaries: query.authorized_boundaries,
            max_results: self.max_candidates,
            options: RetrievalOptions::default(),
        };
        let candidates = self.retriever.retrieve(retrieval).await?;

        if candidates.is_empty() {
            // Short-circuit: no candidates means no relevant content
            // before the LLM is involved. Avoids spending ~30-120s on
            // an inference that has no inputs to synthesise.
            return Ok(ReadResponse {
                synthesis_markdown:
                    "No memories matched this query within the authorized boundaries.".to_string(),
                contradictions_flagged: Vec::new(),
                vault_has_no_relevant_content: true,
            });
        }

        // Stage 2 — Qwen-class synthesis.
        let user_prompt = build_user_prompt(trimmed, &candidates);
        let params = CompletionParams {
            max_tokens: 1024,
            temperature: 0.0,
            top_p: 1.0,
            seed: Some(42),
            system_prompt: Some(self.system_prompt.clone()),
        };
        let raw = self
            .llm
            .complete_json(&user_prompt, READ_TIME_JSON_SCHEMA, &params)
            .await
            .map_err(|e| VaultError::Llm(format!("read pipeline stage 2: {e}")))?;

        serde_json::from_str::<ReadResponse>(&raw).map_err(|e| {
            // Truncate the raw output to keep the error message bounded;
            // full raw is recoverable from tracing logs if needed for
            // debugging.
            let preview: String = raw.chars().take(200).collect();
            VaultError::Llm(format!(
                "read pipeline stage 2: synthesis output did not match schema: {e}; raw[..200]={preview}"
            ))
        })
    }
}

impl std::fmt::Debug for ReadPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadPipeline")
            .field("llm_model_id", &self.llm.model_id())
            .field("max_candidates", &self.max_candidates)
            // retriever + system_prompt intentionally omitted: retriever has
            // no Debug, system_prompt is large.
            .finish_non_exhaustive()
    }
}

/// Build the stage-2 user prompt. Each candidate is rendered as
/// `[<rank>] <content>\n` (1-indexed) so the model can cite by candidate
/// index inline. The closing line tells the model to filter + flag +
/// synthesise.
///
/// **Why rank indices and not memory IDs.** T0.2.7 Phase 5 Step 2
/// (2026-05-23) t030 byte-equality probe confirmed that embedding random
/// per-process UUIDv7 memory IDs into the LLM prompt was the load-bearing
/// source of Q25 verdict variance at SCALE=1000. Retrieval was fully
/// deterministic in-process AND across-process (same memories, same
/// content, same order). The only byte differences across runs were the
/// UUIDs themselves (~300 BPE tokens of randomness per prompt), which
/// changed the model's tokenization and downstream attention patterns
/// enough to flip Qwen-7B's verdict on borderline queries even at
/// temperature=0 with seed=42. Switching to 1-indexed rank makes the
/// prompt a pure function of `(query, ordered candidate content)` —
/// both already deterministic — so the LLM input is identical per query
/// regardless of process spawn timing.
///
/// The `ContradictionRef::memory_ids` field (typed `Vec<String>` per
/// `READ_TIME_JSON_SCHEMA`) carries whatever the model emits; under the
/// new format the model emits rank strings (`"1"`, `"5"`) which can be
/// resolved back to real `vault_core::MemoryId` values downstream via
/// the candidate list already in scope at call sites. The load-bearing
/// field for the agent contract is `contradictions_flagged.positions`
/// (literal values), which is unaffected.
///
/// **Public for diagnostic use only.** Exposed for byte-equality probes
/// (e.g. `examples/t030_q25_byte_equality_probe.rs`) that need to confirm
/// the same prompt bytes are sent to the LLM across runs. Production code
/// calls this through [`ReadPipeline::read`]; no need to call it directly.
pub fn build_user_prompt(query: &str, candidates: &[RetrievedMemory]) -> String {
    // Estimate capacity to avoid repeated growth: ~query + candidates' content + 8-byte rank-line overhead.
    let est_cap: usize = query.len()
        + candidates
            .iter()
            .map(|c| c.memory.content.len() + 8)
            .sum::<usize>()
        + 128;
    let mut s = String::with_capacity(est_cap);
    s.push_str("QUERY: ");
    s.push_str(query);
    s.push_str("\n\nCANDIDATES:\n");
    for (rank, c) in candidates.iter().enumerate() {
        // 1-indexed for human readability (matches the system-prompt example
        // `{memory_ids: ["1", "5"], ...}` and reads naturally as "candidate 1").
        s.push('[');
        s.push_str(&(rank + 1).to_string());
        s.push_str("] ");
        s.push_str(&c.memory.content);
        s.push('\n');
    }
    s.push_str("\nFilter, flag contradictions, synthesize. Return JSON.");
    s
}

#[allow(dead_code)] // exported via lib.rs; keeping the silencer until the type is consumed externally
const _ENSURE_PUBLIC: fn() = || {
    let _: usize = MAX_RESULTS_CAP; // pin: max_candidates clamp depends on this const
};

#[cfg(test)]
mod tests {
    //! Pipeline-wiring unit tests using `MockLlmProvider` + a test-local
    //! mock `Retriever`. Heavy / quality assertions live in the
    //! integration test at `tests/read_pipeline_acceptance.rs` against
    //! the real Qwen-7B model.

    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use vault_core::{Memory, MemoryType, NewMemory};
    use vault_llm::MockLlmProvider;

    /// Test-only retriever that returns a pre-canned candidate list.
    struct MockRetriever {
        canned: Vec<RetrievedMemory>,
        last_query: Mutex<Option<RetrievalQuery>>,
        force_error: Mutex<Option<VaultError>>,
    }

    impl MockRetriever {
        fn new(canned: Vec<RetrievedMemory>) -> Self {
            Self {
                canned,
                last_query: Mutex::new(None),
                force_error: Mutex::new(None),
            }
        }

        fn with_forced_error(self, err: VaultError) -> Self {
            *self.force_error.lock().unwrap() = Some(err);
            self
        }

        fn observed_query(&self) -> Option<RetrievalQuery> {
            self.last_query.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Retriever for MockRetriever {
        async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
            *self.last_query.lock().unwrap() = Some(query);
            if let Some(err) = self.force_error.lock().unwrap().take() {
                return Err(err);
            }
            Ok(self.canned.clone())
        }
    }

    fn boundary() -> Boundary {
        Boundary::new("personal").expect("static-valid boundary")
    }

    fn fake_memory(content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary(),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("static-valid memory")
    }

    fn retrieved(content: &str, score: f32) -> RetrievedMemory {
        RetrievedMemory {
            memory: fake_memory(content),
            score,
            explanation: format!("semantic: cosine={score:.4} (rank 1/1)"),
        }
    }

    fn canned_response_json(synthesis: &str, vault_empty: bool) -> String {
        serde_json::json!({
            "synthesis_markdown": synthesis,
            "contradictions_flagged": [],
            "vault_has_no_relevant_content": vault_empty,
        })
        .to_string()
    }

    #[tokio::test]
    async fn empty_query_text_is_rejected_as_invalid_input() {
        let retriever = Arc::new(MockRetriever::new(Vec::new()));
        let llm = Arc::new(MockLlmProvider::new(
            "mock",
            canned_response_json("", false),
        ));
        let pipeline = ReadPipeline::new(retriever, llm);

        let err = pipeline
            .read(ReadQuery {
                query_text: "   ".into(),
                authorized_boundaries: vec![boundary()],
            })
            .await
            .expect_err("empty query must reject");
        assert!(matches!(err, VaultError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn empty_retrieval_short_circuits_without_calling_llm() {
        let retriever = Arc::new(MockRetriever::new(Vec::new()));
        let llm = Arc::new(MockLlmProvider::new(
            "mock",
            canned_response_json("should-not-appear", false),
        ));
        let pipeline = ReadPipeline::new(retriever, llm.clone());

        let resp = pipeline
            .read(ReadQuery {
                query_text: "what did I decide?".into(),
                authorized_boundaries: vec![boundary()],
            })
            .await
            .expect("empty retrieval must succeed with vault_has_no_relevant_content");
        assert!(resp.vault_has_no_relevant_content);
        assert!(resp.contradictions_flagged.is_empty());
        assert!(
            !resp.synthesis_markdown.contains("should-not-appear"),
            "LLM canned response must NOT leak — LLM should not have been called"
        );
        assert_eq!(
            llm.call_count(),
            0,
            "stage-2 LLM must NOT be called when stage-1 returns no candidates"
        );
    }

    #[tokio::test]
    async fn non_empty_retrieval_invokes_llm_and_returns_parsed_response() {
        let retriever = Arc::new(MockRetriever::new(vec![
            retrieved("Comcast bill is now $109/month.", 0.91),
            retrieved("Comcast bill is $89/month after loyalty discount.", 0.88),
        ]));
        let llm = Arc::new(MockLlmProvider::new(
            "mock",
            serde_json::json!({
                "synthesis_markdown": "Comcast went from $89 to $109.",
                "contradictions_flagged": [{
                    "memory_ids": ["mem-1", "mem-2"],
                    "positions": ["$89", "$109"],
                    "current_position_if_determinable": "$109"
                }],
                "vault_has_no_relevant_content": false,
            })
            .to_string(),
        ));
        let pipeline = ReadPipeline::new(retriever, llm.clone());

        let resp = pipeline
            .read(ReadQuery {
                query_text: "What's the Comcast bill?".into(),
                authorized_boundaries: vec![boundary()],
            })
            .await
            .expect("happy path must succeed");
        assert_eq!(resp.contradictions_flagged.len(), 1);
        assert_eq!(
            resp.contradictions_flagged[0].positions,
            vec!["$89", "$109"]
        );
        assert!(resp.synthesis_markdown.contains("$89"));
        assert!(resp.synthesis_markdown.contains("$109"));
        assert!(!resp.vault_has_no_relevant_content);
        assert_eq!(llm.call_count(), 1);
    }

    #[tokio::test]
    async fn retriever_error_propagates_as_retrieval_error() {
        let retriever = Arc::new(
            MockRetriever::new(Vec::new())
                .with_forced_error(VaultError::Retrieval("disk gone".into())),
        );
        let llm = Arc::new(MockLlmProvider::new(
            "mock",
            canned_response_json("", false),
        ));
        let pipeline = ReadPipeline::new(retriever, llm);

        let err = pipeline
            .read(ReadQuery {
                query_text: "test".into(),
                authorized_boundaries: vec![boundary()],
            })
            .await
            .expect_err("retrieval error must surface");
        assert!(matches!(err, VaultError::Retrieval(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn llm_returns_invalid_json_surfaces_as_llm_error() {
        let retriever = Arc::new(MockRetriever::new(vec![retrieved("anything", 0.5)]));
        // MockLlmProvider returns the canned string verbatim — pass
        // structurally-invalid JSON to exercise the parse-failure path.
        let llm = Arc::new(MockLlmProvider::new("mock", "{not valid json"));
        let pipeline = ReadPipeline::new(retriever, llm);

        let err = pipeline
            .read(ReadQuery {
                query_text: "test".into(),
                authorized_boundaries: vec![boundary()],
            })
            .await
            .expect_err("invalid LLM JSON must surface");
        assert!(matches!(err, VaultError::Llm(_)), "got {err:?}");
        if let VaultError::Llm(msg) = err {
            assert!(
                msg.contains("did not match schema"),
                "error must name the failure mode; got {msg}"
            );
        }
    }

    #[tokio::test]
    async fn retriever_observes_correct_query_construction() {
        let retriever = Arc::new(MockRetriever::new(Vec::new()));
        let llm = Arc::new(MockLlmProvider::new(
            "mock",
            canned_response_json("", false),
        ));
        let pipeline = ReadPipeline::new(retriever.clone(), llm).with_max_candidates(7);

        let _ = pipeline
            .read(ReadQuery {
                query_text: "  trimmed query  ".into(),
                authorized_boundaries: vec![boundary()],
            })
            .await;

        let observed = retriever
            .observed_query()
            .expect("retriever must have been called");
        assert_eq!(
            observed.query_text, "trimmed query",
            "query must be trimmed before stage 1"
        );
        assert_eq!(
            observed.max_results, 7,
            "with_max_candidates must propagate to retriever"
        );
        assert_eq!(observed.authorized_boundaries.len(), 1);
    }

    #[tokio::test]
    async fn system_prompt_override_propagates_to_llm() {
        // We can't directly inspect the prompt MockLlmProvider received
        // (it discards prompt/schema/params and returns canned). But we
        // can pin that with_system_prompt is stored on the struct and
        // doesn't panic.
        let retriever = Arc::new(MockRetriever::new(Vec::new()));
        let llm = Arc::new(MockLlmProvider::new(
            "mock",
            canned_response_json("ok", false),
        ));
        let pipeline = ReadPipeline::new(retriever, llm).with_system_prompt("custom system prompt");

        let resp = pipeline
            .read(ReadQuery {
                query_text: "test".into(),
                authorized_boundaries: vec![boundary()],
            })
            .await
            .expect("with empty retrieval, short-circuits regardless of prompt");
        assert!(resp.vault_has_no_relevant_content);
    }

    #[test]
    fn build_user_prompt_renders_query_then_candidates_in_order() {
        let candidates = [
            retrieved("first content", 0.95),
            retrieved("second content", 0.80),
        ];
        let prompt = build_user_prompt("my question", &candidates);
        assert!(prompt.starts_with("QUERY: my question"));
        let first_idx = prompt
            .find("first content")
            .expect("first content must appear");
        let second_idx = prompt
            .find("second content")
            .expect("second content must appear");
        assert!(
            first_idx < second_idx,
            "candidates must appear in input order"
        );
        assert!(prompt.trim_end().ends_with("Return JSON."));

        // v10 rank-prefix contract (T0.2.7 Phase 5 Step 2, 2026-05-23): each
        // candidate is rendered as `[<1-indexed rank>] <content>\n`. Pins
        // the deterministic-prompt fix: NO UUIDs leak into the LLM input.
        assert!(
            prompt.contains("[1] first content"),
            "first candidate must be rendered with [1] rank prefix"
        );
        assert!(
            prompt.contains("[2] second content"),
            "second candidate must be rendered with [2] rank prefix"
        );
        // Defence-in-depth: scan for any 36-char window matching the
        // UUID 8-4-4-4-12 hex pattern. Catches accidental regressions to
        // the old `[<uuid>]` format even if the [1]/[2] assertions above
        // still hold (e.g. mixed format).
        let bytes = prompt.as_bytes();
        if bytes.len() >= 36 {
            for start in 0..=bytes.len() - 36 {
                let window = &bytes[start..start + 36];
                if window[8] == b'-'
                    && window[13] == b'-'
                    && window[18] == b'-'
                    && window[23] == b'-'
                {
                    let all_hex = window
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| ![8, 13, 18, 23].contains(i))
                        .all(|(_, b)| b.is_ascii_hexdigit());
                    assert!(
                        !all_hex,
                        "UUID-shaped substring detected in prompt; v10 prompt must use rank indices only. Window: {:?}",
                        std::str::from_utf8(window).unwrap_or("<non-utf8>")
                    );
                }
            }
        }
    }

    #[test]
    fn read_time_system_prompt_contains_the_load_bearing_rules() {
        // Tripwire pin — if the prompt text drifts in a way that drops
        // these instructions, the t026 / T0.2.7 quality gates are at
        // risk and we want a unit-test failure before the integration
        // test catches it.
        //
        // Original t026/t027b core (kept):
        assert!(READ_TIME_SYSTEM_PROMPT.contains("filter"));
        assert!(READ_TIME_SYSTEM_PROMPT.contains("contradictions"));
        assert!(READ_TIME_SYSTEM_PROMPT.contains("vault_has_no_relevant_content"));
        assert!(READ_TIME_SYSTEM_PROMPT.contains("Do NOT fabricate"));

        // T0.2.7 Phase 4 v9 promotion (2026-05-20) — these additions
        // are the empirical anchor for the 9/9 SCALE=10K validation:
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("VERBATIM RULE"),
            "v9 verbatim rule must be present (locks the dual-field contract)"
        );
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("TEMPORAL VALUE CHANGES"),
            "v9 temporal-value-change clause must be present (locks Q11/Q13/Q25/Q26 etc.)"
        );
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("NARRATIVE COMPLIANCE"),
            "v9 narrative-compliance anti-pattern must be present"
        );
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("ANTI-PATTERN"),
            "v9 anti-pattern examples must be present (locks the moved-to/renewed-at guidance)"
        );
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("CORRECT"),
            "v9 CORRECT-example label must be present (paired with INCORRECT)"
        );
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("INCORRECT"),
            "v9 INCORRECT-example label must be present (paired with CORRECT)"
        );

        // T0.2.7 Phase 5 Step 2 v10 promotion (2026-05-23) — pins the
        // rank-indexed prompt contract introduced to remove UUID-driven
        // input variance. The Comcast example was rewritten to use rank
        // strings; the OUTPUT section explicitly teaches the LLM that
        // candidates are 1-indexed.
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains(r#"{memory_ids: ["3", "7"]"#),
            "v10 rank-string example must be present (locks the LLM-facing format)"
        );
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("1-indexed"),
            "v10 OUTPUT section must teach the LLM that candidates are 1-indexed"
        );
        assert!(
            READ_TIME_SYSTEM_PROMPT.contains("matching the [N] prefix"),
            "v10 CONTRADICTIONS section must reference the `[N] prefix` rank-string contract"
        );
    }

    #[test]
    fn read_time_json_schema_is_valid_json() {
        let parsed: serde_json::Value =
            serde_json::from_str(READ_TIME_JSON_SCHEMA).expect("schema must be valid JSON");
        assert_eq!(parsed["type"], "object");
        let required: Vec<&str> = parsed["required"]
            .as_array()
            .expect("required is array")
            .iter()
            .map(|v| v.as_str().expect("required entry is string"))
            .collect();
        assert!(required.contains(&"synthesis_markdown"));
        assert!(required.contains(&"contradictions_flagged"));
        assert!(required.contains(&"vault_has_no_relevant_content"));
    }
}
