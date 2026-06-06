//! [`SearchHint`] — an additive, recall-safe relevance signal for `memory_search`
//! (ADR-071 Option B, 2026-06-06).
//!
//! ## Why this exists
//!
//! ADR-071 made `memory_search` **reorder-only**: it never drops candidates, so
//! it never manufactures a false-empty (the failure mode that made weak agents
//! abandon the vault or spin re-querying). The cost of that safety is that on a
//! true no-signal query the tool hands back a ranked list of *irrelevant* facts
//! with no explicit "none of these match" signal — the calling agent has to
//! judge. Weak agents are bad at that.
//!
//! This module restores the judgment aid **without** re-introducing a drop: a
//! small advisory hint (`top_relevance` + `weak_match`) computed from the
//! already-scored results. The results array is returned in full regardless of
//! the hint — the contract "never empty when candidates exist" is preserved.
//!
//! ## The load-bearing insight: separation, not magnitude
//!
//! A naive `weak_match = top_score < floor` repeats the exact mistake ADR-071
//! removed. The cross-encoder reranker is **brittle on terse keyword queries**:
//! it scores even a *correct* answer low. Live dogfood (Opus 4.6, 2026-06-06,
//! `testeval` vault):
//!
//! - "instrument" → cello (the right answer) scored only **0.0469**, runner-up
//!   0.0027 → the top is ~**17×** the pool.
//! - "blood type" (genuinely absent) → top 0.00020, runner-up 0.00016 → ~**1.25×**;
//!   the scores are *flat*.
//!
//! An absolute floor of e.g. 0.1 would wrongly flag the correct cello as weak.
//! But the *separation* of the top from the pool cleanly distinguishes the two:
//! a real answer stands out even when its absolute score is low; a no-signal
//! query is flat. So `weak_match` keys on separation, surviving the reranker's
//! absolute-scale brittleness.

use crate::retriever::RetrievedMemory;

/// The reranker's own relevance decision boundary on the `[0, 1]` sigmoid scale.
/// `sigmoid(0) = 0.5` corresponds to a cross-encoder logit of `0` ("neutral").
/// A top result at or above this is one the reranker itself judged relevant, so
/// it is never weak regardless of separation — this also guards the
/// two-genuinely-strong-matches case (both high, low ratio) from a false
/// `weak_match`.
pub const STRONG_RELEVANCE: f32 = 0.5;

/// How many times the runner-up the top score must clear to count as a
/// "separated" (stand-out) match when it is *below* [`STRONG_RELEVANCE`].
/// Derived from the 2026-06-06 live dogfood: a real match separates sharply
/// (cello `0.0469` vs `0.0027` ≈ 17×) while a true no-signal query is flat
/// (blood-type `0.00020` vs `0.00016` ≈ 1.25×). `3×` sits with wide margin on
/// both sides and does not depend on the reranker's unreliable absolute scale.
pub const SEPARATION_RATIO: f32 = 3.0;

/// An additive relevance hint attached to a `memory_search` response. It NEVER
/// drops or filters results — it only annotates them so a weak agent can tell
/// "the vault has a strong match" from "nothing here really matched", without
/// the false-empty failure mode an absolute floor caused (ADR-071).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchHint {
    /// The top (rank-1) result's `[0, 1]` relevance score, or `0.0` when the
    /// result set is empty.
    pub top_relevance: f32,
    /// `true` when no result meaningfully stands out — the vault most likely
    /// holds nothing matching this query. Advisory only; the results are still
    /// returned in full.
    pub weak_match: bool,
}

