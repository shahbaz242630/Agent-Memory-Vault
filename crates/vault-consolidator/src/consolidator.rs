//! [`Consolidator`] — the sleep-cycle orchestrator.
//!
//! BRD §5.6 lines 894-928 verbatim defines the public surface:
//!
//! ```ignore
//! pub struct Consolidator {
//!     storage: Arc<StorageBackend>,
//!     llm: Arc<dyn LlmProvider>,
//!     embeddings: Arc<dyn EmbeddingProvider>,
//!     config: ConsolidatorConfig,
//! }
//!
//! impl Consolidator {
//!     pub async fn run_consolidation(&self) -> VaultResult<ConsolidationReport> { ... }
//!     pub async fn schedule(&self) -> VaultResult<()> { ... }
//! }
//! ```
//!
//! T0.2.3 commit 1 ships the struct materialisation + [`Consolidator::
//! schedule`] `todo!()` stub. [`Consolidator::run_consolidation`] body
//! lands at commit 2 (Phase 3 `apply_merge` primitive + orchestrator loop).
//! Summary markdown generation lands at commit 3.
//!
//! Per BRD §5.6 lines 971-972 and T0.2.3 iteration 2 boundary-model
//! correction: **one `run_consolidation` call processes ALL boundaries the
//! storage backend reports memory rows in.** The returned
//! [`ConsolidationReport`] carries per-boundary sub-sections inside
//! `summary_markdown`, not separate runs per boundary.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;
use vault_core::{Boundary, Memory, MemoryId, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_llm::LlmProvider;
use vault_storage::{MemoryFilter, StorageBackend};

use crate::phases::cluster::{find_candidate_clusters, Cluster};
use crate::phases::contradiction::detect_contradiction;
use crate::phases::merge::{apply_merge, decide_merge, AppliedMerge, MergeOutcome};
use crate::report::{generate_report, Report};
use crate::summary::generate_summary_markdown;
use crate::topics::discover_topics;

/// Sleep-cycle orchestrator per BRD §5.6 lines 895-913.
///
/// Cheap to clone — all four dependencies are `Arc`-shared. Construct once
/// at application startup; reuse across nightly runs.
#[derive(Clone)]
pub struct Consolidator {
    storage: Arc<StorageBackend>,
    llm: Arc<dyn LlmProvider>,
    embeddings: Arc<dyn EmbeddingProvider>,
    config: ConsolidatorConfig,
}

/// Consolidator configuration knobs per BRD §5.6 lines 902-908 verbatim.
///
/// V0.2 default values match BRD spec exactly; V0.3+ may surface
/// per-vault overrides via the Tauri Settings UI.
#[derive(Clone, Debug)]
pub struct ConsolidatorConfig {
    /// Time of day to schedule the nightly run. T0.2.6 wires the scheduler.
    pub run_at: NaiveTime,
    /// Cosine-similarity threshold above which Phase 1 forms cluster edges.
    /// BRD §5.6 line 904 default: 0.92.
    pub merge_similarity_threshold: f32,
    /// Days of inactivity before a memory's confidence multiplies by 0.9.
    /// BRD §5.6 line 905 default: 180. Phase 4 (T0.2.4) consumes.
    pub decay_after_days: u32,
    /// Days of inactivity before a memory moves to cold archive. BRD §5.6
    /// line 906 default: 365. Phase 4 (T0.2.4) consumes.
    pub archive_after_days: u32,
    /// Hard cap on memories touched per run. BRD §5.6 line 907 default:
    /// 1000. Caps in-memory grouping cost at sane levels for the V0.2
    /// alpha cohort scale (BRD §6.1).
    pub max_memories_per_run: usize,
}

impl Default for ConsolidatorConfig {
    /// Defaults per BRD §5.6 lines 903-907 verbatim.
    fn default() -> Self {
        Self {
            run_at: NaiveTime::from_hms_opt(3, 0, 0).expect("3:00 AM is a valid NaiveTime"),
            merge_similarity_threshold: 0.92,
            decay_after_days: 180,
            archive_after_days: 365,
            max_memories_per_run: 1000,
        }
    }
}

impl Consolidator {
    /// Construct a Consolidator with the given dependencies + config.
    ///
    /// All four fields are `Arc`-shared so the consolidator is cheap to
    /// clone for handing off into scheduler tasks (T0.2.6).
    pub fn new(
        storage: Arc<StorageBackend>,
        llm: Arc<dyn LlmProvider>,
        embeddings: Arc<dyn EmbeddingProvider>,
        config: ConsolidatorConfig,
    ) -> Self {
        Self {
            storage,
            llm,
            embeddings,
            config,
        }
    }

    /// Run a full consolidation cycle per BRD §5.6 lines 933-955.
    ///
    /// **Pipeline (T0.2.3 commit 2):**
    /// 1. Enumerate every memory in the vault via
    ///    [`StorageBackend::list_memories`] (default filter — excludes
    ///    already-superseded memories per `MemoryFilter::include_superseded
    ///    = false`).
    /// 2. Group by boundary into a [`BTreeMap`] for deterministic per-
    ///    boundary iteration order (drives the summary markdown's
    ///    boundary-sub-section ordering at commit 3).
    /// 3. For each boundary: Phase 1 ([`find_candidate_clusters`]) → for
    ///    each cluster: Phase 2 ([`decide_merge`]) → dispatch on
    ///    [`MergeOutcome`]:
    ///    - `Merge`: call Phase 3 [`apply_merge`]; record
    ///      [`AppliedMergeWithContext`] for the summary markdown.
    ///    - `KeepSeparate`: no-op (vector similarity was a false positive).
    ///    - `Contradiction`: build a [`ConflictReview`] row; surfaced via
    ///      [`ConsolidationReport::conflicts_for_user_review`] (do not
    ///      auto-resolve per BRD §5.6 line 944).
    /// 4. Build the [`ConsolidationReport`] with aggregated counts +
    ///    `summary_markdown: String::new()` (commit 3 fills it via
    ///    `generate_summary_markdown(&run_state)`).
    ///
    /// **Phase 4 (decay/archive) is NOT YET WIRED** — `memories_archived`
    /// returns 0 at commit 2. Phase 4 ships at T0.2.4 per BRD §6.2.
    ///
    /// **Checkpoints (BRD §5.6 line 957) are NOT YET WIRED** — checkpoint
    /// creation + rollback ship at T0.2.5 per BRD §6.2. The
    /// `since: Option<DateTime<Utc>>` parameter on
    /// [`find_candidate_clusters`] is passed `None` (full-scan) at T0.2.3.
    #[instrument(skip(self))]
    pub async fn run_consolidation(&self) -> VaultResult<ConsolidationReport> {
        let started_at = Utc::now();

        // Step 1: enumerate all non-superseded memories. Default filter
        // excludes already-superseded rows so Phase 1 clustering never sees
        // them (prevents re-supersession at this layer per ADR-046's
        // single-supersession assumption).
        let all_memories = self
            .storage
            .list_memories(MemoryFilter::default(), None)
            .await?;

        // Step 2: group by boundary. BTreeMap gives deterministic
        // alphabetical iteration (Boundary derives Ord) which downstream
        // summary-markdown generation (commit 3) relies on for stable
        // sub-section ordering.
        let mut by_boundary: BTreeMap<Boundary, Vec<Memory>> = BTreeMap::new();
        for memory in all_memories {
            by_boundary
                .entry(memory.boundary.clone())
                .or_default()
                .push(memory);
        }

        // Step 3: per-boundary Phase 1 → Phase 2 → Phase 3 pipeline.
        let mut run_state = RunState {
            started_at,
            duration: Duration::ZERO, // populated after the loop completes.
            memories_processed: 0,
            per_boundary: BTreeMap::new(),
        };
        for (boundary, memories) in by_boundary {
            run_state.memories_processed += memories.len();

            let clusters = find_candidate_clusters(
                self.storage.as_ref(),
                self.embeddings.as_ref(),
                &boundary,
                self.config.merge_similarity_threshold,
                None, // T0.2.5 wires actual since-checkpoint values.
            )
            .await?;

            let mut boundary_summary = BoundarySummary::default();
            for cluster in &clusters {
                let outcome =
                    decide_merge(cluster, self.llm.as_ref(), self.storage.as_ref()).await?;
                match outcome {
                    MergeOutcome::Merge {
                        merged_text,
                        reasoning,
                    } => {
                        // Capture pre-merge content snippets from the in-scope
                        // per-boundary `memories` Vec BEFORE apply_merge runs
                        // (apply_merge marks members superseded but preserves
                        // their rows; we read from the pre-merge enumeration
                        // here to avoid an extra storage round-trip).
                        let member_ids: HashSet<MemoryId> =
                            cluster.member_row_ids.iter().copied().collect();
                        let pre_merge_contents: Vec<(MemoryId, String)> = memories
                            .iter()
                            .filter(|m| member_ids.contains(&m.id))
                            .map(|m| (m.id, m.content.clone()))
                            .collect();

                        let applied = apply_merge(
                            cluster,
                            &merged_text,
                            &reasoning,
                            self.storage.as_ref(),
                            self.embeddings.as_ref(),
                        )
                        .await?;
                        boundary_summary
                            .applied_merges
                            .push(AppliedMergeWithContext {
                                cluster: cluster.clone(),
                                applied,
                                reasoning,
                                merged_text,
                                pre_merge_contents,
                            });
                    }
                    MergeOutcome::KeepSeparate { .. } => {
                        // Vector similarity was a false positive per Phase 2's
                        // judgement. Originals stay; no state change.
                    }
                    MergeOutcome::Contradiction {
                        reasoning,
                        clear_winner,
                    } => {
                        match clear_winner {
                            Some(winner_id) => {
                                // Auto-resolve via ADR-051 invalidate(): mark
                                // every non-winner cluster member's
                                // `valid_until = now`. Retrieval thereafter
                                // skips invalidated rows by default. Per the
                                // locked-next-arc Step 4 contract, failure to
                                // invalidate is logged-and-continued, NOT a
                                // run abort — the next nightly cycle will
                                // re-detect the contradiction and try again.
                                let now = Utc::now();
                                let losers: Vec<MemoryId> = cluster
                                    .member_row_ids
                                    .iter()
                                    .copied()
                                    .filter(|id| *id != winner_id)
                                    .collect();
                                tracing::info!(
                                    target: "vault_consolidator::contradiction",
                                    cluster_id = cluster.id,
                                    winner = %winner_id,
                                    loser_count = losers.len(),
                                    "Phi-4 surfaced contradiction with clear winner; \
                                     invalidating losers per ADR-051"
                                );
                                for loser in losers {
                                    let invalidate_reason = format!(
                                        "auto-invalidated by consolidator (cluster {}): {reasoning}",
                                        cluster.id
                                    );
                                    if let Err(e) =
                                        self.storage.invalidate(loser, now, invalidate_reason).await
                                    {
                                        tracing::warn!(
                                            target: "vault_consolidator::contradiction",
                                            loser = %loser,
                                            error = %e,
                                            "invalidate failed; next consolidation cycle will retry"
                                        );
                                    }
                                }
                            }
                            None => {
                                // Legacy path — queue ConflictReview for the
                                // user to resolve. BRD §5.6 line 944.
                                boundary_summary.contradictions.push(ConflictReview {
                                    conflict_id: Uuid::new_v4(),
                                    boundary: boundary.clone(),
                                    conflicting_memory_ids: cluster.member_row_ids.clone(),
                                    reasoning,
                                    flagged_at: Utc::now(),
                                });
                            }
                        }
                    }
                }
            }
            run_state.per_boundary.insert(boundary, boundary_summary);
        }

        // ── Phase 2b: topic-level contradiction detection (T0.3.x A5) ──────
        //
        // Decoupled from the 0.92 merge gate, which can never catch a
        // knowledge-update contradiction (the conflicting pair sits below
        // 0.92, so Phase 1 never clusters it). Re-enumerate the post-merge
        // active set — excludes superseded rows (default filter) AND
        // already-invalidated rows (`valid_until` set) so retired facts are
        // not re-judged — group each boundary's memories into K-means topics
        // (grouping only; `llm = None` skips topic-naming calls), and ask the
        // LLM whether each topic group of ≥ 2 holds a same-subject
        // contradiction. Stale facts are invalidated via the bi-temporal
        // `invalidate()` API (ADR-051); retrieval then returns only the
        // current truth.
        //
        // Safety: only ids the model returns that are actually in the group
        // are invalidated, and an entire-group invalidation is refused (at
        // least one fact must survive as the current truth) — a misbehaving
        // model cannot mass-retire a topic.
        //
        // Failure semantics (locked-next-arc Step 4): a per-topic LLM failure
        // or a single failed invalidate is logged-and-continued, NOT a run
        // abort — the merge work already committed durably; the next nightly
        // cycle retries.
        let active: Vec<Memory> = self
            .storage
            .list_memories(MemoryFilter::default(), None)
            .await?
            .into_iter()
            .filter(|m| m.valid_until.is_none())
            .collect();
        let mut active_by_boundary: BTreeMap<Boundary, Vec<Memory>> = BTreeMap::new();
        for memory in active {
            active_by_boundary
                .entry(memory.boundary.clone())
                .or_default()
                .push(memory);
        }
        for (boundary, memories) in &active_by_boundary {
            if memories.len() < 2 {
                continue;
            }
            // `llm = None`: we need the topic GROUPING, not Phi-4 topic
            // labels (labelling is the REPORT's job). This keeps the
            // contradiction pass to exactly one LLM call per multi-member
            // topic.
            let topic_map =
                discover_topics(boundary, memories, self.embeddings.as_ref(), None).await?;
            let lookup: HashMap<MemoryId, &Memory> = memories.iter().map(|m| (m.id, m)).collect();

            for topic in &topic_map.topics {
                if topic.member_ids.len() < 2 {
                    continue;
                }
                let group: Vec<&Memory> = topic
                    .member_ids
                    .iter()
                    .filter_map(|id| lookup.get(id).copied())
                    .collect();
                if group.len() < 2 {
                    continue;
                }

                let verdict = match detect_contradiction(&group, self.llm.as_ref()).await {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(
                            target: "vault_consolidator::contradiction",
                            topic = %topic.label,
                            error = %e,
                            "contradiction detection failed for topic; skipping (next cycle retries)"
                        );
                        continue;
                    }
                };

                let group_ids: HashSet<MemoryId> = group.iter().map(|m| m.id).collect();
                let stale: Vec<MemoryId> = verdict
                    .stale_memory_ids
                    .iter()
                    .copied()
                    .filter(|id| group_ids.contains(id))
                    .collect();

                if stale.is_empty() {
                    continue;
                }
                // Safety net: never retire an entire topic group — at least
                // one fact must remain as the current truth. A model marking
                // everything stale is misbehaving; skip it loudly.
                if stale.len() >= group.len() {
                    tracing::warn!(
                        target: "vault_consolidator::contradiction",
                        topic = %topic.label,
                        stale_count = stale.len(),
                        group_size = group.len(),
                        "contradiction judge marked the ENTIRE group stale; refusing to \
                         mass-invalidate (at least one fact must remain current) — skipping"
                    );
                    continue;
                }

                let now = Utc::now();
                tracing::info!(
                    target: "vault_consolidator::contradiction",
                    topic = %topic.label,
                    stale_count = stale.len(),
                    group_size = group.len(),
                    "topic-level contradiction detected; invalidating stale facts per ADR-051"
                );
                for stale_id in stale {
                    let reason = format!(
                        "auto-invalidated by consolidator (topic '{}'): {}",
                        topic.label, verdict.reasoning
                    );
                    if let Err(e) = self.storage.invalidate(stale_id, now, reason).await {
                        tracing::warn!(
                            target: "vault_consolidator::contradiction",
                            stale = %stale_id,
                            error = %e,
                            "invalidate failed; next consolidation cycle will retry"
                        );
                    }
                }
            }
        }

        // Step 4: build the report. Populate RunState.duration first so
        // generate_summary_markdown can render the header from it.
        let duration = Utc::now()
            .signed_duration_since(started_at)
            .to_std()
            .unwrap_or(Duration::ZERO);
        run_state.duration = duration;

        let memories_merged: usize = run_state
            .per_boundary
            .values()
            .flat_map(|b| &b.applied_merges)
            .map(|m| m.applied.superseded_memory_ids.len())
            .sum();
        let contradictions_resolved: usize = run_state
            .per_boundary
            .values()
            .map(|b| b.contradictions.len())
            .sum();
        let conflicts_for_user_review: Vec<ConflictReview> = run_state
            .per_boundary
            .values()
            .flat_map(|b| b.contradictions.iter().cloned())
            .collect();

        // T0.2.5 wires the real checkpoint identifier here; at T0.2.3 we
        // pass a stable placeholder string so the footer renders the
        // forward-pointer pinned by summary.rs's
        // `footer_emits_checkpoint_placeholder_with_t025_rollback_note`.
        let checkpoint_placeholder = "pending-T0.2.5";
        let summary_markdown = generate_summary_markdown(&run_state, checkpoint_placeholder);

        Ok(ConsolidationReport {
            memories_processed: run_state.memories_processed,
            memories_merged,
            contradictions_resolved,
            memories_archived: 0, // Phase 4 ships at T0.2.4.
            duration,
            conflicts_for_user_review,
            summary_markdown,
        })
    }

    /// Build the per-boundary REPORT artifacts (ADR-053) for the current
    /// vault state — the curated "what is currently true, grouped by
    /// topic" view the structured read pipeline serves from (NOT the
    /// run-audit `summary_markdown` produced by [`Self::run_consolidation`]).
    ///
    /// Intended to be called by the application layer immediately AFTER
    /// [`Self::run_consolidation`] within the same safety wrapper, so the
    /// topics + facts reflect the post-merge / post-invalidate state. The
    /// consolidator builds the [`Report`] values but does NOT persist them:
    /// the filesystem write (`<vault_root>/reports/<boundary>.report.json`)
    /// lives in `vault-app::Application::run_consolidation_with_safety`,
    /// which owns the `vault_root` path. This keeps the consolidator
    /// filesystem-agnostic (it talks only to storage traits + the
    /// embedder + the LLM), mirroring how the cross-process lockfile also
    /// lives in the app layer.
    ///
    /// One [`Report`] per non-empty boundary, in deterministic
    /// (alphabetical) boundary order. `run_id` is stamped into each report
    /// so a reader can correlate a REPORT with the run that produced it.
    ///
    /// # Errors
    ///
    /// - [`vault_core::VaultError`] propagated from `list_memories` or from
    ///   [`discover_topics`] (embedding failure / dim mismatch). A failure
    ///   here aborts report generation for the whole run; the app layer's
    ///   safety wrapper surfaces it. The previous REPORT files (if any)
    ///   stay untouched because no write has happened yet.
    #[instrument(skip(self), fields(run_id = %run_id))]
    pub async fn generate_reports(&self, run_id: Uuid) -> VaultResult<Vec<Report>> {
        let generated_at = Utc::now();

        // Re-enumerate active (non-superseded) memories and group by
        // boundary — mirrors run_consolidation steps 1-2 so topic discovery
        // sees exactly the set that survived the merge pipeline.
        let all_memories = self
            .storage
            .list_memories(MemoryFilter::default(), None)
            .await?;
        let mut by_boundary: BTreeMap<Boundary, Vec<Memory>> = BTreeMap::new();
        for memory in all_memories {
            by_boundary
                .entry(memory.boundary.clone())
                .or_default()
                .push(memory);
        }

        let mut reports = Vec::with_capacity(by_boundary.len());
        for (boundary, memories) in by_boundary {
            // Pass the LLM so Phi-4 names each topic cluster; discover_topics
            // falls back to placeholder labels + topic_names_unavailable=true
            // if the LLM is unavailable or returns malformed JSON, which the
            // read pipeline surfaces as the TOPIC_NAMES_UNAVAILABLE warning.
            let topic_map = discover_topics(
                &boundary,
                &memories,
                self.embeddings.as_ref(),
                Some(self.llm.as_ref()),
            )
            .await?;
            reports.push(generate_report(&topic_map, &memories, run_id, generated_at));
        }
        Ok(reports)
    }

    /// Schedule the consolidator to run at the configured `run_at` time.
    ///
    /// **Body lands at T0.2.6 per BRD §6.2 line 1453** ("vault-consolidator:
    /// Scheduling"). The method signature is present from T0.2.3 commit 1
    /// so the `impl Consolidator` block matches BRD §5.6 line 912 verbatim;
    /// calling `schedule()` before T0.2.6 panics loudly.
    #[allow(clippy::todo)]
    pub async fn schedule(&self) -> VaultResult<()> {
        todo!("T0.2.6 — vault-consolidator: Scheduling")
    }
}

