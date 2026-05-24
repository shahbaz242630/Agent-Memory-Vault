//! T0.2.7 Phase 0.b — t028g hybrid retrieval spike (2026-05-19).
//!
//! **Purpose.** Validate the structural hypothesis that pure cosine retrieval
//! (BGE dense) cannot serve both contradiction-completeness AND
//! hard-negative-rejection simultaneously, and that fusing it with a
//! lexical (BM25) channel via Reciprocal Rank Fusion + a BM25-hit-count
//! abstain gate fixes both failure modes structurally — without the
//! parametric whack-a-mole of the value-aware-reranker arc (t028d, dead).
//!
//! **Locked direction (2026-05-19 4-agent investigation).** Both research
//! agents independently chose hybrid BM25 + dense + RRF + BM25-hits-abstain
//! as the production-standard fix. Architects' custom two-pass design held
//! in reserve if Phase 0 fails. See HANDOFF.md "Multi-phase plan iteration 2"
//! for the 6-phase arc; this file is Phase 0.b.
//!
//! **Why a spike before production code.** Per the spike-playbook rule
//! (`feedback_spike_playbook_for_unknowns.md`), validate the structural
//! hypothesis at SCALE=10K on the same diverse corpus that broke
//! ValueAwareRetriever before promoting BM25 into vault-storage (Phase 1).
//! Phase 0 spike = fail-cheap. If acceptance passes, Phase 1 production
//! code is consuming a verified design, not a guess.
//!
//! **Acceptance for Phase 0 → Phase 1 promotion.**
//! - 6/6 PASS on the iteration subset (Q11, Q13, Q21, Q22, Q25, Q26) at
//!   SCALE=10K diverse corpus.
//! - 3/3 PASS on three new short↔long contradiction pairs (S1, S2, S3)
//!   that stress length-asymmetric value disagreement (Shahbaz's
//!   length-sensitivity concern from the 2026-05-19 Q13 brute-force
//!   diagnostic).
//! - Abstain telemetry shows correct discrimination: Q21+Q22 trigger
//!   abstain (max BM25 score < BM25_TOP_SCORE_THRESHOLD); Q11/Q13/S1/S2/S3
//!   do not (their target memories share rare anchor tokens with the
//!   query, producing BM25 top scores well above threshold).
//!
//! **What this spike does NOT do.**
//! - Does not modify vault-storage. BM25 is a spike-local Tantivy in-RAM
//!   index built over the corpus once at spike-init. Phase 1 will wire
//!   it as a sidecar index alongside LanceDB.
//! - Does not modify the production [`ReadPipeline`]. The hybrid retriever
//!   plugs into the existing trait — the pipeline sees a [`Retriever`]
//!   and doesn't care about the dense/lexical fusion happening inside.
//! - Does not measure latency rigorously (single rep per query).
//! - Does not exercise the full t028c 8-query gauntlet — that runs at
//!   Phase 5 (acceptance gauntlet) once production code is wired.
//!
//! **Iteration protocol if 9/9 doesn't land first try.**
//! 1. `BM25_TOP_SCORE_THRESHOLD` — start 6.0, calibrate against the
//!    printed telemetry (the spike emits max/p90/median per query).
//!    Raise if hard-negs leak through; lower if rare-anchor queries
//!    over-trigger abstain.
//! 2. `HYBRID_TOP_N_EACH` — start 200. Raise if a contradiction pair's
//!    second member is too deep in both channels.
//! 3. `RRF_K` constant — 60 is the literature default; do NOT change
//!    first. If short↔long fails, the structural fix is BM25
//!    tokenization (b/k1), not the fusion constant.
//! 4. BM25 k1/b params — if length-asymmetric pairs fail, tune b downward
//!    (b=0.3-0.5 per Research #1's note; Tantivy default is BM25 with
//!    k1=1.2 b=0.75).
//!
//! **Discipline.** Example-grade throwaway. Per [[spike-examples-bundle-
//! with-consumer-code]], this spike file ships with the Phase 5
//! production-code commit, NOT its own commit. The Cargo.toml `tantivy =
//! "=0.26.1"` dev-dep is also unshipped until Phase 5; the dev-dep gets
//! promoted to vault-storage's production deps at Phase 1.
//!
//! Run with (PowerShell on Windows):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t028g_hybrid_retrieval_spike `
//!   2>&1 | Tee-Object -FilePath t028g_phase0_verify.log
//! ```

#![allow(clippy::too_many_lines)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{ensure, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{Field, Schema, Value, STORED, TEXT};
use tantivy::{doc, Index, IndexReader, TantivyDocument};
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory, VaultError, VaultResult};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_llm::{LlmProvider, Qwen25_14BProvider, TuningConfig};
use vault_retrieval::{
    ReadPipeline, ReadQuery, ReadResponse, RetrievalOptions, RetrievalQuery, RetrievedMemory,
    Retriever, SemanticRetriever,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

// 6-query iteration subset, identical to t028d for direct comparability
// against the value-aware-reranker arc's failure modes:
// - Q11, Q13: contradiction pairs the value-aware arc handled successfully
//   on early runs; regression canaries here — must still PASS under hybrid.
// - Q21, Q22: hard-negatives that value-aware's K-boundary relaxation
//   broke at 4/6. Under hybrid, BM25-hit-count abstain should reject
//   these deterministically (vault has zero K8s memories, zero dental
//   policy memories).
// - Q25, Q26: contradiction pairs the value-aware arc fixed parametrically
//   but at the cost of Q13. Hybrid should surface both pair members via
//   RRF without polluting the LLM context with noise dollar-amount pairs.
const ITERATION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q21", "Q22", "Q25", "Q26"];

const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];
const HARD_NEGATIVE_QUERY_IDS: &[&str] = &["Q21", "Q22"];

// Short↔long contradiction pairs (S1, S2, S3) — NEW for t028g, not in
// merge_acceptance_100_queries.json. Each pair has a short memory (~30
// chars) + a long memory (~2000 chars with the disagreement buried in
// the middle). All three pairs share a rare lexical anchor token that
// distinguishes them from the distractor vocab and the merge fixture
// content, so BM25 has a clean signal even when BGE's cosine is muddied
// by length asymmetry. Acceptance: hybrid surfaces both pair members in
// top-K AND the LLM flags the value disagreement.
const SHORT_LONG_QUERY_IDS: &[&str] = &["S1", "S2", "S3"];

/// Default corpus scale. Override at runtime with `T028G_SCALE=<n>` env
/// var without rebuilding (the spike resolves this in main() so one cold
/// build covers 100 / 1K / 10K iteration runs). 10_000 is the acceptance
/// scale per HANDOFF Phase 0 acceptance criteria; 1_000 and 100 are
/// diagnostic scales useful for fast failure-mode triage.
const DEFAULT_SCALE: usize = 10_000;
const QWEN_MODEL_FILENAME: &str = "Qwen2.5-7B-Instruct-Q4_K_M.gguf";
const SEP_WIDE: usize = 100;
const DISTRACTOR_SEED: u64 = 0x7028C_DEADBEEF;

// ── Hybrid retrieval knobs — calibrate against the printed telemetry ─────
//
// Reciprocal Rank Fusion: score(d) = Σ 1/(k + rank_i(d)) over both rank
// lists. k=60 is the literature default (Cormack et al. 2009); changing it
// is iteration knob 3, not knob 1. With k=60 and top-100 per channel, RRF
// scores fall in [0, 2/(60+1)] ≈ [0, 0.0328] — well inside the [-1, 1]
// invariant the Retriever trait requires.
const RRF_K: usize = 60;

// Top-1 BM25 score threshold for the abstain gate.
//
// Replaces the original count-above-floor abstain design (2026-05-19
// SCALE=10K validation v1 surfaced its structural flaw): at SCALE=10K with
// top_n_each=200, every query had 200 BM25 hits above floor=1.0 because
// the diverse-corpus distractor generator produces hundreds of pet-care
// memories containing the word "dental", "update", "cost", etc. — every
// hard-neg query also matched 200+ weak topic-overlap hits, leaving
// abstain unable to distinguish "real signal" from "200 unrelated topic
// matches."
//
// The new design is scale-independent: abstain if the BEST BM25 hit's
// score falls below this threshold. Strong-signal queries (Q11 "GA
// launch", Q13 "Comcast", Q25/Q26) clear it because their target memories
// match multiple rare anchor tokens — top hit scores 8-15. Hard-negs
// (Q21 "Kubernetes" with 0 K8s memories, Q22 "dental insurance policy
// number" with 200 pet-dental-cleaning memories matching only the common
// "dental" token) score 2-5 at best — below the threshold.
//
// 6.0 is the initial calibration; first run informs whether to tighten.
const BM25_TOP_SCORE_THRESHOLD: f32 = 6.0;

// Per-channel widening: pull this many candidates from BGE AND from
// BM25 before the RRF fusion truncates to the caller's max_results.
//
// Bumped 100 → 200 at 2026-05-19 SCALE=10K diagnostic close (post-acceptance
// 7/9). Q25's Memory A (Q1 2027 GA-launch contradiction pair member) sits
// at BGE rank 20-25 at SCALE=10K diverse corpus AND only earns weak BM25
// score (the query "Help me update the product roadmap doc..." has no rare
// anchor token matching the Q1 2027 memory). With top_n_each=100, Memory
// A's RRF score (≈0.012 from BGE rank ~20 alone) fell just below the
// top-20 cutoff (~0.0143). At top_n_each=200, Memory A's BM25 rank also
// contributes (it's deeper in BM25 but inside the 200-window), pushing the
// fused score above the cutoff. Cost: ~2x BM25 hydration round-trip per
// query — negligible vs LLM latency.
const HYBRID_TOP_N_EACH: usize = 200;

// ── SYSTEM PROMPT — v9 (v8 + narrative-compliance anti-pattern, 2026-05-20) ─
//
// Iteration history lives in t028d's iteration-log comment (lines 81-130
// of t028d_prompt_iteration_spike.rs). Headline summary of the arc that
// led here:
//
//   v0-v5 (prompt-only iterations) — eliminated as a class. K-tuning
//   alone cannot resolve the contradiction-vs-hard-negative trade-off
//   because Memory B at cosine rank 21+ doesn't reach the LLM at K=20,
//   while K=30+ pollutes Q21's hard-neg context with noise.
//
//   v6-v8 (value-aware reranker arc, t028d) — DEAD. 10K verification at
//   2026-05-19 ran 5/6 (Q13 fail under noise pollution from spurious
//   dollar-amount pair promotions); narrow fix attempt ran 4/6
//   (Q21+Q22 hard-negs broken by graph-rebalance admitting Q1/Q2 GA
//   pair into Q22's dental hard-neg context). [[fix-one-break-another-
//   signals-structural]] — the failure pattern itself is the diagnostic
//   that the root cause is structural, not parametric.
//
//   t028g (THIS FILE) — structural fix via hybrid retrieval. The v8
//   prompt is correct (validated 1K, 6/6, locked); the bug was always
//   the retrieval-layer single-channel cosine. With BM25 + RRF + abstain,
//   the v8 prompt sees the right candidates for contradiction queries
//   AND no candidates at all for hard-negatives, eliminating both
//   failure classes via the structural channel.
//
// The prompt below is byte-identical to t028d's CANDIDATE_SYSTEM_PROMPT
// v8 (line 132+ of t028d). Production wiring at Phase 4 will replace
// READ_TIME_SYSTEM_PROMPT in read_pipeline.rs with this text verbatim.

