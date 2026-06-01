//! SPIKE (A5 fix exploration, 2026-06-01) — nearest-neighbor contradiction
//! candidate generation, replacing K-means topic clustering.
//!
//! ## Why
//!
//! The §7 live dogfood proved K-means topic clustering does NOT reliably
//! co-locate a knowledge-update pair (Tesla→Rivian): the conflicting pair was
//! split into different groups, so the pairwise judge never saw it and A5 (the
//! ship-gate) silently missed the contradiction. K-means is the wrong paradigm
//! here — contradiction detection is a NEAREST-NEIGHBOR question ("for fact X,
//! is there a more-recent fact about the same thing?"), not a partitioning one.
//!
//! ## What this spike validates (the decisive unknown)
//!
//! Phi-4's pairwise judging already works (it correctly cleared 6 unrelated
//! pairs in the dogfood). The ONLY thing that broke was *getting the conflicting
//! pair to the judge*. So this spike validates the retrieval half ONLY (real
//! BGE, no Phi-4): for each fact take its top-K cosine neighbors, union into an
//! unordered candidate-pair set, and assert the **(Tesla, Rivian)** pair is in
//! it — the exact pair K-means dropped. It also prints the candidate pairs +
//! cosines (so we can see where a similarity floor sits) and the pair-count
//! bound vs all-pairs.
//!
//! Methodology: compile-and-run, real bge-small fixture. Run:
//! ```text
//! cargo test -p vault-consolidator --test nn_contradiction_spike -- --ignored --nocapture
//! ```

#![forbid(unsafe_code)]

use std::path::PathBuf;

use vault_embedding::{BgeSmallProvider, EmbeddingProvider};

/// The exact post-dedup active set from the §7 dogfood (9 facts, markers
/// included to stay faithful to what the agent actually stored). `tesla` and
/// `rivian` are the knowledge-update pair A5 must catch.
fn dogfood_facts() -> Vec<(&'static str, &'static str)> {
    vec![
        ("color", "C6COLOR The user's favorite color is amber."),
        ("unicode", "C7UNI 🔐 日本語 Ωβγ café résumé ﬁnesse."),
        (
            "cello",
            "Plays the cello in a community orchestra on Sunday afternoons.",
        ),
        ("food", "PRECFOOD The user's favourite cuisine is Japanese."),
        (
            "norm",
            "C9NORM the user is checking normalization determinism.",
        ),
        ("tesla", "A5CAR The user drives a Tesla Model 3."),
        (
            "rivian",
            "A5CAR The user sold the Tesla and now drives a Rivian R1T.",
        ),
        (
            "job",
            "PRECJOB The user works as a data scientist at Helix Labs.",
        ),
        (
            "dog",
            "DEDUPDOG The user's dog is a Labrador named Biscuit.",
        ),
    ]
}

/// Per-fact neighbor count. Each fact contributes its top-K closest facts as
/// candidate contradiction partners. Small K keeps the judge's pair count
/// bounded (~N·K/2) while guaranteeing a fact is paired with its true
/// same-topic neighbors.
const TOP_K: usize = 3;

fn bge_fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vault-embedding/test-fixtures/bge-small-en-v1.5")
        .join(name);
    std::fs::canonicalize(&p).unwrap_or_else(|e| {
        panic!("missing bge fixture {p:?} ({e}); run scripts/setup-dev-env.ps1")
    })
}

#[cfg(target_os = "windows")]
const ORT_LIB: &str = "onnxruntime.dll";
#[cfg(target_os = "macos")]
const ORT_LIB: &str = "libonnxruntime.dylib";
#[cfg(target_os = "linux")]
const ORT_LIB: &str = "libonnxruntime.so";

