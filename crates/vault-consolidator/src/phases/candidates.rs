//! Nearest-neighbor contradiction candidate generation (T0.3.x A5, ADR-065).
//!
//! ## Why this replaces K-means topic grouping
//!
//! The first A5 design (ADR-060/062) judged contradictions *within* K-means
//! topic groups. The §7 live dogfood (2026-06-01) proved that premise FALSE:
//! K-means does NOT reliably co-locate a knowledge-update pair. The
//! conflicting Tesla→Rivian pair was split into different groups, so the
//! pairwise judge never saw it and A5 — the V0.2 ship-gate — silently missed
//! the contradiction. A verbose-logging re-run confirmed the pair was never
//! judged; Phi-4 and the recency aggregator were both sound.
//!
//! Contradiction detection is a **nearest-neighbor** question ("for fact X, is
//! there a more-recent fact about the *same thing*?"), not a partitioning one.
//! K-means is *forced* to fill a fixed number of buckets, so semantically
//! adjacent facts get separated rather than compared. This module replaces the
//! bucketing with the right primitive: for each fact, take its top-K closest
//! cousins by cosine similarity and union them into an unordered candidate-pair
//! set. Those pairs feed the EXISTING pairwise judge
//! ([`crate::phases::contradiction::judge_candidate_pairs`]) and the EXISTING
//! recency aggregator — only candidate *generation* changed.
//!
//! ## Spike provenance (the floor + K were measured, not guessed)
//!
//! `tests/nn_contradiction_spike.rs` ran this exact top-K logic on the real
//! bge-small embeddings of the 9-fact dogfood set:
//! - The (Tesla, Rivian) pair is the **single strongest edge** in the neighbor
//!   graph (cosine **0.823**; next-highest unrelated pair only **0.634**) and
//!   the two are **mutual #1 nearest neighbors**.
//! - The weak distractor pairs (0.43–0.634 — bge-small noise on unrelated
//!   facts) the pairwise judge would reject anyway, so the floor's job is to
//!   keep them away from the small model (fewer wasted Phi-4 calls AND fewer
//!   false-positive-retirement chances); Phi-4 remains the precision gate on
//!   what it IS handed. The floor (0.70) sits in the clean **0.634 → 0.823**
//!   gap the spike measured between the noise ceiling and the real
//!   contradictions, so it admits genuine knowledge-update pairs while
//!   excluding every observed noise pair. See
//!   [`CONTRADICTION_NN_SIMILARITY_FLOOR`] for the full measurement.
//!
//! The spike is kept as executable documentation (run it with `--ignored
//! --nocapture` to re-print the neighbor graph + cosines on the dogfood set).

use std::collections::BTreeSet;

use vault_core::{Boundary, MemoryId, VaultResult};
use vault_storage::StorageBackend;

/// Per-fact neighbor count. Each fact contributes at most its top-K closest
/// other facts as candidate contradiction partners. Small K keeps the judge's
/// pair count bounded (≤ N·K/2 after the symmetric union) while still pairing a
/// fact with its true same-subject neighbor — a knowledge-update pair is each
/// other's *nearest* neighbor (spike: Tesla/Rivian were mutual #1), so it is
/// always inside the top-K.
pub const CONTRADICTION_NN_TOP_K: usize = 3;

/// Minimum cosine similarity for a neighbor to become a candidate pair.
///
/// The floor trims pairs too dissimilar to plausibly describe the same
/// attribute. The pairwise Phi-4 judge (with its required `shared_attribute` +
/// recency) is the precision authority on whether an admitted pair is genuinely
/// contradictory; the floor's job is to keep obvious-noise pairs away from the
/// small model (fewer offline judge calls AND fewer false-positive-retirement
/// chances).
///
/// **0.70 is measured, not guessed** (`nn_contradiction_spike.rs`, real
/// bge-small). The two genuine knowledge-update contradictions sit well above:
/// Vega→Atlas at cosine **0.905** and Tesla→Rivian at **0.823** (the latter a
/// mutual #1 nearest-neighbor pair). The unrelated-fact noise band on the
/// 9-fact dogfood set tops out at **0.634** (e.g. norm~job). 0.70 sits in the
/// clean **0.634 → 0.823** gap: it admits both real contradictions with margin
/// and excludes every observed noise pair. A lower floor would only add recall
/// headroom in an unmeasured 0.55–0.70 band (no contradiction has been observed
/// there) while paying precision cost on the known-noise 0.55–0.634 band, so it
/// is strictly worse on this data.
pub const CONTRADICTION_NN_SIMILARITY_FLOOR: f32 = 0.70;

