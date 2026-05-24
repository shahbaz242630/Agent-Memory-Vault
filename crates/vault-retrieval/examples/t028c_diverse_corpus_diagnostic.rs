//! T0.2.7 Phase 1 follow-up — t028c diverse-corpus diagnostic (2026-05-18).
//!
//! **Question this spike answers.** Does the t028b iteration-3 quality
//! collapse at scale={1K, 10K} reproduce when the corpus is genuinely
//! DIVERSE (template + vocabulary combinatorial distractors) instead of
//! paraphrase-decorated copies of the 100-memory base?
//!
//! **Why this matters.** t028b iteration 3 inflated the corpus by
//! decorating each base memory with `[session-NNN]` prefixes, producing
//! ~100 paraphrases per base entry at scale=10K. HNSW top-20 then filled
//! with ~19 paraphrases of one base memory, drowning out the minority
//! outlier carrying the contradiction. Quality dropped from 4/4 + 2/2 at
//! scale=100 to 0/4 + 1/2 at scale=10K.
//!
//! Real users will NOT have 100 paraphrases of every memory — the
//! vault-consolidator (ADR-044/045/046/047) merges near-duplicates
//! pre-storage daily. The relevant scaling axis for V0.2 read-time
//! quality is "10K diverse memories", not "100 paraphrases × 100 copies".
//! This spike isolates the variable: keep the 100-memory base (preserves
//! contradiction ground truth for Q11/Q13/Q25/Q26) and pad with
//! ~9900 genuinely-distinct distractor memories on topics deliberately
//! chosen NOT to collide with any gauntlet query.
//!
//! **Decision tree on the result.**
//! - If 10K-diverse quality holds at 4/4 + 2/2 → the t028b collapse is
//!   synthetic-stress-only; V0.2 ships without retrieval-side fixes.
//!   Recommended follow-up: add a synthetic-near-dup regression test as
//!   a CI canary so we notice if the consolidator effectiveness regresses.
//! - If 10K-diverse quality also degrades → real RAG-at-scale problem
//!   confirmed. Proceed to Phase B: spike MMR + value-aware guard, then
//!   value-grouping via per-doc extraction if MMR insufficient. Locked
//!   path documented in ADR-050.
//!
//! **Distractor design.**
//! - 10 distractor topic clusters chosen to NOT collide with the 8-query
//!   gauntlet content. Five "work"-boundary topics (office logistics,
//!   vendor renewals for engineering SaaS, doc reviews, internal tooling,
//!   team events), five "personal"-boundary topics (travel, home
//!   maintenance, car service, pet care, cooking).
//! - Templates × vocabulary combinatorial. ~3 templates per topic per
//!   length tier × ~6 slots × ~10-20 vocabulary entries = millions of
//!   unique surface forms; 9900 distractors is comfortably inside the
//!   uniqueness budget.
//! - Length tier mix matches the t026 realism rewrite: 56% short
//!   (50-150 chars) / 30% paragraph (300-1000) / 11% long-form
//!   (1000-2000) / 3% BGE-truncation (2000-2430).
//! - Boundary split 50/50 work/personal, matching the base fixture.
//! - Vocabulary deliberately excludes the strings "89" and "109" (Q13/Q26
//!   contradiction substrings) and "Q1 2027"/"Q2 2027" (Q11/Q25 contradiction
//!   substrings) and "Kubernetes"/"k8s"/"dental"/"insurance" (Q21/Q22
//!   hard-negative anchors). All money figures use the 200-9999 range.
//! - Deterministic via inline SplitMix64 PRNG with fixed seed — same run
//!   produces the same corpus every time.
//!
//! **Methodology declaration.** Compile-and-run on the local Windows dev
//! box (per `feedback_spike_methodology_explicit.md`). Uses the bundled
//! BGE-small ONNX provider for embedding, the sealed LanceVectorStore for
//! storage, lancedb's default `IvfHnswSqIndexBuilder` for the production
//! index, and the locked V0.2 Qwen-7B Q4_K_M GGUF with the production
//! `TuningConfig` from ADR-049 / `read_pipeline_acceptance.rs`. Spike
//! consumes the same `ReadPipeline` surface as production (ADR-048).
//!
//! **Discipline.** Example-grade throwaway. No tests, no commit at run
//! completion, no architectural conclusions in the source. Spike artefact
//! rides with the locked Phase-A-or-B decision per the
//! spike-bundle-with-consumer rule. Matching results markdown lands as
//! `t028c_diverse_corpus_results.md`.
//!
//! Run with (PowerShell on Windows, per standing rules):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t028c_diverse_corpus_diagnostic
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

// Test-only at-rest key. Matches the cross-crate convention.
const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

// 8-query t026 production-acceptance gauntlet.
const GAUNTLET_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q17", "Q19", "Q21", "Q22", "Q25", "Q26"];

const LATENCY_REPS_PER_QUERY: usize = 16;

// Scales to test. {100} is a sanity check (must still pass); {1K, 10K} are
// the diagnostic axis. If 10K passes 4/4 + 2/2, the t028b collapse was
// synthetic-stress-only.
const SCALES: &[usize] = &[100, 1000, 10000];

// LLM stage runs at scale >= this threshold. Mirrors t028b iteration 3.
const LLM_SCALE_THRESHOLD: usize = 1000;

const CONTRADICTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26"];
const HARD_NEGATIVE_QUERY_IDS: &[&str] = &["Q21", "Q22"];

const QWEN_MODEL_FILENAME: &str = "Qwen2.5-7B-Instruct-Q4_K_M.gguf";

const SEP_WIDE: usize = 100;

// Distractor PRNG seed. Fixed so every run produces the same corpus.
const DISTRACTOR_SEED: u64 = 0x7028C_DEADBEEF;

// ── SplitMix64 deterministic PRNG ────────────────────────────────────────

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

// ── Fixture types ────────────────────────────────────────────────────────

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

// ── Result types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct PerQueryRecall {
    query_id: String,
    recall_at_10: f64,
    recall_at_20: f64,
}

#[derive(Debug, Clone)]
struct ScaleResult {
    scale: usize,
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

#[derive(Debug, Clone)]
struct PerQueryLlm {
    query_id: String,
    verdict_label: &'static str,
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

// ── Distractor vocabulary ────────────────────────────────────────────────
//
// Curated to avoid collisions with the 8-query gauntlet:
// - No "Kubernetes" / "k8s" / "dental" / "insurance" (Q21/Q22).
// - No "Q1 2027" / "Q2 2027" / "$89" / "$109" / "89" / "109" (Q11/Q13/Q25/Q26
//   substring matchers). All money figures use 3+ digits and avoid the
//   80-119 range entirely.
// - No "launch" / "alpha" / "beta" / "GA" / "roadmap" (Q19/Q25).
// - No "exercise" / "5K" / "yoga" / "vitamin" / "blood pressure" (Q17/Q18).

// Common slot vocabularies, shared across templates.
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

// Money amounts: deliberately skip 80-119 to avoid Q13/Q26 substring "89"/"109".
const MONEY_AMOUNTS: &[&str] = &[
    "$245", "$312", "$478", "$523", "$640", "$715", "$820", "$945", "$1,200", "$1,475", "$1,820",
    "$2,150", "$2,640", "$3,100", "$4,250", "$5,500", "$6,900", "$8,300",
];

// ── Work cluster: office-logistics ───────────────────────────────────────

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

// ── Work cluster: vendor-renewals (engineering SaaS only) ────────────────

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
    "Procurement update — {VENDOR} {RENEWAL_DETAIL}. The vendor reached out about a multi-year option that would lock pricing for three years at {MONEY} annually. Pros: predictable budget, no annual negotiation churn. Cons: harder to walk away if we outgrow the tool. {PERSON} is collecting team feedback on whether the lock-in is worth the discount.",
];

const VENDOR_LONG_TEMPLATES: &[&str] = &[
    "Full procurement writeup: {VENDOR} {RENEWAL_DETAIL}. The annual cost lands at {MONEY}, which is roughly in line with last year's adjusted for inflation. {PERSON} ran a usage audit over the past quarter and found three things worth noting: first, seat utilization is at 78% — not high enough to push for more seats but not low enough to justify a downgrade tier. Second, the integration we built last quarter against this tool's API has become load-bearing for the deploy pipeline, so any switch would carry a non-trivial migration cost we should factor into any 'should we switch' conversation. Third, the vendor has a new feature in beta that overlaps with one of our internal tools — if it ships and is good, we could deprecate the internal tool and recoup engineering time worth roughly {MONEY} per quarter. Recommendation: renew at the current tier, set a calendar reminder for the next renewal {MONTH}, and revisit the multi-year discount question once the beta-feature decision is settled.",
];

// ── Work cluster: documentation-reviews (NOT roadmap docs) ───────────────

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
    "Doc-review followup: {PERSON} read the {DOC_TYPE} and concluded that it {DOC_FEEDBACK}. We talked through whether to block the merge on the change or land the doc and iterate; decision was to land + iterate since the gap is additive rather than misleading. New issue tracker entry created and targeted for the {MONTH} maintenance window.",
];

const DOC_LONG_TEMPLATES: &[&str] = &[
    "Comprehensive review writeup for the {DOC_TYPE}, conducted by {PERSON} during the {MONTH} doc-quality sprint. Top-line: {DOC_FEEDBACK}. The full review uncovered three classes of issue. First, terminology drift — the doc uses three different names for the same internal concept, which makes it hard for newcomers to follow the chain. Second, the example code blocks are stylized rather than copy-pasteable, which means readers have to mentally translate them to use the actual library; standard practice elsewhere in our docs corpus is to provide working examples. Third, the 'troubleshooting' section is structured around the symptoms the original author hit, not the symptoms readers are likely to encounter; this is a common drift pattern as docs age. {PERSON} is taking the action item to do a full revision over the next two weeks, with {MONEY} budget for one round of contractor copy-edit. The new version targets a {DAY}-of-{MONTH} merge, with a 14-day public-comment window before it replaces the current version on the docs site.",
];

// ── Work cluster: internal-tooling ───────────────────────────────────────

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
    "Platform note for the eng all-hands: the {TOOL_SYSTEM} {TOOL_ACTION}. Background context — the underlying issue had been on the platform team's backlog for two quarters but only got prioritized after the latency incident in {MONTH} made the cost concrete. {PERSON} ran the work; rollout starts {DAY} and completes within two weeks.",
];

