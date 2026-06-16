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
//! T0.2.3 commit 1 shipped the struct materialisation; commit 2 added the
//! [`Consolidator::run_consolidation`] body (Phase 3 `apply_merge` primitive and
//! the orchestrator loop); commit 3 added summary-markdown generation.
//! [`Consolidator::schedule`] (the headless nightly loop, T0.2.6) is now
//! implemented atop [`crate::scheduler`]; the production scheduler lives in
//! `vault-app`, which also owns the lockfile, timeout, and REPORT persistence.
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
use vault_storage::{CheckpointId, MemoryFilter, StorageBackend};

use crate::phases::candidates::nearest_neighbor_candidate_pairs;
use crate::phases::cluster::{find_candidate_clusters, Cluster};
use crate::phases::contradiction::judge_candidate_pairs;
use crate::phases::decay::{self, plan_decay};
use crate::phases::dedup;
use crate::phases::enrich::{enrich_one, EnrichedFact};
use crate::phases::extract;
use crate::phases::merge::{apply_merge, decide_merge, AppliedMerge, MergeOutcome};
use crate::report::{generate_report, Report};
use crate::summary::generate_summary_markdown;
use crate::topics::discover_topics;

/// The outcome of reconciling a contradiction verdict against the topic group
/// it was judged over — the safety net that stops a misbehaving model from
/// retiring an entire topic. See [`resolve_stale_ids`].
#[derive(Debug, PartialEq, Eq)]
enum StaleResolution {
    /// No in-group stale ids were named — nothing to invalidate.
    Nothing,
    /// Invalidate exactly these ids (a strict, non-empty subset of the group).
    Invalidate(Vec<MemoryId>),
    /// The verdict would retire the whole group; refused. Carries counts for
    /// the WARN line.
    RefusedMassInvalidate { in_group: usize, group: usize },
}

