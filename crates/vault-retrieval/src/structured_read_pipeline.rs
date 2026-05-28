//! Structured read-time pipeline — Commit 6 of the locked-next-arc
//! (architectural lock 2026-05-26: LLM out of the read path).
//!
//! Replaces the V0.2-era [`crate::read_pipeline::ReadPipeline`] (Qwen-7B
//! single-call synthesis, mean 86s on Vulkan iGPU) with a deterministic
//! filter + pack stage. Read latency target: ~500ms total (retrieval cost
//! dominates).
//!
//! ## Three-player model
//!
//! - The **calling agent** (Claude / GPT / Codex / Kimi via MCP) composes
//!   the user-facing answer in its own voice. The vault never speaks to
//!   the user directly.
//! - **Phi-4-mini** stays in `vault-consolidator` for nightly merge
//!   classification + topic naming. Its REPORT artifact is what this
//!   pipeline enriches retrieved candidates from.
//! - **No LLM in this module.** The pipeline is pure code: retrieve →
//!   filter → enrich-with-REPORT-topics → emit structured facts +
//!   health signals.
//!
//! ## Two-stage flow
//!
//! 1. **Stage 1 — Retrieval.** Hand the query to the existing
//!    [`crate::Retriever`] (production: BGE-small dense, Tantivy BM25,
//!    RRF fusion, abstain gate). Returns top-N
//!    [`crate::RetrievedMemory`]s already filtered by
//!    `authorized_boundaries`.
//!
//! 2. **Stage 2 — Structured-fact assembly.** Load the per-boundary
//!    REPORT artifact (via [`crate::ReportLoader`]). For each
//!    retrieved memory, look up its topic via the REPORT's
//!    `facts_by_topic` (O(1) after one-pass invert). Build the
//!    [`StructuredReadResponse`] with `relevant_facts` + `abstain` +
//!    `health` warnings.
//!
//! ## Output contract (ADR-054)
//!
//! The MCP tool returns a JSON object with these fields:
//!
//! ```text
//! {
//!   "boundary": "personal" | null,        // null for multi-boundary reads
//!   "query": "<echo of trimmed query>",
//!   "relevant_facts": [
//!     { "fact": "...", "topic": "<label>"|null, "memory_id": "<uuid>",
//!       "as_of": "...", "confidence": 0.95, "source_agent": "<agent>"|null }
//!   ],
//!   "abstain": false,
//!   "health": {
//!     "status": "ok"|"degraded"|"critical",
//!     "warnings": [
//!       { "code": "REPORT_STALE_WARN", "severity": "warn",
//!         "detail": "...", "recovery_hint": "..." }
//!     ]
//!   }
//! }
//! ```
//!
//! The seven warning codes ([`WarningCode`]) are locked by ADR-054
//! Contract 2; any future addition requires a Contract amendment.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;

use vault_core::{Boundary, MemoryId, VaultError, VaultResult};

use crate::report_io::{LoadedReport, ReportLoader};
use crate::retriever::{RetrievalOptions, RetrievalQuery, Retriever};

// =============================================================================
// Constants — locked by ADR-054 (Commit 6)
// =============================================================================

/// Default top-N retrieved candidates handed to the filter+pack stage.
/// Matches the V0.2-era `read_pipeline::DEFAULT_MAX_CANDIDATES` for
/// continuity with the t026 8-query gauntlet anchoring.
pub const DEFAULT_MAX_CANDIDATES: usize = 20;

/// Staleness tier thresholds. Age = `now() - generated_at`.
///
/// - `0 ≤ age < INFO`: status `ok` (no staleness warning).
/// - `INFO ≤ age < WARN` (24h ≤ age < 72h): `REPORT_STALE_INFO`, severity Info.
/// - `WARN ≤ age < CRITICAL` (72h ≤ age < 7d): `REPORT_STALE_WARN`, severity Warn.
/// - `CRITICAL ≤ age` (7d ≤ age): `REPORT_STALE_CRITICAL`, severity Critical.
pub const STALE_INFO_THRESHOLD: Duration = Duration::from_secs(24 * 60 * 60);
/// See [`STALE_INFO_THRESHOLD`].
pub const STALE_WARN_THRESHOLD: Duration = Duration::from_secs(72 * 60 * 60);
/// See [`STALE_INFO_THRESHOLD`].
pub const STALE_CRITICAL_THRESHOLD: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Highest REPORT `schema_version` this read pipeline understands. A
/// REPORT with a higher version is treated as missing — the consumer
/// cannot safely interpret unknown future fields, so the pipeline
/// surfaces `REPORT_MISSING` rather than acting on partial data.
pub const SUPPORTED_REPORT_SCHEMA_VERSION: u32 = 1;

// =============================================================================
// Input
// =============================================================================

