//! ADR-063 Phase 0 — dedup threshold calibration harness (MEASUREMENT ONLY).
//!
//! Picks the deterministic-dedup / LLM-routing thresholds from DATA, not from
//! literature numbers — our BGE-small cosine is measurably unreliable on the
//! relevance task ([[bge-small-cannot-separate-relevant]]), so the dedup gate
//! is two-axis (cosine AND lexical overlap) and BOTH cutoffs must be measured.
//!
//! This is an `#[ignore]`d, real-model harness (loads the ~133 MB bge-small
//! ONNX fixture). It is a measurement instrument, NOT a production-path test —
//! per [[commit-only-with-a-tested-fix]] it rides with the dedup fix it
//! calibrates, never alone.
//!
//! Run it:
//! ```text
//! cargo test -p vault-consolidator --test dedup_threshold_calibration \
//!     -- --ignored --nocapture
//! ```
//!
//! Output: per-pair (class, cosine, containment, jaccard) + per-class
//! min/max/mean, and the derived COS_HI / COS_LO / LEX_HI suggestions with the
//! separation gaps that justify them. Read the printed table; the ADR records
//! the chosen numbers + the run that produced them.
//!
//! Pairs are hand-authored to mirror real dogfood shapes (Tesla/Rivian,
//! Vega/Atlas, the §7 PREC job/hobby/food facts, CAP_OK-style length variants).

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::path::PathBuf;

use vault_embedding::{BgeSmallProvider, EmbeddingProvider};

/// The relationship class of a memory pair — the ground-truth label the
/// thresholds must separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Class {
    /// Same fact, trivial rephrase / length variant. → deterministic dedup
    /// (no LLM). This is also the structural-overflow case we must catch.
    NearIdentical,
    /// Same subject, additional / different attribute. → LLM classifier
    /// (the only case that earns a model call).
    Complementary,
    /// Same subject + attribute, incompatible value. → A5 contradiction path
    /// (already shipped; must NOT be deduped).
    Contradictory,
    /// Different subjects entirely. → keep separate.
    Unrelated,
}

/// A labeled pair: (class, text_a, text_b).
fn labeled_pairs() -> Vec<(Class, &'static str, &'static str)> {
    use Class::*;
    vec![
        // ── Near-identical: must dedup deterministically ──────────────────
        (
            NearIdentical,
            "The user drives a Rivian R1T.",
            "The user drives a Rivian R1T.",
        ),
        (
            NearIdentical,
            "The user prefers dark mode in their editors.",
            "The user prefers dark mode in their code editors.",
        ),
        (
            NearIdentical,
            "As of 2026-05-01 the user drives a Rivian R1T.",
            "The user now drives a Rivian R1T (as of 2026-05-01).",
        ),
        (
            NearIdentical,
            "The user works as a data scientist at Helix Labs.",
            "The user is a data scientist at Helix Labs.",
        ),
        (
            NearIdentical,
            "The user's favourite cuisine is Japanese.",
            "The user's favourite cuisine is Japanese food.",
        ),
        (
            NearIdentical,
            "The user plays the cello in a community orchestra.",
            "The user plays cello in a community orchestra.",
        ),
        (
            NearIdentical,
            "The user is building Memory Vault, a cross-agent personal memory layer for AI agents.",
            "The user is building Memory Vault — a cross-agent, personal memory layer for AI agents.",
        ),
        // ── Complementary: same subject, extra attribute → LLM merge ──────
        (
            Complementary,
            "The user works as a data scientist at Helix Labs.",
            "The user has worked at Helix Labs since 2024.",
        ),
        (
            Complementary,
            "The user's favourite cuisine is Japanese.",
            "The user often cooks ramen at home on weekends.",
        ),
        (
            Complementary,
            "The user plays the cello.",
            "The user performs at local charity concerts twice a year.",
        ),
        (
            Complementary,
            "The user is building Memory Vault.",
            "The user is a non-coder product owner working with an AI partner.",
        ),
        (
            Complementary,
            "The user drives a Rivian R1T.",
            "The user installed a home EV charger in the garage.",
        ),
        (
            Complementary,
            "The user lives in Manchester.",
            "The user commutes to the office three days a week.",
        ),
        // ── Contradictory: same subject+attribute, incompatible value ─────
        (
            Contradictory,
            "The user works at Vega.",
            "The user works at Atlas, having left Vega.",
        ),
        (
            Contradictory,
            "As of 2026-02-01 the user drives a Tesla Model 3.",
            "As of 2026-05-01 the user sold the Tesla and now drives a Rivian R1T.",
        ),
        (
            Contradictory,
            "The user's favourite colour is teal.",
            "The user's favourite colour is amber.",
        ),
        (
            Contradictory,
            "The user lives in Manchester.",
            "The user relocated from Manchester to Bristol.",
        ),
        (
            Contradictory,
            "The user prefers dark mode in their editors.",
            "The user switched to light mode in their editors.",
        ),
        // ── Unrelated: different subjects ─────────────────────────────────
        (
            Unrelated,
            "The user drives a Rivian R1T.",
            "The user's favourite cuisine is Japanese.",
        ),
        (
            Unrelated,
            "The user plays the cello in a community orchestra.",
            "The user works as a data scientist at Helix Labs.",
        ),
        (
            Unrelated,
            "The user prefers dark mode in their editors.",
            "The user lives in Manchester.",
        ),
        (
            Unrelated,
            "The user is building Memory Vault.",
            "The user's favourite colour is teal.",
        ),
        (
            Unrelated,
            "The user commutes by train.",
            "The user enjoys baking sourdough bread.",
        ),
    ]
}