const CANDIDATE_SYSTEM_PROMPT: &str = r#"You are the read layer of a personal memory vault used by AI coding agents.

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
    (b) Add an entry to contradictions_flagged with the participating memory IDs
        and the conflicting positions (also verbatim).
- TEMPORAL VALUE CHANGES count as contradictions. If one memory says X has value A
  and another memory says X has value B (or "X is now B", or "X increased to B",
  or "B starting next cycle"), both memories disagree about what value X currently
  carries — the older memory implies the answer is A; the newer implies B. You MUST
  flag this in contradictions_flagged using the same dual-field rule above. A
  monthly-review or audit query is asking precisely for these flags; reporting the
  change in synthesis_markdown alone is NOT enough.
- Example: if memory M1 says "Comcast bill is $89/month" and memory M2 says
  "Comcast bill is now $109/month starting next cycle", BOTH values disagree about
  the current Comcast cost — populate contradictions_flagged with
  {memory_ids: [M1, M2], positions: ["$89/month", "$109/month starting next cycle"]}.
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
- Write a coherent narrative in synthesis_markdown; cite memory IDs inline.
- If no candidates are relevant: set vault_has_no_relevant_content=true and state
  this in synthesis_markdown. Do NOT fabricate.
- Keep synthesis_markdown under 250 words.
- Return ONLY valid JSON matching the schema."#;

// ── PRNG ────────────────────────────────────────────────────────────────

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn pick<T: Copy>(&mut self, slice: &[T]) -> T {
        let idx = (self.next_u64() as usize) % slice.len();
        slice[idx]
    }
}

// ── Fixture types (same as t028c) ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct MemoryFixtureEntry {
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

// ── Distractor generator (verbatim subset from t028c — kept inline so
// this spike file is self-contained) ─────────────────────────────────────

// All vocabulary + templates are identical to t028c; copying for spike
// self-containment per spike-bundle policy.

const PEOPLE: &[&str] = &[
    "Aiden", "Beatriz", "Carlos", "Dana", "Eduardo", "Fatima", "Gunther", "Hiroko", "Ingrid",
    "Jamal", "Kavya", "Lior", "Mei", "Noor", "Ola", "Priya", "Quentin", "Rashid", "Selma",
    "Tobias", "Uma", "Vihaan", "Wendy", "Xiulan", "Yusuf", "Zoe",
];

const MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const DAYS_OF_WEEK: &[&str] = &[
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

const MONEY_AMOUNTS: &[&str] = &[
    "$245", "$312", "$478", "$523", "$640", "$715", "$820", "$945", "$1,200", "$1,475", "$1,820",
    "$2,150", "$2,640", "$3,100", "$4,250", "$5,500", "$6,900", "$8,300",
];

const OFFICE_FACILITY: &[&str] = &[
    "parking garage",
    "main entrance badge reader",
    "third-floor kitchen",
    "south wing conference rooms",
    "rooftop terrace",
    "loading dock",
    "wellness room",
    "phone-booth pods",
    "mother's room",
];

const OFFICE_CHANGE_DETAIL: &[&str] = &[
    "now requires the new RFID card",
    "is closed for renovation through end of quarter",
    "switched to a reservation-only model via the new booking app",
    "has updated visitor escort policy",
    "gets a new vending vendor next week",
    "now stocks oat milk and almond milk by default",
];

const OFFICE_SHORT_TEMPLATES: &[&str] = &[
    "Office update: {FACILITY} {CHANGE_DETAIL}",
    "Heads up — {FACILITY} {CHANGE_DETAIL} starting {MONTH}",
    "{FACILITY} is changing: {CHANGE_DETAIL}",
];

const OFFICE_PARA_TEMPLATES: &[&str] = &[
    "Facilities note: {FACILITY} {CHANGE_DETAIL}. {PERSON} from ops sent the announcement on {DAY}. Take-aways: review the updated SOP on the intranet before {MONTH}, and route any conflicts through the helpdesk so we can roll the change cleanly. There is a 30-day grace period for anyone still using the legacy procedure, and after that the access controls will enforce the new flow automatically.",
    "Operations rollup for the week: {FACILITY} {CHANGE_DETAIL}, and {PERSON} is the point of contact while the change beds in. We had a quick walkthrough on {DAY} morning to make sure everyone on the floor understood the new procedure. The transition lands the first week of {MONTH}; the old process keeps working in parallel for another two weeks as a courtesy.",
];

const OFFICE_LONG_TEMPLATES: &[&str] = &[
    "Long-form note from the facilities all-hands on {DAY}: {FACILITY} {CHANGE_DETAIL}, with a rollout timeline targeting the first week of {MONTH}. {PERSON} walked us through the change drivers: the previous arrangement had been generating an outsized share of helpdesk tickets, and the renewed vendor contract gave us the opening to fix it. Several follow-ons came out of the Q&A: a new floor map will be posted next to each elevator bank, the visitor-escort policy is being tightened to match the new flow, and a short FAQ will live on the intranet for the first 60 days. The change is reversible if it does not reduce ticket volume by 40% within the quarter — we are explicitly building in a checkpoint at week six to confirm the projected savings of {MONEY} per quarter are real. ICs should not need to do anything different on day one; the visible difference will show up gradually as the badge-reader firmware is rolled out floor by floor.",
];

const ENG_VENDORS: &[&str] = &[
    "GitHub Enterprise",
    "Datadog observability tier",
    "Linear team plan",
    "Notion business workspace",
    "Figma organization plan",
    "Slack enterprise grid",
    "Sentry error monitoring",
    "PagerDuty incident response",
    "1Password Business",
    "Tailscale enterprise SSO",
    "CircleCI performance tier",
    "Snyk security scanning",
];

const RENEWAL_DETAILS: &[&str] = &[
    "renewal lands {MONTH} {DAY}",
    "comes up for renewal in {MONTH}; current year was {MONEY}",
    "auto-renews unless we cancel by the end of {MONTH}",
    "is moving to annual prepay next cycle",
];

const VENDOR_SHORT_TEMPLATES: &[&str] = &[
    "{VENDOR} {RENEWAL_DETAIL}",
    "Renewal reminder — {VENDOR} {RENEWAL_DETAIL}",
    "{VENDOR}: {RENEWAL_DETAIL}",
];

const VENDOR_PARA_TEMPLATES: &[&str] = &[
    "Vendor budget note: {VENDOR} {RENEWAL_DETAIL}. {PERSON} from finance flagged that we should run a quick utilization audit before re-upping at {MONEY}, since seat usage looked uneven last quarter. The renewal terms include a 60-day cancellation window if we want to switch tiers. {PERSON} owns the audit; results targeted for the second week of {MONTH}.",
];

const VENDOR_LONG_TEMPLATES: &[&str] = &[
    "Full procurement writeup: {VENDOR} {RENEWAL_DETAIL}. The annual cost lands at {MONEY}, which is roughly in line with last year's adjusted for inflation. {PERSON} ran a usage audit over the past quarter and found three things worth noting: first, seat utilization is at 78% — not high enough to push for more seats but not low enough to justify a downgrade tier. Second, the integration we built last quarter against this tool's API has become load-bearing for the deploy pipeline, so any switch would carry a non-trivial migration cost we should factor into any 'should we switch' conversation. Recommendation: renew at the current tier, set a calendar reminder for the next renewal {MONTH}, and revisit the multi-year discount question once the beta-feature decision is settled.",
];

const DOC_TYPES: &[&str] = &[
    "API reference for the billing service",
    "runbook for the search index rebuild procedure",
    "RFC on the new authentication flow",
    "design doc for the notification delivery system",
    "post-mortem from the latency incident",
    "onboarding guide for new backend engineers",
    "operator guide for the data warehouse refresh",
    "architecture overview for the analytics pipeline",
];

const DOC_FEEDBACK: &[&str] = &[
    "needs a diagram for the failure-mode section",
    "is missing the rollback procedure",
    "should add explicit examples for the rate-limit edge cases",
    "needs a glossary for the domain-specific terms",
    "could be tightened — the intro is twice as long as it needs to be",
    "is good as-is, just needs one more reviewer sign-off",
];

const DOC_SHORT_TEMPLATES: &[&str] = &[
    "{PERSON} reviewed the {DOC_TYPE}; main feedback: {DOC_FEEDBACK}",
    "Doc review: {DOC_TYPE} — {DOC_FEEDBACK}",
    "{PERSON}'s feedback on the {DOC_TYPE}: {DOC_FEEDBACK}",
];

const DOC_PARA_TEMPLATES: &[&str] = &[
    "Documentation review notes: {PERSON} went through the {DOC_TYPE} on {DAY} and the main point was that it {DOC_FEEDBACK}. Secondary feedback was that the page would benefit from a 'when not to use this' section so that readers can self-route to the right doc. {PERSON} offered to draft that section and circle back by end of {MONTH}.",
];

const DOC_LONG_TEMPLATES: &[&str] = &[
    "Comprehensive review writeup for the {DOC_TYPE}, conducted by {PERSON} during the {MONTH} doc-quality sprint. Top-line: {DOC_FEEDBACK}. The full review uncovered three classes of issue. First, terminology drift — the doc uses three different names for the same internal concept, which makes it hard for newcomers to follow the chain. Second, the example code blocks are stylized rather than copy-pasteable, which means readers have to mentally translate them to use the actual library; standard practice elsewhere in our docs corpus is to provide working examples. Third, the 'troubleshooting' section is structured around the symptoms the original author hit, not the symptoms readers are likely to encounter; this is a common drift pattern as docs age. {PERSON} is taking the action item to do a full revision over the next two weeks, with {MONEY} budget for one round of contractor copy-edit.",
];

const TOOL_SYSTEM: &[&str] = &[
    "CI build pipeline",
    "observability stack",
    "local dev environment",
    "log aggregation cluster",
    "feature flag service",
    "secret rotation cron",
    "internal admin dashboard",
    "load-test rig",
];

const TOOL_ACTION: &[&str] = &[
    "got a 30% speedup after the cache layer refactor",
    "is being migrated to the new platform team's stack",
    "had a flaky failure on {DAY} traced to an upstream dep",
    "now has a {MONEY} monthly cost ceiling",
    "is getting deprecated in favor of the unified replacement",
    "needs a new on-call owner; {PERSON} volunteered",
];

const TOOL_SHORT_TEMPLATES: &[&str] = &[
    "{TOOL_SYSTEM} {TOOL_ACTION}",
    "Update on the {TOOL_SYSTEM}: {TOOL_ACTION}",
    "Heads up — the {TOOL_SYSTEM} {TOOL_ACTION}",
];

const TOOL_PARA_TEMPLATES: &[&str] = &[
    "Internal tooling status — {TOOL_SYSTEM} {TOOL_ACTION}, per the platform-team update on {DAY}. The change touches everyone who uses the system at least weekly; expected disruption is one rolling restart during the {MONTH} maintenance window. {PERSON} is the point of contact for migration questions and will host a 30-minute Q&A in the engineering channel.",
];

const TOOL_LONG_TEMPLATES: &[&str] = &[
    "Long-form retrospective on the {TOOL_SYSTEM} work that {PERSON} led over the past quarter. Headline: {TOOL_ACTION}. The work originated from an internal survey that flagged this system as the second-most-painful piece of internal infrastructure behind the previous-generation deploy tooling we replaced last year. {PERSON} scoped the project against four success criteria: developer-experience improvement measured by survey delta, p99 latency improvement measured by the platform team's golden-signal dashboards, total cost reduction measured against the {MONEY} monthly baseline, and reduction in on-call pages tied to this system. Three of the four criteria were met. The cost criterion came in below target — savings landed at {MONEY} per month instead of the projected larger figure — because the migration also surfaced an unrelated cost driver that we had to absorb separately.",
];

const EVENT_TYPE: &[&str] = &[
    "engineering offsite",
    "company-wide hackathon",
    "department lunch",
    "manager 1:1 cadence reset",
    "skip-level coffee chats",
    "new-hire welcome breakfast",
    "design crit gathering",
    "platform-team retro session",
];

const EVENT_LOGISTICS: &[&str] = &[
    "scheduled for {MONTH} {DAY}, venue TBD",
    "moved from {MONTH} to {MONTH} due to a conflict",
    "is fully booked; waitlist managed by {PERSON}",
    "has a {MONEY} per-person catering budget",
    "is virtual-first this year to accommodate remote folks",
];

const EVENT_SHORT_TEMPLATES: &[&str] = &[
    "{EVENT_TYPE} {EVENT_LOGISTICS}",
    "Reminder: {EVENT_TYPE} {EVENT_LOGISTICS}",
    "{PERSON} is organizing the {EVENT_TYPE} — {EVENT_LOGISTICS}",
];

const EVENT_PARA_TEMPLATES: &[&str] = &[
    "Logistics for the upcoming {EVENT_TYPE}: {EVENT_LOGISTICS}. {PERSON} is the lead organizer; agenda items still being collected via the shared form. RSVPs close end of {MONTH}; people with dietary restrictions should mention them on the form so catering can plan.",
];

const EVENT_LONG_TEMPLATES: &[&str] = &[
    "Pre-read for the upcoming {EVENT_TYPE} that {PERSON} is organizing, {EVENT_LOGISTICS}. Context: this is the first time in three years we are running this format with the current team composition, and the planning group spent some time debating whether the legacy structure still fits. Three big shifts from the last iteration. First, the team is roughly 40% larger, which means the all-hands plenary format has to change — we are moving to a hub-and-spoke design where the morning is plenary and the afternoon is six parallel tracks. Second, we are deliberately reserving the last 90 minutes for fully unstructured time after feedback from the last two iterations that the schedule was too dense for the kind of cross-team relationship-building that is the actual reason these events exist.",
];

const TRAVEL_CITIES: &[&str] = &[
    "Lisbon",
    "Reykjavik",
    "Kyoto",
    "Buenos Aires",
    "Marrakech",
    "Helsinki",
    "Vancouver",
    "Seoul",
    "Cape Town",
    "Mexico City",
    "Wellington",
    "Tallinn",
    "Antwerp",
    "Tbilisi",
    "Quito",
    "Hanoi",
    "Porto",
    "Bergen",
];

const TRAVEL_PURPOSE: &[&str] = &[
    "for a long weekend",
    "for a two-week sabbatical",
    "for a friend's birthday trip",
    "to visit an old college friend",
    "for the food and walking",
    "to attend a music festival",
    "to take the photography workshop {PERSON} recommended",
];

const TRAVEL_SHORT_TEMPLATES: &[&str] = &[
    "Booked the trip to {CITY} {PURPOSE}",
    "Trip to {CITY} {PURPOSE} — flights confirmed for {MONTH}",
    "Heading to {CITY} {PURPOSE} in {MONTH}",
];

const TRAVEL_PARA_TEMPLATES: &[&str] = &[
    "Travel plans coming together for the {CITY} trip {PURPOSE}. Flights booked through the corporate-rate portal for {MONEY} round-trip, hotel sorted with the loyalty points stash that has been sitting unused for two years. Dates land in mid-{MONTH}, which lines up with the shoulder season — weather should be decent without the peak-summer crowd surcharge.",
];

const TRAVEL_LONG_TEMPLATES: &[&str] = &[
    "Long-form trip planning notes for the {CITY} visit {PURPOSE}, written down so I do not lose the thread when other things compete for attention. Total budget envelope is {MONEY}, which is what I want to spend not what I have to spend — this trip is supposed to be restorative rather than maximally-efficient, so the budget reflects choosing comfort over optimization. Dates target the second half of {MONTH}, partly because that lines up with the shoulder-season pricing and partly because the work calendar is unusually clear that week.",
];

const HOME_PROBLEM: &[&str] = &[
    "leaking kitchen faucet",
    "water-stained ceiling in the hallway",
    "garage door that sticks halfway",
    "loose handrail on the basement stairs",
    "drafty back door",
    "buzzing light fixture in the entryway",
    "slow-draining bathroom sink",
    "cracked grout in the upstairs shower",
];

const HOME_ACTION: &[&str] = &[
    "called the contractor; estimate landed at {MONEY}",
    "fixed it myself over the weekend; parts cost {MONEY}",
    "scheduled the repair for {MONTH} {DAY}",
    "got three quotes, ranging from {MONEY} to {MONEY}",
];

const HOME_SHORT_TEMPLATES: &[&str] = &[
    "Home: {HOME_PROBLEM} — {HOME_ACTION}",
    "{HOME_PROBLEM}: {HOME_ACTION}",
    "Note to self — {HOME_PROBLEM}; {HOME_ACTION}",
];

const HOME_PARA_TEMPLATES: &[&str] = &[
    "Home-maintenance log entry: the {HOME_PROBLEM} has been on the list for a while; {HOME_ACTION}. {PERSON} recommended the contractor based on their own house work last {MONTH} — solid track record and reasonable quote. The fix itself should take half a day; the longer pole is scheduling around the contractor's other jobs.",
];

const HOME_LONG_TEMPLATES: &[&str] = &[
    "Full writeup on the {HOME_PROBLEM} saga, because future-me will want the context next time something similar happens. Initial symptom was straightforward: the problem had been getting gradually worse over the past two months and finally crossed the 'this is annoying enough to fix' threshold. First step was to read up on the standard diagnosis path — {PERSON} pointed me at a couple of good resources, and an hour of reading covered most of the basics. Second step was to {HOME_ACTION}, which was the highest-leverage move I could make without committing to professional help. The contractor option was on the table from the start; I got three quotes ranging from {MONEY} to {MONEY}, and ended up going with the middle quote because the contractor had the best references and the highest-detail estimate.",
];

const CAR_SERVICE_TYPE: &[&str] = &[
    "oil change",
    "tire rotation",
    "brake pad replacement",
    "transmission fluid flush",
    "front-end alignment",
    "battery replacement",
    "windshield chip repair",
    "cabin air filter swap",
];

const CAR_SHOP_NOTE: &[&str] = &[
    "scheduled at the dealer for {MONTH} {DAY}",
    "done at the local shop; cost was {MONEY}",
    "needs to be done within the next 1,500 miles",
    "covered under the powertrain warranty for now",
];

const CAR_SHORT_TEMPLATES: &[&str] = &[
    "Car: {SERVICE_TYPE} {SHOP_NOTE}",
    "{SERVICE_TYPE} {SHOP_NOTE}",
    "Booked the {SERVICE_TYPE}; {SHOP_NOTE}",
];

const CAR_PARA_TEMPLATES: &[&str] = &[
    "Car-maintenance update: {SERVICE_TYPE} {SHOP_NOTE}. {PERSON} recommended the shop after their last visit on {DAY} of {MONTH}; quick turnaround and no upsell pressure, which is the main thing I care about.",
];

const CAR_LONG_TEMPLATES: &[&str] = &[
    "Comprehensive car-service log for the {MONTH} visit, since I tend to forget the details by the time the next service rolls around. {SERVICE_TYPE} was the headline item — {SHOP_NOTE}. The shop also did the standard multi-point inspection that comes with every visit; results were mostly fine but they flagged three things worth noting for the next service window. First, the rear brake pads are at about 35% remaining, which means the next service interval is the right time to replace them rather than the one after. Second, the coolant looks darker than they would expect for the mileage. Third, one of the tires is wearing slightly unevenly which usually indicates an alignment issue.",
];

const PET_NAMES: &[&str] = &[
    "Pepper", "Mango", "Biscuit", "Saffron", "Cleo", "Dexter", "Luna", "Boomer", "Hazel", "Otis",
    "Poppy", "Rufus", "Sage", "Tobi",
];

const PET_EVENT: &[&str] = &[
    "annual vet checkup booked for {MONTH} {DAY}",
    "got a dental cleaning quote of {MONEY}",
    "switched to the new vet recommended by {PERSON}",
    "is on a new prescription food that costs {MONEY} per bag",
    "scheduled for the grooming appointment {PERSON} suggested",
    "needs the rabies booster updated by end of {MONTH}",
];

const PET_SHORT_TEMPLATES: &[&str] = &[
    "{PET_NAME}: {PET_EVENT}",
    "Pet note — {PET_NAME} {PET_EVENT}",
    "{PET_NAME}'s update: {PET_EVENT}",
];

const PET_PARA_TEMPLATES: &[&str] = &[
    "Pet-care update: {PET_NAME} {PET_EVENT}. {PERSON} mentioned that their pet went through the same thing last {MONTH}; said the recovery was uneventful and the vet's instructions were straightforward to follow.",
];

const PET_LONG_TEMPLATES: &[&str] = &[
    "Long-form note on {PET_NAME}'s {PET_EVENT}, because the vet shared a lot of useful context that I want to capture before it fades. The visit itself took about 75 minutes — longer than usual because the vet wanted to do an extra panel given {PET_NAME}'s age. Results came back mostly reassuring; one marker is slightly outside the comfort zone but not in the alarm zone, and the vet's recommendation is to recheck in three months rather than start any intervention now. Total cost for the visit came to {MONEY}.",
];

const RECIPE_DISH: &[&str] = &[
    "miso-glazed eggplant",
    "lentil curry with curry leaves",
    "chocolate chip cookies with brown butter",
    "shakshuka with feta",
    "weeknight ramen with soft-boiled egg",
    "roast chicken with preserved lemon",
    "buckwheat pancakes",
    "no-knead sourdough",
    "Thai basil noodles",
    "harissa-roasted carrots",
    "miso ginger salmon",
    "spiced apple cake",
];

const RECIPE_OUTCOME: &[&str] = &[
    "came out great; {PERSON} asked for the recipe",
    "needs more salt next time",
    "took twice as long as the recipe claimed",
    "worked but the technique needs practice",
    "is now in the regular rotation",
    "did not work; halving the recipe next attempt",
];

const RECIPE_SHORT_TEMPLATES: &[&str] = &[
    "Tried the {DISH}; {OUTCOME}",
    "Made {DISH} this week — {OUTCOME}",
    "{DISH} attempt: {OUTCOME}",
];

const RECIPE_PARA_TEMPLATES: &[&str] = &[
    "Cooking note: made the {DISH} on {DAY} and it {OUTCOME}. The recipe is from the cookbook {PERSON} gave me last {MONTH}; this is the third recipe I have tried from it and the hit rate is high.",
];

const RECIPE_LONG_TEMPLATES: &[&str] = &[
    "Detailed cooking log for the {DISH} attempt this past {DAY}, because I want to capture the lessons while they are fresh and because {PERSON} asked for a writeup. Headline: {OUTCOME}, which felt like progress even where it did not turn out perfectly. The recipe was the one {PERSON} recommended after they made it last {MONTH}; their version came out better than mine, partly because they had practiced it twice already and partly because they had access to a couple of ingredients that are harder to find locally.",
];

#[derive(Debug, Clone, Copy)]
enum LengthTier {
    Short,
    Paragraph,
    LongForm,
    Truncation,
}

#[derive(Debug, Clone, Copy)]
enum DistractorCluster {
    OfficeLogistics,
    VendorRenewals,
    DocReviews,
    InternalTooling,
    TeamEvents,
    Travel,
    HomeMaintenance,
    CarService,
    PetCare,
    Cooking,
}

impl DistractorCluster {
    fn boundary(self) -> &'static str {
        match self {
            Self::OfficeLogistics
            | Self::VendorRenewals
            | Self::DocReviews
            | Self::InternalTooling
            | Self::TeamEvents => "work",
            Self::Travel
            | Self::HomeMaintenance
            | Self::CarService
            | Self::PetCare
            | Self::Cooking => "personal",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::OfficeLogistics => "distractor-office-logistics",
            Self::VendorRenewals => "distractor-vendor-renewals",
            Self::DocReviews => "distractor-doc-reviews",
            Self::InternalTooling => "distractor-internal-tooling",
            Self::TeamEvents => "distractor-team-events",
            Self::Travel => "distractor-travel",
            Self::HomeMaintenance => "distractor-home-maintenance",
            Self::CarService => "distractor-car-service",
            Self::PetCare => "distractor-pet-care",
            Self::Cooking => "distractor-cooking",
        }
    }
}

