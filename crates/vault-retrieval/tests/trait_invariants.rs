//! Trait-level invariants for `Retriever` implementers.
//!
//! Two harnesses live here:
//!
//! - [`assert_no_boundary_leak`] — the **corpus-driven** harness. Takes a
//!   [`BoundaryLeakCorpus`] describing how many memories live in each
//!   boundary and which subsets are authorised; populates the bundle's
//!   stores; runs one `retrieve()` per authorised subset; asserts every
//!   returned memory's boundary is in the active subset. Generic over
//!   `R: Retriever` so T0.2.7's `MultiStrategyRetriever` re-uses it
//!   without modification.
//!
//! - [`assert_boundary_leakage_invariant`] — the **single-fixture** entry
//!   point. Wraps `assert_no_boundary_leak` with the same hardcoded
//!   3-boundary fixture (`work` / `personal` / `secret`) Phase 1 + 2
//!   shipped against. Kept so the existing call site in
//!   `semantic_retriever_does_not_leak_across_boundaries` stays
//!   green during the Phase 3 refactor.
//!
//! Phase 3 adds [`proptest_no_boundary_leak_under_random_corpus`] which
//! drives `assert_no_boundary_leak` with proptest-generated corpora.
//! BRD §7.1 Heavy classification — security-critical access-control
//! invariant, so randomised verification carries weight.

mod common;

use std::collections::HashSet;

use common::{boundary, make_memory, make_test_retriever, TestRetriever};
use proptest::prelude::*;
use vault_core::Boundary;
use vault_embedding::EMBEDDING_DIM;
use vault_retrieval::{RetrievalOptions, RetrievalQuery, Retriever};

/// Description of a randomised (or hand-built) test corpus for the
/// boundary-leak invariant.
///
/// `boundaries_with_counts[i] = (boundary, n)` means: insert `n`
/// memories into `boundary`, with synthetically-distinct content + a
/// drift-`i+1` embedding. Total memory count across all entries is
/// expected to be bounded at ~25 by the caller — proptest respects
/// this bound via `prop_assume!`.
///
/// `auth_subsets` is a list of authorised-boundary subsets; the harness
/// runs one `retrieve()` per subset and verifies the leak invariant
/// holds across every result. Each subset is a `Vec<usize>` of indices
/// into `boundaries_with_counts` — empty subsets (`[]`) are valid
/// proptest inputs but exercise only the Q1 short-circuit, not the
/// SQL-layer filter.
#[derive(Clone, Debug)]
pub struct BoundaryLeakCorpus {
    pub boundaries_with_counts: Vec<(Boundary, usize)>,
    pub auth_subsets: Vec<Vec<usize>>,
}

