//! Phase 2-pre (ADR-063) — deterministic dedup for near-identical clusters.
//!
//! The small model never *writes* merged prose. When a cluster's members are
//! near-identical — the structural-overflow case that made the LLM merge
//! response truncate and the cluster skip forever — we collapse them with
//! plain code: keep the best existing copy, mark the rest superseded. No LLM
//! call, so nothing can overflow.
//!
//! This module holds the **pure decision logic** (no storage I/O): the
//! near-identical gate and the golden-record survivor pick. The orchestrator
//! (`crate::consolidator`) applies the resulting plan via the storage
//! supersede + aggregate-bump primitives.
//!
//! ## Thresholds — calibrated, not guessed
//!
//! Our BGE-small cosine is measurably unreliable on relevance
//! ([[bge-small-cannot-separate-relevant]]), so the gate is **two-axis** and
//! both cutoffs are measured. From `tests/dedup_threshold_calibration.rs`
//! (run 2026-05-31, real bge-small on hand-labeled dogfood-shaped pairs):
//!
//! | class           | cosine (min–max) | containment (min–max) |
//! |-----------------|------------------|-----------------------|
//! | near-identical  | 0.962 – 1.000    | 0.889 – 1.000         |
//! | contradictory   | 0.785 – 0.883    | 0.600 – 1.000         |
//! | complementary   | 0.643 – 0.820    | 0.286 – 0.556         |
//! | unrelated       | 0.499 – 0.623    | 0.333 – 0.600         |
//!
//! Near-identical sits cleanly above every other class on cosine (floor 0.962
//! vs the next class's ceiling 0.883), so `NEAR_IDENTICAL_COS = 0.93` has a
//! wide margin. Containment separates near-identical (floor 0.889) from
//! complementary (ceiling 0.556), so `NEAR_IDENTICAL_LEX = 0.80` sits in the
//! gap. Contradictory pairs can share high containment but score < 0.92
//! cosine, so they never cluster at the 0.92 gate AND are caught separately by
//! the topic-level A5 pass — the cosine axis keeps them out of dedup.

use std::collections::HashSet;

use vault_core::{Memory, MemoryId};

/// Cosine-similarity floor for the near-identical gate. Calibrated 2026-05-31
/// (`tests/dedup_threshold_calibration.rs`): near-identical floors at 0.962,
/// the next class (contradictory) ceils at 0.883 — 0.93 sits in the gap.
pub(crate) const NEAR_IDENTICAL_COS: f32 = 0.93;

/// Lexical-containment floor for the near-identical gate. Calibrated
/// 2026-05-31: near-identical floors at 0.889, complementary ceils at 0.556 —
/// 0.80 sits in the gap with margin.
pub(crate) const NEAR_IDENTICAL_LEX: f32 = 0.80;

/// A planned deterministic dedup of one cluster: which member survives and
/// which collapse into it, plus the aggregates to roll onto the survivor.
/// Produced by [`plan_dedup`]; applied by the orchestrator (mark each loser
/// superseded → survivor, bump the survivor's aggregates). No new row, no
/// re-embed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DedupPlan {
    /// The surviving (canonical) member — an existing memory id.
    pub survivor: MemoryId,
    /// Members to mark superseded → `survivor`. Always ≥ 1.
    pub superseded: Vec<MemoryId>,
    /// `Σ(member.access_count)` across ALL members (survivor + superseded),
    /// rolled onto the survivor (BRD §5.6 line 988).
    pub summed_access_count: u32,
    /// `max(member.confidence)` across ALL members, rolled onto the survivor
    /// (BRD §5.6 line 988).
    pub max_confidence: f32,
}