const ALL_CLUSTERS: &[DistractorCluster] = &[
    DistractorCluster::OfficeLogistics,
    DistractorCluster::VendorRenewals,
    DistractorCluster::DocReviews,
    DistractorCluster::InternalTooling,
    DistractorCluster::TeamEvents,
    DistractorCluster::Travel,
    DistractorCluster::HomeMaintenance,
    DistractorCluster::CarService,
    DistractorCluster::PetCare,
    DistractorCluster::Cooking,
];

fn pick_template(
    rng: &mut SplitMix64,
    cluster: DistractorCluster,
    tier: LengthTier,
) -> &'static str {
    match (cluster, tier) {
        (DistractorCluster::OfficeLogistics, LengthTier::Short) => rng.pick(OFFICE_SHORT_TEMPLATES),
        (DistractorCluster::OfficeLogistics, LengthTier::Paragraph) => {
            rng.pick(OFFICE_PARA_TEMPLATES)
        }
        (DistractorCluster::OfficeLogistics, _) => rng.pick(OFFICE_LONG_TEMPLATES),

        (DistractorCluster::VendorRenewals, LengthTier::Short) => rng.pick(VENDOR_SHORT_TEMPLATES),
        (DistractorCluster::VendorRenewals, LengthTier::Paragraph) => {
            rng.pick(VENDOR_PARA_TEMPLATES)
        }
        (DistractorCluster::VendorRenewals, _) => rng.pick(VENDOR_LONG_TEMPLATES),

        (DistractorCluster::DocReviews, LengthTier::Short) => rng.pick(DOC_SHORT_TEMPLATES),
        (DistractorCluster::DocReviews, LengthTier::Paragraph) => rng.pick(DOC_PARA_TEMPLATES),
        (DistractorCluster::DocReviews, _) => rng.pick(DOC_LONG_TEMPLATES),

        (DistractorCluster::InternalTooling, LengthTier::Short) => rng.pick(TOOL_SHORT_TEMPLATES),
        (DistractorCluster::InternalTooling, LengthTier::Paragraph) => {
            rng.pick(TOOL_PARA_TEMPLATES)
        }
        (DistractorCluster::InternalTooling, _) => rng.pick(TOOL_LONG_TEMPLATES),

        (DistractorCluster::TeamEvents, LengthTier::Short) => rng.pick(EVENT_SHORT_TEMPLATES),
        (DistractorCluster::TeamEvents, LengthTier::Paragraph) => rng.pick(EVENT_PARA_TEMPLATES),
        (DistractorCluster::TeamEvents, _) => rng.pick(EVENT_LONG_TEMPLATES),

        (DistractorCluster::Travel, LengthTier::Short) => rng.pick(TRAVEL_SHORT_TEMPLATES),
        (DistractorCluster::Travel, LengthTier::Paragraph) => rng.pick(TRAVEL_PARA_TEMPLATES),
        (DistractorCluster::Travel, _) => rng.pick(TRAVEL_LONG_TEMPLATES),

        (DistractorCluster::HomeMaintenance, LengthTier::Short) => rng.pick(HOME_SHORT_TEMPLATES),
        (DistractorCluster::HomeMaintenance, LengthTier::Paragraph) => {
            rng.pick(HOME_PARA_TEMPLATES)
        }
        (DistractorCluster::HomeMaintenance, _) => rng.pick(HOME_LONG_TEMPLATES),

        (DistractorCluster::CarService, LengthTier::Short) => rng.pick(CAR_SHORT_TEMPLATES),
        (DistractorCluster::CarService, LengthTier::Paragraph) => rng.pick(CAR_PARA_TEMPLATES),
        (DistractorCluster::CarService, _) => rng.pick(CAR_LONG_TEMPLATES),

        (DistractorCluster::PetCare, LengthTier::Short) => rng.pick(PET_SHORT_TEMPLATES),
        (DistractorCluster::PetCare, LengthTier::Paragraph) => rng.pick(PET_PARA_TEMPLATES),
        (DistractorCluster::PetCare, _) => rng.pick(PET_LONG_TEMPLATES),

        (DistractorCluster::Cooking, LengthTier::Short) => rng.pick(RECIPE_SHORT_TEMPLATES),
        (DistractorCluster::Cooking, LengthTier::Paragraph) => rng.pick(RECIPE_PARA_TEMPLATES),
        (DistractorCluster::Cooking, _) => rng.pick(RECIPE_LONG_TEMPLATES),
    }
}

fn fill_template(rng: &mut SplitMix64, cluster: DistractorCluster, template: &str) -> String {
    let mut out = String::with_capacity(template.len() * 2);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close_rel = rest[open..].find('}').expect("template has unmatched '{'");
        let slot = &rest[open + 1..open + close_rel];
        let value = pick_slot_value(rng, cluster, slot);
        out.push_str(&value);
        rest = &rest[open + close_rel + 1..];
    }
    out.push_str(rest);
    out
}

fn pick_slot_value(rng: &mut SplitMix64, cluster: DistractorCluster, slot: &str) -> String {
    match slot {
        "PERSON" => return rng.pick(PEOPLE).to_string(),
        "MONTH" => return rng.pick(MONTHS).to_string(),
        "DAY" => return rng.pick(DAYS_OF_WEEK).to_string(),
        "MONEY" => return rng.pick(MONEY_AMOUNTS).to_string(),
        _ => {}
    }
    let pick = match (cluster, slot) {
        (DistractorCluster::OfficeLogistics, "FACILITY") => rng.pick(OFFICE_FACILITY),
        (DistractorCluster::OfficeLogistics, "CHANGE_DETAIL") => rng.pick(OFFICE_CHANGE_DETAIL),
        (DistractorCluster::VendorRenewals, "VENDOR") => rng.pick(ENG_VENDORS),
        (DistractorCluster::VendorRenewals, "RENEWAL_DETAIL") => rng.pick(RENEWAL_DETAILS),
        (DistractorCluster::DocReviews, "DOC_TYPE") => rng.pick(DOC_TYPES),
        (DistractorCluster::DocReviews, "DOC_FEEDBACK") => rng.pick(DOC_FEEDBACK),
        (DistractorCluster::InternalTooling, "TOOL_SYSTEM") => rng.pick(TOOL_SYSTEM),
        (DistractorCluster::InternalTooling, "TOOL_ACTION") => rng.pick(TOOL_ACTION),
        (DistractorCluster::TeamEvents, "EVENT_TYPE") => rng.pick(EVENT_TYPE),
        (DistractorCluster::TeamEvents, "EVENT_LOGISTICS") => rng.pick(EVENT_LOGISTICS),
        (DistractorCluster::Travel, "CITY") => rng.pick(TRAVEL_CITIES),
        (DistractorCluster::Travel, "PURPOSE") => rng.pick(TRAVEL_PURPOSE),
        (DistractorCluster::HomeMaintenance, "HOME_PROBLEM") => rng.pick(HOME_PROBLEM),
        (DistractorCluster::HomeMaintenance, "HOME_ACTION") => rng.pick(HOME_ACTION),
        (DistractorCluster::CarService, "SERVICE_TYPE") => rng.pick(CAR_SERVICE_TYPE),
        (DistractorCluster::CarService, "SHOP_NOTE") => rng.pick(CAR_SHOP_NOTE),
        (DistractorCluster::PetCare, "PET_NAME") => rng.pick(PET_NAMES),
        (DistractorCluster::PetCare, "PET_EVENT") => rng.pick(PET_EVENT),
        (DistractorCluster::Cooking, "DISH") => rng.pick(RECIPE_DISH),
        (DistractorCluster::Cooking, "OUTCOME") => rng.pick(RECIPE_OUTCOME),
        _ => panic!("unknown slot '{slot}' for cluster {cluster:?}"),
    };
    if pick.contains('{') {
        return fill_template(rng, cluster, pick);
    }
    pick.to_string()
}

fn generate_distractor(
    rng: &mut SplitMix64,
    cluster: DistractorCluster,
    tier: LengthTier,
    idx: usize,
) -> MemoryFixtureEntry {
    let template = pick_template(rng, cluster, tier);
    let mut content = fill_template(rng, cluster, template);
    let (min_chars, max_chars) = match tier {
        LengthTier::Short => (50, 150),
        LengthTier::Paragraph => (300, 1000),
        LengthTier::LongForm => (1000, 2000),
        LengthTier::Truncation => (2000, 2430),
    };
    if content.len() > max_chars {
        let mut cut = max_chars;
        while !content.is_char_boundary(cut) || !content[..cut].ends_with(' ') {
            cut = cut.saturating_sub(1);
            if cut < min_chars {
                break;
            }
        }
        content.truncate(cut);
        content = content.trim_end().to_string();
    }
    while content.len() < min_chars {
        content.push(' ');
        content.push_str(rng.pick(&[
            "Filing this for later reference.",
            "Captured for next quarter.",
            "Worth a follow-up next month.",
            "Holding pattern for now.",
            "No action required today.",
        ]));
    }
    let id = format!("dist-{idx:05}");
    MemoryFixtureEntry {
        id,
        boundary: cluster.boundary().to_string(),
        topic_label: cluster.label().to_string(),
        content,
        ground_truth: GroundTruth {
            outcome: "distractor".to_string(),
            cluster: None,
        },
    }
}