/// User-facing read query. Mirrors the V0.2-era `ReadQuery` shape so the
/// MCP `tool_read` handler doesn't need to migrate its construction site.
#[derive(Debug, Clone)]
pub struct ReadQuery {
    /// Raw user / agent question text. Trimmed + validated when [`StructuredReadPipeline::read`] runs.
    pub query_text: String,
    /// Boundaries the caller is authorised to read from. Empty slice
    /// short-circuits to `abstain=true` per BRD §11.4.3 — never an
    /// authorisation error.
    pub authorized_boundaries: Vec<Boundary>,
}

// =============================================================================
// Output — locked by ADR-054 Contract 2
// =============================================================================

/// The structured read-pipeline response the MCP `memory_read` tool
/// surfaces to the calling agent.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StructuredReadResponse {
    /// The single boundary in scope when `authorized_boundaries.len() == 1`.
    /// `None` for multi-boundary reads — facts from multiple boundaries
    /// may be intermixed in `relevant_facts`.
    pub boundary: Option<String>,
    /// Echo of `query.query_text` post-trim. Aids agent-side diagnosis
    /// and audit trails.
    pub query: String,
    /// Ordered facts most relevant to the query. Empty when `abstain=true`.
    pub relevant_facts: Vec<RelevantFact>,
    /// `true` when the pipeline returned no facts — either retrieval was
    /// empty, the abstain gate fired, or zero boundaries were authorised.
    /// The calling agent MUST tell the user the vault has nothing matching;
    /// fabricating an answer is a contract violation.
    pub abstain: bool,
    /// Health of the vault state behind this response.
    pub health: HealthInfo,
}

/// One structured fact in the response. Field names match the
/// `vault_consolidator::report::ReportFact` shape so REPORT topics flow
/// through to the agent without translation.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RelevantFact {
    pub fact: String,
    /// Consolidator-assigned topic label. `None` when:
    /// - the memory was written since the last consolidation run, or
    /// - no REPORT exists for the boundary, or
    /// - `topic_names_unavailable` on the loaded REPORT (placeholder labels).
    pub topic: Option<String>,
    /// UUID string. Agents can resolve back to a typed `MemoryId` if
    /// needed for follow-up MCP calls (e.g. `memory_delete`).
    pub memory_id: String,
    /// Fact-time anchor — when this fact became true in the world
    /// (`Memory::valid_from`). NOT when the memory row was added.
    pub as_of: DateTime<Utc>,
    pub confidence: f32,
    pub source_agent: Option<String>,
}

/// Aggregate health of the response, plus per-warning detail.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HealthInfo {
    pub status: HealthStatus,
    /// Empty when `status == HealthStatus::Ok`. Ordered by emission
    /// order (boundary order × per-boundary code emission order).
    pub warnings: Vec<HealthWarning>,
}

/// Aggregate severity of the response. Rule:
/// - Any [`WarningSeverity::Critical`] warning → [`HealthStatus::Critical`].
/// - Else any [`WarningSeverity::Warn`] or `Info` → [`HealthStatus::Degraded`].
/// - Else no warnings → [`HealthStatus::Ok`].
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Ok,
    Degraded,
    Critical,
}

/// One health warning. Surfaces to the calling agent via the
/// `health.warnings` array.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct HealthWarning {
    pub code: WarningCode,
    pub severity: WarningSeverity,
    /// Human-readable detail the agent can mention to the user when
    /// relevant. Bounded length; never includes vault contents or
    /// memory IDs.
    pub detail: String,
    /// Action the user can take to clear the warning. Bounded length;
    /// e.g. "Run the consolidator to refresh the REPORT".
    pub recovery_hint: String,
}

/// The six warning codes locked by ADR-054 Contract 2 (2026-05-26),
/// amended by ADR-054 Amendment 2 (2026-05-27) which retired
/// `DELTA_LOG_UNAVAILABLE` together with Plan Iteration 3 Contract 4.
/// Adding a seventh requires a Contract amendment.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WarningCode {
    /// No REPORT artifact exists for the boundary in scope. Most
    /// common cause: nightly consolidator hasn't run yet on a fresh
    /// vault.
    ReportMissing,
    /// REPORT age in the 24-72h band.
    ReportStaleInfo,
    /// REPORT age in the 72h-7d band.
    ReportStaleWarn,
    /// REPORT age ≥ 7d. Vault state has drifted; consolidator hasn't
    /// run in a week.
    ReportStaleCritical,
    /// REPORT's topic labels are placeholder `"topic_<id>"` strings.
    /// Driven by the `topic_names_unavailable: true` flag in the loaded
    /// REPORT (Phi-4-mini was unavailable at consolidation time).
    TopicNamesUnavailable,
    /// REPORT `generated_at` is in the future relative to the read-time
    /// clock. Indicates clock drift; staleness math becomes unreliable.
    ClockSkewDetected,
}

