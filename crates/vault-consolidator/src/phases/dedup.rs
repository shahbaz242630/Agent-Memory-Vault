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
//! gap.
//!
//! ## ⚠️ The cosine axis does NOT keep contradictions out — third axis added
//!
//! This module previously claimed that *"contradictory pairs can share high
//! containment but score < 0.92 cosine, so they never cluster at the 0.92 gate
//! AND are caught separately by the topic-level A5 pass — the cosine axis keeps
//! them out of dedup."* **That is false, and was falsified by measurement on
//! 2026-07-26.**
//!
//! Both axes above are blind to word ORDER by construction: bge-small scores
//! `"The user prefers tea over coffee."` against `"The user prefers coffee over
//! tea."` at **0.9979** — higher than any true paraphrase measured — and
//! [`token_containment`] compares token *sets*, which cannot represent order at
//! all. Two order-blind checks cannot catch a reordering. Re-measured over 8
//! contradictory pairs, the class spans **0.7199 – 0.9979**; four breach the
//! documented 0.883 ceiling. The 2026-05-31 calibration only ever sampled
//! *different-value* contradictions (Vega/Atlas — 0.7199), never
//! *reversed-argument* ones.
//!
//! **Confirmed live**, clean 4-memory vault, 2026-07-26: the pair cleared both
//! gates, was collapsed by plain code, the older fact was superseded, and the
//! run reported `contradictions queued: 0` with an empty `## Merges` section —
//! a fact left the vault and the user-facing summary showed nothing.
//!
//! **The defect is not that a fact was retired — it is that the pair was
//! treated as a DUPLICATE, so contradiction detection never ran on it at all.**
//! Nothing was recorded as a contradiction, and dedup does not render into
//! `## Merges` either, so the collapse appeared nowhere a user would look.
//! BRD §5.6 line 985 requires contradictions reach the review path — *"For
//! contradictions, write to `ConflictReview` queue, do not auto-resolve"* — and
//! V0.2 refines that per `Memory Vault Tests.md` K6: *"clear-winner
//! contradictions auto-`invalidate()`; ambiguous ones queue to
//! `conflicts_for_user_review`, not silently picked"*. Auto-resolving a clear
//! winner is therefore correct and intended; **reaching that path at all is
//! what this pair never did.**
//!
//! So the gate takes a **third axis**: [`is_pure_reordering`] — same word
//! multiset in a different order → decline. Declining costs no legitimate
//! merge: the cluster falls through to the Phase-2 LLM path, which per BRD §5.6
//! line 983 decides *"merge into one memory" / "keep separate" / "contradiction
//! — flag for user"*. We trade one model call for never blind-collapsing a
//! reversal.
//!
//! **Re-verified live on the same 4-memory vault after the fix, 2026-07-26:**
//! `clusters deduped: 0` / `memories deduped: 0` (the blind collapse is gone),
//! this module's decline logged, and the pair reached the contradiction path —
//! `contradictions_auto_resolved=1`, rendered in the run summary as
//! `**Auto-resolved (newer fact won):** 1` plus a per-boundary detail line. The
//! newer fact still wins, which is the intended clear-winner outcome; the
//! difference is that it is now a recorded contradiction rather than an
//! invisible deduplication.

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

