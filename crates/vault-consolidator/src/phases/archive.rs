//! Phase 4 (second half) — cold archive (BRD §5.6 lines 995-996; ADR-084).
//!
//! ## What this is
//!
//! The companion to confidence decay ([`crate::phases::decay`]). Decay quietly
//! *demotes* a cold fact's confidence; cold archive *removes* a fact that has
//! stayed cold even longer (`archive_after_days`, default 365 — see
//! [`crate::ConsolidatorConfig`]) from default retrieval entirely, by stamping
//! its `archived_at` marker. The fact is never deleted: it still lives in
//! `vault.db` and surfaces via an explicit "search archive" call
//! (`RetrievalOptions::include_archived`). Archive is the demote-not-delete
//! tool the "keep when unsure" posture leans on — over-retention is the
//! unrescuable sin, so bloat is handled by decay + cold archive + the
//! reranker, never by risky deletion (ADR-083 founder posture, ADR-084).
//!
//! ## Soft state, not a separate store (ADR-084)
//!
//! BRD §5.6 line 995 says "move to cold archive (encrypted blob, removed from
//! active stores)". We keep the fact in the already-SQLCipher-encrypted
//! `vault.db` and just flip a marker that drops it out of default retrieval —
//! same zero-knowledge guarantee, no new crypto path, and reversible (the
//! checkpoint captures the pre-archive state). The separate-blob store is a
//! large-scale index-shrink optimization deferred to V1.0+; the BRD's INTENT
//! ("out of default retrieval, searchable via a separate call") is met.
//!
//! ## Idempotency (the property test demands it)
//!
//! BRD §5.6 line 1022 requires consolidation to converge on a second run.
//! Archive is idempotent for free: a fact with `archived_at` already set is
//! skipped (it is no longer in the active set the consolidator enumerates,
//! AND [`plan_archive`] guards on `is_archived()` regardless). No metadata
//! marker is needed — the `archived_at` column IS the marker. BRD §5.6 line
//! 1023's "no memory ever lost" holds with *archived* as a first-class third
//! end-state alongside active and superseded.

use chrono::{DateTime, Duration, Utc};
use vault_core::{Memory, MemoryId};

/// One planned archive: a still-active fact that has stayed cold past
/// `archive_after_days` and will have its `archived_at` marker set this run.
/// `idle_since` (the fact's `last_accessed`) is carried for the audit trail
/// and the run summary's Archive line.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedArchive {
    pub id: MemoryId,
    pub idle_since: DateTime<Utc>,
}

/// Decide which of `memories` should be moved to cold archive this run.
///
/// A fact is archived when ALL hold:
/// - it is still active — not superseded, not already invalidated/expired
///   (`valid_until` in the past), and not already archived (the idempotency
///   guard — `archived_at` is the marker),
/// - it has been idle for at least `archive_after_days`
///   (`now - last_accessed`).
///
/// Pure: takes the active set + the threshold + a caller-supplied `now`, and
/// returns the plan. The caller ([`crate::Consolidator::archive_memories`])
/// applies it via the metadata-only [`StorageBackend::apply_archive`]
/// primitive.
///
/// [`StorageBackend::apply_archive`]: vault_storage::StorageBackend::apply_archive
pub fn plan_archive(
    memories: &[Memory],
    archive_after_days: u32,
    now: DateTime<Utc>,
) -> Vec<PlannedArchive> {
    let threshold = Duration::days(i64::from(archive_after_days));
    let mut plan = Vec::new();
    for m in memories {
        // Already archived — skip (idempotency; archived_at IS the marker).
        if m.is_archived() {
            continue;
        }
        // Already retired — skip (don't archive superseded merge-losers).
        if m.superseded_by.is_some() {
            continue;
        }
        // Already invalidated / expired — skip (already out of retrieval).
        if m.valid_until.is_some_and(|vu| vu <= now) {
            continue;
        }
        // Not idle long enough — skip.
        if now.signed_duration_since(m.last_accessed) < threshold {
            continue;
        }
        plan.push(PlannedArchive {
            id: m.id,
            idle_since: m.last_accessed,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vault_core::{Boundary, MemoryType, NewMemory};

    fn active_memory() -> Memory {
        Memory::try_new(NewMemory {
            content: "a fact left cold for a very long time".to_string(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("personal").unwrap(),
            source_agent: None,
            confidence: 0.5,
            valid_from: None,
            valid_until: None,
            metadata: json!({}),
        })
        .unwrap()
    }

    #[test]
    fn archives_fact_idle_past_threshold() {
        let now = Utc::now();
        let mut m = active_memory();
        m.last_accessed = now - Duration::days(400);

        let plan = plan_archive(std::slice::from_ref(&m), 365, now);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].id, m.id);
        assert_eq!(plan[0].idle_since, m.last_accessed);
    }

    #[test]
    fn archives_fact_exactly_at_threshold() {
        // Idle == threshold passes (>= boundary), mirroring decay.
        let now = Utc::now();
        let mut m = active_memory();
        m.last_accessed = now - Duration::days(365);

        let plan = plan_archive(std::slice::from_ref(&m), 365, now);

        assert_eq!(plan.len(), 1, "exactly-at-threshold idle must archive");
    }

    #[test]
    fn does_not_archive_recently_accessed() {
        let now = Utc::now();
        let mut m = active_memory();
        m.last_accessed = now - Duration::days(30);

        assert!(plan_archive(std::slice::from_ref(&m), 365, now).is_empty());
    }

    #[test]
    fn does_not_re_archive_already_archived_fact() {
        // Idempotency: a fact with archived_at set is skipped even if cold.
        let now = Utc::now();
        let mut m = active_memory();
        m.last_accessed = now - Duration::days(800);
        m.archived_at = Some(now - Duration::days(100));

        assert!(
            plan_archive(std::slice::from_ref(&m), 365, now).is_empty(),
            "an already-archived fact must not be re-archived (idempotency)"
        );
    }

    #[test]
    fn does_not_archive_superseded_fact() {
        let now = Utc::now();
        let mut m = active_memory();
        m.last_accessed = now - Duration::days(400);
        m.superseded_by = Some(MemoryId::new());

        assert!(plan_archive(std::slice::from_ref(&m), 365, now).is_empty());
    }

    #[test]
    fn does_not_archive_invalidated_fact() {
        let now = Utc::now();
        let mut m = active_memory();
        m.last_accessed = now - Duration::days(400);
        m.valid_until = Some(now - Duration::days(1));

        assert!(plan_archive(std::slice::from_ref(&m), 365, now).is_empty());
    }
}