/// Three-level severity scale. Drives the [`HealthStatus`] aggregation.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WarningSeverity {
    Info,
    Warn,
    Critical,
}

// =============================================================================
// Clock abstraction — testable time without `tokio::time::pause()` brittleness
// =============================================================================

/// Wall-clock provider. Production uses [`SystemClock`]; tests inject
/// a fixed-point implementation so staleness-tier assertions are
/// deterministic.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Production [`Clock`] impl — delegates to `chrono::Utc::now()`.
#[derive(Debug, Clone, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

// =============================================================================
// Pipeline
// =============================================================================

/// Production read-time pipeline. Pair an `Arc<dyn Retriever>` (V0.2
/// production: `AbstainingRetriever` wrapping the BGE + Tantivy + RRF
/// stack) with an `Arc<dyn ReportLoader>` (V0.2 production:
/// [`crate::FilesystemReportLoader`]) at construction; call
/// [`Self::read`] per agent query.
///
/// Concrete struct (NOT a trait surface) per
/// [[forward-compat-concrete-vs-hypothetical]] — promote to a trait when
/// V0.3 cloud-tier becomes the imminent next task and a second concrete
/// implementation surfaces.
#[derive(Clone)]
pub struct StructuredReadPipeline {
    retriever: Arc<dyn Retriever>,
    report_loader: Arc<dyn ReportLoader>,
    clock: Arc<dyn Clock>,
    max_candidates: usize,
}

impl StructuredReadPipeline {
    /// Construct with default [`DEFAULT_MAX_CANDIDATES`] and a production
    /// [`SystemClock`].
    #[must_use]
    pub fn new(retriever: Arc<dyn Retriever>, report_loader: Arc<dyn ReportLoader>) -> Self {
        Self {
            retriever,
            report_loader,
            clock: Arc::new(SystemClock),
            max_candidates: DEFAULT_MAX_CANDIDATES,
        }
    }

    /// Override the wall-clock provider. Tests inject a fixed-point
    /// clock to assert staleness-tier boundaries deterministically.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Override the top-N retrieval cap. Defaults to
    /// [`DEFAULT_MAX_CANDIDATES`]; clamped by the underlying retriever
    /// to `[1, crate::retriever::MAX_RESULTS_CAP]`.
    #[must_use]
    pub fn with_max_candidates(mut self, n: usize) -> Self {
        self.max_candidates = n;
        self
    }