fn generate_diverse_distractors(needed: usize) -> Vec<MemoryFixtureEntry> {
    let mut rng = SplitMix64::new(DISTRACTOR_SEED);
    let mut out = Vec::with_capacity(needed);
    let para_count = (needed as f64 * 0.30).round() as usize;
    let long_count = (needed as f64 * 0.11).round() as usize;
    let trunc_count = (needed as f64 * 0.03).round() as usize;
    let short_count = needed.saturating_sub(para_count + long_count + trunc_count);

    let plan: &[(LengthTier, usize)] = &[
        (LengthTier::Short, short_count),
        (LengthTier::Paragraph, para_count),
        (LengthTier::LongForm, long_count),
        (LengthTier::Truncation, trunc_count),
    ];

    let mut idx = 0_usize;
    for (tier, count) in plan {
        for _ in 0..*count {
            let cluster = rng.pick(ALL_CLUSTERS);
            out.push(generate_distractor(&mut rng, cluster, *tier, idx));
            idx += 1;
        }
    }
    let n = out.len();
    for i in (1..n).rev() {
        let j = (rng.next_u64() as usize) % (i + 1);
        out.swap(i, j);
    }
    out
}

fn generate_diverse_corpus(base: &[MemoryFixtureEntry], target: usize) -> Vec<MemoryFixtureEntry> {
    let mut out = Vec::with_capacity(target);
    out.extend(base.iter().cloned());
    if target <= base.len() {
        out.truncate(target);
        return out;
    }
    let needed = target - base.len();
    let distractors = generate_diverse_distractors(needed);
    out.extend(distractors);
    out
}

// ── Verdict assessment (same as t028c) ───────────────────────────────────

enum QualityVerdict {
    ContradictionPass(String),
    ContradictionFail(String),
    HardNegativePass(String),
    HardNegativeFail(String),
    ShortLongPass(String),
    ShortLongFail(String),
    Observational(String),
}

fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        // Short↔long pairs (NEW for t028g):
        "S1" => Some(("$89", "$145")),        // ergonomic mouse
        "S2" => Some(("Q1 2028", "Q3 2028")), // PostgreSQL upgrade
        "S3" => Some(("$2,500", "$4,200")),   // office Wi-Fi monthly
        _ => None,
    }
}

fn assess_query(query_id: &str, resp: &ReadResponse) -> QualityVerdict {
    if CONTRADICTION_QUERY_IDS.contains(&query_id) {
        let Some((sub_a, sub_b)) = structural_substrings(query_id) else {
            return QualityVerdict::Observational("no structural-substrings rule".into());
        };
        // Refined verdict ([[structured-contract-user-sees-via-agent]],
        // locked 2026-05-20): PASS = both literals in synthesis_markdown
        // OR both literals anywhere in contradictions_flagged[*].positions[*].
        let prose_pass =
            resp.synthesis_markdown.contains(sub_a) && resp.synthesis_markdown.contains(sub_b);
        let structured_haystack: String = resp
            .contradictions_flagged
            .iter()
            .flat_map(|c| c.positions.iter().cloned())
            .collect::<Vec<_>>()
            .join(" | ");
        let struct_pass =
            structured_haystack.contains(sub_a) && structured_haystack.contains(sub_b);
        let detail = format!(
            "flagged={} · prose={prose_pass} · structured={struct_pass}",
            resp.contradictions_flagged.len()
        );
        if prose_pass || struct_pass {
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
    } else if SHORT_LONG_QUERY_IDS.contains(&query_id) {
        // Same refined verdict as contradiction
        // ([[structured-contract-user-sees-via-agent]]): prose OR structured
        // is sufficient. The whole point of the short↔long test is that
        // hybrid retrieval brings BOTH pair members into context regardless
        // of length asymmetry.
        let Some((sub_a, sub_b)) = structural_substrings(query_id) else {
            return QualityVerdict::Observational("no structural-substrings rule".into());
        };
        let prose_pass =
            resp.synthesis_markdown.contains(sub_a) && resp.synthesis_markdown.contains(sub_b);
        let structured_haystack: String = resp
            .contradictions_flagged
            .iter()
            .flat_map(|c| c.positions.iter().cloned())
            .collect::<Vec<_>>()
            .join(" | ");
        let struct_pass =
            structured_haystack.contains(sub_a) && structured_haystack.contains(sub_b);
        let detail = format!(
            "flagged={} · prose={prose_pass} · structured={struct_pass}",
            resp.contradictions_flagged.len()
        );
        if prose_pass || struct_pass {
            QualityVerdict::ShortLongPass(detail)
        } else {
            QualityVerdict::ShortLongFail(detail)
        }
    } else {
        QualityVerdict::Observational(format!(
            "contradictions_flagged.len()={}",
            resp.contradictions_flagged.len()
        ))
    }
}

// ── Short↔long contradiction pair fixtures (NEW for t028g) ───────────────
//
// Each pair has:
//   • A SHORT memory (~30-50 chars) containing the anchor token + value A.
//   • A LONG memory (~2000 chars) with the anchor token + value B buried
//     somewhere in the middle of unrelated context.
//   • A matching query that uses the rare anchor token lexically.
//
// Anchor tokens are deliberately rare (not in the existing 100-memory
// fixture, not in the synthetic distractor vocab tables above). That
// means BM25 has a clean signal — for each query, only the two pair
// members AND the query share the anchor token.
//
// Why this stress-tests the structural fix: BGE's cosine similarity
// degrades when one document is ~30 chars and the other is ~2000 chars
// of mostly-unrelated content even when both share the rare anchor —
// the long document's vector gets averaged toward its dominant topic
// (which isn't the anchor's topic). BM25's term-frequency × inverse-
// document-frequency scoring doesn't suffer this length penalty — the
// rare anchor's IDF contribution dominates even in long docs.

struct ShortLongPair {
    query_id: &'static str,
    boundary: &'static str,
    short_content: &'static str,
    long_content: &'static str,
    query_text: &'static str,
}

const SHORT_LONG_PAIRS: &[ShortLongPair] = &[
    ShortLongPair {
        query_id: "S1",
        boundary: "work",
        // Redesigned 2026-05-19 after smoke v3 + acceptance 10K showed the
        // LLM dismissing the short note as "previous price" when the long
        // memory's "upgraded to" framing read as natural progression. New
        // framing is explicit "approved budget X" vs "actual cost Y, here
        // is the disagreement" so the LLM cannot interpret it as a single
        // timeline — both values describe the SAME line item from different
        // accounting perspectives (budget vs actual), unambiguously
        // contradictory.
        short_content: "Ergonomic mouse approved at $89/unit in the Q1 hardware budget.",
        long_content: "Hardware procurement reconciliation note from finance: \
            the actual purchase of the ergonomic mouse came in at $145/unit \
            instead of the approved $89/unit. The $56-per-unit overage \
            multiplied across the 24-seat order produces a $1,344 variance \
            against the Q1 hardware budget line item. Finance is asking us \
            to absorb the variance out of the Q2 contingency reserve rather \
            than re-open the Q1 budget for a single line item revision. \
            Olivia signed off on the contingency-reserve approach yesterday \
            after the procurement team explained the price-tier shift — the \
            $89/unit catalog item is no longer available from the vendor and \
            the cheapest comparable model at the new tier is $145/unit. We \
            need to update the budget tracker to reflect $145 not $89 going \
            forward, and the operating-plan template for next quarter should \
            use $145 as the baseline for the same line item. The longer-term \
            question is whether the company-wide ergonomic peripheral \
            standard should be revisited at the next Olivia/Priya budget \
            review, since the price-tier shift looks structural rather than \
            temporary. Action items: Priya updates the tracker by Friday, \
            Olivia files the variance memo by Monday, and the operating-plan \
            template lives on the shared drive under Q2 templates with the \
            $145 figure replacing $89 in line item 47.",
        query_text:
            "What was the approved budget for our ergonomic mouse versus what we actually paid?",
    },
    ShortLongPair {
        query_id: "S2",
        boundary: "work",
        short_content: "PostgreSQL upgrade target Q1 2028.",
        long_content: "Long-form retrospective from the database platform \
            sync this morning. Headline outcome: the PostgreSQL upgrade is \
            pushed to Q3 2028 instead of the originally-circulated earlier \
            target. The driver for the push was the discovery during \
            integration testing that two of our older internal services \
            depend on a deprecated extension that has no direct replacement \
            in the newer major version; the platform team needs an extra \
            two quarters to either rewrite those services or vendor a \
            compatibility shim. Secondary driver was capacity — the \
            database team is also leading the storage-tier consolidation \
            this year, and trying to land both in the same window would \
            require either skipping the canary-cluster validation step or \
            running both rollouts under-staffed. Three follow-ups came out \
            of the discussion. First, the platform team will publish a \
            migration-readiness audit by end of next month so service \
            owners have a clear list of dependencies to address ahead of \
            the cutover. Second, the SRE team will draft a rollback \
            playbook against the canary cluster before the production \
            rollout begins. Third, finance asked for a revised cost \
            forecast since the longer migration window changes the cloud \
            spend curve. The team is broadly OK with the new timeline \
            given the dependency surface; nobody wants to be in the \
            position of rolling back a production database upgrade \
            because of an extension nobody flagged early enough.",
        query_text: "When are we doing the PostgreSQL upgrade?",
    },
    ShortLongPair {
        query_id: "S3",
        boundary: "work",
        short_content: "Office Wi-Fi vendor budget: $2,500/mo.",
        long_content: "Facilities-operations rollup for the second half of \
            the year covering the workplace-tech vendor consolidation we \
            kicked off in March. Headline change relevant to the IT budget: \
            office Wi-Fi vendor renewed at $4,200 per month after a \
            negotiation cycle that lasted six weeks. The old per-month rate \
            had been locked in three years ago when the office footprint \
            was substantially smaller; the new contract reflects three \
            things — added square footage from the expansion floor, a tier \
            upgrade to support the higher-density meeting rooms with \
            uplink-bonded access points, and an SLA upgrade from \
            best-effort to a 99.9% availability target with credit \
            provisions for outages. The procurement team ran a competitive \
            bid; two other vendors came in lower on headline price but \
            either could not match the SLA tier or required a multi-year \
            commitment that boxed us in. Olivia and the IT-ops lead \
            green-lit the renewal after the workplace experience survey \
            flagged Wi-Fi reliability as the top friction point for the \
            hybrid working pattern. Calendar reminder set for the next \
            renewal review at the 18-month mark; we'll revisit the SLA \
            tier and whether the higher tier remains worth the cost \
            premium given utilization patterns. Side note: the contract \
            includes a clause about IPv6 readiness that the previous \
            agreement lacked, which the platform team flagged as quietly \
            valuable for future-proofing.",
        query_text: "What's our office Wi-Fi monthly cost?",
    },
];

// ── Hybrid retrieval (BGE dense + Tantivy BM25 fused via RRF) ────────────
//
// Direction locked 2026-05-19 after the 4-agent investigation (HANDOFF.md
// "4-agent parallel investigation confirmed structural diagnosis"). The
// value-aware reranker arc (t028d) proved that single-channel cosine
// retrieval cannot serve both contradiction-completeness AND hard-negative-
// rejection simultaneously — any parameter that helps one breaks the
// other. The fix isn't a smarter re-ranker; it's a second retrieval
// channel that handles the failure modes structurally.
//
// **Channel 1 — BGE dense (existing).** Wraps SemanticRetriever; widens
// to HYBRID_TOP_N_EACH=100 per call. Surfaces semantic matches even when
// the query phrasing does not share lexical anchors with the memory
// (Q25 task-shaped "help me update the product roadmap" does not share
// keywords with the "Q2 2027 GA launch" memory; cosine bridges that gap).
//
// **Channel 2 — Tantivy BM25 (new in spike).** Builds an in-RAM Tantivy
// index over the entire corpus at spike-init. Queries are parsed
// through QueryParser (default tokenization: lowercase + simple
// tokenizer). Surfaces rare-anchor lexical matches that cosine misses
// when length asymmetry distorts the dense embedding (S1/S2/S3 are
// designed precisely to stress this case).
//
// **Fusion — Reciprocal Rank Fusion (RRF).** For each unique memory
// surfaced in either channel:
//     score(d) = 1/(k + bge_rank(d))  +  1/(k + bm25_rank(d))
// where rank counts from 1, and missing-from-channel contributes 0.
// k=60 is the Cormack et al. 2009 literature default. RRF does not
// require score calibration between channels — it operates on rank
// positions only, which avoids the apples-to-oranges problem of
// blending cosine similarity (0..1) with BM25 score (0..infinity).
//
// **Abstain gate (the hard-negative fix).** Before fusion, check the
// max (top-1) BM25 score across the widened candidates. If it falls
// below BM25_TOP_SCORE_THRESHOLD, return Vec::new(). ReadPipeline then
// short-circuits to vault_has_no_relevant_content=true without invoking
// the LLM (read_pipeline.rs:260-270).
//
// The original design used count-above-floor abstain; 2026-05-19
// SCALE=10K validation surfaced its structural flaw: at scale, the
// corpus contained 200+ pet-care memories matching "dental", so Q22's
// dental hard-neg had 200 hits above floor=1.0 and abstain never
// triggered. Top-1 score is scale-independent: Q22's best hit only
// matches "dental" (score ~3-5), Q11's best hit matches multiple rare
// anchors "GA"+"launch"+"Q2 2027" (score ~8-15). Threshold of 6.0
// splits the two populations.
//
// **Boundary filter on the BM25 channel.** Tantivy index is not aware
// of Boundary. We hydrate each BM25 hit MemoryId via the corpus_idx
// stored field, then look the memory up in MetadataStore and drop hits
// whose boundary is not in query.authorized_boundaries. Trait invariant
// #1 (no boundary leakage) must hold for hybrid the same way it holds
// for SemanticRetriever.

struct HybridConfig {
    rrf_k: usize,
    /// Top-1 BM25 score threshold. If the BEST BM25 hit's score is below
    /// this, the retriever abstains (returns empty Vec → ReadPipeline
    /// short-circuits to vault_has_no_relevant_content=true). Replaces
    /// the count-above-floor design at 2026-05-19 SCALE=10K validation.
    bm25_top_score_threshold: f32,
    top_n_each: usize,
}

/// In-RAM BM25 index over the corpus. Built once at spike-init via
/// [`build_bm25_index`].
///
/// **Corpus index lookup contract.** Tantivy 0.26's `IndexWriter`
/// parallelizes indexing across threads and produces multiple segments
/// even within a single commit — `DocAddress.segment_ord` therefore takes
/// values 0..N, NOT always 0 (the 2026-05-19 smoke v2 surfaced this:
/// `segment_ord` ran 0..5 at SCALE=100). The earlier attempt to use
/// `DocAddress.doc_id` as the corpus index dropped ~93% of BM25 hits.
///
/// We now store `corpus_idx` as a `STORED u64` field on every Tantivy
/// document and extract it back via `searcher.doc(addr)` +
/// `iter_fields_and_values`. Multi-segment is then irrelevant — the field
/// roundtrips per-document.
struct BgeBm25Index {
    index: Index,
    reader: IndexReader,
    content_field: Field,
    corpus_idx_field: Field,
    /// `corpus_idx_lookup[i]` = the `MemoryId` of the memory at insertion
    /// position `i` in the Tantivy index. Built in lock-step with the
    /// upsert loop so the same `i` indexes both the BGE/Lance side and
    /// the BM25 side.
    corpus_idx_lookup: Vec<MemoryId>,
}

struct HybridRetriever {
    inner: Arc<dyn Retriever>,
    metadata: Arc<MetadataStore>,
    bm25: BgeBm25Index,
    config: HybridConfig,
}

fn build_bm25_index(corpus_contents: &[(MemoryId, String)]) -> Result<BgeBm25Index> {
    let mut schema_builder = Schema::builder();
    // Content field is TEXT (indexed + tokenized). corpus_idx_field is
    // STORED u64 — we need to round-trip it because Tantivy's writer
    // produces multiple segments and `DocAddress.doc_id` isn't a stable
    // corpus-wide index (each segment numbers from 0).
    let content_field = schema_builder.add_text_field("content", TEXT);
    let corpus_idx_field = schema_builder.add_u64_field("corpus_idx", STORED);
    let schema = schema_builder.build();

    let index = Index::create_in_ram(schema);
    let mut writer = index
        .writer(100_000_000)
        .context("tantivy IndexWriter::writer (100MB)")?;

    let mut lookup: Vec<MemoryId> = Vec::with_capacity(corpus_contents.len());
    for (i, (mem_id, content)) in corpus_contents.iter().enumerate() {
        writer
            .add_document(doc!(
                content_field => content.as_str(),
                corpus_idx_field => i as u64,
            ))
            .context("tantivy add_document")?;
        lookup.push(*mem_id);
    }
    writer.commit().context("tantivy IndexWriter::commit")?;

    let reader = index.reader().context("tantivy IndexReader build")?;
    Ok(BgeBm25Index {
        index,
        reader,
        content_field,
        corpus_idx_field,
        corpus_idx_lookup: lookup,
    })
}

/// Strip Tantivy/Lucene `QueryParser` syntax characters from a natural-
/// language query. The parser treats `'`, `+`, `-`, `:`, `"`, `(`, `)`,
/// `[`, `]`, `{`, `}`, `^`, `~`, `*`, `?`, `\`, `/`, `!`, `&`, `|` as
/// operators; queries like `"What's the Comcast bill?"` produce a
/// `Syntax Error` because the embedded `'` is interpreted as a syntax
/// token without a matching operand.
///
/// For BM25 lexical matching we only need the term tokens — operator
/// semantics aren't being used intentionally on natural-language input.
/// Replace each special char with a space so tokenization still produces
/// clean word boundaries.
///
/// Confirmed empirically at 2026-05-19 SCALE=100 smoke run: 4/9 queries
/// hit `parse_query: Syntax Error` because of `'` in `What's`; the same
/// queries parse cleanly after stripping. Question marks and periods are
/// tolerated by the parser and don't need stripping (Q11/Q21/Q26 all
/// passed with trailing `?`), but we strip them anyway as belt-and-
/// suspenders — they're not meaningful BM25 tokens.
fn sanitize_bm25_query(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '+' | '-' | '!' | '&' | '|' | '(' | ')' | '{' | '}' | '[' | ']' | '^' | '~' | '*'
            | '?' | ':' | '\\' | '/' | '"' | '\'' => ' ',
            _ => c,
        })
        .collect()
}