// ── Lexical overlap (prototype of the Phase 1 production helper) ───────────

/// Lowercased alphanumeric word tokens. Punctuation and whitespace split.
fn tokens(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Containment = |A ∩ B| / min(|A|, |B|). Robust to length differences — the
/// right signal for "one memory is a near-duplicate / extension of the other".
fn containment(a: &str, b: &str) -> f32 {
    let (ta, tb) = (tokens(a), tokens(b));
    let min_len = ta.len().min(tb.len());
    if min_len == 0 {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count();
    inter as f32 / min_len as f32
}

/// Jaccard = |A ∩ B| / |A ∪ B|. Reported alongside containment for contrast.
fn jaccard(a: &str, b: &str) -> f32 {
    let (ta, tb) = (tokens(a), tokens(b));
    let union = ta.union(&tb).count();
    if union == 0 {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count();
    inter as f32 / union as f32
}

/// Cosine similarity of two L2-normalised vectors = dot product.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

// ── Fixture loading (cross-crate to vault-embedding's bge-small fixture) ───

fn bge_fixture(name: &str) -> PathBuf {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vault-embedding/test-fixtures/bge-small-en-v1.5")
        .join(name);
    let p = std::fs::canonicalize(&p).unwrap_or_else(|e| {
        panic!("missing bge fixture {p:?} ({e}); run scripts/setup-dev-env.ps1")
    });
    p
}

#[cfg(target_os = "windows")]
const ORT_LIB: &str = "onnxruntime.dll";
#[cfg(target_os = "macos")]
const ORT_LIB: &str = "libonnxruntime.dylib";
#[cfg(target_os = "linux")]
const ORT_LIB: &str = "libonnxruntime.so";

#[derive(Default, Clone)]
struct Stats {
    cos: Vec<f32>,
    cont: Vec<f32>,
}

fn summarize(label: &str, s: &Stats) {
    let min = |v: &[f32]| v.iter().copied().fold(f32::INFINITY, f32::min);
    let max = |v: &[f32]| v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    println!(
        "  {label:<14} n={:<2}  cosine[min {:.3}  mean {:.3}  max {:.3}]  containment[min {:.3}  mean {:.3}  max {:.3}]",
        s.cos.len(),
        min(&s.cos), mean(&s.cos), max(&s.cos),
        min(&s.cont), mean(&s.cont), max(&s.cont),
    );
}

#[tokio::test]
#[ignore = "real-model calibration harness; run with --ignored --nocapture"]
async fn calibrate_dedup_thresholds() {
    let provider = BgeSmallProvider::open(
        &bge_fixture("model.onnx"),
        &bge_fixture("tokenizer.json"),
        &bge_fixture(ORT_LIB),
    )
    .expect("open bge-small fixture");

    let pairs = labeled_pairs();
    let mut by_class: std::collections::HashMap<Class, Stats> = std::collections::HashMap::new();

    println!("\n=== ADR-063 Phase 0 — dedup threshold calibration ===\n");
    println!(
        "{:<14} {:>7} {:>12} {:>9}   pair",
        "class", "cosine", "containment", "jaccard"
    );
    for &(class, a, b) in &pairs {
        let ea = provider.embed(a).await.expect("embed a");
        let eb = provider.embed(b).await.expect("embed b");
        let cos = cosine(&ea, &eb);
        let cont = containment(a, b);
        let jac = jaccard(a, b);
        println!(
            "{:<14} {:>7.3} {:>12.3} {:>9.3}   {:.40} | {:.40}",
            format!("{class:?}"),
            cos,
            cont,
            jac,
            a,
            b
        );
        let e = by_class.entry(class).or_default();
        e.cos.push(cos);
        e.cont.push(cont);
    }

    println!("\n--- per-class summary ---");
    for class in [
        Class::NearIdentical,
        Class::Complementary,
        Class::Contradictory,
        Class::Unrelated,
    ] {
        if let Some(s) = by_class.get(&class) {
            summarize(&format!("{class:?}"), s);
        }
    }

    // Separation analysis: the near-identical gate must sit ABOVE the highest
    // non-near-identical cosine/containment, and the complementary band floor
    // must sit ABOVE the unrelated ceiling. Print the gaps that justify the
    // chosen thresholds; the ADR records the picks.
    let ni = by_class
        .get(&Class::NearIdentical)
        .cloned()
        .unwrap_or_default();
    let comp = by_class
        .get(&Class::Complementary)
        .cloned()
        .unwrap_or_default();
    let unrel = by_class.get(&Class::Unrelated).cloned().unwrap_or_default();

    let ni_cos_min = ni.cos.iter().copied().fold(f32::INFINITY, f32::min);
    let ni_cont_min = ni.cont.iter().copied().fold(f32::INFINITY, f32::min);
    let comp_cos_max = comp.cos.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let comp_cont_max = comp.cont.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let unrel_cos_max = unrel.cos.iter().copied().fold(f32::NEG_INFINITY, f32::max);

    println!("\n--- separation (for threshold choice) ---");
    println!("  near-identical cosine floor      : {ni_cos_min:.3}");
    println!("  complementary  cosine ceiling    : {comp_cos_max:.3}");
    println!("  near-identical containment floor : {ni_cont_min:.3}");
    println!("  complementary  containment ceiling: {comp_cont_max:.3}");
    println!("  unrelated      cosine ceiling    : {unrel_cos_max:.3}");
    println!(
        "\n  → COS_HI must lie in ({comp_cos_max:.3}, {ni_cos_min:.3}]   (near-identical vs complementary, cosine)"
    );
    println!(
        "  → LEX_HI must lie in ({comp_cont_max:.3}, {ni_cont_min:.3}]  (near-identical vs complementary, containment)"
    );
    println!(
        "  → COS_LO (complementary-band floor) must lie above unrelated ceiling {unrel_cos_max:.3}\n"
    );

    // Soft sanity: near-identical should out-score unrelated on cosine. A hard
    // failure here means the embedder or fixtures are wrong, not a tuning miss.
    let ni_cos_mean = ni.cos.iter().sum::<f32>() / ni.cos.len().max(1) as f32;
    let unrel_cos_mean = unrel.cos.iter().sum::<f32>() / unrel.cos.len().max(1) as f32;
    assert!(
        ni_cos_mean > unrel_cos_mean,
        "near-identical mean cosine ({ni_cos_mean:.3}) must exceed unrelated ({unrel_cos_mean:.3})"
    );
}