/// Generic boundary-leak invariant. **Caller must ensure `retriever`
/// uses `b`'s stores** — in V0.1 this means passing `&b.retriever`
/// (which was constructed from `b.metadata` + `b.vectors`).
///
/// For each `(boundary, count)` entry the harness inserts `count`
/// memories into `boundary` via the bundle's metadata + vector stores.
/// For each `auth_subset`, it issues one `retrieve()` with
/// `authorized_boundaries` set to the subset's boundaries and asserts
/// every returned memory's boundary is in that subset.
///
/// **Load-bearing assertion (BRD §11.4.3):** no result can carry a
/// boundary outside `auth_subset`. A single counter-example fails the
/// build loudly — this is the leak-detection contract for V0.1 and
/// every future `Retriever` implementer.
pub async fn assert_no_boundary_leak<R: Retriever>(
    b: &TestRetriever,
    retriever: &R,
    corpus: BoundaryLeakCorpus,
) {
    // Populate the bundle's stores. Drift indices are derived from the
    // (boundary_index, memory_index_within_boundary) pair so different
    // memories get distinguishable cosine distances.
    for (boundary_idx, (boundary, count)) in corpus.boundaries_with_counts.iter().enumerate() {
        for memory_idx in 0..*count {
            let m = make_memory(&format!("b{boundary_idx}-m{memory_idx}"), boundary);
            b.metadata.create_memory(&m).await.expect("create memory");
            // Synthesise a unit vector with a small per-memory drift so
            // distances are distinct. We don't go through
            // `insert_memory_with_drift` because that uses a single
            // monotone drift; here we want per-(boundary, memory) drift
            // so identical positions across boundaries don't collide.
            let drift = boundary_idx * 10 + memory_idx + 1;
            let mut emb = vec![0.0_f32; EMBEDDING_DIM];
            emb[0] = 1.0;
            if drift > 0 && drift + 1 < EMBEDDING_DIM {
                emb[drift + 1] = (drift as f32) * 1e-3;
            }
            let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
            for x in &mut emb {
                *x /= norm;
            }
            b.vectors
                .upsert(&m.id, &emb, boundary)
                .await
                .expect("vector upsert");
        }
    }

    // For each authorised subset, retrieve and assert the leak invariant.
    for auth_subset_indices in &corpus.auth_subsets {
        let authorised: Vec<Boundary> = auth_subset_indices
            .iter()
            .map(|i| corpus.boundaries_with_counts[*i].0.clone())
            .collect();
        let authorised_set: HashSet<Boundary> = authorised.iter().cloned().collect();

        let q = RetrievalQuery {
            query_text: "anything".into(),
            authorized_boundaries: authorised.clone(),
            // 100 is well below MAX_RESULTS_CAP (200 post 2026-05-18); the
            // synthetic property-test corpus stays well below this too.
            max_results: 100,
            options: RetrievalOptions::default(),
        };
        let res = retriever.retrieve(q).await.expect("retrieve");
        for r in &res {
            assert!(
                authorised_set.contains(&r.memory.boundary),
                "boundary leak: returned memory {:?} has boundary {:?}, not in authorised subset {:?}",
                r.memory.id,
                r.memory.boundary,
                authorised_set
            );
        }
    }
}

/// Single-fixture wrapper preserved from Phase 1 + 2. Drives
/// [`assert_no_boundary_leak`] with the same 3-boundary corpus
/// (`work` ×3, `personal` ×2, `secret` ×2) and the same two auth
/// subsets (`[work]`, `[work, personal]`) the original harness used.
pub async fn assert_boundary_leakage_invariant<R: Retriever>(b: &TestRetriever, retriever: &R) {
    let corpus = BoundaryLeakCorpus {
        boundaries_with_counts: vec![
            (boundary("work"), 3),
            (boundary("personal"), 2),
            (boundary("secret"), 2),
        ],
        auth_subsets: vec![vec![0], vec![0, 1]],
    };
    assert_no_boundary_leak(b, retriever, corpus).await;
}

/// Drive the single-fixture invariant against `SemanticRetriever`. The
/// proptest counterpart below exercises the same harness across
/// randomised corpora.
#[tokio::test]
async fn semantic_retriever_does_not_leak_across_boundaries() {
    let b = make_test_retriever().await;
    assert_boundary_leakage_invariant(&b, &b.retriever).await;
}

