//! DIAGNOSTIC + CALIBRATION (Bug-2 investigation, 2026-06-01) — two `#[ignore]`
//! instruments for the read over-abstention fix. Neither is a gate; both print
//! numbers that ground the fix decision (run with `--ignored --nocapture`).
//!
//! 1. [`reranker_logits_for_fun_vs_hobby_queries`] — the original probe: does
//!    the production reranker judge a hobby fact relevant to a "for fun"
//!    question? Prints the production-reranker logit for one doc vs a spread of
//!    queries (the failing "for fun", the working "hobby", paraphrases, and the
//!    A6 no-signal "blood type" guard).
//!
//! 2. [`conformal_calibrate_reranker_floor`] — split-conformal calibration of
//!    the reranker relevance floor against the A7 labelled fixture
//!    (`read_quality_eval.json`). The production floor `RERANK_RELEVANCE_FLOOR =
//!    0.0` is a *guessed* absolute cut-off; the research (handoff 2026-06-01)
//!    found that is structurally wrong — reranker logits are only meaningful
//!    relative to the query, so a guessed global floor over-abstains. This
//!    instrument *measures* a data-derived threshold τ from our own labelled
//!    (query, relevant-fact) pairs, reports leave-one-out recall + guard-leakage
//!    per miss-rate knob α, and — decisively — prints whether the relevant and
//!    guard logits are separable by ONE threshold at all. If they interleave, no
//!    floor can fix Bug 2 and we need a sharper instruction / stronger reranker
//!    (handoff step 6); if they separate, τ is the fix.
//!
//! ```text
//! cargo test -p vault-embedding --test reranker_fun_diagnostic -- --ignored --nocapture
//! ```

#![cfg(not(target_os = "macos"))]

use std::path::PathBuf;

use vault_embedding::{Qwen3RerankerProvider, RerankProvider, QWEN3_RERANKER_INSTRUCT};

fn fixture(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-fixtures")
        .join(sub)
}

#[cfg(target_os = "windows")]
fn ort_lib() -> PathBuf {
    fixture("bge-small-en-v1.5/onnxruntime.dll")
}
#[cfg(target_os = "linux")]
fn ort_lib() -> PathBuf {
    fixture("bge-small-en-v1.5/libonnxruntime.so")
}

/// Open the production reranker over the on-disk fixture. Shared by both
/// instruments. Panics (test-only) if the fixture / ORT dylib is missing.
fn open_reranker() -> Qwen3RerankerProvider {
    Qwen3RerankerProvider::open(
        &fixture("qwen3-reranker-0.6b-seq-cls/model.onnx"),
        &fixture("qwen3-reranker-0.6b-seq-cls/tokenizer.json"),
        &ort_lib(),
    )
    .expect("open reranker")
}