const TOOL_LONG_TEMPLATES: &[&str] = &[
    "Long-form retrospective on the {TOOL_SYSTEM} work that {PERSON} led over the past quarter. Headline: {TOOL_ACTION}. The work originated from an internal survey that flagged this system as the second-most-painful piece of internal infrastructure behind the previous-generation deploy tooling we replaced last year. {PERSON} scoped the project against four success criteria: developer-experience improvement measured by survey delta, p99 latency improvement measured by the platform team's golden-signal dashboards, total cost reduction measured against the {MONEY} monthly baseline, and reduction in on-call pages tied to this system. Three of the four criteria were met. The cost criterion came in below target — savings landed at {MONEY} per month instead of the projected larger figure — because the migration also surfaced an unrelated cost driver that we had to absorb separately. Lessons learned, captured here so the next project in this category does not repeat them: scope the migration window against the maintenance calendar early, get sign-off from at least three downstream consumers before the first PR lands, and budget for one engineer-week of on-call backstop in the first month after switchover.",
];

// ── Work cluster: team-events (offsites, hackathons, lunches) ────────────

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
    "Logistics for the upcoming {EVENT_TYPE}: {EVENT_LOGISTICS}. {PERSON} is the lead organizer; agenda items still being collected via the shared form. RSVPs close end of {MONTH}; people with dietary restrictions should mention them on the form so catering can plan. Travel reimbursement guidance went out separately on {DAY} for the folks coming in from remote offices.",
    "{EVENT_TYPE} planning rollup — {EVENT_LOGISTICS}. {PERSON} pulled together a draft agenda based on submissions from the leadership team; the most-requested sessions are around career development, internal mobility, and how the platform-team work intersects with product-team roadmaps. Expect a 70/30 split between structured sessions and unstructured time for cross-team conversations.",
];

const EVENT_LONG_TEMPLATES: &[&str] = &[
    "Pre-read for the upcoming {EVENT_TYPE} that {PERSON} is organizing, {EVENT_LOGISTICS}. Context: this is the first time in three years we are running this format with the current team composition, and the planning group spent some time debating whether the legacy structure still fits. Three big shifts from the last iteration. First, the team is roughly 40% larger, which means the all-hands plenary format has to change — we are moving to a hub-and-spoke design where the morning is plenary and the afternoon is six parallel tracks. Second, we are deliberately reserving the last 90 minutes for fully unstructured time after feedback from the last two iterations that the schedule was too dense for the kind of cross-team relationship-building that is the actual reason these events exist. Third, the {MONEY} catering budget per person allows us to bring in a local vendor rather than the on-site default; {PERSON} is sourcing options and will share three finalists by end of {MONTH}. The session-proposal form will stay open through {MONTH} {DAY}, after which the organizing group will lock the agenda and start publishing it.",
];

// ── Personal cluster: travel-bookings ────────────────────────────────────

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
    "Travel plans coming together for the {CITY} trip {PURPOSE}. Flights booked through the corporate-rate portal for {MONEY} round-trip, hotel sorted with the loyalty points stash that has been sitting unused for two years. Dates land in mid-{MONTH}, which lines up with the shoulder season — weather should be decent without the peak-summer crowd surcharge. {PERSON} has been there twice and offered to share a curated list of restaurants and walking routes.",
    "Putting together the {CITY} trip {PURPOSE}. Total budget penciled at {MONEY} including flights, hotel, food, and a buffer for unplanned experiences. {PERSON} forwarded the spreadsheet they used for their trip last {MONTH}, which is a great starting template. Main open question is whether to rent a car for the day trips or rely on regional trains — leaning trains for stress reduction even though it adds a couple of hours each way.",
];

const TRAVEL_LONG_TEMPLATES: &[&str] = &[
    "Long-form trip planning notes for the {CITY} visit {PURPOSE}, written down so I do not lose the thread when other things compete for attention. Total budget envelope is {MONEY}, which is what I want to spend not what I have to spend — this trip is supposed to be restorative rather than maximally-efficient, so the budget reflects choosing comfort over optimization. Dates target the second half of {MONTH}, partly because that lines up with the shoulder-season pricing and partly because the work calendar is unusually clear that week. {PERSON} sent over a detailed writeup from their trip last year that I am leaning on heavily for the itinerary skeleton: two full days walking the historic core, a third day for the museum cluster {PERSON} flagged as 'underrated' (their word), and three days reserved as fully unstructured with no plans more specific than 'walk and see.' Booked flights through the standard corporate-rate portal even though this is a personal trip because the rate was better than the consumer side by enough to justify the extra paperwork. Hotel is mid-tier; the loyalty-points balance covers most of the stay so the out-of-pocket is closer to {MONEY} than the rack rate. Restaurant list is still being curated; {PERSON} mentioned three places they want me to check out and report back.",
];

// ── Personal cluster: home-maintenance ───────────────────────────────────

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
    "Home-maintenance log entry: the {HOME_PROBLEM} has been on the list for a while; {HOME_ACTION}. {PERSON} recommended the contractor based on their own house work last {MONTH} — solid track record and reasonable quote. The fix itself should take half a day; the longer pole is scheduling around the contractor's other jobs. Backup plan if the quote balloons is to do the work myself with a YouTube assist.",
    "Got around to the {HOME_PROBLEM} this week — {HOME_ACTION}. The diagnosis took longer than the fix because the obvious cause turned out to be a red herring; the real issue was upstream. {PERSON} stopped by to help on {DAY} since the second pair of hands made the work safer. Final cost came in under budget at {MONEY}, mostly because the parts ended up being cheaper than the initial estimate.",
];