/// One consolidation run's report per BRD §5.6 lines 915-928 verbatim.
///
/// T0.2.3 commit 1 lands the type shape; fields are populated by commits
/// 2-3. Tauri's Consolidation Report viewer (T0.2.15) consumes this directly
/// via the cross-crate re-export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidationReport {
    pub memories_processed: usize,
    pub memories_merged: usize,
    pub contradictions_resolved: usize,
    pub memories_archived: usize,
    pub duration: Duration,
    pub conflicts_for_user_review: Vec<ConflictReview>,
    /// Human-readable Markdown summary per BRD §5.6 lines 959-973. Outer
    /// document is run-scoped (Run header + Footer with checkpoint ID);
    /// Merges + Contradictions sections contain per-boundary sub-sections
    /// per T0.2.3 iteration 3 §item-4 lock. Decay section is aggregate
    /// with inline per-boundary counts per BRD §5.6 line 968 "no per-memory
    /// detail."
    ///
    /// Generated at T0.2.3 commit 3 (`generate_summary_markdown`).
    pub summary_markdown: String,
}

/// One contradiction flagged by Phase 2 for user review per BRD §5.6
/// line 944 + line 921.
///
/// Locked field shape per T0.2.3 iteration 2 §"ConflictReview source-read"
/// and iteration 3 §item-3 confirmation. Lives in vault-consolidator
/// (concrete-vs-hypothetical-consumer rule); promote to vault-core at
/// T0.2.15 (Tauri ConflictReview viewer) if cross-crate need surfaces.
///
/// Per BRD §5.6 line 944: "For contradictions, write to `ConflictReview`
/// queue, do not auto-resolve." T0.2.3 returns the list of conflicts via
/// [`ConsolidationReport::conflicts_for_user_review`]; persistent queue is
/// a forward-task (T0.2.15 or a dedicated T0.2.x).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConflictReview {
    /// Stable identifier for the review queue. Generated at Phase 2
    /// dispatch time; preserved across consolidation runs once persisted.
    pub conflict_id: Uuid,
    /// Boundary the conflict was found in. Per BRD §11.4.3 every memory
    /// has exactly one boundary; conflicts inherit that constraint.
    pub boundary: Boundary,
    /// IDs of the memories Phase 2 flagged as contradicting. Always size
    /// ≥ 2 (singletons can't contradict).
    pub conflicting_memory_ids: Vec<MemoryId>,
    /// Phase 2's natural-language explanation. Surfaced in the summary
    /// markdown's Contradictions section and (later) the Tauri review UI.
    pub reasoning: String,
    /// When the conflict was flagged. Useful for review-queue ordering
    /// and for the audit log.
    pub flagged_at: chrono::DateTime<chrono::Utc>,
}