    /// Run the two-stage structured read.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] — `query_text` is empty after trim.
    /// - Any [`VaultError`] surfaced by the retriever stage.
    /// - [`VaultError::Io`] / [`VaultError::Serde`] from the
    ///   REPORT loader on malformed-JSON failures (file-missing
    ///   becomes the `REPORT_MISSING` warning, not an error).
    #[tracing::instrument(
        skip_all,
        fields(
            query_len = query.query_text.len(),
            boundary_count = query.authorized_boundaries.len()
        )
    )]
    pub async fn read(&self, query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        // Validate query text — empty after trim is the only InvalidInput
        // surface; oversize / control chars get caught by the retriever.
        let trimmed = query.query_text.trim();
        if trimmed.is_empty() {
            return Err(VaultError::InvalidInput(
                "structured read pipeline: query_text is empty after trim".into(),
            ));
        }
        let query_echo = trimmed.to_string();

        // boundary field — Some(name) for single-boundary, None for multi
        // or zero. Zero-boundary case also short-circuits below.
        let response_boundary = if query.authorized_boundaries.len() == 1 {
            Some(query.authorized_boundaries[0].as_str().to_string())
        } else {
            None
        };

        // Zero-boundary short-circuit per BRD §11.4.3 — the auth gate is
        // a short, not an error. No REPORTs to load, no warnings to emit.
        if query.authorized_boundaries.is_empty() {
            return Ok(StructuredReadResponse {
                boundary: response_boundary,
                query: query_echo,
                relevant_facts: Vec::new(),
                abstain: true,
                health: HealthInfo {
                    status: HealthStatus::Ok,
                    warnings: Vec::new(),
                },
            });
        }

        // Load REPORT per authorised boundary in input order. Each load is
        // independent: a missing/malformed REPORT for one boundary does
        // not poison the others — per ADR-053's "one file per boundary"
        // isolation contract.
        let mut loaded_reports: Vec<(Boundary, Option<LoadedReport>)> =
            Vec::with_capacity(query.authorized_boundaries.len());
        for b in &query.authorized_boundaries {
            let report = self.report_loader.load(b).await?;
            loaded_reports.push((b.clone(), report));
        }

        // Build the warnings vector. Order is boundary-order × per-boundary
        // code emission order: schema-guard → clock-skew → staleness tier
        // → topic-names-unavailable. Deterministic so consecutive responses
        // diff cleanly under identical state.
        let now = self.clock.now();
        let mut warnings: Vec<HealthWarning> = Vec::new();
        for (b, maybe_report) in &loaded_reports {
            match maybe_report {
                None => {
                    warnings.push(report_missing_warning(b));
                }
                Some(report) => {
                    // Unsupported future schema_version → surface as missing.
                    // The consumer can't safely interpret unknown fields.
                    if report.schema_version > SUPPORTED_REPORT_SCHEMA_VERSION {
                        warnings.push(HealthWarning {
                            code: WarningCode::ReportMissing,
                            severity: WarningSeverity::Warn,
                            detail: format!(
                                "REPORT for boundary '{}' has schema_version {} (this binary supports {})",
                                b.as_str(),
                                report.schema_version,
                                SUPPORTED_REPORT_SCHEMA_VERSION
                            ),
                            recovery_hint:
                                "Upgrade the vault binary to a version that understands this REPORT schema."
                                    .into(),
                        });
                        continue;
                    }

                    // Clock-skew dominates staleness (a future-dated REPORT
                    // makes age math meaningless). When skew fires, skip the
                    // staleness tier check for this REPORT.
                    if report.generated_at > now {
                        let skew_secs = (report.generated_at - now).num_seconds();
                        warnings.push(HealthWarning {
                            code: WarningCode::ClockSkewDetected,
                            severity: WarningSeverity::Critical,
                            detail: format!(
                                "REPORT for boundary '{}' generated_at is {skew_secs}s ahead of read-time clock",
                                b.as_str()
                            ),
                            recovery_hint:
                                "Check the system clock on the consolidator host against an authoritative time source (e.g. NTP)."
                                    .into(),
                        });
                    } else {
                        // Staleness tier check.
                        let age_chrono = now - report.generated_at;
                        let age = age_chrono.to_std().unwrap_or(Duration::ZERO);
                        if let Some(w) = staleness_warning(b, age) {
                            warnings.push(w);
                        }
                    }

                    if report.topic_names_unavailable {
                        warnings.push(HealthWarning {
                            code: WarningCode::TopicNamesUnavailable,
                            severity: WarningSeverity::Info,
                            detail: format!(
                                "topic labels for boundary '{}' are placeholder identifiers (Phi-4-mini was unavailable at consolidation)",
                                b.as_str()
                            ),
                            recovery_hint:
                                "Ensure phi4_model_path is configured + the model file is present before the next consolidation run."
                                    .into(),
                        });
                    }
                }
            }
        }

        // Build memory_id → topic_label lookup across all loaded REPORTs.
        // O(1) lookup at the per-fact mapping step below. Cross-boundary
        // collisions are impossible in practice — MemoryId is UUID v7
        // unique, no two boundaries can hold the same id.
        let mut topic_lookup: HashMap<MemoryId, String> = HashMap::new();
        for (_, maybe_report) in &loaded_reports {
            if let Some(report) = maybe_report {
                for (topic_label, facts) in &report.facts_by_topic {
                    for fact in facts {
                        topic_lookup.insert(fact.memory_id, topic_label.clone());
                    }
                }
            }
        }

        // Stage 1 — retrieval. Hands `authorized_boundaries` to the
        // retriever; the abstain gate (production: AbstainingRetriever)
        // returns empty when the top-1 BM25 score is below the cliff,
        // which we surface as abstain=true below.
        let retrieval_query = RetrievalQuery {
            query_text: query_echo.clone(),
            authorized_boundaries: query.authorized_boundaries.clone(),
            max_results: self.max_candidates,
            options: RetrievalOptions::default(),
        };
        let candidates = self.retriever.retrieve(retrieval_query).await?;

        // Stage 2 — pack into RelevantFacts. Empty retrieval is the only
        // abstain trigger after the zero-boundary short-circuit above.
        let abstain = candidates.is_empty();
        let relevant_facts: Vec<RelevantFact> = candidates
            .into_iter()
            .map(|c| {
                let topic = topic_lookup.get(&c.memory.id).cloned();
                RelevantFact {
                    fact: c.memory.content,
                    topic,
                    memory_id: c.memory.id.0.to_string(),
                    as_of: c.memory.valid_from,
                    confidence: c.memory.confidence,
                    source_agent: c.memory.source_agent,
                }
            })
            .collect();

        let status = aggregate_status(&warnings);

        Ok(StructuredReadResponse {
            boundary: response_boundary,
            query: query_echo,
            relevant_facts,
            abstain,
            health: HealthInfo { status, warnings },
        })
    }
}

// =============================================================================
// Helpers — pure functions over warning construction + aggregation
// =============================================================================