/// Reconcile a contradiction verdict's stale ids against the id set it was
/// judged over (the boundary's active set in the ADR-065 nearest-neighbor
/// path):
/// - keep only ids that are actually members of the set (ignore strays a
///   misbehaving model might invent),
/// - deduplicate,
/// - refuse a sweep that would retire every member — at least one fact must
///   remain as the current truth.
///
/// Extracted as a pure function so the safety net is unit-testable
/// independently of candidate generation. Used by `run_consolidation`'s
/// Phase 2b. `group_ids` is named generically — it is whatever id set the
/// verdict was produced over.
fn resolve_stale_ids(verdict_ids: &[MemoryId], group_ids: &HashSet<MemoryId>) -> StaleResolution {
    let mut in_group: Vec<MemoryId> = Vec::new();
    for id in verdict_ids {
        if group_ids.contains(id) && !in_group.contains(id) {
            in_group.push(*id);
        }
    }
    if in_group.is_empty() {
        return StaleResolution::Nothing;
    }
    if in_group.len() >= group_ids.len() {
        return StaleResolution::RefusedMassInvalidate {
            in_group: in_group.len(),
            group: group_ids.len(),
        };
    }
    StaleResolution::Invalidate(in_group)
}

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

    /// The configured local time-of-day for the nightly run (BRD §5.6
    /// `run_at`). Exposed so the application-layer scheduler can compute the
    /// next run instant without reaching into the private [`ConsolidatorConfig`].
    pub fn run_at(&self) -> NaiveTime {
        self.config.run_at
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

        // Step 1: snapshot the full memory set (incl. superseded) up-front —
        // this is the pre-run baseline the T0.2.5 checkpoint diffs against at
        // the end of the run (see `capture_checkpoint`). Including superseded
        // rows means a row that was already superseded before this run is NOT
        // mis-classified as "created" by the diff.
        let pre_snapshot = self
            .storage
            .list_memories(
                MemoryFilter {
                    include_superseded: true,
                    ..MemoryFilter::default()
                },
                None,
            )
            .await?;

        // The run's own logic operates on the active (non-superseded) subset —
        // identical to the previous `MemoryFilter::default()` enumeration, so
        // Phase 1 clustering never sees superseded rows (ADR-046's
        // single-supersession assumption). Derived in-process to avoid a second
        // DB scan.
        let all_memories: Vec<Memory> = pre_snapshot
            .iter()
            .filter(|m| m.superseded_by.is_none())
            .cloned()
            .collect();

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
            memories_decayed: 0, // populated by Phase 4 below.
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
                // ── Phase 2-pre (ADR-063): deterministic dedup ────────────
                // If the cluster is near-identical (calibrated two-axis gate),
                // collapse it with plain code — keep the canonical survivor,
                // supersede the rest, roll aggregates — and SKIP the LLM. This
                // is the structural-overflow case, so removing it from the LLM
                // path makes the overflow/skip class disappear. A dedup failure
                // is logged-and-skipped (counted), never a run abort.
                match self.try_dedup_cluster(cluster, &memories).await {
                    Ok(Some(superseded_count)) => {
                        boundary_summary.deduped_clusters += 1;
                        boundary_summary.deduped_memories += superseded_count;
                        continue;
                    }
                    Ok(None) => { /* not near-identical — fall through to LLM */ }
                    Err(e) => {
                        tracing::warn!(
                            target: "vault_consolidator::dedup",
                            cluster_id = cluster.id,
                            error = %e,
                            "deterministic dedup failed for cluster; skipping (next cycle retries)"
                        );
                        boundary_summary.skipped_clusters += 1;
                        continue;
                    }
                }

                // Per-cluster resilience (ADR-062 iter 2): a Phase-2 LLM
                // failure (e.g. a truncated/malformed merge response on an
                // oversized cluster) is logged-and-skipped, NOT propagated —
                // one bad cluster must never abort the whole consolidation run.
                // Mirrors the topic-level contradiction pass's failure
                // semantics. The skipped cluster's members stay active
                // (unmerged); the next nightly cycle retries.
                let outcome =
                    match decide_merge(cluster, self.llm.as_ref(), self.storage.as_ref()).await {
                        Ok(o) => o,
                        Err(e) => {
                            tracing::warn!(
                                target: "vault_consolidator::merge",
                                cluster_size = cluster.member_row_ids.len(),
                                error = %e,
                                "Phase 2 merge decision failed for cluster; skipping \
                                 (next cycle retries)"
                            );
                            boundary_summary.skipped_clusters += 1;
                            continue;
                        }
                    };
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

                        let applied = match apply_merge(
                            cluster,
                            &merged_text,
                            &reasoning,
                            self.storage.as_ref(),
                            self.embeddings.as_ref(),
                        )
                        .await
                        {
                            Ok(a) => a,
                            Err(e) => {
                                tracing::warn!(
                                    target: "vault_consolidator::merge",
                                    cluster_size = cluster.member_row_ids.len(),
                                    error = %e,
                                    "Phase 3 apply_merge failed for cluster; skipping \
                                     (next cycle retries)"
                                );
                                boundary_summary.skipped_clusters += 1;
                                continue;
                            }
                        };
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

        // ── Phase 2b: nearest-neighbor contradiction detection (T0.3.x A5, ADR-065) ──
        //
        // Decoupled from the 0.92 merge gate (which never clusters a
        // knowledge-update pair — it sits below 0.92) AND from K-means topic
        // grouping. ADR-060's premise — that a topic co-locates the
        // conflicting pair — was proven FALSE in the §7 dogfood (2026-06-01):
        // K-means split the Tesla→Rivian pair across groups, so it was never
        // judged and A5 silently failed. Contradiction detection is a
        // nearest-neighbor problem: for each active fact, its top-K cosine
        // neighbors above a floor are the candidate partners (the conflicting
        // pair is each other's *nearest* neighbor, so it is always surfaced).
        //
        // Re-enumerate the post-merge active set — excludes superseded rows
        // (default filter) AND already-invalidated rows (`valid_until` set) so
        // retired facts are not re-judged — embed each fact, generate
        // candidate pairs (`phases::candidates`), and hand them to the pairwise
        // judge + recency aggregator (`judge_candidate_pairs`). Stale facts are
        // invalidated via the bi-temporal `invalidate()` API (ADR-051);
        // retrieval then returns only the current truth.
        //
        // Safety: only ids actually in the boundary's active set are
        // invalidated, and a sweep of the ENTIRE set is refused (≥ 1 fact must
        // remain current). Recency already keeps the newest fact in any chain;
        // this guard is belt-and-braces against a misbehaving model.
        //
        // Failure semantics (locked-next-arc Step 4): a per-pair LLM failure or
        // a single failed invalidate is logged-and-continued, NOT a run abort —
        // the merge work already committed durably; the next nightly cycle
        // retries.
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

            // Embed each active fact. Memory rows carry `embedding: None`
            // (vectors live in LanceDB per ADR-045 §c); re-embed via the shared
            // provider — embeds are deterministic, so the vectors match those
            // Phase 1 clustering computed.
            let mut embeddings = Vec::with_capacity(memories.len());
            for m in memories {
                embeddings.push(self.embeddings.embed(&m.content).await?);
            }

            // Nearest-neighbor candidate pairs (index pairs into `memories`).
            let pair_indices = nearest_neighbor_candidate_pairs(&embeddings);
            if pair_indices.is_empty() {
                continue;
            }
            let pairs: Vec<(&Memory, &Memory)> = pair_indices
                .iter()
                .map(|&(i, j)| (&memories[i], &memories[j]))
                .collect();
            tracing::info!(
                target: "vault_consolidator::contradiction",
                boundary = %boundary,
                active = memories.len(),
                candidate_pairs = pairs.len(),
                "Phase 2b: judging nearest-neighbor contradiction candidates"
            );

            let verdict = match judge_candidate_pairs(&pairs, self.llm.as_ref()).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        target: "vault_consolidator::contradiction",
                        boundary = %boundary,
                        error = %e,
                        "contradiction judging failed for boundary; skipping (next cycle retries)"
                    );
                    continue;
                }
            };

            // Reconcile the verdict against the boundary's active set: keep
            // only real members, dedup, and refuse a sweep of the entire set
            // (≥ 1 fact must remain the current truth).
            let active_ids: HashSet<MemoryId> = memories.iter().map(|m| m.id).collect();
            let stale = match resolve_stale_ids(&verdict.stale_memory_ids, &active_ids) {
                StaleResolution::Nothing => continue,
                StaleResolution::RefusedMassInvalidate {
                    in_group,
                    group: active_count,
                } => {
                    tracing::warn!(
                        target: "vault_consolidator::contradiction",
                        boundary = %boundary,
                        stale_count = in_group,
                        active = active_count,
                        "contradiction judging marked the ENTIRE active set stale; refusing to \
                         mass-invalidate (at least one fact must remain current) — skipping"
                    );
                    continue;
                }
                StaleResolution::Invalidate(ids) => ids,
            };

            let now = Utc::now();
            tracing::info!(
                target: "vault_consolidator::contradiction",
                boundary = %boundary,
                stale_count = stale.len(),
                active = memories.len(),
                "nearest-neighbor contradiction(s) detected; invalidating stale facts per ADR-051"
            );
            for stale_id in stale {
                let reason = format!(
                    "auto-invalidated by consolidator (boundary '{boundary}'): {}",
                    verdict.reasoning
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

        // ── Phase 4: confidence decay (BRD §5.6 line 994; T0.2.4) ──
        //
        // The sleep cycle's final pass: a fact left untouched for
        // `decay_after_days` has its confidence multiplied by 0.9, quietly
        // demoting stale knowledge without deleting it. Metadata-only (never
        // re-embeds — preserves the ADR-074 enriched vector) and idempotent (a
        // per-fact marker stops a second back-to-back run re-decaying, so the
        // run converges per BRD §5.6 line 1022). Cold archive (BRD §5.6 lines
        // 995-996) is the other half of Phase 4 and lands in a follow-up batch.
        run_state.memories_decayed = self.decay_memories().await;

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
        // ADR-063 dedup + skip accounting, summed across boundaries.
        let clusters_deduped: usize = run_state
            .per_boundary
            .values()
            .map(|b| b.deduped_clusters)
            .sum();
        let memories_deduped: usize = run_state
            .per_boundary
            .values()
            .map(|b| b.deduped_memories)
            .sum();
        let clusters_skipped: usize = run_state
            .per_boundary
            .values()
            .map(|b| b.skipped_clusters)
            .sum();

        // T0.2.5 / A2: capture a rollback checkpoint of everything this run
        // changed (diff of the pre-run snapshot vs the post-run state). A
        // capture failure is logged-and-continued — the run's mutations are
        // already durably committed; the only loss is this run's undo-ability,
        // and the next cycle creates a fresh checkpoint. Never aborts the run.
        let checkpoint_id = match self.capture_checkpoint(&pre_snapshot).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(
                    target: "vault_consolidator::checkpoint",
                    error = %e,
                    "checkpoint capture failed; run committed but is NOT rollback-able \
                     (next cycle creates a fresh checkpoint)"
                );
                None
            }
        };
        let checkpoint_label = checkpoint_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none (no changes this run)".to_string());
        let summary_markdown = generate_summary_markdown(&run_state, &checkpoint_label);

        Ok(ConsolidationReport {
            memories_processed: run_state.memories_processed,
            memories_merged,
            contradictions_resolved,
            memories_archived: 0, // Cold archive lands in a follow-up batch.
            memories_decayed: run_state.memories_decayed,
            clusters_deduped,
            memories_deduped,
            clusters_skipped,
            duration,
            conflicts_for_user_review,
            checkpoint_id,
            summary_markdown,
        })
    }

    /// T0.2.5 / A2 — capture a rollback checkpoint for the run that just
    /// mutated the vault.
    ///
    /// Re-enumerates the full memory set (incl. superseded), diffs it against
    /// `pre_snapshot` to find every changed / created memory
    /// ([`crate::checkpoint::diff_to_entries`]), and persists them via
    /// [`StorageBackend::create_checkpoint`] (which prunes to the retention cap).
    /// Returns the new [`CheckpointId`], or `Ok(None)` when the run changed
    /// nothing (no checkpoint is created for a no-op run — schema invariant:
    /// a checkpoint records a run that changed ≥ 1 memory).
    async fn capture_checkpoint(
        &self,
        pre_snapshot: &[Memory],
    ) -> VaultResult<Option<CheckpointId>> {
        let post_snapshot = self
            .storage
            .list_memories(
                MemoryFilter {
                    include_superseded: true,
                    ..MemoryFilter::default()
                },
                None,
            )
            .await?;
        let entries = crate::checkpoint::diff_to_entries(
            pre_snapshot,
            &post_snapshot,
            self.embeddings.as_ref(),
        )
        .await?;
        if entries.is_empty() {
            return Ok(None);
        }
        let id = self.storage.create_checkpoint(&entries).await?;
        tracing::info!(
            target: "vault_consolidator::checkpoint",
            checkpoint = %id,
            entries = entries.len(),
            "consolidation checkpoint created"
        );
        Ok(Some(id))
    }

    /// Phase 4 (T0.2.4): decay the confidence of cold facts.
    ///
    /// Enumerates the active set, plans which facts are past the
    /// `decay_after_days` idle threshold (and not already decayed this period —
    /// see [`plan_decay`]), and applies each via the metadata-only
    /// [`StorageBackend::apply_decay`] primitive, stamping the idempotency
    /// marker so a second back-to-back run converges. Returns the number of
    /// facts decayed.
    ///
    /// Failure semantics (locked-next-arc Step 4): a failed enumeration returns
    /// 0 (the run's merge/contradiction work already committed durably); a
    /// per-fact `apply_decay` failure is logged-and-counted and the loop
    /// continues — one bad fact never aborts the pass, and it retries next cycle
    /// (its marker was never written).
    ///
    /// [`StorageBackend::apply_decay`]: vault_storage::StorageBackend::apply_decay
    async fn decay_memories(&self) -> usize {
        let now = Utc::now();
        let memories = match self
            .storage
            .list_memories(MemoryFilter::default(), None)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!(
                    target: "vault_consolidator::decay",
                    error = %e,
                    "Phase 4 decay: failed to enumerate active set; skipping (next cycle retries)"
                );
                return 0;
            }
        };
        let plan = plan_decay(&memories, self.config.decay_after_days, now);
        if plan.is_empty() {
            return 0;
        }
        let by_id: HashMap<MemoryId, &Memory> = memories.iter().map(|m| (m.id, m)).collect();
        let mut decayed = 0;
        for pd in plan {
            // The id came from `plan_decay(&memories, …)`, so it is always present.
            let Some(mem) = by_id.get(&pd.id) else {
                continue;
            };
            let mut new_metadata = mem.metadata.clone();
            decay::set_decay_marker(&mut new_metadata, now);
            match self
                .storage
                .apply_decay(pd.id, pd.new_confidence, new_metadata)
                .await
            {
                Ok(_) => decayed += 1,
                Err(e) => {
                    tracing::warn!(
                        target: "vault_consolidator::decay",
                        memory = %pd.id,
                        error = %e,
                        "Phase 4 decay: apply_decay failed for fact; skipping (next cycle retries)"
                    );
                }
            }
        }
        tracing::info!(
            target: "vault_consolidator::decay",
            decayed,
            examined = memories.len(),
            "Phase 4 decay pass complete"
        );
        decayed
    }

    /// Phase 2-pre (ADR-063): attempt deterministic dedup of one cluster.
    ///
    /// Hydrates the cluster's members from the in-scope per-boundary
    /// enumeration and re-embeds each (embeds are deterministic — the vectors
    /// match the ones clustering computed), then applies the calibrated
    /// two-axis near-identical gate ([`dedup::plan_dedup`]). On eligibility it
    /// supersedes the losers into the canonical survivor and rolls the
    /// aggregates atomically via [`StorageBackend::apply_dedup`], returning
    /// `Ok(Some(n))` where `n` is the number of superseded members. `Ok(None)`
    /// means "not near-identical" — the caller falls through to the LLM merge.
    ///
    /// A hydration mismatch (a member absent from the enumeration — the
    /// SQLite/LanceDB divergence steady-state) returns `Ok(None)`, so the
    /// cluster falls through to the LLM path rather than dedup-ing on partial
    /// data.
    ///
    /// [`StorageBackend::apply_dedup`]: vault_storage::StorageBackend::apply_dedup
    async fn try_dedup_cluster(
        &self,
        cluster: &Cluster,
        memories: &[Memory],
    ) -> VaultResult<Option<usize>> {
        let member_set: HashSet<MemoryId> = cluster.member_row_ids.iter().copied().collect();
        let members: Vec<Memory> = memories
            .iter()
            .filter(|m| member_set.contains(&m.id))
            .cloned()
            .collect();
        if members.len() != cluster.size() {
            return Ok(None);
        }
        let mut embeddings = Vec::with_capacity(members.len());
        for m in &members {
            embeddings.push(self.embeddings.embed(&m.content).await?);
        }
        match dedup::plan_dedup(&members, &embeddings) {
            None => Ok(None),
            Some(plan) => {
                self.storage
                    .apply_dedup(
                        plan.survivor,
                        &plan.superseded,
                        plan.summed_access_count,
                        plan.max_confidence,
                    )
                    .await?;
                Ok(Some(plan.superseded.len()))
            }
        }
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

    /// Document-side alias enrichment (ADR-074, T0.3.x Thread-2 Gap-2 fix).
    ///
    /// For each active (non-superseded, non-invalidated) fact whose aliases are
    /// not already current, ask Phi-4 for 4–8 alternative search keywords, store
    /// them on `metadata.enrichment`, and re-embed `content + " Topics: " +
    /// aliases` into the vector store via
    /// [`StorageBackend::update_memory`] — an in-place, by-id metadata + vector
    /// update. The display `content` is never modified, so the alias line cannot
    /// leak into the read response. See [`crate::phases::enrich`] for the full
    /// rationale + the proven `probe_enrichment` evidence.
    ///
    /// **Idempotent.** A fact already enriched for its current content
    /// ([`enrich::is_enriched_for_current_content`]) is skipped, so the first
    /// run backfills the whole vault and steady-state runs only re-embed facts
    /// whose content was newly written or changed. This bounds the per-night
    /// re-embed cost to what actually changed.
    ///
    /// **Failure semantics (locked-next-arc Step 4).** A per-fact LLM or
    /// embedding failure, or a failed `update_memory`, is logged-and-counted
    /// (`facts_failed`) and the loop continues — one bad fact never aborts the
    /// run, and the fact is retried next cycle (no fingerprint was written).
    ///
    /// Intended to be called by the app-layer safety wrapper immediately AFTER
    /// [`Self::run_consolidation`] (so it enriches the post-merge / post-
    /// invalidate active set) and BEFORE [`Self::generate_reports`].
    ///
    /// # Errors
    ///
    /// [`vault_core::VaultError`] propagated only from the initial
    /// `list_memories` enumeration. Per-fact failures do NOT propagate — they
    /// are counted into [`EnrichmentReport::facts_failed`].
    ///
    /// [`enrich::is_enriched_for_current_content`]: crate::phases::enrich::is_enriched_for_current_content
    #[instrument(skip(self))]
    pub async fn enrich_facts(&self) -> VaultResult<EnrichmentReport> {
        // Active set only: default filter drops superseded rows; the
        // `valid_until` filter drops invalidated rows (retired facts are never
        // returned at read, so enriching them is wasted re-embed work). Mirrors
        // the Phase 2b active-set definition.
        let active: Vec<Memory> = self
            .storage
            .list_memories(MemoryFilter::default(), None)
            .await?
            .into_iter()
            .filter(|m| m.valid_until.is_none())
            .collect();

        let mut report = EnrichmentReport::default();
        for memory in &active {
            match enrich_one(memory, self.llm.as_ref(), self.embeddings.as_ref()).await {
                Ok(None) => report.facts_skipped += 1,
                Ok(Some(EnrichedFact {
                    memory: enriched,
                    embedding,
                    graph,
                })) => match self.storage.update_memory(&enriched, &embedding).await {
                    Ok(_) => {
                        report.facts_enriched += 1;
                        // Graph-fill AFTER the vector is persisted: the
                        // content_fp is now written, so a transient graph
                        // failure is never re-extracted into duplicate edges on
                        // the next run (see extract.rs idempotency note).
                        if !graph.is_empty() {
                            match extract::write_extracted_to_graph(
                                self.storage.graph_store().as_ref(),
                                &enriched.boundary,
                                &graph,
                                enriched.confidence,
                            )
                            .await
                            {
                                Ok(stats) => {
                                    report.entities_created += stats.entities_created;
                                    report.entities_reused += stats.entities_reused;
                                    report.relationships_created += stats.relationships_created;
                                    report.relationships_failed += stats.relationships_failed;
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        target: "vault_consolidator::enrich",
                                        memory_id = %enriched.id,
                                        error = %e,
                                        "graph extraction write failed; aliases still enriched"
                                    );
                                    report.graph_write_failures += 1;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "vault_consolidator::enrich",
                            memory_id = %enriched.id,
                            error = %e,
                            "enrichment update_memory failed; skipping (next cycle retries)"
                        );
                        report.facts_failed += 1;
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        target: "vault_consolidator::enrich",
                        memory_id = %memory.id,
                        error = %e,
                        "enrichment failed for fact; skipping (next cycle retries)"
                    );
                    report.facts_failed += 1;
                }
            }
        }
        tracing::info!(
            target: "vault_consolidator::enrich",
            active = active.len(),
            enriched = report.facts_enriched,
            skipped = report.facts_skipped,
            failed = report.facts_failed,
            "alias enrichment pass complete"
        );
        Ok(report)
    }

    /// Headless scheduling loop (T0.2.6) — sleep until the configured local
    /// `run_at`, run the consolidator pipeline, repeat forever.
    ///
    /// Each cycle runs [`Self::run_consolidation`] (merge / contradiction /
    /// decay) then [`Self::enrich_facts`] (ADR-074 vocabulary-gap enrichment,
    /// load-bearing for correct recall). A failing run is logged and the loop
    /// waits for the next `run_at` — one bad night never tears down the
    /// schedule (mirrors the retry worker's "log and keep going" lifecycle).
    ///
    /// **Production note.** `vault-app` does NOT call this method; it runs its
    /// own scheduler ([`Application::start_with_mcp`]) on the same
    /// [`scheduler::duration_until_next_run`] timer, because the full
    /// production cycle also needs the app-layer cross-process lockfile, the
    /// 30-minute hard timeout, and per-boundary REPORT persistence to disk —
    /// none of which the consolidator can own (it is filesystem-agnostic by
    /// architecture lock). This method is the library/headless equivalent for
    /// embedders that drive the consolidator without the application wrapper.
    /// It never returns under normal operation (the loop is infinite); the
    /// `VaultResult` return exists to match the BRD §5.6 line 953 signature.
    #[instrument(skip(self))]
    pub async fn schedule(&self) -> VaultResult<()> {
        loop {
            let wait =
                crate::scheduler::duration_until_next_run(chrono::Local::now(), self.config.run_at);
            tracing::info!(
                target: "vault_consolidator::scheduler",
                run_at = %self.config.run_at,
                wait_secs = wait.as_secs(),
                "next consolidation scheduled"
            );
            tokio::time::sleep(wait).await;

            match self.run_consolidation().await {
                Ok(report) => {
                    if let Err(e) = self.enrich_facts().await {
                        tracing::warn!(
                            target: "vault_consolidator::scheduler",
                            error = %e,
                            "post-run enrichment failed; next cycle retries"
                        );
                    }
                    tracing::info!(
                        target: "vault_consolidator::scheduler",
                        processed = report.memories_processed,
                        merged = report.memories_merged,
                        "scheduled consolidation cycle complete"
                    );
                }
                Err(e) => tracing::error!(
                    target: "vault_consolidator::scheduler",
                    error = %e,
                    "scheduled consolidation failed; retrying at next run_at"
                ),
            }
        }
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
    /// Phase 4 (T0.2.4): facts whose confidence was decayed this run. Additive
    /// to the BRD §5.6 baseline shape; `#[serde(default)]` keeps reports
    /// serialised before T0.2.4 loadable.
    #[serde(default)]
    pub memories_decayed: usize,
    /// ADR-063: clusters resolved by deterministic dedup (near-identical,
    /// no LLM). Distinct from `memories_merged` (LLM-driven merges).
    #[serde(default)]
    pub clusters_deduped: usize,
    /// ADR-063: memories superseded by deterministic dedup across the run.
    #[serde(default)]
    pub memories_deduped: usize,
    /// ADR-063: clusters skipped due to a per-cluster failure (dedup or LLM
    /// merge). Previously log-only; surfaced so persistent skips aren't
    /// silent (closes the "skips are invisible" gap).
    #[serde(default)]
    pub clusters_skipped: usize,
    pub duration: Duration,
    pub conflicts_for_user_review: Vec<ConflictReview>,
    /// T0.2.5 / A2: the checkpoint capturing everything this run changed, or
    /// `None` if the run changed nothing (or capture failed — see logs). Undo a
    /// run with `vault-cli consolidate rollback <id>`. `#[serde(default)]` keeps
    /// reports serialised before T0.2.5 loadable.
    #[serde(default)]
    pub checkpoint_id: Option<CheckpointId>,
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

/// Outcome counts of one [`Consolidator::enrich_facts`] pass (ADR-074).
///
/// `facts_enriched` + `facts_skipped` + `facts_failed` sum to the number of
/// active facts the pass examined. `facts_skipped` is the steady-state majority
/// (already enriched for current content); `facts_enriched` is the first-run
/// backfill + newly-written/changed facts; `facts_failed` is per-fact LLM /
/// embed / write failures that will be retried next cycle.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrichmentReport {
    /// Facts that were (re-)enriched and written back this pass.
    pub facts_enriched: usize,
    /// Facts already enriched for their current content (no work).
    pub facts_skipped: usize,
    /// Facts whose enrichment failed (LLM / embed / write) — retried next cycle.
    pub facts_failed: usize,
    /// Graph-fill (tech-debt #2): distinct entities created this pass.
    #[serde(default)]
    pub entities_created: usize,
    /// Graph-fill: entities already present, reused by id (idempotent).
    #[serde(default)]
    pub entities_reused: usize,
    /// Graph-fill: relationships written this pass.
    #[serde(default)]
    pub relationships_created: usize,
    /// Graph-fill: relationships dropped at write (validation / store error).
    #[serde(default)]
    pub relationships_failed: usize,
    /// Facts whose graph write failed wholesale (storage error) though their
    /// aliases were still enriched.
    #[serde(default)]
    pub graph_write_failures: usize,
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
    /// Phase 4 (T0.2.4): count of facts whose confidence was decayed this run.
    /// Aggregate per BRD §5.6 line 968 ("no per-memory detail"); drives the
    /// summary's Decay section.
    pub memories_decayed: usize,
    pub per_boundary: BTreeMap<Boundary, BoundarySummary>,
}

/// One boundary's contribution to the run: applied merges + contradictions
/// captured for that boundary. Decay (T0.2.4) will extend this struct with
/// per-boundary decay/archive counts.
#[derive(Debug, Default)]
pub(crate) struct BoundarySummary {
    pub applied_merges: Vec<AppliedMergeWithContext>,
    pub contradictions: Vec<ConflictReview>,
    /// ADR-063: count of clusters resolved by deterministic dedup (no LLM).
    pub deduped_clusters: usize,
    /// ADR-063: count of memories superseded by deterministic dedup (the
    /// non-survivor members rolled into a canonical survivor).
    pub deduped_memories: usize,
    /// ADR-063: count of clusters skipped due to a per-cluster failure
    /// (dedup error, or LLM merge decision / apply failure). Previously
    /// log-only — surfaced here so persistent skips are visible, not silent.
    pub skipped_clusters: usize,
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

    // ─── resolve_stale_ids — Phase 2b mass-invalidate safety net ─────────────

    fn id_set(ids: &[MemoryId]) -> HashSet<MemoryId> {
        ids.iter().copied().collect()
    }

    #[test]
    fn resolve_stale_invalidates_a_strict_subset() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let c = MemoryId::new();
        let group = id_set(&[a, b, c]);
        // One stale of three → invalidate exactly that one.
        assert_eq!(
            resolve_stale_ids(&[a], &group),
            StaleResolution::Invalidate(vec![a])
        );
    }

    #[test]
    fn resolve_stale_refuses_whole_group_sweep() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let group = id_set(&[a, b]);
        // Both members stale → refused (≥1 must remain current).
        assert_eq!(
            resolve_stale_ids(&[a, b], &group),
            StaleResolution::RefusedMassInvalidate {
                in_group: 2,
                group: 2
            }
        );
    }

    #[test]
    fn resolve_stale_refuses_whole_group_via_cycle_union() {
        // The pairwise aggregator can surface all members in a cycle; the
        // orchestrator must still refuse to wipe the topic.
        let a = MemoryId::new();
        let b = MemoryId::new();
        let c = MemoryId::new();
        let group = id_set(&[a, b, c]);
        assert!(matches!(
            resolve_stale_ids(&[a, b, c], &group),
            StaleResolution::RefusedMassInvalidate { .. }
        ));
    }

    #[test]
    fn resolve_stale_ignores_ids_outside_the_group() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let outsider = MemoryId::new();
        let group = id_set(&[a, b]);
        // The stray id is dropped; `a` remains a strict subset → invalidate a.
        assert_eq!(
            resolve_stale_ids(&[a, outsider], &group),
            StaleResolution::Invalidate(vec![a])
        );
    }

    #[test]
    fn resolve_stale_dedups_before_the_whole_group_check() {
        let a = MemoryId::new();
        let b = MemoryId::new();
        let group = id_set(&[a, b]);
        // Duplicated `a` must dedup to one — NOT be mistaken for a 2-of-2 sweep.
        assert_eq!(
            resolve_stale_ids(&[a, a], &group),
            StaleResolution::Invalidate(vec![a])
        );
    }

    #[test]
    fn resolve_stale_nothing_when_no_in_group_ids() {
        let a = MemoryId::new();
        let outsider = MemoryId::new();
        let group = id_set(&[a]);
        assert_eq!(
            resolve_stale_ids(&[outsider], &group),
            StaleResolution::Nothing
        );
        assert_eq!(resolve_stale_ids(&[], &group), StaleResolution::Nothing);
    }

    // ─── enrich_facts — end-to-end idempotency (ADR-074) ─────────────────────

    use vault_core::{MemoryType, NewMemory};
    use vault_embedding::EMBEDDING_DIM;
    use vault_llm::MockLlmProvider;
    use vault_storage::SqlCipherKey;

    const TEST_AT_REST_KEY: [u8; 32] = [0xcd; 32];

    /// Stub embedder returning a fixed unit-norm vector (enrich_facts only
    /// needs a valid embedding; rank quality is validated by the live
    /// `probe_enrichment` harness, not here).
    struct FixedEmbedder;

    #[async_trait::async_trait]
    impl EmbeddingProvider for FixedEmbedder {
        async fn embed(&self, _text: &str) -> VaultResult<Vec<f32>> {
            Ok(vec![1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM])
        }
    }

    async fn open_test_storage() -> (Arc<StorageBackend>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = SqlCipherKey::new("enrich-facts-test");
        let storage = StorageBackend::open_with_at_rest_key(
            &dir.path().join("metadata.db"),
            &dir.path().join("vectors"),
            &dir.path().join("graph.duckdb"),
            key,
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .expect("open StorageBackend");
        (Arc::new(storage), dir)
    }

    fn consolidator_with(storage: Arc<StorageBackend>, llm_response: &str) -> Consolidator {
        Consolidator::new(
            storage,
            Arc::new(MockLlmProvider::new("mock", llm_response)),
            Arc::new(FixedEmbedder),
            ConsolidatorConfig::default(),
        )
    }

    async fn write_fact(storage: &StorageBackend, content: &str) -> MemoryId {
        let m = Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("personal").unwrap(),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory");
        let embedding = vec![1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
        storage
            .write_memory(&m, &embedding)
            .await
            .expect("write_memory");
        m.id
    }

    #[tokio::test]
    async fn enrich_facts_backfills_then_is_idempotent_on_second_run() {
        let (storage, _dir) = open_test_storage().await;
        write_fact(&storage, "The user settled in Porto after years of moving.").await;
        write_fact(&storage, "The user is raising twins in primary school.").await;

        let consolidator =
            consolidator_with(storage.clone(), r#"{"aliases":["home","lives","kids"]}"#);

        // First run: both facts are fresh → both enriched.
        let r1 = consolidator.enrich_facts().await.expect("enrich pass 1");
        assert_eq!(r1.facts_enriched, 2, "first run backfills every fact");
        assert_eq!(r1.facts_skipped, 0);
        assert_eq!(r1.facts_failed, 0);

        // Metadata persisted: each fact carries an enrichment object.
        let after = storage
            .list_memories(MemoryFilter::default(), None)
            .await
            .expect("list");
        assert_eq!(after.len(), 2);
        for m in &after {
            let enr = m
                .metadata
                .get(crate::phases::enrich::ENRICHMENT_METADATA_KEY)
                .unwrap_or_else(|| panic!("fact {} missing enrichment metadata", m.id));
            assert_eq!(
                enr.get("aliases").and_then(|v| v.as_str()),
                Some("home, lives, kids")
            );
            assert!(
                crate::phases::enrich::is_enriched_for_current_content(m),
                "fact {} should be recognised as enriched for its content",
                m.id
            );
        }

        // Second run over unchanged content: every fact skipped (no re-embed).
        let r2 = consolidator.enrich_facts().await.expect("enrich pass 2");
        assert_eq!(r2.facts_enriched, 0, "no fact should be re-enriched");
        assert_eq!(r2.facts_skipped, 2, "all facts already current → skipped");
        assert_eq!(r2.facts_failed, 0);
    }

    #[tokio::test]
    async fn enrich_facts_counts_llm_failure_without_aborting_run() {
        let (storage, _dir) = open_test_storage().await;
        write_fact(&storage, "The user settled in Porto.").await;
        write_fact(&storage, "The user is raising twins.").await;

        // Mock returns non-JSON → every fact's alias generation fails.
        let consolidator = consolidator_with(storage.clone(), "not json at all");

        let r = consolidator
            .enrich_facts()
            .await
            .expect("pass must not abort");
        assert_eq!(r.facts_failed, 2, "both facts fail but the run completes");
        assert_eq!(r.facts_enriched, 0);
        assert_eq!(r.facts_skipped, 0);

        // No enrichment metadata was written → both retry next cycle.
        let after = storage
            .list_memories(MemoryFilter::default(), None)
            .await
            .expect("list");
        for m in &after {
            assert!(
                m.metadata
                    .get(crate::phases::enrich::ENRICHMENT_METADATA_KEY)
                    .is_none(),
                "failed enrichment must not write a fingerprint (so it retries)"
            );
        }
    }

    #[tokio::test]
    async fn enrich_facts_fills_graph_with_entities_and_relationships() {
        use vault_core::EntityType;
        use vault_storage::TraversalOptions;

        let (storage, _dir) = open_test_storage().await;
        write_fact(&storage, "The user works at Acme Corp.").await;

        // The combined enrichment response shape: aliases + entities +
        // relationships (the same JSON the production Phi-4 call emits).
        let combined = r#"{"aliases":["work","career","job"],"entities":[{"name":"the user","type":"person"},{"name":"Acme Corp","type":"organization"}],"relationships":[{"from":"the user","relation":"works_at","to":"Acme Corp"}]}"#;
        let consolidator = consolidator_with(storage.clone(), combined);

        // First run: fact enriched AND graph filled in the same pass.
        let r = consolidator.enrich_facts().await.expect("enrich pass");
        assert_eq!(r.facts_enriched, 1);
        assert_eq!(r.entities_created, 2, "the user + Acme Corp");
        assert_eq!(r.relationships_created, 1);
        assert_eq!(r.relationships_failed, 0);

        // The entities landed under the fact's own boundary...
        let graph = storage.graph_store();
        let personal = Boundary::new("personal").unwrap();
        let user = graph
            .get_entity("the user", &EntityType::Person, &personal)
            .await
            .unwrap()
            .expect("user entity present");
        let acme = graph
            .get_entity("Acme Corp", &EntityType::Organization, &personal)
            .await
            .unwrap()
            .expect("Acme entity present");

        // ...and are linked (user --works_at--> Acme), reachable in one hop.
        let reached = graph
            .traverse(
                &user.id,
                std::slice::from_ref(&personal),
                TraversalOptions {
                    max_hops: 1,
                    relation_filter: None,
                    follow_aliases: false,
                },
            )
            .await
            .unwrap();
        assert!(
            reached.iter().any(|(e, _)| e.id == acme.id),
            "the user should be linked to Acme Corp via the extracted relationship"
        );

        // Idempotent: the second run skips the unchanged fact → NO duplicate
        // entities or relationships written.
        let r2 = consolidator.enrich_facts().await.expect("enrich pass 2");
        assert_eq!(r2.facts_skipped, 1);
        assert_eq!(r2.entities_created, 0, "no duplicate entities on re-run");
        assert_eq!(
            r2.relationships_created, 0,
            "no duplicate relationships on re-run"
        );
    }
}