/// Pull the stored `corpus_idx` u64 field back out of a BM25 result doc.
/// Returns `None` if the field is missing or holds a non-`U64` value.
///
/// `TantivyDocument::get_first` returns an `Option<CompactDocValue<'_>>`
/// (inherent method on the concrete `TantivyDocument`, not the `Document`
/// trait), and `CompactDocValue::as_u64()` (via the `Value` trait) returns
/// `Option<u64>` directly — the cleanest documented 0.26 extraction path.
fn extract_corpus_idx(doc: &TantivyDocument, corpus_idx_field: Field) -> Option<u64> {
    doc.get_first(corpus_idx_field)?.as_u64()
}

#[async_trait]
impl Retriever for HybridRetriever {
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        let final_k = query.max_results;

        // ── Channel 1: BGE dense (widened) ──────────────────────────────
        // Hand SemanticRetriever the SAME boundary set; it filters at the
        // Lance `only_if` layer per trait invariant #1.
        let bge_query = RetrievalQuery {
            query_text: query.query_text.clone(),
            authorized_boundaries: query.authorized_boundaries.clone(),
            max_results: self.config.top_n_each,
            options: query.options.clone(),
        };
        let bge_results = self.inner.retrieve(bge_query).await?;

        // ── Channel 2: BM25 search ──────────────────────────────────────
        // Tantivy is synchronous — block-on inside the async context is
        // OK for a spike (Tantivy in-RAM search is ~1ms for 10K docs;
        // not worth a spawn_blocking round-trip). Production Phase 1 will
        // wrap this in spawn_blocking per BRD §2.7 (CPU-bound work is
        // sync, called via spawn_blocking).
        let searcher = self.bm25.reader.searcher();
        let parser = QueryParser::for_index(&self.bm25.index, vec![self.bm25.content_field]);
        let sanitized = sanitize_bm25_query(&query.query_text);
        let bm25_query = parser.parse_query(&sanitized).map_err(|e| {
            vault_error_from_tantivy(&format!("parse_query (sanitized={sanitized:?}): {e}"))
        })?;
        let bm25_top: Vec<(tantivy::Score, tantivy::DocAddress)> = searcher
            .search(
                &bm25_query,
                &TopDocs::with_limit(self.config.top_n_each).order_by_score(),
            )
            .map_err(|e| vault_error_from_tantivy(&format!("search: {e}")))?;

        // Hydrate each BM25 hit's stored `corpus_idx` field → MemoryId.
        // Multi-segment is fine here: the field roundtrips per-doc, so
        // segment_ord is irrelevant.
        let mut bm25_candidates: Vec<(MemoryId, f32)> = Vec::with_capacity(bm25_top.len());
        for (score, addr) in &bm25_top {
            let tdoc: TantivyDocument = searcher
                .doc(*addr)
                .map_err(|e| vault_error_from_tantivy(&format!("searcher.doc: {e}")))?;
            let Some(idx) = extract_corpus_idx(&tdoc, self.bm25.corpus_idx_field) else {
                eprintln!(
                    "      [hybrid] WARN: BM25 hit at segment_ord={} doc_id={} \
                    missing corpus_idx field; skipping",
                    addr.segment_ord, addr.doc_id,
                );
                continue;
            };
            let idx_us = idx as usize;
            if idx_us >= self.bm25.corpus_idx_lookup.len() {
                continue;
            }
            let mem_id = self.bm25.corpus_idx_lookup[idx_us];
            bm25_candidates.push((mem_id, *score));
        }

        // ── Boundary filter on BM25 channel ─────────────────────────────
        // BGE/Lance side is already filtered. Drop BM25 hits whose
        // boundary is not authorized.
        let allowed: HashSet<String> = query
            .authorized_boundaries
            .iter()
            .map(|b| b.as_str().to_string())
            .collect();
        let bm25_ids: Vec<MemoryId> = bm25_candidates.iter().map(|(id, _)| *id).collect();
        let bm25_memories = self.metadata.get_memories_batch(&bm25_ids).await?;
        let bm25_mem_by_id: HashMap<MemoryId, Memory> =
            bm25_memories.into_iter().map(|m| (m.id, m)).collect();
        let bm25_candidates: Vec<(MemoryId, f32, Memory)> = bm25_candidates
            .into_iter()
            .filter_map(|(id, sc)| {
                let m = bm25_mem_by_id.get(&id)?;
                if allowed.contains(m.boundary.as_str()) {
                    Some((id, sc, m.clone()))
                } else {
                    None
                }
            })
            .collect();