/// Cosine similarity of two embedding vectors. For the L2-normalised vectors
/// the [`vault_embedding::EmbeddingProvider`] contract guarantees this reduces
/// to a dot product; the explicit norm denominator (clamped away from zero) is
/// belt-and-braces against a degenerate input and matches `topics.rs`.
///
/// Precondition: `a` and `b` have the same dimension (the caller embeds every
/// fact with one provider, so they do). Extra coordinates on a longer slice are
/// ignored by `zip`.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-12);
    dot / denom
}

/// Build the unordered set of nearest-neighbor candidate contradiction pairs
/// from a boundary's fact embeddings.
///
/// For each fact `i`, the [`CONTRADICTION_NN_TOP_K`] most-similar *other* facts
/// are taken; those at or above [`CONTRADICTION_NN_SIMILARITY_FLOOR`] become
/// candidate pairs. Pairs are canonicalised `(min, max)` and deduplicated, so a
/// mutually-near pair appears exactly once. Indices refer into the caller's
/// `embeddings` slice (and the parallel memory slice).
///
/// Returns `(i, j)` index pairs with `i < j`, in ascending order (a
/// [`BTreeSet`] backs the union, so the output is deterministic for a given
/// input). Returns empty for fewer than two facts.
///
/// Cost: O(N²) cosine evaluations to rank neighbors (cheap, in-memory, offline)
/// and at most `N · K / 2` output pairs → at most that many Phi-4 judge calls.
pub fn nearest_neighbor_candidate_pairs(embeddings: &[Vec<f32>]) -> Vec<(usize, usize)> {
    let n = embeddings.len();
    if n < 2 {
        return Vec::new();
    }

    let mut pairs: BTreeSet<(usize, usize)> = BTreeSet::new();
    for i in 0..n {
        // Similarity of i to every other fact.
        let mut sims: Vec<(usize, f32)> = (0..n)
            .filter(|&j| j != i)
            .map(|j| (j, cosine_similarity(&embeddings[i], &embeddings[j])))
            .collect();
        // Descending by similarity; ascending index breaks ties so the top-K
        // selection is deterministic regardless of input order. `total_cmp`
        // is NaN-safe (no `.unwrap()` on `partial_cmp` — hard rule).
        sims.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (j, sim) in sims.into_iter().take(CONTRADICTION_NN_TOP_K) {
            if sim >= CONTRADICTION_NN_SIMILARITY_FLOOR {
                pairs.insert((i.min(j), i.max(j)));
            }
        }
    }
    pairs.into_iter().collect()
}