/// Cosine of two L2-normalised BGE vectors = dot product.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[tokio::test]
#[ignore = "real bge-small spike; run with --ignored --nocapture"]
async fn nearest_neighbor_surfaces_the_pair_kmeans_dropped() {
    let provider = BgeSmallProvider::open(
        &bge_fixture("model.onnx"),
        &bge_fixture("tokenizer.json"),
        &bge_fixture(ORT_LIB),
    )
    .expect("open bge-small fixture");

    let facts = dogfood_facts();
    let n = facts.len();

    // Embed all facts.
    let mut emb: Vec<Vec<f32>> = Vec::with_capacity(n);
    for (_, text) in &facts {
        emb.push(provider.embed(text).await.expect("embed"));
    }

    // Full cosine matrix.
    let mut cos = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in 0..n {
            cos[i][j] = if i == j {
                1.0
            } else {
                cosine(&emb[i], &emb[j])
            };
        }
    }

    let tag = |i: usize| facts[i].0;
    let tesla = facts.iter().position(|(t, _)| *t == "tesla").unwrap();
    let rivian = facts.iter().position(|(t, _)| *t == "rivian").unwrap();

    // Per-fact top-K neighbors (exclude self), and the unordered candidate set.
    use std::collections::BTreeSet;
    let mut candidates: BTreeSet<(usize, usize)> = BTreeSet::new();

    println!("\n=== NN CONTRADICTION SPIKE — per-fact top-{TOP_K} neighbors ===\n");
    for (i, row) in cos.iter().enumerate() {
        let mut others: Vec<usize> = (0..n).filter(|&j| j != i).collect();
        others.sort_by(|&a, &b| row[b].partial_cmp(&row[a]).unwrap());
        let topk = &others[..TOP_K.min(others.len())];
        let shown: Vec<String> = topk
            .iter()
            .map(|&j| format!("{}({:.3})", tag(j), row[j]))
            .collect();
        println!("  {:<8} -> {}", tag(i), shown.join("  "));
        for &j in topk {
            candidates.insert((i.min(j), i.max(j)));
        }
    }

    // The decisive check: is the Tesla/Rivian pair a candidate?
    let pair = (tesla.min(rivian), tesla.max(rivian));
    let pair_present = candidates.contains(&pair);
    let tr_cos = cos[tesla][rivian];

    // Rank of rivian among tesla's neighbors (1 = nearest) and vice-versa.
    let rank_of = |from: usize, to: usize| -> usize {
        let mut others: Vec<usize> = (0..n).filter(|&j| j != from).collect();
        others.sort_by(|&a, &b| cos[from][b].partial_cmp(&cos[from][a]).unwrap());
        others.iter().position(|&j| j == to).unwrap() + 1
    };

    println!(
        "\n=== CANDIDATE PAIR SET ({} pairs; all-pairs would be {}) ===",
        candidates.len(),
        n * (n - 1) / 2
    );
    let mut sorted: Vec<(usize, usize)> = candidates.iter().copied().collect();
    sorted.sort_by(|a, b| cos[b.0][b.1].partial_cmp(&cos[a.0][a.1]).unwrap());
    for (a, b) in &sorted {
        let mark = if (*a, *b) == pair {
            "  <-- TESLA/RIVIAN"
        } else {
            ""
        };
        println!(
            "  cos={:.3}   {:<8} ~ {:<8}{}",
            cos[*a][*b],
            tag(*a),
            tag(*b),
            mark
        );
    }

    println!("\n=== VERDICT ===");
    println!("  Tesla/Rivian cosine        : {tr_cos:.3}");
    println!(
        "  rivian's rank for tesla    : {} of {}",
        rank_of(tesla, rivian),
        n - 1
    );
    println!(
        "  tesla's rank for rivian    : {} of {}",
        rank_of(rivian, tesla),
        n - 1
    );
    println!(
        "  (Tesla,Rivian) is a candidate pair? {}",
        if pair_present { "YES ✅" } else { "NO ❌" }
    );
    println!(
        "  candidate pairs = {} vs all-pairs {} ({}% — bounded)",
        candidates.len(),
        n * (n - 1) / 2,
        candidates.len() * 100 / (n * (n - 1) / 2)
    );
    if pair_present {
        println!("  → NN candidate generation SURFACES the pair K-means dropped. The judge");
        println!("    (already proven) would then catch the Tesla→Rivian knowledge update.");
    } else {
        println!("  → NN did NOT surface the pair — raise TOP_K or reconsider the approach.");
    }
    println!("=========================================================\n");

    assert!(
        pair_present,
        "SPIKE GOAL: the (Tesla, Rivian) knowledge-update pair MUST be a nearest-neighbor \
         candidate (K-means dropped it). Tesla/Rivian cosine={tr_cos:.3}, \
         rivian rank for tesla={}",
        rank_of(tesla, rivian)
    );
    assert!(
        candidates.len() < n * (n - 1) / 2,
        "candidate set must be bounded below all-pairs"
    );
}