fn report_missing_warning(boundary: &Boundary) -> HealthWarning {
    HealthWarning {
        code: WarningCode::ReportMissing,
        severity: WarningSeverity::Warn,
        detail: format!(
            "no REPORT artifact exists for boundary '{}'",
            boundary.as_str()
        ),
        recovery_hint: "Run `vault-cli consolidate run` to generate the per-boundary REPORT."
            .into(),
    }
}

fn staleness_warning(boundary: &Boundary, age: Duration) -> Option<HealthWarning> {
    if age >= STALE_CRITICAL_THRESHOLD {
        Some(HealthWarning {
            code: WarningCode::ReportStaleCritical,
            severity: WarningSeverity::Critical,
            detail: format!(
                "REPORT for boundary '{}' is {} days old (≥ 7d)",
                boundary.as_str(),
                age.as_secs() / 86_400
            ),
            recovery_hint: "Run `vault-cli consolidate run` to refresh the REPORT.".into(),
        })
    } else if age >= STALE_WARN_THRESHOLD {
        Some(HealthWarning {
            code: WarningCode::ReportStaleWarn,
            severity: WarningSeverity::Warn,
            detail: format!(
                "REPORT for boundary '{}' is {} hours old (72h-7d band)",
                boundary.as_str(),
                age.as_secs() / 3_600
            ),
            recovery_hint: "Run `vault-cli consolidate run` to refresh the REPORT.".into(),
        })
    } else if age >= STALE_INFO_THRESHOLD {
        Some(HealthWarning {
            code: WarningCode::ReportStaleInfo,
            severity: WarningSeverity::Info,
            detail: format!(
                "REPORT for boundary '{}' is {} hours old (24-72h band)",
                boundary.as_str(),
                age.as_secs() / 3_600
            ),
            recovery_hint: "Run `vault-cli consolidate run` to refresh the REPORT.".into(),
        })
    } else {
        None
    }
}

fn aggregate_status(warnings: &[HealthWarning]) -> HealthStatus {
    if warnings
        .iter()
        .any(|w| w.severity == WarningSeverity::Critical)
    {
        HealthStatus::Critical
    } else if warnings.is_empty() {
        HealthStatus::Ok
    } else {
        HealthStatus::Degraded
    }
}

impl std::fmt::Debug for StructuredReadPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredReadPipeline")
            .field("max_candidates", &self.max_candidates)
            // retriever / report_loader / clock intentionally omitted: trait
            // objects with no Debug impl.
            .finish_non_exhaustive()
    }
}

// =============================================================================
// Tests — scaffolded Commit 6 (failing until Task 5 implements `read()`)
// =============================================================================

#[cfg(test)]
mod tests {
    //! Unit tests pinning the locked-next-arc Plan Iteration 3 contracts:
    //! - Contract 2 (this module): MCP `memory_read` response shape +
    //!   7 health-warning codes + severity / aggregate-status rules.
    //! - Contract 3 (light coverage): consolidator-produced REPORT shape
    //!   that this pipeline consumes.
    //!
    //! Heavy quality assertions (multi-boundary correctness against real
    //! LanceDB / Tantivy / BGE) live at the application layer; this
    //! module's tests use mock implementations of `Retriever` +
    //! `ReportLoader` to exercise the pipeline-shape contracts.
    //!
    //! All tests scaffolded Commit 6 (locked-next-arc, 2026-05-26).
    //! Tests panic at `todo!()` until Task 5 implements `read()`.

    use super::*;
    use crate::report_io::{LoadedReport, LoadedReportFact, ReportLoader};
    use crate::retriever::{RetrievalQuery, RetrievedMemory};
    use async_trait::async_trait;
    use chrono::TimeZone;
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;
    use vault_core::{Memory, MemoryId, MemoryType, NewMemory};