// =============================================================================
// Proptest — randomised corpora over the boundary-leak invariant
// =============================================================================
//
// proptest is sync; the harness is async. Wrap each case body in
// `tokio::runtime::Runtime::new().unwrap().block_on(...)` (the standard
// pattern). Each case allocates its own bundle (tempdir + fresh
// LanceVectorStore + SQLCipher MetadataStore) so cases are fully
// independent.
//
// `prop_assume!` calls below are scoped strictly to *input-shaping
// artefacts* — dedup collisions in the boundary-name regex and the
// 25-memory total bound. They are **never** used to silently filter
// invariant-failing cases. If a real leak surfaces, the
// `assert!(authorised_set.contains(...))` panics and the build fails
// loudly with the shrunk counter-example — that's the failure mode
// the standing-rule "stop and escalate" applies to (per Phase 3
// watch-point #1 — don't paper over with prop_assume rejections).

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Boundary-leak invariant under randomised corpora.
    /// 2-5 boundaries × 0-5 memories each; total bounded at 25 per case.
    /// Authorised subset is a non-empty bitmask-derived selection of
    /// boundary indices. The single retrieval per case asserts the
    /// load-bearing access-control invariant.
    ///
    /// **Generator design note (initial first-run regression):** the
    /// initial draft of this proptest used two parallel vecs
    /// (`boundary_names` + `memory_counts`) which proptest could
    /// generate at different lengths, leading to a length-mismatch
    /// panic at the shrunk input `["A","a","0"] / [0,0]` — an
    /// input-shaping bug in the harness, NOT an invariant violation.
    /// Per Phase 3 watch-point #1 (don't paper over with `prop_assume!`),
    /// the generator was restructured to a single `Vec<(name, count)>`
    /// tuple so length-mismatch is structurally impossible. This is
    /// the kind of refinement the watch-point asked for: understand
    /// what `prop_assume!` would have masked, then fix the structural
    /// issue rather than rejecting cases.
    #[test]
    fn proptest_no_boundary_leak_under_random_corpus(
        // 2-5 (boundary-name, memory-count) tuples. Generating as a
        // single Vec<tuple> guarantees the lengths match — the parallel
        // vec design was the source of the initial panic.
        // Length 1-16 keeps fixtures readable while exercising the full
        // identifier-character set defined in `Boundary::new`.
        raw_corpus in proptest::collection::vec(
            ("[a-zA-Z0-9_-]{1,16}", 0_usize..=5_usize),
            2_usize..=5_usize,
        ),
        // Bitmask used to derive a non-empty authorised subset.
        // Constraining to `1..` rules out the 0 mask; we mod by
        // `(1<<n) - 1` and `+1` below to land in `1..=(1<<n - 1)`,
        // which is non-empty for any `n >= 1`.
        auth_bitmask in 1_u32..u32::MAX,
    ) {
        // -- Input shaping (sync; prop_assume! works here) -----------------
        // Dedup by boundary name while preserving first-seen order +
        // its paired count. The regex generator doesn't dedup, so two
        // generated tuples may share a name.
        let mut seen = HashSet::new();
        let mut bws: Vec<(Boundary, usize)> = Vec::new();
        for (name, count) in &raw_corpus {
            if seen.insert(name.clone()) {
                let b = Boundary::new(name).expect("regex matches Boundary charset");
                bws.push((b, *count));
            }
        }
        // After dedup, need at least 2 boundaries for a meaningful test.
        // Rejection scope: dedup collisions (input-shaping artefact, NOT
        // an invariant violation). Per Phase 3 watch-point #1.
        prop_assume!(bws.len() >= 2);
        let n = bws.len();

        // Bound total memory count at 25 per case. Rejection scope:
        // keeps individual cases under ~3s of LanceDB+SQLite work.
        prop_assume!(bws.iter().map(|(_, c)| c).sum::<usize>() <= 25);

        // Derive non-empty auth subset from bitmask. For `n` boundaries,
        // valid subsets are bitmasks `1..=(1<<n - 1)` — `1` selects only
        // boundary 0, `(1<<n - 1)` selects all. The arithmetic guarantees
        // `mask >= 1`, so `auth_subset` is always non-empty.
        let max_mask = (1_u32 << n) - 1;
        let mask = (auth_bitmask % max_mask) + 1;
        let auth_subset: Vec<usize> = (0..n)
            .filter(|i| (mask >> i) & 1 == 1)
            .collect();

        let corpus = BoundaryLeakCorpus {
            boundaries_with_counts: bws,
            auth_subsets: vec![auth_subset],
        };

        // -- Async work (run via block_on; assertions inside drive the
        //    proptest pass/fail signal via panic).
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        rt.block_on(async move {
            let b = make_test_retriever().await;
            assert_no_boundary_leak(&b, &b.retriever, corpus).await;
        });
    }
}