/// Lock the production similarity floor on MEASURED bge-small cosines, not a
/// guess. Runs the PRODUCTION candidate generator
/// ([`nearest_neighbor_candidate_pairs`]) over the exact pairs the A5
/// integration test (`contradiction_resolution.rs`) uses, and asserts the
/// genuine Vega→Atlas knowledge-update is surfaced as a candidate at the
/// production floor. Prints every pairwise cosine + floor verdict so the
/// recorded number can go straight into the ADR. If the Vega/Atlas assertion
/// fails, `CONTRADICTION_NN_SIMILARITY_FLOOR` is too high and must be lowered.
#[tokio::test]
#[ignore = "real bge-small spike; run with --ignored --nocapture"]
async fn production_floor_admits_the_integration_contradiction_pair() {
    use vault_consolidator::phases::candidates::{
        nearest_neighbor_candidate_pairs, CONTRADICTION_NN_SIMILARITY_FLOOR,
    };

    let provider = BgeSmallProvider::open(
        &bge_fixture("model.onnx"),
        &bge_fixture("tokenizer.json"),
        &bge_fixture(ORT_LIB),
    )
    .expect("open bge-small fixture");

    // Verbatim from contradiction_resolution.rs (Vega/Atlas + the
    // employer/commute false-positive guard pair).
    let facts: Vec<(&str, &str)> = vec![
        (
            "vega",
            "As of 2026-01-10 the user worked as a structural engineer at Vega Bridgeworks.",
        ),
        (
            "atlas",
            "As of 2026-04-01 the user works as a structural engineer at Atlas Structures, \
             having left Vega Bridgeworks.",
        ),
        (
            "employer",
            "As of 2026-04-01 the user works as a structural engineer at Atlas Structures.",
        ),
        (
            "commute",
            "The user commutes to work by train every weekday.",
        ),
    ];
    let n = facts.len();
    let mut emb: Vec<Vec<f32>> = Vec::with_capacity(n);
    for (_, text) in &facts {
        emb.push(provider.embed(text).await.expect("embed"));
    }

    let tag = |i: usize| facts[i].0;
    println!("\n=== INTEGRATION-PAIR COSINES (production floor = {CONTRADICTION_NN_SIMILARITY_FLOOR}) ===");
    for i in 0..n {
        for j in (i + 1)..n {
            let c = cosine(&emb[i], &emb[j]);
            let verdict = if c >= CONTRADICTION_NN_SIMILARITY_FLOOR {
                "≥ floor (candidate)"
            } else {
                "< floor (skipped)"
            };
            println!("  cos={c:.3}   {:>8} ~ {:<8}  {verdict}", tag(i), tag(j));
        }
    }

    // Run the PRODUCTION candidate generator end-to-end on the real vectors.
    let pairs = nearest_neighbor_candidate_pairs(&emb);
    let pair_tags: Vec<(&str, &str)> = pairs.iter().map(|&(i, j)| (tag(i), tag(j))).collect();
    println!("  production candidate pairs: {pair_tags:?}\n");

    let (vega, atlas) = (0usize, 1usize);
    let vega_atlas = (vega.min(atlas), vega.max(atlas));
    assert!(
        pairs.contains(&vega_atlas),
        "Vega/Atlas (the A5 integration contradiction) MUST be a candidate pair at the \
         production floor {CONTRADICTION_NN_SIMILARITY_FLOOR}; if this fails, lower \
         CONTRADICTION_NN_SIMILARITY_FLOOR. cos={:.3}, pairs={pair_tags:?}",
        cosine(&emb[vega], &emb[atlas])
    );
}