/// Lowercased alphanumeric word tokens of `s`. Splits on any non-alphanumeric
/// char (Unicode-aware via [`char::is_alphanumeric`]); empty tokens dropped.
fn tokens(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Lexical containment = `|A ∩ B| / min(|A|, |B|)` over word-token sets.
///
/// Range `[0.0, 1.0]`. `1.0` means the smaller memory's tokens are entirely
/// contained in the larger — the right signal for "one memory is a
/// near-duplicate / length-variant of the other", robust to length differences
/// (unlike Jaccard). Empty input on either side → `0.0` (cannot be a duplicate).
pub(crate) fn token_containment(a: &str, b: &str) -> f32 {
    let (ta, tb) = (tokens(a), tokens(b));
    let min_len = ta.len().min(tb.len());
    if min_len == 0 {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count();
    inter as f32 / min_len as f32
}

/// Cosine similarity of two embeddings. BGE-small outputs are L2-normalised
/// (pinned by `vault-embedding`'s `test_2_embed_output_is_l2_normalized`), so
/// the dot product IS the cosine similarity. Defensive against length mismatch
/// (returns 0.0 — treated as "not near-identical") rather than panicking.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Plan a deterministic dedup for a cluster, or `None` if it is not eligible.
///
/// Eligible iff **every pair** of members is near-identical on BOTH axes
/// (`cosine ≥ NEAR_IDENTICAL_COS` AND `containment ≥ NEAR_IDENTICAL_LEX`). The
/// all-pairs requirement is the **over-merge guard**: a cluster member that is
/// only transitively connected (close to a middle member but not to the rest)
/// fails a pair and blocks the whole-cluster dedup, so the cluster falls
/// through to the LLM merge path instead of being wrongly collapsed.
///
/// `members[i]` corresponds to `embeddings[i]` (same order). Both must be the
/// same length `n ≥ 2`; otherwise returns `None` (caller falls through).
///
/// On eligibility, the survivor is chosen by the golden-record rule
/// ([`pick_survivor`]) and the aggregates summed across all members.
pub(crate) fn plan_dedup(members: &[Memory], embeddings: &[Vec<f32>]) -> Option<DedupPlan> {
    if members.len() < 2 || members.len() != embeddings.len() {
        return None;
    }

    // All-pairs near-identical check (the over-merge guard).
    for i in 0..members.len() {
        for j in (i + 1)..members.len() {
            let cos = cosine_similarity(&embeddings[i], &embeddings[j]);
            if cos < NEAR_IDENTICAL_COS {
                return None;
            }
            let lex = token_containment(&members[i].content, &members[j].content);
            if lex < NEAR_IDENTICAL_LEX {
                return None;
            }
        }
    }

    let survivor_idx = pick_survivor(members);
    let survivor = members[survivor_idx].id;
    let superseded: Vec<MemoryId> = members
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != survivor_idx)
        .map(|(_, m)| m.id)
        .collect();
    let summed_access_count: u32 = members.iter().map(|m| m.access_count).sum();
    let max_confidence: f32 = members.iter().map(|m| m.confidence).fold(0.0_f32, f32::max);

    Some(DedupPlan {
        survivor,
        superseded,
        summed_access_count,
        max_confidence,
    })
}

/// Pick the canonical survivor index by the data-fusion "golden record" rule:
/// **newest `valid_from` → newest `created_at` → longest content → highest
/// confidence → most-accessed → lowest id**. Each tiebreak is total and the
/// final id tiebreak is deterministic, so the pick is stable across runs.
///
/// `members` must be non-empty.
fn pick_survivor(members: &[Memory]) -> usize {
    let mut best = 0;
    for i in 1..members.len() {
        if is_better_survivor(&members[i], &members[best]) {
            best = i;
        }
    }
    best
}

/// `true` if `cand` should win survivorship over `cur` per the golden-record
/// ordering (see [`pick_survivor`]).
fn is_better_survivor(cand: &Memory, cur: &Memory) -> bool {
    use std::cmp::Ordering::{Equal, Greater, Less};
    // Each level: Greater → cand wins, Less → cur wins, Equal → next level.
    macro_rules! decide {
        ($ord:expr) => {
            match $ord {
                Greater => return true,
                Less => return false,
                Equal => {}
            }
        };
    }
    decide!(cand.valid_from.cmp(&cur.valid_from));
    decide!(cand.created_at.cmp(&cur.created_at));
    decide!(cand.content.len().cmp(&cur.content.len()));
    decide!(cand
        .confidence
        .partial_cmp(&cur.confidence)
        .unwrap_or(Equal));
    decide!(cand.access_count.cmp(&cur.access_count));
    // Final deterministic tiebreak: lowest id wins → cand wins iff smaller.
    cand.id < cur.id
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use vault_core::{Boundary, MemoryType, NewMemory};

    fn mem(content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("work").expect("boundary"),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("memory")
    }

    // ── token_containment ──────────────────────────────────────────────
    #[test]
    fn containment_identical_is_one() {
        assert!(
            (token_containment("the user drives a Rivian", "the user drives a Rivian") - 1.0).abs()
                < 1e-6
        );
    }

    #[test]
    fn containment_subset_is_one() {
        // Every token of the shorter is present in the longer.
        let c = token_containment("dark mode", "the user prefers dark mode in editors");
        assert!(
            (c - 1.0).abs() < 1e-6,
            "subset containment must be 1.0, got {c}"
        );
    }

    #[test]
    fn containment_case_and_punctuation_insensitive() {
        let c = token_containment("Dark Mode.", "dark mode");
        assert!(
            (c - 1.0).abs() < 1e-6,
            "case/punct must not matter, got {c}"
        );
    }

    #[test]
    fn containment_disjoint_is_zero() {
        assert_eq!(token_containment("cello orchestra", "Rivian truck"), 0.0);
    }

    #[test]
    fn containment_empty_is_zero() {
        assert_eq!(token_containment("", "anything"), 0.0);
        assert_eq!(token_containment("!!!", "anything"), 0.0);
    }

    #[test]
    fn containment_partial_is_fractional() {
        // {a,b} vs {a,c}: intersection 1, min len 2 → 0.5.
        let c = token_containment("alpha beta", "alpha gamma");
        assert!((c - 0.5).abs() < 1e-6, "got {c}");
    }

    // ── cosine_similarity ──────────────────────────────────────────────
    #[test]
    fn cosine_identical_unit_vectors_is_one() {
        let v = vec![0.6, 0.8]; // already unit length
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    }

    #[test]
    fn cosine_length_mismatch_is_zero_not_panic() {
        assert_eq!(cosine_similarity(&[1.0, 0.0], &[1.0]), 0.0);
    }

    // ── pick_survivor (golden record) ──────────────────────────────────
    #[test]
    fn survivor_prefers_newest_valid_from() {
        let mut older = mem("the user drives a Rivian R1T");
        let mut newer = mem("the user drives a Rivian R1T");
        older.valid_from = Utc::now() - Duration::days(10);
        newer.valid_from = Utc::now();
        // newer is index 1.
        assert_eq!(pick_survivor(&[older, newer]), 1);
    }

    #[test]
    fn survivor_breaks_valid_from_tie_by_longer_content() {
        let t = Utc::now();
        let mut short = mem("dark mode");
        let mut long = mem("the user prefers dark mode in their code editors");
        short.valid_from = t;
        long.valid_from = t;
        short.created_at = t;
        long.created_at = t;
        // longer content (index 1) wins the tie.
        assert_eq!(pick_survivor(&[short, long]), 1);
    }

    #[test]
    fn survivor_final_tiebreak_is_lowest_id_deterministic() {
        let t = Utc::now();
        let mut a = mem("identical text here");
        let mut b = mem("identical text here");
        for m in [&mut a, &mut b] {
            m.valid_from = t;
            m.created_at = t;
            m.confidence = 0.9;
            m.access_count = 3;
        }
        let (lo, hi) = if a.id < b.id { (a, b) } else { (b, a) };
        // lowest id wins regardless of slice order.
        assert_eq!(pick_survivor(&[lo.clone(), hi.clone()]), 0);
        assert_eq!(pick_survivor(&[hi, lo]), 1);
    }

    // ── plan_dedup gate ────────────────────────────────────────────────
    fn unit(x: f32, y: f32) -> Vec<f32> {
        let n = (x * x + y * y).sqrt();
        vec![x / n, y / n]
    }

    #[test]
    fn plan_dedup_near_identical_cluster_is_eligible() {
        let m0 = mem("the user drives a Rivian R1T");
        let m1 = mem("the user drives a Rivian R1T truck");
        // cosine ~1.0 (same direction), containment high (subset).
        let e = vec![unit(1.0, 0.02), unit(1.0, 0.01)];
        let plan = plan_dedup(&[m0.clone(), m1.clone()], &e).expect("eligible");
        assert_eq!(plan.superseded.len(), 1);
        assert!(plan.survivor == m0.id || plan.survivor == m1.id);
        assert_eq!(plan.summed_access_count, 0); // fresh memories
    }

    #[test]
    fn plan_dedup_blocks_when_cosine_below_gate() {
        let m0 = mem("the user drives a Rivian R1T");
        let m1 = mem("the user drives a Rivian R1T"); // identical text → containment 1.0
                                                      // but cosine far below gate.
        let e = vec![unit(1.0, 0.0), unit(0.5, 1.0)];
        assert!(
            plan_dedup(&[m0, m1], &e).is_none(),
            "low cosine must block dedup even with identical text"
        );
    }

    #[test]
    fn plan_dedup_blocks_when_lexical_below_gate() {
        let m0 = mem("the user works as a data scientist at Helix Labs");
        let m1 = mem("the user enjoys baking sourdough bread on weekends");
        // high cosine but disjoint words.
        let e = vec![unit(1.0, 0.0), unit(1.0, 0.0)];
        assert!(
            plan_dedup(&[m0, m1], &e).is_none(),
            "low containment must block dedup even with high cosine"
        );
    }

    #[test]
    fn plan_dedup_over_merge_guard_one_far_member_blocks_whole_cluster() {
        // m0 ≈ m1 (near-identical) but m2 is far from both → all-pairs fails.
        let m0 = mem("the user drives a Rivian R1T");
        let m1 = mem("the user drives a Rivian R1T truck");
        let m2 = mem("the user drives a Rivian R1T"); // text close...
        let e = vec![unit(1.0, 0.02), unit(1.0, 0.01), unit(0.3, 1.0)]; // ...but vector far
        assert!(
            plan_dedup(&[m0, m1, m2], &e).is_none(),
            "a transitively-connected far member must block whole-cluster dedup"
        );
    }

    #[test]
    fn plan_dedup_aggregates_sum_access_and_max_confidence() {
        let mut m0 = mem("identical fact text");
        let mut m1 = mem("identical fact text");
        m0.access_count = 5;
        m1.access_count = 7;
        m0.confidence = 0.8;
        m1.confidence = 0.95;
        let e = vec![unit(1.0, 0.0), unit(1.0, 0.0)];
        let plan = plan_dedup(&[m0, m1], &e).expect("eligible");
        assert_eq!(plan.summed_access_count, 12);
        assert!((plan.max_confidence - 0.95).abs() < 1e-6);
    }

    #[test]
    fn plan_dedup_rejects_mismatched_lengths_and_singletons() {
        let m0 = mem("x");
        assert!(
            plan_dedup(std::slice::from_ref(&m0), &[unit(1.0, 0.0)]).is_none(),
            "singleton"
        );
        assert!(plan_dedup(&[m0], &[]).is_none(), "length mismatch");
    }
}