/// Lowercased alphanumeric word tokens of `s` **in order**. Splits on any
/// non-alphanumeric char (Unicode-aware via [`char::is_alphanumeric`]); empty
/// tokens dropped. The single tokenisation definition in this module — both
/// [`tokens`] and [`is_pure_reordering`] derive from it, so the set-based and
/// order-based axes can never drift apart on what counts as a word.
fn token_sequence(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Lowercased alphanumeric word tokens of `s`, order discarded.
fn tokens(s: &str) -> HashSet<String> {
    token_sequence(s).into_iter().collect()
}

/// `true` when `a` and `b` contain **exactly the same words** (same multiset,
/// so repeats count) arranged in a **different order**.
///
/// This is the order-sensitive axis the cosine and containment gates cannot
/// provide — see the module header. It is deliberately narrow: it fires only on
/// a pure permutation, which is precisely the reversed-argument shape
/// (`"prefers tea over coffee"` / `"prefers coffee over tea"`) that scored
/// 0.9979 cosine and 1.00 containment and was collapsed blind.
///
/// Identical text returns `false` — that is a genuine duplicate, not a
/// reordering, and must remain eligible for collapse.
///
/// **False positives are cheap by design.** A same-words-different-order pair
/// that really IS a paraphrase (`"Mondays and Thursdays"` /
/// `"Thursdays and Mondays"`) is not lost — it merely stops qualifying for the
/// *deterministic* collapse and is handed to the Phase-2 judgement path, which
/// can still merge it. The asymmetry is intentional: an unnecessary model call
/// costs seconds, a blind-collapsed contradiction costs a fact.
fn is_pure_reordering(a: &str, b: &str) -> bool {
    let (seq_a, seq_b) = (token_sequence(a), token_sequence(b));
    // Same words in the SAME order is a duplicate, not a reordering.
    if seq_a == seq_b {
        return false;
    }
    let (mut sorted_a, mut sorted_b) = (seq_a, seq_b);
    sorted_a.sort();
    sorted_b.sort();
    sorted_a == sorted_b
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
            // Third axis. Checked LAST on purpose: reaching here means the pair
            // already cleared both similarity gates, so this log line marks a
            // collapse that WOULD have happened silently. Ids only — never
            // memory content in logs.
            if is_pure_reordering(&members[i].content, &members[j].content) {
                tracing::info!(
                    left = ?members[i].id,
                    right = ?members[j].id,
                    "declining deterministic dedup: identical words in a different order — \
                     routing to the merge/contradiction path so the pair is judged rather than \
                     collapsed as a duplicate"
                );
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

    // ── is_pure_reordering (the order axis) ────────────────────────────
    #[test]
    fn reordering_detects_reversed_arguments() {
        // The live-confirmed case: cosine 0.9979, containment 1.00.
        assert!(is_pure_reordering(
            "The user prefers tea over coffee.",
            "The user prefers coffee over tea."
        ));
    }

    #[test]
    fn reordering_is_false_for_identical_text() {
        // A genuine duplicate must stay eligible for collapse.
        assert!(!is_pure_reordering(
            "the user drinks tea",
            "the user drinks tea"
        ));
    }

    #[test]
    fn reordering_is_false_when_the_words_differ() {
        // Battery pair 1 — a real near-duplicate the live run correctly
        // deduped. "editor" vs "editors" is a different multiset.
        assert!(!is_pure_reordering(
            "The user prefers dark mode in their code editor.",
            "The user prefers dark mode in their code editors."
        ));
    }

    #[test]
    fn reordering_ignores_case_and_punctuation() {
        assert!(is_pure_reordering("Tea over coffee!", "coffee over tea"));
    }

    #[test]
    fn reordering_compares_multisets_not_sets() {
        // Same SET {tea, coffee} but different counts → not a permutation.
        // A set-based check would wrongly call this a reordering.
        assert!(!is_pure_reordering("tea tea coffee", "tea coffee coffee"));
    }

    #[test]
    fn plan_dedup_blocks_reversed_argument_contradiction() {
        // Regression pin for the live 2026-07-26 finding: this pair cleared
        // BOTH similarity gates and was collapsed as a DUPLICATE, so
        // contradiction detection never ran and nothing was recorded. After
        // the fix the same vault reports `contradictions_auto_resolved=1`.
        let m0 = mem("The user prefers tea over coffee.");
        let m1 = mem("The user prefers coffee over tea.");
        // Cosine 1.0 — comfortably above the gate, as measured in reality.
        let e = vec![unit(1.0, 0.0), unit(1.0, 0.0)];
        assert!(
            plan_dedup(&[m0, m1], &e).is_none(),
            "a reversed-argument contradiction must never be blind-collapsed"
        );
    }

    #[test]
    fn plan_dedup_still_collapses_a_genuine_near_duplicate() {
        // Over-fire guard for the new axis: battery pair 1, which the live
        // 2026-07-25 run correctly deduped, must still be eligible.
        let m0 = mem("The user prefers dark mode in their code editor.");
        let m1 = mem("The user prefers dark mode in their code editors.");
        let e = vec![unit(1.0, 0.01), unit(1.0, 0.0)];
        assert!(
            plan_dedup(&[m0, m1], &e).is_some(),
            "the order axis must not block genuine near-duplicates"
        );
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