// ─────────────────────────────────────────────────────────────────────────
// Crate-private orchestration types — consumed by `run_consolidation` and by
// `summary::generate_summary_markdown`. Visibility promoted from `private`
// to `pub(crate)` at T0.2.3 commit 3 per ADR-047 §b (new `src/summary.rs`
// module needs to name them in test helpers + its function signatures).
// Not part of the public crate surface.
// ─────────────────────────────────────────────────────────────────────────

/// Accumulator threaded through `run_consolidation` and consumed by
/// `summary::generate_summary_markdown`. Captures everything the summary
/// renderer needs without re-reading state from storage.
///
/// `started_at` + `duration` populate the Run header (BRD §5.6 line 965);
/// `memories_processed` populates the header's total; `per_boundary` drives
/// per-boundary Merges + Contradictions sub-sections.
#[derive(Debug)]
pub(crate) struct RunState {
    pub started_at: DateTime<Utc>,
    pub duration: Duration,
    pub memories_processed: usize,
    pub per_boundary: BTreeMap<Boundary, BoundarySummary>,
}

/// One boundary's contribution to the run: applied merges + contradictions
/// captured for that boundary. Decay (T0.2.4) will extend this struct with
/// per-boundary decay/archive counts.
#[derive(Debug, Default)]
pub(crate) struct BoundarySummary {
    pub applied_merges: Vec<AppliedMergeWithContext>,
    pub contradictions: Vec<ConflictReview>,
}