const HOME_LONG_TEMPLATES: &[&str] = &[
    "Full writeup on the {HOME_PROBLEM} saga, because future-me will want the context next time something similar happens. Initial symptom was straightforward: the problem had been getting gradually worse over the past two months and finally crossed the 'this is annoying enough to fix' threshold. First step was to read up on the standard diagnosis path — {PERSON} pointed me at a couple of good resources, and an hour of reading covered most of the basics. Second step was to {HOME_ACTION}, which was the highest-leverage move I could make without committing to professional help. The contractor option was on the table from the start; I got three quotes ranging from {MONEY} to {MONEY}, and ended up going with the middle quote because the contractor had the best references and the highest-detail estimate (the cheapest quote was a flat number with no breakdown, which felt like a red flag). The work itself happened on a {DAY} morning, took about four hours, and the contractor was thorough about explaining what they were doing and why. Total cost including parts and labor came to {MONEY}, slightly above the original estimate because of a small adjacent issue they spotted while they were in there — flagged it to me, got a verbal okay before adding it to the bill, which is the right way to handle that kind of thing. Maintenance follow-up: check the work in three months and again at the one-year mark.",
];

// ── Personal cluster: car-service ────────────────────────────────────────

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
    "Car-maintenance update: {SERVICE_TYPE} {SHOP_NOTE}. {PERSON} recommended the shop after their last visit on {DAY} of {MONTH}; quick turnaround and no upsell pressure, which is the main thing I care about. They flagged a couple of items to keep an eye on at the next visit — tires are within spec but getting close to the wear bars, and one of the wiper blades is starting to chatter on the driver side.",
    "Routine car-service note — {SERVICE_TYPE} {SHOP_NOTE}. Total wall time was about 90 minutes including the wait, which is fine for a {DAY} morning slot. Final cost {MONEY}, which lines up with what {PERSON} paid for the same service at the same shop in {MONTH}. Next reminder will pop up at the next mileage milestone.",
];

const CAR_LONG_TEMPLATES: &[&str] = &[
    "Comprehensive car-service log for the {MONTH} visit, since I tend to forget the details by the time the next service rolls around. {SERVICE_TYPE} was the headline item — {SHOP_NOTE}. The shop also did the standard multi-point inspection that comes with every visit; results were mostly fine but they flagged three things worth noting for the next service window. First, the rear brake pads are at about 35% remaining, which means the next service interval is the right time to replace them rather than the one after. Second, the coolant looks darker than they would expect for the mileage, which sometimes points to a cooling-system issue but more often just means the previous flush did not catch all the old fluid; they recommended a flush at the next visit to baseline it. Third, one of the tires is wearing slightly unevenly which usually indicates an alignment issue; the alignment they did today should fix that going forward but they want to spot-check at the next visit. Total bill came to {MONEY}, which is roughly what {PERSON} paid for the equivalent service when they visited the same shop in {MONTH}. Receipt and inspection writeup are filed in the shared car-records folder.",
];

// ── Personal cluster: pet-care ───────────────────────────────────────────

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
    "Pet-care update: {PET_NAME} {PET_EVENT}. {PERSON} mentioned that their pet went through the same thing last {MONTH}; said the recovery was uneventful and the vet's instructions were straightforward to follow. Total cost projected at {MONEY} including the visit, any meds, and the follow-up visit two weeks later to confirm everything is settling.",
    "Took {PET_NAME} in this week — {PET_EVENT}. Vet did the standard panel and everything came back in range. {PERSON} swung by to dog-sit so I could focus on the appointment without juggling logistics; will return the favor when their pet has its annual visit next {MONTH}. Costs landed at {MONEY}, in line with what the vet quoted on the phone.",
];

const PET_LONG_TEMPLATES: &[&str] = &[
    "Long-form note on {PET_NAME}'s {PET_EVENT}, because the vet shared a lot of useful context that I want to capture before it fades. The visit itself took about 75 minutes — longer than usual because the vet wanted to do an extra panel given {PET_NAME}'s age. Results came back mostly reassuring; one marker is slightly outside the comfort zone but not in the alarm zone, and the vet's recommendation is to recheck in three months rather than start any intervention now. Total cost for the visit came to {MONEY}, which is on the high end of what {PERSON} described for their pet's equivalent visit but inside the range I had budgeted for. The vet also gave me a longer-term plan that I want to write down so I do not lose it. First, the recheck in three months is the most important data point; if the marker has stabilized we are good for another year, if it is trending up we start the medication conversation. Second, the food change we made last {MONTH} can stay — the vet thinks it is helping. Third, exercise levels are about right for {PET_NAME}'s age and breed; no changes needed there. Fourth, the dental cleaning {PERSON} suggested earlier in the year is now on the calendar for {MONTH}; the vet confirmed the quote of {MONEY} is reasonable for the work involved.",
];