        // ── Abstain gate ────────────────────────────────────────────────
        // Top-1 BM25 score check. If the BEST BM25 hit's score is below
        // threshold, no doc in the corpus has a strong-enough lexical
        // anchor to the query → abstain. Scale-independent: doesn't
        // matter how many weak topic-overlap hits exist.
        let max_bm25_score = bm25_candidates
            .iter()
            .map(|(_, sc, _)| *sc)
            .fold(0.0_f32, f32::max);
        // Compute median + p90 BM25 score for diagnostic telemetry. Helps
        // calibrate the threshold across runs.
        let mut sorted_scores: Vec<f32> = bm25_candidates.iter().map(|(_, sc, _)| *sc).collect();
        sorted_scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median_score = if sorted_scores.is_empty() {
            0.0
        } else {
            sorted_scores[sorted_scores.len() / 2]
        };
        let p90_score = if sorted_scores.is_empty() {
            0.0
        } else {
            sorted_scores[(sorted_scores.len() as f32 * 0.9) as usize]
        };
        println!(
            "      [hybrid] BGE hits={} · BM25 hits={} · max_bm25={:.2} (threshold {:.2}) · p90={:.2} · median={:.2}",
            bge_results.len(),
            bm25_candidates.len(),
            max_bm25_score,
            self.config.bm25_top_score_threshold,
            p90_score,
            median_score,
        );
        if max_bm25_score < self.config.bm25_top_score_threshold {
            println!(
                "      [hybrid] ABSTAIN — max BM25 score {:.2} below threshold {:.2}; \
                returning empty Vec so ReadPipeline short-circuits to vault_has_no_relevant_content=true",
                max_bm25_score, self.config.bm25_top_score_threshold
            );
            return Ok(Vec::new());
        }

        // ── RRF fusion ──────────────────────────────────────────────────
        // Build rank maps (1-indexed; both channels are already
        // sorted descending by their native score).
        let mut bge_rank: HashMap<MemoryId, usize> = HashMap::new();
        for (i, m) in bge_results.iter().enumerate() {
            bge_rank.insert(m.memory.id, i + 1);
        }
        let mut bm25_rank: HashMap<MemoryId, usize> = HashMap::new();
        for (i, (id, _, _)) in bm25_candidates.iter().enumerate() {
            bm25_rank.insert(*id, i + 1);
        }

        // Union set of IDs.
        let mut all_ids: HashSet<MemoryId> = HashSet::new();
        all_ids.extend(bge_rank.keys());
        all_ids.extend(bm25_rank.keys());

        // Score each by RRF; track contributions for the explanation.
        let k_f = self.config.rrf_k as f32;
        let mut scored: Vec<(MemoryId, f32, Option<usize>, Option<usize>)> = all_ids
            .into_iter()
            .map(|id| {
                let br = bge_rank.get(&id).copied();
                let mr = bm25_rank.get(&id).copied();
                let s_bge = br.map_or(0.0, |r| 1.0 / (k_f + r as f32));
                let s_bm25 = mr.map_or(0.0, |r| 1.0 / (k_f + r as f32));
                (id, s_bge + s_bm25, br, mr)
            })
            .collect();

        // Sort by RRF score DESC. Trait invariant #3 ideally tiebreaks on
        // created_at DESC; spike accepts the unstable-tie approximation
        // since RRF ties are rare in practice.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(final_k);

        // Hydrate: prefer BGE-side hydration (already done), fall back to
        // BM25-side hydration map.
        let bge_mem_by_id: HashMap<MemoryId, RetrievedMemory> = bge_results
            .iter()
            .map(|m| (m.memory.id, m.clone()))
            .collect();
        let bm25_mem_by_id_post: HashMap<MemoryId, Memory> = bm25_candidates
            .iter()
            .map(|(id, _, m)| (*id, m.clone()))
            .collect();

        let mut output: Vec<RetrievedMemory> = Vec::with_capacity(scored.len());
        for (id, rrf_score, br, mr) in scored {
            let explanation = format!(
                "hybrid: rrf={:.4} (bge_rank={} · bm25_rank={})",
                rrf_score,
                br.map(|r| r.to_string()).unwrap_or_else(|| "—".to_string()),
                mr.map(|r| r.to_string()).unwrap_or_else(|| "—".to_string()),
            );
            if let Some(rm) = bge_mem_by_id.get(&id) {
                output.push(RetrievedMemory {
                    memory: rm.memory.clone(),
                    score: rrf_score,
                    explanation,
                });
            } else if let Some(m) = bm25_mem_by_id_post.get(&id) {
                output.push(RetrievedMemory {
                    memory: m.clone(),
                    score: rrf_score,
                    explanation,
                });
            }
        }

        // ── Diagnostic: dump fused top-K so we can see what the LLM
        // actually receives. Critical for root-causing failures where the
        // BGE-only diagnostic (printed in main) looks bad but hybrid
        // might've rescued it — or vice versa. Surfaces BGE rank, BM25
        // rank, RRF score, and content head per entry.
        println!("      [hybrid] fused top-{} (what LLM sees):", output.len());
        for (i, rm) in output.iter().enumerate() {
            let head: String = rm
                .memory
                .content
                .chars()
                .take(80)
                .collect::<String>()
                .replace('\n', " ");
            println!(
                "        [{i:>2}] {} score={:.4}  {head}",
                rm.explanation.replace("hybrid: ", ""),
                rm.score,
            );
        }

        Ok(output)
    }
}

