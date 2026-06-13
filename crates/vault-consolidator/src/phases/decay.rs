//! Phase 4 — confidence decay (BRD §5.6 line 994; T0.2.4).
//!
//! ## What this is
//!
//! The sleep cycle's final pass. For a fact the user (and every agent) has left
//! untouched for `decay_after_days` (default 180 — see
//! [`crate::ConsolidatorConfig`]), the run multiplies its `confidence` by
//! [`DECAY_FACTOR`] (0.9, BRD §5.6 line 994 verbatim). A fact that stays cold
//! keeps fading a little each decay period; a fact that gets accessed again
//! resets the clock (its `last_accessed` moves forward, so it drops out of the
//! plan). Retrieval already weights by confidence, so decay quietly demotes
//! stale knowledge without ever deleting it.
//!
//! **Cold archive** (BRD §5.6 lines 995-996 — move facts untouched for
//! `archive_after_days` to an out-of-default-retrieval store) is the *other*
//! half of Phase 4 and lands in a follow-up batch: it is a first-class
//! `Memory` state change (schema + retrieval-filter reach) far larger than
//! decay, so it is scoped separately to keep each batch debuggable.
//!
//! ## Decay is metadata-only (never re-embeds)
//!
//! Decay changes `confidence` and nothing else. It deliberately does **not**
//! go through the re-embedding update path: a fact's stored vector may be the
//! *enriched* one (ADR-074 alias boost), and re-embedding from raw `content`
//! would silently clobber that enrichment. The apply step
//! ([`crate::Consolidator::decay_memories`]) therefore uses a metadata-only
//! storage primitive; this module only decides *which* facts decay and to
//! *what* new confidence.
//!
//! ## Idempotency (the property test demands it)
//!
//! BRD §5.6 line 1022 requires consolidation to be idempotent — running it
//! twice on the same data converges. Without a guard, a second back-to-back run
//! would decay an already-decayed fact again (×0.9 ×0.9). So each decayed fact
//! records a `last_decay_at` marker in [`Memory::metadata`] (mirroring how
//! [`crate::phases::enrich`] records `content_fp`). A fact is re-decayed only
//! once its marker is itself older than `decay_after_days` — i.e., once a full
//! decay period has elapsed. A second immediate run finds a fresh marker and
//! skips, so the run converges.

use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use vault_core::{Memory, MemoryId};

/// Metadata key under which the decay idempotency marker is stored on
/// [`Memory::metadata`]. The marker shape is `{"last_decay_at": "<rfc3339>"}`.
pub(crate) const DECAY_METADATA_KEY: &str = "decay";

/// Spec-mandated decay multiplier — BRD §5.6 line 994: "multiply confidence by
/// 0.9". A fact untouched across N decay periods reaches `confidence × 0.9ⁿ`.
pub const DECAY_FACTOR: f32 = 0.9;

/// One planned decay: a still-active fact whose confidence this run will
/// multiply by [`DECAY_FACTOR`]. `old_confidence` is carried for the audit
/// trail and the run summary's Decay section.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedDecay {
    pub id: MemoryId,
    pub old_confidence: f32,
    pub new_confidence: f32,
}

/// Decide which of `memories` should have their confidence decayed this run.
///
/// A fact is decayed when ALL hold:
/// - it is still active — not superseded, and not already invalidated/expired
///   (`valid_until` in the past); decaying a retired fact is pointless,
/// - it has been idle for at least `decay_after_days` (`now - last_accessed`),
/// - it has not already been decayed within the current decay period (the
///   `last_decay_at` marker is absent or itself older than `decay_after_days`)
///   — this is the idempotency guard,
/// - decaying would actually change the confidence (a fact already at 0.0 is
///   skipped — `0.0 × 0.9` is a no-op write).
///
/// Pure: takes the active set + the threshold + a caller-supplied `now`, and
/// returns the plan. The caller ([`crate::Consolidator::decay_memories`])
/// applies it via the metadata-only storage primitive.
pub fn plan_decay(
    memories: &[Memory],
    decay_after_days: u32,
    now: DateTime<Utc>,
) -> Vec<PlannedDecay> {
    let threshold = Duration::days(i64::from(decay_after_days));
    let mut plan = Vec::new();
    for m in memories {
        // Already retired — skip.
        if m.superseded_by.is_some() {
            continue;
        }
        // Already invalidated / expired — skip (don't decay dead facts).
        if m.valid_until.is_some_and(|vu| vu <= now) {
            continue;
        }
        // Not idle long enough — skip.
        if now.signed_duration_since(m.last_accessed) < threshold {
            continue;
        }
        // Already decayed within this decay period — skip (idempotency).
        if let Some(last) = last_decay_at(m) {
            if now.signed_duration_since(last) < threshold {
                continue;
            }
        }
        let new_confidence = m.confidence * DECAY_FACTOR;
        // No-op (confidence already 0.0, or float underflow) — skip the write.
        if (m.confidence - new_confidence).abs() < f32::EPSILON {
            continue;
        }
        plan.push(PlannedDecay {
            id: m.id,
            old_confidence: m.confidence,
            new_confidence,
        });
    }
    plan
}