// ── Personal cluster: cooking-recipes ────────────────────────────────────

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
    "Cooking note: made the {DISH} on {DAY} and it {OUTCOME}. The recipe is from the cookbook {PERSON} gave me last {MONTH}; this is the third recipe I have tried from it and the hit rate is high. Total cost of ingredients was about {MONEY} including the specialty items I had to source separately. Will make again, with the small tweak of starting the resting step 30 minutes earlier to avoid the timing crunch at the end.",
    "Recipe writeup for {DISH}: {OUTCOME}. The trickiest step was the one I expected to be easiest — the foundational technique that the recipe took for granted. Will look up a video tutorial before the next attempt. {PERSON} mentioned they ran into the same issue when they tried this recipe in {MONTH}, which made me feel less alone in struggling with it.",
];

const RECIPE_LONG_TEMPLATES: &[&str] = &[
    "Detailed cooking log for the {DISH} attempt this past {DAY}, because I want to capture the lessons while they are fresh and because {PERSON} asked for a writeup. Headline: {OUTCOME}, which felt like progress even where it did not turn out perfectly. The recipe was the one {PERSON} recommended after they made it last {MONTH}; their version came out better than mine, partly because they had practiced it twice already and partly because they had access to a couple of ingredients that are harder to find locally. I sourced substitutes for two of those ingredients, which worked in the sense that the dish came together at all but probably costs me 15-20% of the depth of flavor compared to the original. Total ingredient cost was {MONEY}, plus the time investment which I always under-estimate by about 50% for recipes I have not made before. Lessons learned, written down so I do not have to relearn them next time. First, the prep step listed as 'while the X is reducing' is actually a hard bottleneck — if the reduction runs faster than the prep, the whole sequence falls apart. Pre-stage everything before turning on any heat. Second, the seasoning balance described as 'salt to taste' actually has a narrow window; the recipe should have given a starting quantity. Third, the resting step is not optional even though it sounds optional. Fourth, the dish reheats well, which is a useful property given how much effort the initial cook takes. Recipe goes into the 'will-make-again' folder, with the note that the second attempt should incorporate the four lessons above.",
];

// ── Distractor generator ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
enum LengthTier {
    Short,      // 50-150 chars
    Paragraph,  // 300-1000 chars
    LongForm,   // 1000-2000 chars
    Truncation, // 2000-2430 chars
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
        (DistractorCluster::OfficeLogistics, LengthTier::LongForm) => {
            rng.pick(OFFICE_LONG_TEMPLATES)
        }
        (DistractorCluster::OfficeLogistics, LengthTier::Truncation) => {
            rng.pick(OFFICE_LONG_TEMPLATES)
        }

        (DistractorCluster::VendorRenewals, LengthTier::Short) => rng.pick(VENDOR_SHORT_TEMPLATES),
        (DistractorCluster::VendorRenewals, LengthTier::Paragraph) => {
            rng.pick(VENDOR_PARA_TEMPLATES)
        }
        (DistractorCluster::VendorRenewals, LengthTier::LongForm) => {
            rng.pick(VENDOR_LONG_TEMPLATES)
        }
        (DistractorCluster::VendorRenewals, LengthTier::Truncation) => {
            rng.pick(VENDOR_LONG_TEMPLATES)
        }

        (DistractorCluster::DocReviews, LengthTier::Short) => rng.pick(DOC_SHORT_TEMPLATES),
        (DistractorCluster::DocReviews, LengthTier::Paragraph) => rng.pick(DOC_PARA_TEMPLATES),
        (DistractorCluster::DocReviews, LengthTier::LongForm) => rng.pick(DOC_LONG_TEMPLATES),
        (DistractorCluster::DocReviews, LengthTier::Truncation) => rng.pick(DOC_LONG_TEMPLATES),

        (DistractorCluster::InternalTooling, LengthTier::Short) => rng.pick(TOOL_SHORT_TEMPLATES),
        (DistractorCluster::InternalTooling, LengthTier::Paragraph) => {
            rng.pick(TOOL_PARA_TEMPLATES)
        }
        (DistractorCluster::InternalTooling, LengthTier::LongForm) => rng.pick(TOOL_LONG_TEMPLATES),
        (DistractorCluster::InternalTooling, LengthTier::Truncation) => {
            rng.pick(TOOL_LONG_TEMPLATES)
        }

        (DistractorCluster::TeamEvents, LengthTier::Short) => rng.pick(EVENT_SHORT_TEMPLATES),
        (DistractorCluster::TeamEvents, LengthTier::Paragraph) => rng.pick(EVENT_PARA_TEMPLATES),
        (DistractorCluster::TeamEvents, LengthTier::LongForm) => rng.pick(EVENT_LONG_TEMPLATES),
        (DistractorCluster::TeamEvents, LengthTier::Truncation) => rng.pick(EVENT_LONG_TEMPLATES),

        (DistractorCluster::Travel, LengthTier::Short) => rng.pick(TRAVEL_SHORT_TEMPLATES),
        (DistractorCluster::Travel, LengthTier::Paragraph) => rng.pick(TRAVEL_PARA_TEMPLATES),
        (DistractorCluster::Travel, LengthTier::LongForm) => rng.pick(TRAVEL_LONG_TEMPLATES),
        (DistractorCluster::Travel, LengthTier::Truncation) => rng.pick(TRAVEL_LONG_TEMPLATES),