/// Score a single (query, doc) pair with the production reranker → relevance
/// logit. One pair per call (the fixture is tiny; ~13 pairs total).
async fn score(reranker: &Qwen3RerankerProvider, query: &str, doc: &str) -> f32 {
    let scores = reranker
        .rerank(query, std::slice::from_ref(&doc.to_string()))
        .await
        .expect("rerank");
    scores[0]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real Qwen3-Reranker; run with --ignored --nocapture to read the logits"]
async fn reranker_logits_for_fun_vs_hobby_queries() {
    let reranker = open_reranker();

    let floor = reranker.relevance_floor();
    // Exact B-seed cello content (subject-LESS, NO marker — the marker prefix
    // contaminates the read). `score` goes through the PRODUCTION `rerank` path,
    // which applies DOC_SUBJECT_FRAME — confirms the live §7 A7/B3 reads surface
    // it post-ADR-064.
    let doc = "Plays the cello in a community orchestra on Sunday afternoons.";

    let queries = [
        "what does the user do for fun?", // A7 read 1
        "what music does the user play?", // A7 read 2 (worst pre-fix score)
        "what hobby does the user have?", // B3 hobby read
        "what does the user enjoy in their spare time?",
        "what does the user do to relax?",
        "what is the user's blood type?", // A6 no-signal guard — expect below floor
    ];

    println!("\n===== RERANKER LOGIT DIAGNOSTIC (Bug-2) =====");
    println!("doc: {doc:?}");
    println!("floor (logit): {floor:.4}  (>= floor = relevant, surfaces)\n");
    for q in queries {
        let s = score(&reranker, q, doc).await;
        let verdict = if s >= floor { "SURFACE" } else { "abstain" };
        println!("  [{verdict:>7}]  logit={s:+.4}   query={q:?}");
    }
    println!("=============================================\n");
}

// =============================================================================
// Conformal calibration of the reranker relevance floor (handoff step 1).
// =============================================================================

/// One labelled (query, fact) pair drawn from the A7 fixture.
struct Pair {
    /// `true` = known-relevant (a `must_surface` pair); `false` = a guard
    /// (a `must_exclude` distractor or a no-signal `abstain:true` case fact).
    relevant: bool,
    /// Short tag for the printout (`<case-id> <#idx>`).
    tag: String,
    query: String,
    doc: String,
}

/// The A7 fixture, parsed down to the fields the calibration consumes.
struct FixtureCase {
    id: String,
    query: String,
    seeds: Vec<String>,
    must_surface: Vec<String>,
    must_exclude: Vec<String>,
    abstain: bool,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vault-retrieval/tests/fixtures/read_quality_eval.json")
}

fn load_cases() -> Vec<FixtureCase> {
    let bytes = std::fs::read(fixture_path()).expect("read read_quality_eval.json");
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("parse fixture json");
    v["cases"]
        .as_array()
        .expect("fixture must have a `cases` array")
        .iter()
        .map(|c| {
            let str_vec = |key: &str| -> Vec<String> {
                c["expect"]
                    .get(key)
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            FixtureCase {
                id: c["id"].as_str().expect("case.id").to_string(),
                query: c["query"].as_str().expect("case.query").to_string(),
                seeds: c
                    .get("seed_memories")
                    .and_then(|x| x.as_array())
                    .map(|a| {
                        a.iter()
                            .map(|s| s["content"].as_str().unwrap_or_default().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                must_surface: str_vec("must_surface"),
                must_exclude: str_vec("must_exclude"),
                abstain: c["expect"]
                    .get("abstain")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            }
        })
        .collect()
}

/// Resolve a `"<case-id>#<idx>"` reference to the seed content within `case`.
/// In this fixture all `must_surface` / `must_exclude` keys reference their own
/// case, so the suffix index is sufficient.
fn resolve<'a>(case: &'a FixtureCase, key: &str) -> Option<&'a str> {
    let idx: usize = key.rsplit('#').next()?.parse().ok()?;
    case.seeds.get(idx).map(String::as_str)
}

/// Build the labelled (query, fact) pair set:
/// - **relevant**: each `must_surface` pair (the known query→answer match);
/// - **guard**: each `must_exclude` distractor + every fact of an `abstain:true`
///   case (no-signal / near-miss — must NOT clear the floor).
fn build_pairs(cases: &[FixtureCase]) -> Vec<Pair> {
    let mut pairs = Vec::new();
    for case in cases {
        for key in &case.must_surface {
            if let Some(doc) = resolve(case, key) {
                pairs.push(Pair {
                    relevant: true,
                    tag: format!(
                        "{} (surface {})",
                        case.id,
                        key.rsplit('#').next().unwrap_or("?")
                    ),
                    query: case.query.clone(),
                    doc: doc.to_string(),
                });
            }
        }
        for key in &case.must_exclude {
            if let Some(doc) = resolve(case, key) {
                pairs.push(Pair {
                    relevant: false,
                    tag: format!(
                        "{} (exclude {})",
                        case.id,
                        key.rsplit('#').next().unwrap_or("?")
                    ),
                    query: case.query.clone(),
                    doc: doc.to_string(),
                });
            }
        }
        if case.abstain {
            for (i, doc) in case.seeds.iter().enumerate() {
                pairs.push(Pair {
                    relevant: false,
                    tag: format!("{} (no-signal #{i})", case.id),
                    query: case.query.clone(),
                    doc: doc.clone(),
                });
            }
        }
    }
    pairs
}

/// Split-conformal LOWER threshold on the reranker logit. We surface a
/// (query, fact) pair when `logit >= τ`; a "miss" is a known-relevant pair
/// scoring below τ. Nonconformity `s_i = -logit_i` over the calibration
/// (relevant) logits; the conformal quantile is the `⌈(n+1)(1-α)⌉`-th smallest
/// `s_i`, and `τ = -that`. Equivalently, τ is the `⌈(n+1)(1-α)⌉`-th LARGEST
/// relevant logit. When `⌈(n+1)(1-α)⌉ > n` the calibration set is too small to
/// certify `(1-α)` coverage at a finite cut-off → `τ = -∞` (surface everything;
/// the honest small-n answer).
fn conformal_tau(relevant_logits: &[f32], alpha: f32) -> f32 {
    let n = relevant_logits.len();
    if n == 0 {
        return f32::NEG_INFINITY;
    }
    let rank = ((n as f32 + 1.0) * (1.0 - alpha)).ceil() as usize; // 1-based
    if rank > n {
        return f32::NEG_INFINITY;
    }
    let mut desc = relevant_logits.to_vec();
    desc.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal)); // DESC
    desc[rank - 1]
}

fn min_f32(v: &[f32]) -> f32 {
    v.iter().copied().fold(f32::INFINITY, f32::min)
}
fn max_f32(v: &[f32]) -> f32 {
    v.iter().copied().fold(f32::NEG_INFINITY, f32::max)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real Qwen3-Reranker over the A7 fixture; run with --ignored --nocapture for the τ scorecard"]
async fn conformal_calibrate_reranker_floor() {
    let reranker = open_reranker();
    let production_floor = reranker.relevance_floor();
    let cases = load_cases();
    let pairs = build_pairs(&cases);

    // Score every labelled pair → logit, keeping the relevant/guard split.
    let mut relevant: Vec<(String, f32)> = Vec::new();
    let mut guard: Vec<(String, f32)> = Vec::new();
    for p in &pairs {
        let logit = score(&reranker, &p.query, &p.doc).await;
        if p.relevant {
            relevant.push((p.tag.clone(), logit));
        } else {
            guard.push((p.tag.clone(), logit));
        }
    }

    let rel_logits: Vec<f32> = relevant.iter().map(|(_, l)| *l).collect();
    let guard_logits: Vec<f32> = guard.iter().map(|(_, l)| *l).collect();

    println!("\n================ RERANKER CONFORMAL CALIBRATION (Bug-2 / A7) ================");
    println!("production floor (guessed): logit {production_floor:+.4}");
    println!(
        "calibration set: {} relevant pairs, {} guard pairs\n",
        rel_logits.len(),
        guard_logits.len()
    );

    let mut rel_sorted = relevant.clone();
    rel_sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    println!("RELEVANT logits (ascending — these MUST clear τ to be surfaced):");
    for (tag, l) in &rel_sorted {
        let under = if *l < production_floor {
            "  (< prod floor 0)"
        } else {
            ""
        };
        println!("  logit={l:+.4}   {tag}{under}");
    }
    let mut guard_sorted = guard.clone();
    guard_sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    println!("\nGUARD logits (ascending — these MUST stay below τ):");
    for (tag, l) in &guard_sorted {
        let over = if *l >= production_floor {
            "  (>= prod floor 0 — would leak!)"
        } else {
            ""
        };
        println!("  logit={l:+.4}   {tag}{over}");
    }

    // The decisive question: separable by ONE threshold?
    let min_rel = min_f32(&rel_logits);
    let max_guard = max_f32(&guard_logits);
    let separable = min_rel > max_guard;
    println!("\n---- SEPARABILITY (the decisive question) ----");
    println!("  min(relevant) = {min_rel:+.4}");
    println!("  max(guard)    = {max_guard:+.4}");
    println!(
        "  gap = {:+.4}   → separable by ONE threshold? {}",
        min_rel - max_guard,
        if separable {
            "YES ✅"
        } else {
            "NO ❌ (interleaved — handoff step 6)"
        }
    );

    // Per-α conformal τ + leave-one-out recall + guard-leakage.
    println!("\n---- CONFORMAL τ (per miss-rate knob α) ----");
    println!("  α      τ(logit)    LOO-recall   LOO-guard-leak   note");
    for alpha in [0.05_f32, 0.10, 0.20, 0.30] {
        let tau = conformal_tau(&rel_logits, alpha);

        // Leave-one-out: hold out each relevant pair, recompute τ on the rest,
        // check (a) held-out clears τ (recall) and (b) no guard clears τ (leak).
        let n = rel_logits.len();
        let mut loo_recall_hits = 0usize;
        let mut loo_leak_total = 0usize;
        for j in 0..n {
            let rest: Vec<f32> = rel_logits
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != j)
                .map(|(_, l)| *l)
                .collect();
            let tau_j = conformal_tau(&rest, alpha);
            if rel_logits[j] >= tau_j {
                loo_recall_hits += 1;
            }
            loo_leak_total += guard_logits.iter().filter(|g| **g >= tau_j).count();
        }

        let tau_str = if tau.is_finite() {
            format!("{tau:+.4}")
        } else {
            "  -∞    ".to_string()
        };
        let note = if !tau.is_finite() {
            "surface-all (n too small for this α)"
        } else if guard_logits.iter().any(|g| *g >= tau) {
            "LEAKS a guard at this τ"
        } else {
            "clean on full set"
        };
        println!(
            "  {alpha:.2}   {tau_str}    {loo_recall_hits}/{n}          {loo_leak_total}                {note}",
        );
    }

    println!("\n---- VERDICT ----");
    if separable {
        // Recall-first (locked stance): the most permissive clean threshold is
        // anywhere in (max_guard, min_rel]. A midpoint maximises robustness to
        // unseen pairs on both sides.
        let midpoint = (min_rel + max_guard) / 2.0;
        println!("  ✅ Relevant and guard logits ARE separable. A single floor fixes Bug 2.");
        println!("     Recommended τ (gap midpoint): {midpoint:+.4}");
        println!("     (Most recall-aggressive clean τ = just above max(guard) = {max_guard:+.4};");
        println!(
            "      conformal α≈0.20 τ = {:+.4}.)",
            conformal_tau(&rel_logits, 0.20)
        );
        println!("     → set RERANK_RELEVANCE_FLOOR accordingly in crates/vault-embedding/src/reranker.rs");
    } else {
        println!("  ❌ Relevant and guard logits INTERLEAVE — NO single floor separates them.");
        println!("     Conformal cannot fix Bug 2 here. Escalate to handoff step 6:");
        println!("     a sharper 'v5' reranker instruction OR a stronger local reranker.");
        println!(
            "     (Same structural wall BGE-cosine hit; the 0.6B reranker may have a ceiling.)"
        );
    }
    println!("============================================================================\n");

    // Characterization harness — assert only that we scored every pair, so a
    // silent fixture-parse regression fails loudly. NOT a quality gate.
    assert_eq!(
        relevant.len() + guard.len(),
        pairs.len(),
        "every labelled pair must be scored exactly once"
    );
    assert!(
        !rel_logits.is_empty(),
        "fixture must yield ≥1 relevant pair"
    );
}

// =============================================================================
// Subject-prefix diagnostic (Bug-2 root cause, 2026-06-01).
//
// The conformal calibration showed the reranker separates the A7 fixture
// cleanly at floor 0 — yet the LIVE cello fact ("Plays the cello…", no subject)
// scores deeply negative for legit hobby queries. The distinguishing feature is
// the missing subject: A7 facts read "The user …". This probes both directions
// to confirm/refute that the reranker mis-scores subject-less fact fragments:
//   - ADD a subject to the cello fact → does it lift above floor 0?
//   - STRIP the subject from known-good A7 facts → do they collapse below 0?
// If the sign flips with the subject, the root cause is phrasing, not the floor.
// =============================================================================

/// One subject-prefix probe: the same `(query, fact)` relevance scored with and
/// without the "The user" subject on the fact.
struct SubjProbe {
    label: &'static str,
    query: &'static str,
    with_subject: &'static str,
    without_subject: &'static str,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real Qwen3-Reranker; run with --ignored --nocapture for the subject-prefix deltas"]
async fn subject_prefix_diagnostic() {
    let reranker = open_reranker();
    let floor = reranker.relevance_floor();

    let probes = [
        // ADD-subject direction — the live Bug-2 cello fact.
        SubjProbe {
            label: "ADD  cello / for-fun",
            query: "what does the user do for fun?",
            with_subject: "The user plays the cello in a community orchestra on Sunday afternoons.",
            without_subject: "Plays the cello in a community orchestra on Sunday afternoons.",
        },
        SubjProbe {
            label: "ADD  cello / music",
            query: "what music does the user play?",
            with_subject: "The user plays the cello in a community orchestra on Sunday afternoons.",
            without_subject: "Plays the cello in a community orchestra on Sunday afternoons.",
        },
        // STRIP-subject direction — A7 facts that scored strongly POSITIVE with
        // their subject in the conformal run.
        SubjProbe {
            label: "STRIP editor / visual-setup",
            query: "what kind of visual setup does the user use when coding?",
            with_subject:
                "The user works primarily in a dark-themed editor and finds light themes straining.",
            without_subject:
                "Works primarily in a dark-themed editor and finds light themes straining.",
        },
        SubjProbe {
            label: "STRIP lisbon / based",
            query: "where is the user based these days?",
            with_subject: "The user relocated to Lisbon in March 2026 for a fresh start.",
            without_subject: "Relocated to Lisbon in March 2026 for a fresh start.",
        },
        SubjProbe {
            label: "STRIP workflow / prefs",
            query: "what are the user's development workflow preferences?",
            with_subject:
                "The user prefers per-action commit approvals and four definition-of-done gates (build, test, clippy, fmt) in their development workflow.",
            without_subject:
                "Prefers per-action commit approvals and four definition-of-done gates (build, test, clippy, fmt) in the development workflow.",
        },
    ];

    println!("\n================ SUBJECT-PREFIX DIAGNOSTIC (Bug-2 root cause) ================");
    println!("floor (logit): {floor:+.4}  (>= floor = SURFACE)\n");
    println!("  probe                          with-subj   without-subj   Δ(with−without)   flip?");

    let mut add_lifts_above = 0usize;
    let mut add_total = 0usize;
    let mut strip_collapses_below = 0usize;
    let mut strip_total = 0usize;

    for p in &probes {
        let s_with = score(&reranker, p.query, p.with_subject).await;
        let s_without = score(&reranker, p.query, p.without_subject).await;
        let delta = s_with - s_without;
        // "flip" = the subject moves the fact across the surface/abstain line.
        let crossed = (s_with >= floor) != (s_without >= floor);
        let flip = if crossed {
            "YES — crosses floor"
        } else {
            "no"
        };
        println!(
            "  {:<28}  {:+8.4}    {:+8.4}     {:+8.4}        {}",
            p.label, s_with, s_without, delta, flip
        );

        if p.label.starts_with("ADD") {
            add_total += 1;
            if s_with >= floor && s_without < floor {
                add_lifts_above += 1;
            }
        } else {
            strip_total += 1;
            if s_without < floor && s_with >= floor {
                strip_collapses_below += 1;
            }
        }
    }

    println!("\n---- VERDICT ----");
    println!(
        "  ADD-subject lifted cello above floor: {add_lifts_above}/{add_total}  (subject-less was below, subjectful clears)"
    );
    println!(
        "  STRIP-subject collapsed A7 below floor: {strip_collapses_below}/{strip_total}  (subjectful cleared, subject-less drops)"
    );
    if add_lifts_above == add_total && strip_collapses_below == strip_total {
        println!("  ✅ ROOT CAUSE CONFIRMED: the missing subject — not the floor — drives the");
        println!("     mis-score. Fix lives read-side (give the reranker a subject-bearing");
        println!("     candidate), robust to uncontrolled stored prose.");
    } else {
        println!("  ⚠️ MIXED: the subject explains SOME but not all of the gap — read the deltas");
        println!("     above; a sharper instruction / stronger reranker may still be needed.");
    }
    println!("=============================================================================\n");

    // Characterization only — assert the probes ran, not any quality target.
    assert_eq!(add_total + strip_total, probes.len());
}

// =============================================================================
// Framing-variant sweep (Bug-2 fix selection, 2026-06-01).
//
// Root cause = the reranker mis-scores subject-less facts. This sweep measures
// the candidate READ-SIDE fixes across the FULL A7 set + the live cello fact:
//   - Variant A: frame the document so it always reads as a fact about the user
//     ("The user — {fact}" etc.) — measured test-side via the doc string.
//   - Variant B: strengthen the task instruction to treat subject-less docs as
//     facts about the user — measured via the `testing`-gated
//     `rerank_with_instruction` seam (no production-path change).
// Winning bar (hard): every relevant fact (incl. the subject-less cello) clears
// floor 0, AND every guard (incl. a no-signal query on the cello) stays below.
// =============================================================================

fn frame_identity(d: &str) -> String {
    d.to_string()
}
fn frame_user_dash(d: &str) -> String {
    format!("The user — {d}")
}
fn frame_user_colon(d: &str) -> String {
    format!("The user: {d}")
}
fn frame_about_user(d: &str) -> String {
    format!("About the user: {d}")
}

/// Production instruction + an explicit subject-less hint (Variant B).
const INSTRUCT_B: &str = "You are matching a question about a user to a personal fact. Answer yes only if the fact lets you answer the question with confidence. Same-topic facts that do not contain the answer must be answered no. The fact is always a statement about the user even when it omits the subject (e.g. 'Plays the cello' means the user plays the cello).";

struct Variant {
    name: &'static str,
    instruct: &'static str,
    frame: fn(&str) -> String,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "real Qwen3-Reranker framing sweep over A7 + cello; run with --ignored --nocapture"]
async fn framing_variant_sweep() {
    let reranker = open_reranker();
    let floor = reranker.relevance_floor();

    // A7 labelled pairs + the live cello fact (2 relevant phrasings + 1
    // no-signal guard) — the subject-less case the fix must rescue without
    // turning the read into a firehose.
    let cases = load_cases();
    let mut pairs = build_pairs(&cases);
    const CELLO: &str = "Plays the cello in a community orchestra on Sunday afternoons.";
    pairs.push(Pair {
        relevant: true,
        tag: "cello / for-fun".into(),
        query: "what does the user do for fun?".into(),
        doc: CELLO.into(),
    });
    pairs.push(Pair {
        relevant: true,
        tag: "cello / music".into(),
        query: "what music does the user play?".into(),
        doc: CELLO.into(),
    });
    pairs.push(Pair {
        relevant: false,
        tag: "cello / blood-type (no-signal)".into(),
        query: "what is the user's blood type?".into(),
        doc: CELLO.into(),
    });

    let variants = [
        Variant {
            name: "baseline   (prod instruct, no frame)",
            instruct: QWEN3_RERANKER_INSTRUCT,
            frame: frame_identity,
        },
        Variant {
            name: "A1 doc 'The user — '              ",
            instruct: QWEN3_RERANKER_INSTRUCT,
            frame: frame_user_dash,
        },
        Variant {
            name: "A2 doc 'The user: '               ",
            instruct: QWEN3_RERANKER_INSTRUCT,
            frame: frame_user_colon,
        },
        Variant {
            name: "A3 doc 'About the user: '         ",
            instruct: QWEN3_RERANKER_INSTRUCT,
            frame: frame_about_user,
        },
        Variant {
            name: "B  instruct+subjectless hint      ",
            instruct: INSTRUCT_B,
            frame: frame_identity,
        },
        Variant {
            name: "A2+B  frame + hint                ",
            instruct: INSTRUCT_B,
            frame: frame_user_colon,
        },
    ];

    let n_rel = pairs.iter().filter(|p| p.relevant).count();
    let n_guard = pairs.iter().filter(|p| !p.relevant).count();

    println!("\n================ FRAMING-VARIANT SWEEP (Bug-2 fix selection) ================");
    println!("floor (logit): {floor:+.4}   |   {n_rel} relevant pairs, {n_guard} guard pairs\n");
    println!("  variant                              rel≥floor  guard-leak  min(rel)  max(guard)  gap     cello?");

    let mut best: Option<(&str, f32)> = None; // (name, gap) among clean variants
    for v in &variants {
        let mut rel_scores: Vec<f32> = Vec::new();
        let mut guard_scores: Vec<f32> = Vec::new();
        let mut cello_rel: Vec<f32> = Vec::new();
        let mut cello_guard: f32 = f32::NAN;
        for p in &pairs {
            let doc = (v.frame)(&p.doc);
            let logit = reranker
                .rerank_with_instruction(v.instruct, &p.query, std::slice::from_ref(&doc))
                .await
                .expect("rerank_with_instruction")[0];
            if p.relevant {
                rel_scores.push(logit);
                if p.tag.starts_with("cello") {
                    cello_rel.push(logit);
                }
            } else {
                guard_scores.push(logit);
                if p.tag.starts_with("cello") {
                    cello_guard = logit;
                }
            }
        }

        let rel_pass = rel_scores.iter().filter(|s| **s >= floor).count();
        let guard_leak = guard_scores.iter().filter(|s| **s >= floor).count();
        let min_rel = min_f32(&rel_scores);
        let max_guard = max_f32(&guard_scores);
        let gap = min_rel - max_guard;
        let cello_ok = cello_rel.iter().all(|s| *s >= floor) && cello_guard < floor;
        let clean = rel_pass == n_rel && guard_leak == 0;
        if clean {
            match best {
                Some((_, g)) if g >= gap => {}
                _ => best = Some((v.name, gap)),
            }
        }
        println!(
            "  {}   {rel_pass}/{n_rel}      {guard_leak}        {min_rel:+7.3}   {max_guard:+7.3}   {gap:+6.3}  {}{}",
            v.name,
            if cello_ok { "✅" } else { "❌" },
            if clean { "  <- clean" } else { "" },
        );
        // Cello detail (the case that motivated the fix).
        println!(
            "        cello: for-fun={:+.3}  music={:+.3}  blood-type(guard)={:+.3}",
            cello_rel.first().copied().unwrap_or(f32::NAN),
            cello_rel.get(1).copied().unwrap_or(f32::NAN),
            cello_guard,
        );
    }

    println!("\n---- VERDICT ----");
    match best {
        Some((name, gap)) => {
            println!("  ✅ WINNER: {name}  (clean: all relevant clear floor, 0 guard leaks; gap={gap:+.3})");
            println!("     → bake this framing into reranker.rs::format_prompt (Variant A) and/or");
            println!("       QWEN3_RERANKER_INSTRUCT (Variant B), then green the regression test.");
        }
        None => {
            println!("  ❌ No variant cleanly separated relevant (incl. cello) from guards.");
            println!("     Escalate: stronger local reranker (handoff step 6).");
        }
    }
    println!("============================================================================\n");

    assert!(!pairs.is_empty());
}