/// An applied merge plus the inputs the summary markdown needs:
/// - `cluster`: pre-merge IDs (ADR-045 §a sort-by-id-ascending order).
/// - `applied`: post-merge id + aggregated access/confidence per BRD §5.6
///   line 947.
/// - `reasoning`: LLM's natural-language explanation, surfaced verbatim in
///   the Merges section per BRD §5.6 line 966.
/// - `merged_text`: the consolidated content the LLM produced (captured
///   from `MergeOutcome::Merge` before the apply step consumes it).
/// - `pre_merge_contents`: each cluster member's original content (id,
///   content) captured from the in-scope per-boundary memory enumeration
///   BEFORE `apply_merge` marks members superseded. Required for BRD §5.6
///   line 966 "pre-merge memory IDs (truncated content snippets)".
#[derive(Debug)]
pub(crate) struct AppliedMergeWithContext {
    pub cluster: Cluster,
    pub applied: AppliedMerge,
    pub reasoning: String,
    pub merged_text: String,
    pub pre_merge_contents: Vec<(MemoryId, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ConsolidatorConfig defaults match BRD §5.6 lines 903-907 ─────────

    #[test]
    fn consolidator_config_default_matches_brd_spec() {
        let c = ConsolidatorConfig::default();
        assert_eq!(c.run_at, NaiveTime::from_hms_opt(3, 0, 0).unwrap());
        assert_eq!(c.merge_similarity_threshold, 0.92);
        assert_eq!(c.decay_after_days, 180);
        assert_eq!(c.archive_after_days, 365);
        assert_eq!(c.max_memories_per_run, 1000);
    }
}