        (DistractorCluster::HomeMaintenance, LengthTier::Short) => rng.pick(HOME_SHORT_TEMPLATES),
        (DistractorCluster::HomeMaintenance, LengthTier::Paragraph) => {
            rng.pick(HOME_PARA_TEMPLATES)
        }
        (DistractorCluster::HomeMaintenance, LengthTier::LongForm) => rng.pick(HOME_LONG_TEMPLATES),
        (DistractorCluster::HomeMaintenance, LengthTier::Truncation) => {
            rng.pick(HOME_LONG_TEMPLATES)
        }

        (DistractorCluster::CarService, LengthTier::Short) => rng.pick(CAR_SHORT_TEMPLATES),
        (DistractorCluster::CarService, LengthTier::Paragraph) => rng.pick(CAR_PARA_TEMPLATES),
        (DistractorCluster::CarService, LengthTier::LongForm) => rng.pick(CAR_LONG_TEMPLATES),
        (DistractorCluster::CarService, LengthTier::Truncation) => rng.pick(CAR_LONG_TEMPLATES),

        (DistractorCluster::PetCare, LengthTier::Short) => rng.pick(PET_SHORT_TEMPLATES),
        (DistractorCluster::PetCare, LengthTier::Paragraph) => rng.pick(PET_PARA_TEMPLATES),
        (DistractorCluster::PetCare, LengthTier::LongForm) => rng.pick(PET_LONG_TEMPLATES),
        (DistractorCluster::PetCare, LengthTier::Truncation) => rng.pick(PET_LONG_TEMPLATES),

        (DistractorCluster::Cooking, LengthTier::Short) => rng.pick(RECIPE_SHORT_TEMPLATES),
        (DistractorCluster::Cooking, LengthTier::Paragraph) => rng.pick(RECIPE_PARA_TEMPLATES),
        (DistractorCluster::Cooking, LengthTier::LongForm) => rng.pick(RECIPE_LONG_TEMPLATES),
        (DistractorCluster::Cooking, LengthTier::Truncation) => rng.pick(RECIPE_LONG_TEMPLATES),
    }
}

/// Iteratively substitute slots in `template`. Each `{SLOT}` is replaced
/// with a pick from the appropriate vocabulary, drawn from `rng`. The
/// `PERSON`/`MONTH`/`DAY` shared slots can appear multiple times in the
/// same template; each occurrence draws independently.
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
    // Shared slots first.
    match slot {
        "PERSON" => return (rng.pick(PEOPLE)).to_string(),
        "MONTH" => return (rng.pick(MONTHS)).to_string(),
        "DAY" => return (rng.pick(DAYS_OF_WEEK)).to_string(),
        "MONEY" => return (rng.pick(MONEY_AMOUNTS)).to_string(),
        _ => {}
    }
    // Per-cluster slots.
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
    // A few of the nested vocab entries themselves carry slot markers (e.g.
    // RENEWAL_DETAILS includes `{MONTH}`). Recursively expand them so the
    // final string has no leftover braces.
    if pick.contains('{') {
        return fill_template(rng, cluster, pick);
    }
    pick.to_string()
}