/// Read the decay idempotency marker (`metadata.decay.last_decay_at`) from a
/// memory, if present and well-formed. A missing/malformed marker reads as
/// `None` (the fact is treated as never-decayed) — fail-open toward decaying,
/// never toward a panic.
pub(crate) fn last_decay_at(m: &Memory) -> Option<DateTime<Utc>> {
    m.metadata
        .get(DECAY_METADATA_KEY)?
        .get("last_decay_at")?
        .as_str()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// Stamp the decay idempotency marker onto a memory's metadata, preserving any
/// other keys (e.g. the ADR-074 `enrichment` object). Used by the apply step
/// when it writes the decayed confidence back. If `metadata` is not an object
/// (it should always be), it is replaced with a fresh object so the marker can
/// be set rather than silently dropped.
pub(crate) fn set_decay_marker(metadata: &mut Value, decayed_at: DateTime<Utc>) {
    if !metadata.is_object() {
        *metadata = json!({});
    }
    metadata[DECAY_METADATA_KEY] = json!({ "last_decay_at": decayed_at.to_rfc3339() });
}

#[cfg(test)]
mod tests {
    use super::*;
    use vault_core::{Boundary, MemoryType, NewMemory};

    fn active_memory(confidence: f32) -> Memory {
        Memory::try_new(NewMemory {
            content: "a fact the user once cared about".to_string(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("personal").unwrap(),
            source_agent: None,
            confidence,
            valid_from: None,
            valid_until: None,
            metadata: json!({}),
        })
        .unwrap()
    }

    fn days_ago(days: i64) -> DateTime<Utc> {
        Utc::now() - Duration::days(days)
    }

    #[test]
    fn decays_fact_idle_past_threshold() {
        let now = Utc::now();
        let mut m = active_memory(0.8);
        m.last_accessed = now - Duration::days(200);

        let plan = plan_decay(std::slice::from_ref(&m), 180, now);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].id, m.id);
        assert_eq!(plan[0].old_confidence, 0.8);
        assert!((plan[0].new_confidence - 0.8 * 0.9).abs() < 1e-6);
    }

    #[test]
    fn decays_fact_exactly_at_threshold() {
        // Idle == threshold passes (>= boundary).
        let now = Utc::now();
        let mut m = active_memory(0.5);
        m.last_accessed = now - Duration::days(180);

        let plan = plan_decay(std::slice::from_ref(&m), 180, now);

        assert_eq!(plan.len(), 1, "exactly-at-threshold idle must decay");
    }

    #[test]
    fn does_not_decay_recently_accessed() {
        let now = Utc::now();
        let mut m = active_memory(0.8);
        m.last_accessed = now - Duration::days(10);

        assert!(plan_decay(std::slice::from_ref(&m), 180, now).is_empty());
    }

    #[test]
    fn does_not_decay_superseded_fact() {
        let now = Utc::now();
        let mut m = active_memory(0.8);
        m.last_accessed = now - Duration::days(200);
        m.superseded_by = Some(MemoryId::new());

        assert!(plan_decay(std::slice::from_ref(&m), 180, now).is_empty());
    }

    #[test]
    fn does_not_decay_invalidated_fact() {
        let now = Utc::now();
        let mut m = active_memory(0.8);
        m.last_accessed = now - Duration::days(200);
        m.valid_until = Some(now - Duration::days(1));

        assert!(plan_decay(std::slice::from_ref(&m), 180, now).is_empty());
    }

    #[test]
    fn does_not_decay_already_decayed_this_period() {
        // Idle long enough, but decayed only 10 days ago → within the period → skip.
        let now = Utc::now();
        let mut m = active_memory(0.72);
        m.last_accessed = now - Duration::days(200);
        set_decay_marker(&mut m.metadata, now - Duration::days(10));

        assert!(
            plan_decay(std::slice::from_ref(&m), 180, now).is_empty(),
            "a fact decayed within the current period must not re-decay (idempotency)"
        );
    }

    #[test]
    fn re_decays_after_a_full_period_elapsed() {
        // Marker is itself older than a full decay period → eligible again.
        let now = Utc::now();
        let mut m = active_memory(0.72);
        m.last_accessed = now - Duration::days(400);
        set_decay_marker(&mut m.metadata, now - Duration::days(200));

        let plan = plan_decay(std::slice::from_ref(&m), 180, now);
        assert_eq!(plan.len(), 1, "after a full period the fact decays again");
    }

    #[test]
    fn skips_zero_confidence_noop() {
        let now = Utc::now();
        let mut m = active_memory(0.0);
        m.last_accessed = now - Duration::days(200);

        assert!(
            plan_decay(std::slice::from_ref(&m), 180, now).is_empty(),
            "0.0 × 0.9 is a no-op — skip the pointless write"
        );
    }

    #[test]
    fn marker_round_trips_and_preserves_other_metadata() {
        let stamp = days_ago(5);
        let mut metadata = json!({ "enrichment": { "content_fp": "abc123" } });
        set_decay_marker(&mut metadata, stamp);

        // The enrichment object is preserved.
        assert_eq!(metadata["enrichment"]["content_fp"], "abc123");

        // And the marker reads back to (approximately) the same instant.
        let mut m = active_memory(0.5);
        m.metadata = metadata;
        let read = last_decay_at(&m).expect("marker should read back");
        assert!((read - stamp).num_seconds().abs() <= 1);
    }

    #[test]
    fn malformed_marker_reads_as_none() {
        let mut m = active_memory(0.5);
        m.metadata = json!({ "decay": { "last_decay_at": "not-a-date" } });
        assert!(last_decay_at(&m).is_none());

        m.metadata = json!({ "decay": "wrong-shape" });
        assert!(last_decay_at(&m).is_none());
    }
}