/// Compute the [`SearchHint`] for an already relevance-sorted (DESC) result list
/// — the [`RerankedRetriever`](crate::RerankedRetriever) guarantees both the
/// ordering and the `[0, 1]` sigmoid score domain.
///
/// `weak_match` is `false` (a strong match) when the top result is EITHER
/// absolutely strong (≥ [`STRONG_RELEVANCE`] — the reranker said "yes") OR
/// clearly separated from the runner-up (≥ [`SEPARATION_RATIO`]× the #2 score).
/// Otherwise `weak_match` is `true`. Separation, not absolute magnitude, is the
/// load-bearing test (see the module docs).
///
/// Score-domain note: calibrated for the reranked `[0, 1]` path. On the
/// raw-hybrid fallback (no reranker model configured) the scores are cosine/RRF
/// rather than sigmoid, so the hint is best-effort there — acceptable, since the
/// reranked path is the production default.
pub fn search_hint(results: &[RetrievedMemory]) -> SearchHint {
    let Some(top) = results.first() else {
        return SearchHint {
            top_relevance: 0.0,
            weak_match: true,
        };
    };
    let top_relevance = top.score;

    // The reranker itself judged the top relevant → never weak (also covers two
    // genuinely-strong matches, whose ratio would otherwise read as "flat").
    if top_relevance >= STRONG_RELEVANCE {
        return SearchHint {
            top_relevance,
            weak_match: false,
        };
    }

    // Below the reranker's yes-boundary: decide by separation from the pool.
    // A lone candidate (second == 0.0) with any positive score counts as
    // separated — it is the only thing the reranker scored at all.
    let second = results.get(1).map(|r| r.score).unwrap_or(0.0);
    let separated = top_relevance > 0.0 && top_relevance >= SEPARATION_RATIO * second;
    SearchHint {
        top_relevance,
        weak_match: !separated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use vault_core::{Boundary, Memory, MemoryType, NewMemory};

    /// Build a `RetrievedMemory` carrying just the score under test (the hint
    /// never inspects content/id — only relevance scores and their order).
    fn scored(score: f32) -> RetrievedMemory {
        let memory = Memory::try_new(NewMemory {
            content: "fact".to_string(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("personal").expect("valid test boundary"),
            source_agent: None,
            confidence: 0.9,
            valid_from: Some(
                DateTime::parse_from_rfc3339("2026-06-06T12:00:00Z")
                    .expect("static rfc3339")
                    .with_timezone(&Utc),
            ),
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("static-valid test memory");
        RetrievedMemory {
            memory,
            score,
            explanation: String::new(),
        }
    }

    /// Empty pool → weak, with a zero top score (no candidates at all).
    #[test]
    fn empty_results_are_weak_with_zero_top() {
        let hint = search_hint(&[]);
        assert!(hint.weak_match, "an empty result set must read as weak");
        assert_eq!(hint.top_relevance, 0.0);
    }

    /// The live "instrument" case: the correct answer scores LOW in absolute
    /// terms (reranker brittleness) but stands far out from the pool → NOT weak.
    /// This is the case an absolute floor would have wrongly flagged.
    #[test]
    fn low_but_separated_top_is_a_strong_match() {
        let hint = search_hint(&[scored(0.0469), scored(0.0027), scored(0.0001)]);
        assert!(
            !hint.weak_match,
            "a low-but-separated top (cello 0.0469 vs 0.0027) must NOT be weak"
        );
        assert_eq!(hint.top_relevance, 0.0469);
    }

    /// The live "blood type" case (genuinely absent): scores are flat → weak.
    #[test]
    fn flat_low_scores_are_weak() {
        let hint = search_hint(&[scored(0.00020), scored(0.00016), scored(0.00014)]);
        assert!(
            hint.weak_match,
            "a flat pool (0.00020 vs 0.00016) must read as weak/no-signal"
        );
    }

    /// Two genuinely-strong matches have a low ratio, but the absolute
    /// [`STRONG_RELEVANCE`] gate keeps them from being mislabelled weak.
    #[test]
    fn two_strong_matches_are_not_weak_despite_low_ratio() {
        let hint = search_hint(&[scored(0.62), scored(0.55)]);
        assert!(
            !hint.weak_match,
            "a top ≥ STRONG_RELEVANCE is never weak (guards the two-strong case)"
        );
    }

    /// A single candidate is the only thing scored → treated as separated (the
    /// score is still surfaced via `top_relevance` for the agent to judge).
    #[test]
    fn single_candidate_is_not_weak() {
        let hint = search_hint(&[scored(0.3)]);
        assert!(!hint.weak_match, "a lone candidate counts as separated");
        assert_eq!(hint.top_relevance, 0.3);
    }

    /// Exactly at the separation ratio boundary counts as separated (≥, not >).
    #[test]
    fn exactly_at_separation_ratio_is_not_weak() {
        let hint = search_hint(&[scored(0.03), scored(0.01)]); // 3.0× exactly
        assert!(!hint.weak_match, "top == RATIO × second is separated (≥)");
    }
}