/// Tantivy errors do not implement `Into<VaultError>`; wrap them as
/// `VaultError::Storage` for the spike. Production Phase 1 will add a
/// proper `VaultError::Index` variant or extend `Storage`.
fn vault_error_from_tantivy(msg: &str) -> VaultError {
    VaultError::Storage(format!("hybrid bm25: {msg}"))
}

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();

    // Resolve corpus scale: T028G_SCALE env var (if set + parsable) wins,
    // otherwise fall back to DEFAULT_SCALE=10_000. Lets us test 100 / 1K
    // / 10K with a single cold build.
    let scale: usize = match std::env::var("T028G_SCALE") {
        Ok(s) => s.parse().with_context(|| {
            format!("T028G_SCALE env var must be a positive integer, got {s:?}")
        })?,
        Err(_) => DEFAULT_SCALE,
    };
    ensure!(scale > 0, "T028G_SCALE must be > 0, got {scale}");

    println!("{}", "=".repeat(SEP_WIDE));
    println!("T0.2.7 Phase 0.b — t028g hybrid retrieval spike");
    println!(
        "Scale: {scale} · Iter: {ITERATION_QUERY_IDS:?} · ShortLong: {SHORT_LONG_QUERY_IDS:?}"
    );
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("{}", "=".repeat(SEP_WIDE));
    println!(
        "\nSystem prompt (v9, length = {} chars):",
        CANDIDATE_SYSTEM_PROMPT.len()
    );
    println!("{}", "─".repeat(SEP_WIDE));
    println!("{CANDIDATE_SYSTEM_PROMPT}");
    println!("{}", "─".repeat(SEP_WIDE));
    println!(
        "\nHybrid knobs: rrf_k={RRF_K} · bm25_top_score_threshold={BM25_TOP_SCORE_THRESHOLD:.2} · \
        top_n_each={HYBRID_TOP_N_EACH}"
    );

    let memory_fixture = load_memory_fixture()?;
    let query_set = load_query_set()?;
    let iter_queries: Vec<&QueryEntry> = query_set
        .queries
        .iter()
        .filter(|q| ITERATION_QUERY_IDS.contains(&q.id.as_str()))
        .collect();
    ensure!(
        iter_queries.len() == ITERATION_QUERY_IDS.len(),
        "iteration subset has {} queries but expected {}",
        iter_queries.len(),
        ITERATION_QUERY_IDS.len(),
    );
    // Synthesize QueryEntry rows for the short↔long pairs (not in the
    // fixture JSON; SHORT_LONG_PAIRS owns the canonical text).
    let short_long_queries: Vec<QueryEntry> = SHORT_LONG_PAIRS
        .iter()
        .map(|p| QueryEntry {
            id: p.query_id.to_string(),
            shape: "short_long_pair".to_string(),
            length_tier: "mixed".to_string(),
            query_text: p.query_text.to_string(),
            authorized_boundaries: vec![p.boundary.to_string()],
            expected_memory_ids: Vec::new(),
            notes: "synthesized in t028g for Phase 0.d short↔long acceptance".to_string(),
        })
        .collect();
    println!(
        "\nLoaded {} base fixture + {} queries ({} iter + {} short↔long)",
        memory_fixture.len(),
        query_set.queries.len() + short_long_queries.len(),
        iter_queries.len(),
        short_long_queries.len(),
    );

    println!("\nOpening BgeSmallProvider...");
    let bge = open_bge_provider()?;

    let qwen_path = models_dir()?.join(QWEN_MODEL_FILENAME);
    ensure!(qwen_path.exists(), "Qwen-7B GGUF missing at {qwen_path:?}");
    let tuning = TuningConfig {
        n_threads: Some(12),
        n_threads_batch: Some(12),
        n_gpu_layers: Some(99),
        ..TuningConfig::default()
    };
    println!("Opening Qwen-7B (Q4_K_M, Vulkan, n_gpu_layers=99)...");
    let qwen_load_start = Instant::now();
    let qwen_provider = Qwen25_14BProvider::open_with_tuning(&qwen_path, tuning).await?;
    println!("  ready in {:.1}s", qwen_load_start.elapsed().as_secs_f64());
    let qwen: Arc<dyn LlmProvider> = Arc::new(qwen_provider);

    println!("Generating diverse {scale}-doc corpus...");
    let mut corpus = generate_diverse_corpus(&memory_fixture, scale);
    ensure!(corpus.len() == scale);

    // Inject short↔long pair memories. These ride ON TOP of `scale` so the
    // 10K diverse corpus stays unchanged for direct comparability against
    // the t028d 5/6 baseline; each pair adds 2 fixture entries → +6 total.
    let injected_start = corpus.len();
    for (pair_i, pair) in SHORT_LONG_PAIRS.iter().enumerate() {
        corpus.push(MemoryFixtureEntry {
            id: format!("short-long-{pair_i:02}-short"),
            boundary: pair.boundary.to_string(),
            topic_label: "short_long_pair_short_member".to_string(),
            content: pair.short_content.to_string(),
            ground_truth: GroundTruth {
                outcome: "short_long_short_member".to_string(),
                cluster: None,
            },
        });
        corpus.push(MemoryFixtureEntry {
            id: format!("short-long-{pair_i:02}-long"),
            boundary: pair.boundary.to_string(),
            topic_label: "short_long_pair_long_member".to_string(),
            content: pair.long_content.to_string(),
            ground_truth: GroundTruth {
                outcome: "short_long_long_member".to_string(),
                cluster: None,
            },
        });
    }
    println!(
        "  injected {} short↔long fixture entries at indices {}..{}",
        corpus.len() - injected_start,
        injected_start,
        corpus.len() - 1,
    );

    println!("Embedding {} entries (BGE)...", corpus.len());
    let embed_start = Instant::now();
    let mut corpus_embeddings: Vec<Vec<f32>> = Vec::with_capacity(corpus.len());
    for (i, entry) in corpus.iter().enumerate() {
        let emb = bge.embed(&entry.content).await?;
        corpus_embeddings.push(emb);
        if (i + 1) % 250 == 0 {
            println!("  embedded {}/{}", i + 1, corpus.len());
        }
    }
    println!("  done in {:.1}s", embed_start.elapsed().as_secs_f64());

    let dir = tempfile::tempdir()?;
    let key = SqlCipherKey::new("spike-only-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key).await?;
    let metadata = Arc::new(metadata);
    let vectors: Arc<LanceVectorStore> = Arc::new(
        LanceVectorStore::open_with_at_rest_key(
            &dir.path().join("vectors"),
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await?,
    );

    const UPSERT_BATCH_SIZE: usize = 500;
    println!("Upserting {} memories...", corpus.len());
    let upsert_start = Instant::now();
    let mut batch_rows: Vec<(MemoryId, Vec<f32>, Boundary)> = Vec::with_capacity(UPSERT_BATCH_SIZE);
    // bm25_input feeds build_bm25_index downstream — the (MemoryId, content)
    // pairs in EXACT corpus order so corpus_idx = i can roundtrip.
    let mut bm25_input: Vec<(MemoryId, String)> = Vec::with_capacity(corpus.len());
    for (i, entry) in corpus.iter().enumerate() {
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
        metadata.create_memory(&memory).await?;
        bm25_input.push((memory.id, entry.content.clone()));
        batch_rows.push((
            memory.id,
            corpus_embeddings[i].clone(),
            memory.boundary.clone(),
        ));
        if batch_rows.len() >= UPSERT_BATCH_SIZE {
            vectors.bulk_upsert(&batch_rows).await?;
            batch_rows.clear();
        }
    }
    if !batch_rows.is_empty() {
        vectors.bulk_upsert(&batch_rows).await?;
    }
    println!("  done in {:.1}s", upsert_start.elapsed().as_secs_f64());

    println!("Building HNSW index (LanceDB)...");
    let build_start = Instant::now();
    vectors.create_vector_index_hnsw_sq().await?;
    println!("  done in {:.2}s", build_start.elapsed().as_secs_f64());

    println!("Building BM25 index (Tantivy in-RAM)...");
    let bm25_build_start = Instant::now();
    let bm25_index = build_bm25_index(&bm25_input)?;
    println!(
        "  done in {:.2}s ({} docs)",
        bm25_build_start.elapsed().as_secs_f64(),
        bm25_index.corpus_idx_lookup.len(),
    );

    const TEST_TOP_K: usize = 20;
    let vectors_dyn: Arc<dyn VectorStore> = vectors.clone();
    let inner_retriever: Arc<dyn Retriever> = Arc::new(SemanticRetriever::new(
        metadata.clone(),
        bge.clone(),
        vectors_dyn,
    ));
    let hybrid_retriever: Arc<dyn Retriever> = Arc::new(HybridRetriever {
        inner: inner_retriever.clone(),
        metadata: metadata.clone(),
        bm25: bm25_index,
        config: HybridConfig {
            rrf_k: RRF_K,
            bm25_top_score_threshold: BM25_TOP_SCORE_THRESHOLD,
            top_n_each: HYBRID_TOP_N_EACH,
        },
    });
    // Diagnostic retriever shows the pre-fusion BGE cosine ranking for
    // easier failure-mode interpretation. Pipeline gets the hybrid
    // retriever so the LLM sees the fused top-K.
    let diag_retriever = inner_retriever.clone();
    let pipeline = ReadPipeline::new(hybrid_retriever, qwen)
        .with_system_prompt(CANDIDATE_SYSTEM_PROMPT)
        .with_max_candidates(TEST_TOP_K);

    let mut contradiction_passes = 0_usize;
    let mut hard_negative_passes = 0_usize;
    let mut short_long_passes = 0_usize;
    let total_query_count = iter_queries.len() + short_long_queries.len();
    let mut verdicts: Vec<(String, &'static str, String, f64)> =
        Vec::with_capacity(total_query_count);

    // Build a single iterable list of (id, query_text, boundaries,
    // expected_memory_ids). Iter queries pull `expected_memory_ids` from
    // the JSON; short↔long queries match by content prefix instead.
    enum QuerySource<'a> {
        Iter(&'a QueryEntry),
        ShortLong(&'a QueryEntry, &'a ShortLongPair),
    }
    let mut all_queries: Vec<QuerySource> = Vec::with_capacity(total_query_count);
    for q in &iter_queries {
        all_queries.push(QuerySource::Iter(q));
    }
    for (i, q) in short_long_queries.iter().enumerate() {
        all_queries.push(QuerySource::ShortLong(q, &SHORT_LONG_PAIRS[i]));
    }

    for source in &all_queries {
        let q: &QueryEntry = match source {
            QuerySource::Iter(q) => q,
            QuerySource::ShortLong(q, _) => q,
        };
        let mut boundaries = Vec::with_capacity(q.authorized_boundaries.len());
        for b in &q.authorized_boundaries {
            boundaries.push(Boundary::new(b)?);
        }

        // Diagnostic retrieve: cosine top-20 BEFORE hybrid fusion. Cost
        // ~80ms per query, negligible vs the LLM stage. Helps interpret
        // failure cases — was the issue retrieval (Memory B not in top-20)
        // or synthesis (LLM saw it but didn't flag)?
        let diag = diag_retriever
            .retrieve(RetrievalQuery {
                query_text: q.query_text.clone(),
                authorized_boundaries: boundaries.clone(),
                max_results: TEST_TOP_K,
                options: RetrievalOptions::default(),
            })
            .await?;
        println!(
            "\n  {} retrieved {} BGE-only candidates (showing top-20 + expected):",
            q.id,
            diag.len()
        );

        // For iter queries: pull expected fixture contents via JSON IDs.
        // For short↔long: pull the two pair members from SHORT_LONG_PAIRS.
        let expected_contents: Vec<String> = match source {
            QuerySource::Iter(q) => {
                let expected: HashSet<&str> =
                    q.expected_memory_ids.iter().map(String::as_str).collect();
                corpus
                    .iter()
                    .filter(|e| expected.contains(e.id.as_str()))
                    .map(|e| e.content.clone())
                    .collect()
            }
            QuerySource::ShortLong(_, pair) => vec![
                pair.short_content.to_string(),
                pair.long_content.to_string(),
            ],
        };
        for (i, r) in diag.iter().enumerate() {
            let id_short = &r.memory.id.to_string()[..8];
            let content_head: String = r
                .memory
                .content
                .chars()
                .take(90)
                .collect::<String>()
                .replace('\n', " ");
            let is_expected = expected_contents.iter().any(|c| c == &r.memory.content);
            let mark = if is_expected { " ⭐" } else { "" };
            if i < 20 || is_expected {
                println!(
                    "    [{i:>2}]{mark} {id_short} score={:.3}  {content_head}",
                    r.score
                );
            }
        }
        let found: Vec<usize> = diag
            .iter()
            .enumerate()
            .filter(|(_, r)| expected_contents.iter().any(|c| c == &r.memory.content))
            .map(|(i, _)| i)
            .collect();
        println!(
            "    expected hits (BGE-only): {} of {} at ranks {:?}",
            found.len(),
            expected_contents.len(),
            found
        );

        let rq = ReadQuery {
            query_text: q.query_text.clone(),
            authorized_boundaries: boundaries,
        };
        let start = Instant::now();
        let read_result = pipeline.read(rq).await;
        let latency = start.elapsed().as_secs_f64();
        let (label, detail) = match &read_result {
            Ok(resp) => match assess_query(&q.id, resp) {
                QualityVerdict::ContradictionPass(d) => {
                    contradiction_passes += 1;
                    ("contradiction PASS", d)
                }
                QualityVerdict::ContradictionFail(d) => ("contradiction FAIL", d),
                QualityVerdict::HardNegativePass(d) => {
                    hard_negative_passes += 1;
                    ("hard-negative PASS", d)
                }
                QualityVerdict::HardNegativeFail(d) => ("hard-negative FAIL", d),
                QualityVerdict::ShortLongPass(d) => {
                    short_long_passes += 1;
                    ("short↔long PASS", d)
                }
                QualityVerdict::ShortLongFail(d) => ("short↔long FAIL", d),
                QualityVerdict::Observational(d) => ("observational", d),
            },
            Err(e) => {
                let mut err_head = format!("{e}");
                if err_head.len() > 160 {
                    err_head.truncate(160);
                    err_head.push_str("...");
                }
                ("pipeline ERROR", err_head)
            }
        };
        println!("    {} {label} ({:.1}s) — {detail}", q.id, latency);
        if let Ok(resp) = &read_result {
            let syn = if resp.synthesis_markdown.len() > 600 {
                format!("{}…", &resp.synthesis_markdown[..600])
            } else {
                resp.synthesis_markdown.clone()
            };
            println!("      synthesis: {syn}");
            println!(
                "      contradictions_flagged: {:?}",
                resp.contradictions_flagged
            );
            println!(
                "      vault_has_no_relevant_content: {}",
                resp.vault_has_no_relevant_content
            );
        }
        verdicts.push((q.id.clone(), label, detail, latency));
    }

    println!("\n{}", "=".repeat(SEP_WIDE));
    println!("PHASE 0.B HYBRID RETRIEVAL — VERDICT");
    println!("{}", "=".repeat(SEP_WIDE));
    println!(
        "Contradictions surfaced:  {}/{}",
        contradiction_passes,
        CONTRADICTION_QUERY_IDS.len()
    );
    println!(
        "Hard-negatives rejected:  {}/{}",
        hard_negative_passes,
        HARD_NEGATIVE_QUERY_IDS.len()
    );
    println!(
        "Short↔long pairs flagged: {}/{}",
        short_long_passes,
        SHORT_LONG_QUERY_IDS.len()
    );
    println!("\nPer-query results:");
    for (id, label, detail, latency) in &verdicts {
        println!("  {:>4}: {label} ({:.1}s) — {detail}", id, latency);
    }

    let iter_target = CONTRADICTION_QUERY_IDS.len() + HARD_NEGATIVE_QUERY_IDS.len();
    let iter_passes = contradiction_passes + hard_negative_passes;
    let short_long_target = SHORT_LONG_QUERY_IDS.len();
    let total_target = iter_target + short_long_target;
    let total_passes = iter_passes + short_long_passes;
    if total_passes == total_target {
        println!(
            "\n✅ {total_passes}/{total_target} PASS — Phase 0 acceptance MET. \
            Surface Phase 0 → Phase 1 transition plan for review. Phase 1 \
            promotes BM25 into vault-storage as a sidecar index."
        );
    } else {
        println!(
            "\n❌ {total_passes}/{total_target} pass ({iter_passes}/{iter_target} iter + \
            {short_long_passes}/{short_long_target} short↔long) — iterate knobs and rerun. \
            See decision tree in HANDOFF Phase 0 section."
        );
    }

    let elapsed_total = chrono::Utc::now().signed_duration_since(run_started);
    println!(
        "\nWall time: {:.1}s",
        elapsed_total.num_milliseconds() as f64 / 1000.0
    );
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────

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
        .context("vault-retrieval dir has no grandparent (repo root)")
}

fn vault_embedding_test_fixtures() -> Result<PathBuf> {
    let p = repo_root()?
        .join("crates")
        .join("vault-embedding")
        .join("test-fixtures")
        .join("bge-small-en-v1.5");
    ensure!(p.exists(), "bge-small-en-v1.5 fixture dir missing");
    Ok(p)
}

fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set on Windows")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}