/// Generate a single distractor memory at the given length tier.
fn generate_distractor(
    rng: &mut SplitMix64,
    cluster: DistractorCluster,
    tier: LengthTier,
    idx: usize,
) -> MemoryFixtureEntry {
    let template = pick_template(rng, cluster, tier);
    let mut content = fill_template(rng, cluster, template);
    // Long-form and truncation tiers: extend by chaining another template
    // of the same length tier as a continuation paragraph, until target is hit.
    let (min_chars, max_chars) = match tier {
        LengthTier::Short => (50, 150),
        LengthTier::Paragraph => (300, 1000),
        LengthTier::LongForm => (1000, 2000),
        LengthTier::Truncation => (2000, 2430),
    };
    // If too long, truncate at a word boundary.
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
    // If too short (mostly only short-tier templates filled to a stub), pad
    // with a brief tail. Avoid the gauntlet collision tokens.
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

/// Generate `needed` diverse distractors with the t026 realism mix:
/// 56% short, 30% paragraph, 11% long-form, 3% truncation. Cluster pick
/// uniform across the 10 distractor clusters; PRNG seeded with
/// `DISTRACTOR_SEED` for full reproducibility.
fn generate_diverse_distractors(needed: usize) -> Vec<MemoryFixtureEntry> {
    let mut rng = SplitMix64::new(DISTRACTOR_SEED);
    let mut out = Vec::with_capacity(needed);
    // Compute per-tier counts; rounding error goes to short.
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
    // Shuffle so the corpus doesn't have all-short-then-all-long order.
    // Fisher-Yates with the same PRNG.
    let n = out.len();
    for i in (1..n).rev() {
        let j = (rng.next_u64() as usize) % (i + 1);
        out.swap(i, j);
    }
    out
}

/// Produce a corpus of size `target`:
/// - First 100 entries are the base fixture (preserves contradiction
///   ground truth for Q11/Q13/Q25/Q26).
/// - Remaining entries are diverse distractors generated combinatorially.
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

// ── Main ─────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let run_started = chrono::Utc::now();
    println!("{}", "=".repeat(SEP_WIDE));
    println!("T0.2.7 Phase 1 — t028c diverse-corpus diagnostic (scales={SCALES:?})");
    println!("Started: {}", run_started.format("%Y-%m-%d %H:%M:%S UTC"));
    println!("Host:    {}", std::env::consts::OS);
    println!("{}", "=".repeat(SEP_WIDE));

    let memory_fixture = load_memory_fixture()?;
    let query_set = load_query_set()?;
    println!(
        "Loaded {} base memories + {} queries (gauntlet subset: {})",
        memory_fixture.len(),
        query_set.queries.len(),
        GAUNTLET_QUERY_IDS.len()
    );

    let gauntlet_queries: Vec<&QueryEntry> = query_set
        .queries
        .iter()
        .filter(|q| GAUNTLET_QUERY_IDS.contains(&q.id.as_str()))
        .collect();
    ensure!(
        gauntlet_queries.len() == GAUNTLET_QUERY_IDS.len(),
        "gauntlet subset has {} queries but expected {}",
        gauntlet_queries.len(),
        GAUNTLET_QUERY_IDS.len(),
    );

    println!("\nOpening BgeSmallProvider against bundled fixtures...");
    let bge = open_bge_provider().context("open BgeSmallProvider")?;

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

    println!("\nEmbedding {} gauntlet queries...", gauntlet_queries.len());
    let mut query_embeddings: HashMap<String, Vec<f32>> = HashMap::new();
    for q in &gauntlet_queries {
        let emb = bge.embed(&q.query_text).await.context("bge.embed query")?;
        query_embeddings.insert(q.id.clone(), emb);
    }

    let mut all_results: Vec<ScaleResult> = Vec::with_capacity(SCALES.len());

    for &scale in SCALES {
        println!("\n{}", "█".repeat(SEP_WIDE));
        println!("SCALE = {scale}");
        println!("{}", "█".repeat(SEP_WIDE));

        let scaled_corpus = generate_diverse_corpus(&memory_fixture, scale);
        ensure!(
            scaled_corpus.len() == scale,
            "scaled corpus has {} entries but expected {scale}",
            scaled_corpus.len(),
        );
        println!(
            "Generated {} corpus entries (base={} + diverse distractors={})",
            scaled_corpus.len(),
            memory_fixture.len(),
            scale.saturating_sub(memory_fixture.len()),
        );

        // Brief content-shape audit so the log carries evidence the corpus
        // really is diverse (not paraphrases).
        report_corpus_shape(&scaled_corpus);

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

        println!("Computing brute-force top-20 ground truth for each query...");
        let mut ground_truth: HashMap<String, Vec<usize>> = HashMap::new();
        for q in &gauntlet_queries {
            let q_emb = &query_embeddings[&q.id];
            let mut scored: Vec<(usize, f32)> = corpus_embeddings
                .iter()
                .enumerate()
                .map(|(i, m_emb)| (i, dot(q_emb, m_emb)))
                .collect();
            scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            ground_truth.insert(
                q.id.clone(),
                scored.into_iter().take(20).map(|(i, _)| i).collect(),
            );
        }

        let llm_for_scale: Option<Arc<dyn LlmProvider>> = if scale >= LLM_SCALE_THRESHOLD {
            Some(qwen.clone())
        } else {
            None
        };

        let result = run_scale(
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
        .with_context(|| format!("scale={scale} run"))?;

        println!("\n--- Scale {scale} summary ---");
        print_scale_summary(&result);

        all_results.push(result);
    }

    let results_path = vault_retrieval_root()
        .join("examples")
        .join("t028c_diverse_corpus_results.md");
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

fn report_corpus_shape(corpus: &[MemoryFixtureEntry]) {
    let mut tiers = [0_usize; 4];
    for e in corpus {
        let l = e.content.len();
        let bucket = match l {
            0..=150 => 0,
            151..=1000 => 1,
            1001..=2000 => 2,
            _ => 3,
        };
        tiers[bucket] += 1;
    }
    let total = corpus.len() as f64;
    println!(
        "  shape: short={} ({:.0}%) · paragraph={} ({:.0}%) · long-form={} ({:.0}%) · truncation={} ({:.0}%)",
        tiers[0],
        (tiers[0] as f64 / total) * 100.0,
        tiers[1],
        (tiers[1] as f64 / total) * 100.0,
        tiers[2],
        (tiers[2] as f64 / total) * 100.0,
        tiers[3],
        (tiers[3] as f64 / total) * 100.0,
    );
}

// ── Scale runner ─────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn run_scale(
    scale: usize,
    embed_secs: f64,
    memory_fixture: &[MemoryFixtureEntry],
    memory_embeddings: &[Vec<f32>],
    gauntlet_queries: &[&QueryEntry],
    query_embeddings: &HashMap<String, Vec<f32>>,
    ground_truth: &HashMap<String, Vec<usize>>,
    bge: Arc<dyn EmbeddingProvider>,
    llm: Option<Arc<dyn LlmProvider>>,
) -> Result<ScaleResult> {
    println!("\n{}", "─".repeat(SEP_WIDE));
    println!("RUN: HNSW (IvfHnswSq, default) @ scale={scale}");
    println!("{}", "─".repeat(SEP_WIDE));

    let dir = tempfile::tempdir().context("tempdir")?;
    let key = SqlCipherKey::new("spike-only-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key)
        .await
        .context("MetadataStore::open")?;
    let metadata = Arc::new(metadata);

    let vectors: Arc<LanceVectorStore> = Arc::new(
        LanceVectorStore::open_with_at_rest_key(
            &dir.path().join("vectors"),
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .context("LanceVectorStore::open_with_at_rest_key")?,
    );

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

    println!("Building HNSW index...");
    let build_start = Instant::now();
    vectors
        .create_vector_index_hnsw_sq()
        .await
        .context("create_vector_index_hnsw_sq")?;
    let build_secs = build_start.elapsed().as_secs_f64();
    println!("  built in {build_secs:.2}s");

    let memory_id_to_fixture_idx: HashMap<MemoryId, usize> = fixture_idx_to_memory_id
        .iter()
        .map(|(idx, mid)| (*mid, *idx))
        .collect();

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

    Ok(ScaleResult {
        scale,
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

fn print_scale_summary(r: &ScaleResult) {
    println!("\nHNSW @ scale={}", r.scale);
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
    all_results: &[ScaleResult],
    base_fixture_size: usize,
) -> Result<()> {
    let mut out = String::new();
    out.push_str("# T0.2.7 Phase 1 — t028c diverse-corpus diagnostic\n\n");
    out.push_str(
        "**Diagnostic question.** Does the t028b iteration-3 quality collapse at scale {1K, 10K} reproduce when the corpus is genuinely DIVERSE (template + vocabulary combinatorial distractors) instead of paraphrase-decorated copies of the 100-memory base?\n\n",
    );
    out.push_str(
        "**Decision tree.** If 10K-diverse quality holds at 4/4 contradictions + 2/2 hard-negatives → the t028b collapse was synthetic-stress-only and V0.2 ships without retrieval-side fixes (add a synthetic-near-dup regression test as a CI canary). If 10K-diverse quality also degrades → real RAG-at-scale problem confirmed, proceed to Phase B (MMR + value-aware guard, then value-grouping if insufficient).\n\n",
    );
    out.push_str(&format!(
        "**Run started:** {}\n",
        run_started.format("%Y-%m-%d %H:%M:%S UTC")
    ));
    out.push_str(&format!("**Host OS:** {}\n\n", std::env::consts::OS));

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

    out.push_str("## Methodology\n\n");
    out.push_str(&format!(
        "- {base_fixture_size}-memory base fixture from `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json` (preserves contradiction ground truth for Q11/Q13/Q25/Q26).\n"
    ));
    out.push_str("- Diverse distractors generated via template + vocabulary combinatorial generator with `SplitMix64` PRNG (seed=`0x7028C_DEADBEEF`). 10 distractor clusters (5 work + 5 personal), chosen NOT to collide with any gauntlet query content. See module doc-comment for full rationale.\n");
    out.push_str("- Length-tier mix matches t026 realism rewrite: 56% short / 30% paragraph / 11% long-form / 3% truncation. Boundary split 50/50 work/personal.\n");
    out.push_str("- Vocabulary deliberately excludes `\"89\"`, `\"109\"`, `\"Q1 2027\"`, `\"Q2 2027\"`, `\"Kubernetes\"`, `\"dental\"`, `\"insurance\"` substrings to avoid gauntlet-test collision. Money figures span 200-9999 only.\n");
    out.push_str("- BGE-small-en-v1.5 ONNX provider for embedding.\n");
    out.push_str("- Sealed `LanceVectorStore` per scale (fresh tempdir). HNSW index built via `IvfHnswSqIndexBuilder::default()`. Bulk inserts via `bulk_upsert` in chunks of 500.\n");
    out.push_str(&format!(
        "- `{LATENCY_REPS_PER_QUERY}` search-latency reps × 8 queries = {} samples per scale.\n",
        LATENCY_REPS_PER_QUERY * 8
    ));
    out.push_str("- Brute-force ground truth: dot product on BGE's L2-normalized vectors, top-20 per query (recomputed at each scale).\n");
    out.push_str("- LLM stage uses production `ReadPipeline` (ADR-048) + `SemanticRetriever`. Same Qwen-7B + locked V0.2 `TuningConfig` (n_threads=12, n_threads_batch=12, n_gpu_layers=99) as `read_pipeline_acceptance.rs`.\n");
    out.push_str(&format!(
        "- LLM stage runs at scale >= {LLM_SCALE_THRESHOLD} (scale=100 covered by t026 + `read_pipeline_acceptance`).\n\n"
    ));

    out.push_str("## Cross-reference\n\n");
    out.push_str("- `t028b_hnsw_vs_ivf_results.md` — the iteration 3 paraphrase-corpus run that triggered this diagnostic.\n");
    out.push_str("- ADR-048 — Read-time pipeline architecture (V0.2 read contract).\n");
    out.push_str("- ADR-049 — Qwen-7B model lock.\n");
    out.push_str("- ADR-050 — V0.2 production index lock (HNSW), pending Phase A/B outcome documented here.\n\n");

    std::fs::write(path, out).context("std::fs::write results.md")?;
    Ok(())
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

fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set on Windows")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}

fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        _ => None,
    }
}

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