    // ---------------------------------------------------------------------
    // Helpers
    // ---------------------------------------------------------------------

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("static-valid test boundary")
    }

    /// Construct a Memory with explicit valid_from so `as_of` assertions
    /// are deterministic. Other fields are filled in as plausible defaults.
    fn fake_memory(
        id_n: u128,
        content: &str,
        boundary_name: &str,
        valid_from: DateTime<Utc>,
        confidence: f32,
        source_agent: Option<&str>,
    ) -> Memory {
        let mut m = Memory::try_new(NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary(boundary_name),
            source_agent: source_agent.map(str::to_string),
            confidence,
            valid_from: Some(valid_from),
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("static-valid test memory");
        m.id = MemoryId(uuid_from_id(id_n));
        m
    }

    fn uuid_from_id(n: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(n)
    }

    fn retrieved(memory: Memory, score: f32) -> RetrievedMemory {
        RetrievedMemory {
            memory,
            score,
            explanation: format!("test: score={score:.4}"),
        }
    }

    /// Build a LoadedReport with one topic containing the supplied memory
    /// IDs. Useful for the topic-lookup tests.
    fn loaded_report_with_topic(
        boundary_name: &str,
        generated_at: DateTime<Utc>,
        topic_label: &str,
        member_ids: &[MemoryId],
        topic_names_unavailable: bool,
    ) -> LoadedReport {
        let facts: Vec<LoadedReportFact> = member_ids
            .iter()
            .map(|id| LoadedReportFact {
                fact: format!("fact-for-{id}"),
                memory_id: *id,
                as_of: generated_at,
                confidence: 0.9,
                source_agent: None,
            })
            .collect();
        let mut facts_by_topic = BTreeMap::new();
        facts_by_topic.insert(topic_label.to_string(), facts);
        LoadedReport {
            schema_version: 1,
            boundary: boundary(boundary_name),
            generated_at,
            consolidator_run_id: "00000000-0000-0000-0000-000000000000".into(),
            facts_by_topic,
            topic_names_unavailable,
        }
    }

    /// Mock retriever — returns canned candidates regardless of query.
    /// Tests that need query-shape inspection can call `observed_query()`.
    struct MockRetriever {
        canned: Vec<RetrievedMemory>,
        last_query: Mutex<Option<RetrievalQuery>>,
    }

    impl MockRetriever {
        fn new(canned: Vec<RetrievedMemory>) -> Arc<Self> {
            Arc::new(Self {
                canned,
                last_query: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl Retriever for MockRetriever {
        async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
            *self.last_query.lock().unwrap() = Some(query);
            Ok(self.canned.clone())
        }
    }

    /// Mock REPORT loader — returns canned `Option<LoadedReport>` keyed
    /// by boundary name. Boundaries without a canned entry return Ok(None)
    /// (the REPORT_MISSING surfacing path).
    struct MockReportLoader {
        canned: HashMap<String, LoadedReport>,
    }

    impl MockReportLoader {
        fn new(reports: HashMap<String, LoadedReport>) -> Arc<Self> {
            Arc::new(Self { canned: reports })
        }

        fn empty() -> Arc<Self> {
            Arc::new(Self {
                canned: HashMap::new(),
            })
        }
    }

    #[async_trait]
    impl ReportLoader for MockReportLoader {
        async fn load(&self, boundary: &Boundary) -> VaultResult<Option<LoadedReport>> {
            Ok(self.canned.get(boundary.as_str()).cloned())
        }
    }

    /// Fixed-point Clock for staleness-tier tests.
    #[derive(Debug, Clone)]
    struct FixedClock {
        now: DateTime<Utc>,
    }

    impl FixedClock {
        fn arc(now: DateTime<Utc>) -> Arc<Self> {
            Arc::new(Self { now })
        }
    }

    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.now
        }
    }

    /// 2026-06-01T12:00:00Z — fixed read-time anchor for deterministic
    /// staleness math. Pick a date well after Batch A's 2026-05-26 ship
    /// to avoid accidental overlap with any side data.
    fn read_clock_now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap()
    }

    // ---------------------------------------------------------------------
    // Group A — Query validation + abstain short-circuits
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn empty_query_text_returns_invalid_input_error() {
        let pipeline =
            StructuredReadPipeline::new(MockRetriever::new(vec![]), MockReportLoader::empty());
        let err = pipeline
            .read(ReadQuery {
                query_text: "   ".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .expect_err("empty-after-trim MUST surface as InvalidInput, not pass to retriever");
        assert!(
            matches!(err, VaultError::InvalidInput(_)),
            "expected VaultError::InvalidInput; got {err:?}"
        );
    }

    #[tokio::test]
    async fn zero_authorized_boundaries_short_circuits_to_abstain_ok_health() {
        // Per BRD §11.4.3 the auth gate is short-circuit, not an error.
        // The agent might legitimately ask the vault when no boundaries
        // are authorized; the right response is "no relevant content".
        let pipeline =
            StructuredReadPipeline::new(MockRetriever::new(vec![]), MockReportLoader::empty());
        let resp = pipeline
            .read(ReadQuery {
                query_text: "anything".into(),
                authorized_boundaries: vec![],
            })
            .await
            .expect("zero-boundaries MUST short-circuit, never error");
        assert!(resp.abstain, "zero boundaries MUST set abstain=true");
        assert!(resp.relevant_facts.is_empty());
        assert_eq!(resp.health.status, HealthStatus::Ok);
        assert!(
            resp.health.warnings.is_empty(),
            "zero-boundaries short-circuit MUST NOT emit warnings (no REPORT to check)"
        );
    }

    #[tokio::test]
    async fn empty_retrieval_returns_abstain_true_with_facts_empty() {
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![]), // retriever returns 0 candidates
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "anything".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .expect("empty retrieval MUST succeed with abstain=true");
        assert!(resp.abstain);
        assert!(resp.relevant_facts.is_empty());
    }

    #[tokio::test]
    async fn query_text_is_trimmed_before_echoing_to_response() {
        let mem = fake_memory(1, "a fact", "personal", read_clock_now(), 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(mem, 0.9)]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "  whitespace-padded  ".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.query, "whitespace-padded",
            "response.query MUST be the trimmed query text"
        );
    }

    // ---------------------------------------------------------------------
    // Group B — Boundary field semantics
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn single_boundary_query_sets_response_boundary_to_that_name() {
        let mem = fake_memory(1, "fact", "personal", read_clock_now(), 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(mem, 0.9)]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.boundary.as_deref(),
            Some("personal"),
            "single-boundary read MUST set response.boundary = Some(boundary_name)"
        );
    }

    #[tokio::test]
    async fn multi_boundary_query_sets_response_boundary_to_none() {
        let m1 = fake_memory(1, "fact1", "personal", read_clock_now(), 0.9, None);
        let m2 = fake_memory(2, "fact2", "work", read_clock_now(), 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m1, 0.9), retrieved(m2, 0.8)]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal"), boundary("work")],
            })
            .await
            .unwrap();
        assert!(
            resp.boundary.is_none(),
            "multi-boundary read MUST set response.boundary = None; got {:?}",
            resp.boundary
        );
    }

    // ---------------------------------------------------------------------
    // Group C — Filter + pack: relevant_facts construction
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn retrieved_memories_become_relevant_facts_in_retrieval_order() {
        let now = read_clock_now();
        let m1 = fake_memory(1, "first", "personal", now, 0.95, Some("claude"));
        let m2 = fake_memory(2, "second", "personal", now, 0.80, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![
                retrieved(m1.clone(), 0.95),
                retrieved(m2.clone(), 0.80),
            ]),
            MockReportLoader::empty(),
        );
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.relevant_facts.len(), 2);
        assert_eq!(resp.relevant_facts[0].fact, "first");
        assert_eq!(
            resp.relevant_facts[0].source_agent.as_deref(),
            Some("claude")
        );
        assert_eq!(resp.relevant_facts[0].confidence, 0.95);
        assert_eq!(resp.relevant_facts[1].fact, "second");
        assert!(resp.relevant_facts[1].source_agent.is_none());
        assert!(!resp.abstain);
    }

    #[tokio::test]
    async fn topic_lookup_from_report_when_memory_id_matches() {
        let now = read_clock_now();
        let m = fake_memory(7, "BP 132/85", "personal", now, 0.95, None);
        let report = loaded_report_with_topic(
            "personal",
            now,
            "blood_pressure",
            &[m.id],
            false, // topic_names_unavailable=false
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.relevant_facts.len(), 1);
        assert_eq!(
            resp.relevant_facts[0].topic.as_deref(),
            Some("blood_pressure"),
            "memory present in REPORT.facts_by_topic MUST get topic=Some(label)"
        );
    }

    #[tokio::test]
    async fn topic_is_none_when_memory_not_in_report() {
        // Memory written since last consolidation — not in REPORT yet.
        let now = read_clock_now();
        let m_new = fake_memory(99, "fresh fact", "personal", now, 0.9, None);
        let m_old = fake_memory(1, "old fact", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", now, "old_topic", &[m_old.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m_new, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.relevant_facts.len(), 1);
        assert!(
            resp.relevant_facts[0].topic.is_none(),
            "memory NOT in REPORT MUST get topic=None"
        );
    }

    #[tokio::test]
    async fn topic_is_none_when_no_report_for_boundary() {
        let now = read_clock_now();
        let m = fake_memory(1, "fact", "personal", now, 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::empty(), // no REPORT
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert!(
            resp.relevant_facts[0].topic.is_none(),
            "REPORT_MISSING case MUST set topic=None on all facts"
        );
    }

    // ---------------------------------------------------------------------
    // Group D — 7 warning codes (ADR-054 Contract 2)
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn report_missing_emits_report_missing_warning_with_warn_severity() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::empty(),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportMissing)
            .expect("REPORT_MISSING warning MUST be emitted when no REPORT exists for boundary");
        assert_eq!(w.severity, WarningSeverity::Warn);
        assert!(!w.recovery_hint.is_empty(), "recovery_hint MUST be set");
    }

    #[tokio::test]
    async fn report_age_24_to_72_hours_emits_stale_info_with_info_severity() {
        let now = read_clock_now();
        let generated = now - chrono::Duration::hours(36); // 36h → INFO band
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportStaleInfo)
            .expect("REPORT_STALE_INFO MUST fire for 24h ≤ age < 72h");
        assert_eq!(w.severity, WarningSeverity::Info);
    }

    #[tokio::test]
    async fn report_age_72_hours_to_7_days_emits_stale_warn_with_warn_severity() {
        let now = read_clock_now();
        let generated = now - chrono::Duration::days(4); // 4d → WARN band
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportStaleWarn)
            .expect("REPORT_STALE_WARN MUST fire for 72h ≤ age < 7d");
        assert_eq!(w.severity, WarningSeverity::Warn);
    }

    #[tokio::test]
    async fn report_age_7_plus_days_emits_stale_critical_with_critical_severity() {
        let now = read_clock_now();
        let generated = now - chrono::Duration::days(14); // 14d → CRITICAL band
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ReportStaleCritical)
            .expect("REPORT_STALE_CRITICAL MUST fire for age ≥ 7d");
        assert_eq!(w.severity, WarningSeverity::Critical);
    }

    #[tokio::test]
    async fn report_with_topic_names_unavailable_emits_topic_names_unavailable_info_warning() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic(
            "personal",
            now,
            "topic_0", // placeholder label
            &[m.id],
            true, // topic_names_unavailable=true
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::TopicNamesUnavailable)
            .expect("TOPIC_NAMES_UNAVAILABLE MUST fire when REPORT.topic_names_unavailable=true");
        assert_eq!(w.severity, WarningSeverity::Info);
    }

    #[tokio::test]
    async fn report_generated_at_in_future_emits_clock_skew_critical_warning() {
        let now = read_clock_now();
        let generated = now + chrono::Duration::hours(2); // 2h in the FUTURE
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        let report = loaded_report_with_topic("personal", generated, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        let w = resp
            .health
            .warnings
            .iter()
            .find(|w| w.code == WarningCode::ClockSkewDetected)
            .expect("CLOCK_SKEW_DETECTED MUST fire when REPORT.generated_at > now()");
        assert_eq!(w.severity, WarningSeverity::Critical);
    }

    // ---------------------------------------------------------------------
    // Group E — Aggregate status rules
    // ---------------------------------------------------------------------

    #[tokio::test]
    async fn aggregate_status_is_ok_when_no_warnings() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // Fresh REPORT (generated_at == now), labels available — no warning
        // surfaces.
        let report = loaded_report_with_topic("personal", now, "t", &[m.id], false);
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.health.status,
            HealthStatus::Ok,
            "no warnings MUST → HealthStatus::Ok; got warnings: {:?}",
            resp.health.warnings
        );
        assert!(resp.health.warnings.is_empty());
    }

    #[tokio::test]
    async fn aggregate_status_is_degraded_when_only_info_warnings() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // 36h-old REPORT with topic_names_unavailable=true → two Info warnings.
        let report = loaded_report_with_topic(
            "personal",
            now - chrono::Duration::hours(36),
            "topic_0",
            &[m.id],
            true,
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.health.status, HealthStatus::Degraded);
        assert!(resp
            .health
            .warnings
            .iter()
            .all(|w| w.severity == WarningSeverity::Info));
    }

    #[tokio::test]
    async fn aggregate_status_is_degraded_when_warn_present_no_critical() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // 4d-old REPORT → REPORT_STALE_WARN (severity Warn). No Critical.
        let report = loaded_report_with_topic(
            "personal",
            now - chrono::Duration::days(4),
            "t",
            &[m.id],
            false,
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(resp.health.status, HealthStatus::Degraded);
        assert!(resp
            .health
            .warnings
            .iter()
            .any(|w| w.severity == WarningSeverity::Warn));
        assert!(!resp
            .health
            .warnings
            .iter()
            .any(|w| w.severity == WarningSeverity::Critical));
    }

    #[tokio::test]
    async fn aggregate_status_is_critical_when_any_critical_warning_present() {
        let now = read_clock_now();
        let m = fake_memory(1, "f", "personal", now, 0.9, None);
        // 14d-old REPORT → REPORT_STALE_CRITICAL (severity Critical).
        let report = loaded_report_with_topic(
            "personal",
            now - chrono::Duration::days(14),
            "t",
            &[m.id],
            false,
        );
        let mut reports = HashMap::new();
        reports.insert("personal".to_string(), report);

        let pipeline = StructuredReadPipeline::new(
            MockRetriever::new(vec![retrieved(m, 0.9)]),
            MockReportLoader::new(reports),
        )
        .with_clock(FixedClock::arc(now));
        let resp = pipeline
            .read(ReadQuery {
                query_text: "q".into(),
                authorized_boundaries: vec![boundary("personal")],
            })
            .await
            .unwrap();
        assert_eq!(
            resp.health.status,
            HealthStatus::Critical,
            "any Critical warning MUST escalate aggregate status to Critical"
        );
    }
}