/// Incremental (ADR-082) contradiction-candidate neighbours for ONE seed:
/// the seed's top-[`CONTRADICTION_NN_TOP_K`] LanceDB neighbours at/above
/// [`CONTRADICTION_NN_SIMILARITY_FLOOR`], excluding the self-match, boundary-
/// scoped.
///
/// This is the incremental analogue of [`nearest_neighbor_candidate_pairs`]: the
/// full-sweep path embeds every active fact and computes all-pairs in memory,
/// which costs O(N) embeddings; the incremental path instead seeds only on
/// *changed* facts and asks LanceDB (which already holds every vector) for each
/// seed's neighbours. The search hits the whole boundary corpus, so a NEW fact
/// still surfaces an OLD same-subject neighbour (the cross-corpus invariant —
/// ADR-082 §D4). The caller validates returned ids against the active set and
/// pairs each `(seed, neighbour)` for the existing pairwise judge.
///
/// `LanceVectorStore::search` returns cosine *distance* (smaller = closer); for
/// the L2-normalised embeddings the provider guarantees, the similarity floor
/// `f` maps to `distance ≤ 1 - f` (mirrors `phases::cluster`).
///
/// # Errors
///
/// [`vault_core::VaultError`] propagated from the vector-store search.
pub async fn contradiction_candidate_neighbours(
    storage: &StorageBackend,
    self_id: &MemoryId,
    embedding: &[f32],
    boundary: &Boundary,
) -> VaultResult<Vec<MemoryId>> {
    // Request one extra to absorb the self-match the store returns at distance 0.
    let raw = storage
        .vector_store()
        .search(
            embedding,
            CONTRADICTION_NN_TOP_K + 1,
            std::slice::from_ref(boundary),
        )
        .await?;
    let max_distance = 1.0_f32 - CONTRADICTION_NN_SIMILARITY_FLOOR;
    Ok(raw
        .into_iter()
        .filter(|(id, _)| id != self_id)
        .filter(|(_, distance)| *distance <= max_distance)
        .map(|(id, _)| id)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2-D unit vector at cosine `c` to the reference axis `[1, 0]`:
    /// `[c, sqrt(1 - c²)]`. Lets a test pin an exact pairwise similarity
    /// relative to the floor without hand-computing norms.
    fn unit_at_cos(c: f32) -> Vec<f32> {
        vec![c, (1.0 - c * c).max(0.0).sqrt()]
    }

    fn axis_x() -> Vec<f32> {
        vec![1.0, 0.0]
    }

    #[test]
    fn no_pairs_for_fewer_than_two_facts() {
        assert!(nearest_neighbor_candidate_pairs(&[]).is_empty());
        assert!(nearest_neighbor_candidate_pairs(&[axis_x()]).is_empty());
    }

    #[test]
    fn surfaces_a_strong_pair_amid_unrelated_noise() {
        // v0/v1 are near-identical (cos 0.95 — a "contradiction"); v2 and v3
        // are orthogonal / opposite, well below any sane floor.
        let v0 = axis_x();
        let v1 = unit_at_cos(0.95);
        let v2 = vec![0.0, 1.0]; // cos 0 with v0
        let v3 = vec![-1.0, 0.0]; // cos -1 with v0
        let pairs = nearest_neighbor_candidate_pairs(&[v0, v1, v2, v3]);
        assert!(
            pairs.contains(&(0, 1)),
            "the strong (0,1) pair MUST be a candidate; got {pairs:?}"
        );
        // No pair should reach back to the opposite vector v3 (cos ≤ 0).
        assert!(
            !pairs.iter().any(|&(a, b)| a == 3 || b == 3),
            "the opposite vector must never pair (all its cosines are ≤ 0); got {pairs:?}"
        );
    }

    #[test]
    fn floor_excludes_a_pair_just_below_it() {
        // Two facts whose mutual cosine sits just BELOW the floor → no pair.
        let below = CONTRADICTION_NN_SIMILARITY_FLOOR - 0.05;
        let pairs = nearest_neighbor_candidate_pairs(&[axis_x(), unit_at_cos(below)]);
        assert!(
            pairs.is_empty(),
            "a pair at cosine {below:.3} (below the {:.3} floor) must be excluded; got {pairs:?}",
            CONTRADICTION_NN_SIMILARITY_FLOOR
        );
    }

    #[test]
    fn floor_includes_a_pair_just_above_it() {
        // Symmetric to the exclusion test: just ABOVE the floor → one pair.
        let above = CONTRADICTION_NN_SIMILARITY_FLOOR + 0.05;
        let pairs = nearest_neighbor_candidate_pairs(&[axis_x(), unit_at_cos(above)]);
        assert_eq!(
            pairs,
            vec![(0, 1)],
            "a pair at cosine {above:.3} (above the {:.3} floor) must be a candidate",
            CONTRADICTION_NN_SIMILARITY_FLOOR
        );
    }

    #[test]
    fn pairs_are_unordered_canonical_and_deduped() {
        // Two near-identical facts: querying from either endpoint yields the
        // same edge; it must appear exactly once as (min, max).
        let pairs = nearest_neighbor_candidate_pairs(&[unit_at_cos(0.99), axis_x()]);
        assert_eq!(pairs, vec![(0, 1)], "edge must be canonical + appear once");
    }

    #[test]
    fn candidate_set_is_bounded_below_all_pairs() {
        // A spread of distinct directions: top-K + floor must keep the
        // candidate count strictly below the all-pairs count N·(N-1)/2.
        let embeddings: Vec<Vec<f32>> = (0..8)
            .map(|i| {
                let theta = (i as f32) * std::f32::consts::FRAC_PI_2 / 8.0; // 0..45°
                vec![theta.cos(), theta.sin()]
            })
            .collect();
        let n = embeddings.len();
        let pairs = nearest_neighbor_candidate_pairs(&embeddings);
        let all_pairs = n * (n - 1) / 2;
        assert!(
            pairs.len() < all_pairs,
            "candidate set ({}) must be bounded below all-pairs ({all_pairs})",
            pairs.len()
        );
        // Every pair is canonical (i < j).
        assert!(pairs.iter().all(|&(a, b)| a < b), "pairs must be (min,max)");
    }

    #[test]
    fn output_is_deterministic_across_runs() {
        let embeddings: Vec<Vec<f32>> =
            vec![axis_x(), unit_at_cos(0.9), unit_at_cos(0.8), vec![0.0, 1.0]];
        let a = nearest_neighbor_candidate_pairs(&embeddings);
        let b = nearest_neighbor_candidate_pairs(&embeddings);
        assert_eq!(a, b, "identical input MUST yield identical candidate pairs");
    }

    #[test]
    fn cosine_of_identical_vectors_is_one() {
        let v = vec![0.6f32, 0.8];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }
}
