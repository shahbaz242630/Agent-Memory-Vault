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

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveTime;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vault_core::{Boundary, MemoryId, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_llm::LlmProvider;
use vault_storage::StorageBackend;

/// Sleep-cycle orchestrator per BRD §5.6 lines 895-913.
///
/// Cheap to clone — all four dependencies are `Arc`-shared. Construct once
/// at application startup; reuse across nightly runs.
#[derive(Clone)]
pub struct Consolidator {
    #[allow(dead_code)] // wired at commit 2 in run_consolidation
    storage: Arc<StorageBackend>,
    #[allow(dead_code)] // wired at commit 2 in run_consolidation
    llm: Arc<dyn LlmProvider>,
    #[allow(dead_code)] // wired at commit 2 in run_consolidation
    embeddings: Arc<dyn EmbeddingProvider>,
    #[allow(dead_code)] // wired at commit 2 in run_consolidation
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
    /// **T0.2.3 commit 1 status: NOT IMPLEMENTED.** Body lands at commit 2
    /// (Phase 3 `apply_merge` primitive + orchestrator boundary-iteration
    /// loop + Phase 1→2→3 pipeline). The struct + this signature ship at
    /// commit 1 so the public surface matches BRD §5.6 line 911 verbatim
    /// from day one.
    #[allow(clippy::todo)]
    pub async fn run_consolidation(&self) -> VaultResult<ConsolidationReport> {
        todo!("T0.2.3 commit 2 — Phase 3 apply_merge + orchestrator loop")
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
