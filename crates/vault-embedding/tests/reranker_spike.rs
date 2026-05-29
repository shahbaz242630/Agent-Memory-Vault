//! SPIKE (meaning-similarity calibration workstream, 2026-05-29) — cross-encoder re-ranker.
//!
//! The read-quality baseline proved BGE-small's bi-encoder cosine CANNOT separate
//! relevant from irrelevant (real-answer + guard cosines interleave; 0.461 real <
//! 0.593 guard). A cross-encoder reads the (query, fact) pair TOGETHER and scores
//! actual relevance — the textbook fix. This spike measures whether
//! `ms-marco-MiniLM-L-6-v2` (22M, ONNX) separates the SAME fixture cases the
//! cosine could not.
//!
//! Self-contained: the 8 A7 cases are inlined here (vault-embedding has no
//! serde_json) so the spike is executable documentation with no cross-crate
//! fixture coupling. It reuses the bundled ONNX Runtime dylib from the BGE
//! fixtures and the downloaded re-ranker model.
//!
//! PASS = real-answer top-1 scores separate from guard scores (a threshold sits
//! between min(real) and max(guard)), including the "Lisbon" case that cosine
//! buried. Run:
//!
//! ```text
//! cargo test -p vault-embedding --test reranker_spike -- --ignored --nocapture
//! ```

#![cfg(not(target_os = "macos"))]

use std::path::PathBuf;

use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use tokenizers::Tokenizer;

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

/// One re-ranker forward pass over a (query, passage) pair → single relevance
/// logit (higher = more relevant). Mirrors `BgeSmallProvider`'s ort usage but
/// with pair tokenization + a `logits` classification output.
fn rerank(session: &mut Session, tok: &Tokenizer, query: &str, passage: &str) -> f32 {
    let mut enc = tok
        .encode((query, passage), true)
        .expect("tokenize (query, passage) pair");
    if enc.len() > 512 {
        enc.truncate(512, 0, tokenizers::TruncationDirection::Right);
    }
    let ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
    let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
    let types: Vec<i64> = enc.get_type_ids().iter().map(|&x| x as i64).collect();
    let n = ids.len();

    let input_ids = Tensor::from_array(([1_usize, n], ids)).expect("input_ids tensor");
    let attention_mask = Tensor::from_array(([1_usize, n], mask)).expect("attention_mask tensor");
    let token_type_ids = Tensor::from_array(([1_usize, n], types)).expect("token_type_ids tensor");

    let outputs = session
        .run(ort::inputs![
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
            "token_type_ids" => token_type_ids,
        ])
        .expect("session run");

    let logits = outputs
        .get("logits")
        .expect("re-ranker must expose a 'logits' output (BertForSequenceClassification)");
    let (_shape, data) = logits
        .try_extract_tensor::<f32>()
        .expect("extract logits tensor");
    data[0]
}

/// Two-input forward pass (`input_ids` + `attention_mask` only — no
/// `token_type_ids`) → single relevance logit. ModernBERT and the Qwen3
/// seq-cls reranker are both type-id-free (ModernBERT has no token-type
/// embeddings; Qwen3 is a decoder). Higher logit = more relevant.
fn score_2input(session: &mut Session, ids: Vec<i64>, mask: Vec<i64>) -> f32 {
    let n = ids.len();
    let input_ids = Tensor::from_array(([1_usize, n], ids)).expect("input_ids tensor");
    let attention_mask = Tensor::from_array(([1_usize, n], mask)).expect("attention_mask tensor");
    let outputs = session
        .run(ort::inputs![
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
        ])
        .expect("session run");
    let logits = outputs
        .get("logits")
        .expect("seq-cls reranker must expose a 'logits' output");
    // Logit dtype differs by model: gte-modernbert emits f32, Qwen3 emits f16.
    if let Ok((_shape, data)) = logits.try_extract_tensor::<f32>() {
        data[0]
    } else {
        let (_shape, data) = logits
            .try_extract_tensor::<half::f16>()
            .expect("extract logits tensor as f32 or f16");
        data[0].to_f32()
    }
}

/// gte-reranker-modernbert-base: classic cross-encoder pair encode
/// (`query [SEP] passage`), special tokens added, no token_type_ids.
fn gte_score(session: &mut Session, tok: &Tokenizer, query: &str, passage: &str) -> f32 {
    let mut enc = tok
        .encode((query, passage), true)
        .expect("tokenize (query, passage) pair");
    if enc.len() > 512 {
        enc.truncate(512, 0, tokenizers::TruncationDirection::Right);
    }
    let ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
    let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
    score_2input(session, ids, mask)
}

// Qwen3-Reranker-0.6B-seq-cls chat-template scaffolding (from the model card).
// The instruction is OUR task description — the instruction-aware lever that
// generic MS-MARCO rerankers lack. Per the card: write the instruct in English,
// tailored to the scenario.
const QWEN_PREFIX: &str = "<|im_start|>system\nJudge whether the Document meets the requirements based on the Query and the Instruct provided. Note that the answer can only be \"yes\" or \"no\".<|im_end|>\n<|im_start|>user\n";
const QWEN_SUFFIX: &str = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";
const QWEN_INSTRUCT: &str =
    "Given a conversational question about a user, retrieve the personal fact about the user that answers the question.";

fn qwen_format_instr(instruct: &str, query: &str, doc: &str) -> String {
    format!("{QWEN_PREFIX}<Instruct>: {instruct}\n<Query>: {query}\n<Document>: {doc}{QWEN_SUFFIX}")
}

/// Qwen3-Reranker-0.6B-seq-cls with a caller-supplied instruction (the lever).
/// Returns the raw logit (sigmoid → yes-probability; ranking is monotonic in
/// the logit so we compare logits directly).
fn qwen_score_instr(
    session: &mut Session,
    tok: &Tokenizer,
    instruct: &str,
    query: &str,
    doc: &str,
) -> f32 {
    let text = qwen_format_instr(instruct, query, doc);
    let mut enc = tok
        .encode(text.as_str(), false)
        .expect("tokenize qwen formatted string");
    if enc.len() > 1024 {
        enc.truncate(1024, 0, tokenizers::TruncationDirection::Right);
    }
    let ids: Vec<i64> = enc.get_ids().iter().map(|&x| x as i64).collect();
    let mask: Vec<i64> = enc.get_attention_mask().iter().map(|&x| x as i64).collect();
    score_2input(session, ids, mask)
}

/// Qwen3 scorer with the default task instruction.
fn qwen_score(session: &mut Session, tok: &Tokenizer, query: &str, doc: &str) -> f32 {
    qwen_score_instr(session, tok, QWEN_INSTRUCT, query, doc)
}

const QWEN_PAD_ID: i64 = 151643; // pad_token_id from config.json

/// Batched Qwen3 scoring: all `docs` for one query in a SINGLE forward pass.
/// Left-padded to the batch max length (the model uses padding_side="left", so
/// the last-token-pooled seq-cls head stays aligned). Returns one logit per doc.
fn qwen_score_batch(
    session: &mut Session,
    tok: &Tokenizer,
    instruct: &str,
    query: &str,
    docs: &[&str],
) -> Vec<f32> {
    let encs: Vec<Vec<i64>> = docs
        .iter()
        .map(|d| {
            let text = qwen_format_instr(instruct, query, d);
            tok.encode(text.as_str(), false)
                .expect("encode")
                .get_ids()
                .iter()
                .map(|&x| x as i64)
                .collect()
        })
        .collect();
    let maxlen = encs.iter().map(|e| e.len()).max().unwrap_or(0);
    let b = encs.len();
    let mut ids = vec![QWEN_PAD_ID; b * maxlen];
    let mut mask = vec![0_i64; b * maxlen];
    for (row, e) in encs.iter().enumerate() {
        let pad = maxlen - e.len(); // left pad
        for (j, &t) in e.iter().enumerate() {
            ids[row * maxlen + pad + j] = t;
            mask[row * maxlen + pad + j] = 1;
        }
    }
    let input_ids = Tensor::from_array(([b, maxlen], ids)).expect("ids tensor");
    let attention_mask = Tensor::from_array(([b, maxlen], mask)).expect("mask tensor");
    let outputs = session
        .run(ort::inputs!["input_ids" => input_ids, "attention_mask" => attention_mask])
        .expect("batched run");
    let logits = outputs.get("logits").expect("logits");
    if let Ok((_s, data)) = logits.try_extract_tensor::<f32>() {
        data.iter().take(b).copied().collect()
    } else {
        let (_s, data) = logits
            .try_extract_tensor::<half::f16>()
            .expect("f16 logits");
        data.iter().take(b).map(|v| v.to_f32()).collect()
    }
}

/// Confusion matrix at a logit threshold: a non-guard case "proceeds" when its
/// top-1 ≥ thr (recall); a guard case "false-answers" when its top-1 ≥ thr.
/// Returns (false_answers, false_abstains, recall_hits, real_total, guard_total).
fn confusion(rows: &[(bool, f32)], thr: f32) -> (usize, usize, usize, usize, usize) {
    let mut false_answers = 0;
    let mut false_abstains = 0;
    let mut recall_hits = 0;
    let mut real_total = 0;
    let mut guard_total = 0;
    for &(guard, top1) in rows {
        if guard {
            guard_total += 1;
            if top1 >= thr {
                false_answers += 1;
            }
        } else {
            real_total += 1;
            if top1 >= thr {
                recall_hits += 1;
            } else {
                false_abstains += 1;
            }
        }
    }
    (
        false_answers,
        false_abstains,
        recall_hits,
        real_total,
        guard_total,
    )
}

/// Shared separability read-out. `real` = top-1 score for each non-guard case
/// (should be HIGH), `guard` = top-1 for each must-abstain case (should be LOW).
fn report(label: &str, mut real: Vec<f32>, mut guard: Vec<f32>, wins: usize, total: usize) {
    real.sort_by(|a, b| a.partial_cmp(b).unwrap());
    guard.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let real_min = real.first().copied().unwrap_or(f32::NAN);
    let guard_max = guard.last().copied().unwrap_or(f32::NAN);
    println!("\n---------------- SEPARABILITY: {label} ----------------");
    println!(
        "  real-answer top-1 (should be HIGH): {}",
        real.iter()
            .map(|s| format!("{s:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  guard top-1       (should be LOW) : {}",
        guard
            .iter()
            .map(|s| format!("{s:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  min(real)={real_min:.3}   max(guard)={guard_max:.3}");
    println!(
        "  SEPARABLE by a single threshold? {}   (threshold ~{:.3} would work)",
        real_min > guard_max,
        (real_min + guard_max) / 2.0
    );
    println!("  intended answer beat distractors in {wins}/{total} non-guard cases (rank quality)");
    println!("==============================================\n");
}

/// (id, query, seeds, expect_abstain). Inlined from read_quality_eval.json.
/// seeds[0] is the intended answer for non-guard cases.
fn cases() -> Vec<(&'static str, &'static str, Vec<&'static str>, bool)> {
    let workflow = "The user prefers per-action commit approvals and four definition-of-done gates (build, test, clippy, fmt) in their development workflow.";
    let trail = "The user enjoys trail running in the foothills on weekends.";
    let editor =
        "The user works primarily in a dark-themed editor and finds light themes straining.";
    let lisbon = "The user relocated to Lisbon in March 2026 for a fresh start.";
    let keyboards = "The user collects vintage mechanical keyboards.";
    vec![
        (
            "A7-workflow-lexical",
            "what are the user's development workflow preferences?",
            vec![workflow, trail],
            false,
        ),
        (
            "A7-workflow-paraphrase",
            "how does the user like to sign off on changes and verify things are solid before merging?",
            vec![workflow, trail],
            false,
        ),
        (
            "A7-rank-displacement",
            "how does the user like to sign off on changes and verify quality before shipping?",
            vec![
                workflow,
                "The user spent the afternoon verifying that the read-after-write behaviour was fixed.",
                "The user wrote a long note on how coding agents hand off correctness and quality context between sessions.",
            ],
            false,
        ),
        (
            "A7-editor-medium",
            "what kind of visual setup does the user use when coding?",
            vec![editor],
            false,
        ),
        (
            "A7-editor-far",
            "is the user bothered by bright screens?",
            vec![editor],
            false,
        ),
        (
            "A7-multisession-far",
            "where is the user based these days?",
            vec![
                "The user relocated to Lisbon in March 2026 for a fresh start.",
                "The user collects vintage mechanical keyboards.",
            ],
            false,
        ),
        (
            "A7-guard-absent",
            "what is the user's preferred database indexing strategy?",
            vec!["The user prefers per-action commit approvals and four definition-of-done gates in their development workflow."],
            true,
        ),
        (
            "A7-guard-nearmiss",
            "how many years of professional experience does the user have?",
            vec!["The user prefers per-action commit approvals in their development workflow."],
            true,
        ),
        // ===== HARD Q21 class: topically-ADJACENT-but-WRONG (the ADR-057 deferral) =====
        // These guards share strong domain vocabulary with the query (high cosine),
        // but the held fact does NOT answer the specific attribute asked. A thin-margin
        // separator collapses here; the question is whether either reranker scores the
        // adjacent-but-non-answering fact LOW enough to abstain.
        (
            "H-editor-font-abstain",
            "what font family and size does the user use in their code editor?",
            vec![editor], // editor domain, but says nothing about fonts
            true,
        ),
        (
            "H-keyboard-switches-abstain",
            "does the user prefer linear or tactile mechanical keyboard switches?",
            vec![keyboards], // keyboards domain, but no switch preference stated
            true,
        ),
        (
            "H-location-origin-abstain",
            "what city did the user grow up in as a child?",
            vec![lisbon], // location domain, but relocation != childhood origin
            true,
        ),
        (
            "H-running-pb-abstain",
            "what is the user's fastest marathon finish time?",
            vec![trail], // running domain, but no race time stated
            true,
        ),
        // Answer PRESENT amid a same-domain adjacent distractor: must NOT abstain,
        // and the real answer (seed 0) must out-rank the topically-adjacent wrong fact.
        (
            "H-location-adjacent-present",
            "where is the user living now?",
            vec![
                lisbon,
                "The user frequently travels to Berlin and Tokyo for work conferences.",
            ],
            false,
        ),
        (
            "H-editor-adjacent-present",
            "does the user find bright displays uncomfortable?",
            vec![
                editor,
                "The user recently bought a high-brightness portable monitor for outdoor demos.",
            ],
            false,
        ),
    ]
}

#[test]
#[ignore = "spike: real cross-encoder; run with --ignored --nocapture"]
fn reranker_separability_spike() {
    let ort_lib = ort_lib();
    assert!(
        ort_lib.exists(),
        "missing ONNX Runtime dylib {ort_lib:?} — run scripts/setup-dev-env first"
    );
    ort::init_from(ort_lib.to_str().expect("ort lib path utf-8"))
        .commit()
        .expect("ort init");

    let model = fixture("ms-marco-minilm-l6-v2/model.onnx");
    let tok_path = fixture("ms-marco-minilm-l6-v2/tokenizer.json");
    assert!(model.exists(), "missing re-ranker model {model:?}");

    let mut session = Session::builder()
        .expect("session builder")
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .expect("opt level")
        .commit_from_file(&model)
        .expect("load re-ranker onnx");
    let tok = Tokenizer::from_file(&tok_path).expect("load tokenizer");

    // --- reference sanity check: the model card's OWN example with published
    // outputs. Unquantized ms-marco-MiniLM-L-6-v2 → relevant ~+8.8, irrelevant
    // ~-11.2. If we don't reproduce that, the spike's tokenization/IO is wrong
    // and any verdict on our data is meaningless. Verify the instrument first.
    let ref_q = "How many people live in Berlin?";
    let ref_pos = rerank(
        &mut session,
        &tok,
        ref_q,
        "Berlin has a population of 3,520,031 registered inhabitants in an area of 891.82 square kilometers.",
    );
    let ref_neg = rerank(
        &mut session,
        &tok,
        ref_q,
        "New York City is famous for the Metropolitan Museum of Art.",
    );
    println!("REFERENCE CHECK (model-card example; expect relevant ~+8.8, irrelevant ~-11.2):");
    println!("  relevant pair   = {ref_pos:.3}");
    println!("  irrelevant pair = {ref_neg:.3}");
    println!(
        "  -> integration {}\n",
        if ref_pos > 5.0 && ref_neg < -5.0 {
            "LOOKS CORRECT (proceed to read the A7 verdict below)"
        } else {
            "SUSPECT — likely a tokenization/IO bug; A7 numbers below are NOT a model verdict"
        }
    );

    let mut real_top1: Vec<f32> = Vec::new();
    let mut guard_top1: Vec<f32> = Vec::new();
    let mut answer_wins = 0usize;
    let mut answer_total = 0usize;

    println!("\n========= CROSS-ENCODER RE-RANKER SPIKE (ms-marco-MiniLM-L-6-v2) =========\n");

    for (id, query, seeds, guard) in cases() {
        let scores: Vec<f32> = seeds
            .iter()
            .map(|s| rerank(&mut session, &tok, query, s))
            .collect();
        let top1 = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let argmax = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);

        if guard {
            guard_top1.push(top1);
        } else {
            real_top1.push(top1);
            answer_total += 1;
            if argmax == 0 {
                answer_wins += 1; // the intended answer (seed 0) beat the distractors
            }
        }

        let scores_str = scores
            .iter()
            .map(|s| format!("{s:.3}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:<22} expect_abstain={:<5} top1={:>8.3}  answer_wins={}  [scores: {}]",
            id,
            guard,
            top1,
            if guard {
                "-".to_string()
            } else {
                (argmax == 0).to_string()
            },
            scores_str,
        );
    }

    real_top1.sort_by(|a, b| a.partial_cmp(b).unwrap());
    guard_top1.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let real_min = real_top1.first().copied().unwrap_or(f32::NAN);
    let guard_max = guard_top1.last().copied().unwrap_or(f32::NAN);

    println!("\n---------------- SEPARABILITY ----------------");
    println!(
        "  real-answer top-1 logits (should be HIGH): {}",
        real_top1
            .iter()
            .map(|s| format!("{s:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  guard top-1 logits       (should be LOW) : {}",
        guard_top1
            .iter()
            .map(|s| format!("{s:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  min(real)={real_min:.3}   max(guard)={guard_max:.3}");
    println!(
        "  SEPARABLE by a single threshold? {}   (a re-ranker threshold ~{:.2} would work)",
        real_min > guard_max,
        (real_min + guard_max) / 2.0
    );
    println!(
        "  intended answer beat distractors in {answer_wins}/{answer_total} non-guard cases (rank quality)"
    );
    println!("==============================================\n");
}

/// Load an ONNX session + tokenizer from a fixture dir, printing the model's
/// declared input/output names (ground truth for the IO wiring).
fn load(model_sub: &str, tok_sub: &str) -> (Session, Tokenizer) {
    let model = fixture(model_sub);
    let tok_path = fixture(tok_sub);
    assert!(model.exists(), "missing model {model:?}");
    assert!(tok_path.exists(), "missing tokenizer {tok_path:?}");
    let session = Session::builder()
        .expect("session builder")
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .expect("opt level")
        .commit_from_file(&model)
        .expect("load onnx");
    println!(
        "  model inputs : {:?}",
        session.inputs.iter().map(|i| &i.name).collect::<Vec<_>>()
    );
    println!(
        "  model outputs: {:?}",
        session.outputs.iter().map(|o| &o.name).collect::<Vec<_>>()
    );
    let tok = Tokenizer::from_file(&tok_path).expect("load tokenizer");
    (session, tok)
}

/// Run the 8 A7 cases through a model-specific scorer and report separability.
fn run_cases(label: &str, mut score: impl FnMut(&str, &str) -> f32) {
    let mut real_top1: Vec<f32> = Vec::new();
    let mut guard_top1: Vec<f32> = Vec::new();
    let mut wins = 0usize;
    let mut total = 0usize;
    println!("\n========= RE-RANKER SPIKE ({label}) =========\n");
    for (id, query, seeds, guard) in cases() {
        let scores: Vec<f32> = seeds.iter().map(|s| score(query, s)).collect();
        let top1 = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let argmax = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        if guard {
            guard_top1.push(top1);
        } else {
            real_top1.push(top1);
            total += 1;
            if argmax == 0 {
                wins += 1;
            }
        }
        let scores_str = scores
            .iter()
            .map(|s| format!("{s:.3}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{id:<22} expect_abstain={guard:<5} top1={top1:>8.3}  answer_wins={}  [scores: {scores_str}]",
            if guard { "-".to_string() } else { (argmax == 0).to_string() },
        );
    }
    report(label, real_top1, guard_top1, wins, total);
}

#[test]
#[ignore = "spike: real cross-encoder; run with --ignored --nocapture"]
fn gte_modernbert_separability_spike() {
    let ort_lib = ort_lib();
    assert!(ort_lib.exists(), "missing ONNX Runtime dylib {ort_lib:?}");
    ort::init_from(ort_lib.to_str().expect("ort lib path utf-8"))
        .commit()
        .expect("ort init");

    println!("=== gte-reranker-modernbert-base ===");
    let (mut session, tok) = load(
        "gte-reranker-modernbert-base/model.onnx",
        "gte-reranker-modernbert-base/tokenizer.json",
    );

    // Reference directional check (model card: relevant +2.1/+2.4, irrelevant -1.7).
    let ref_pos = gte_score(
        &mut session,
        &tok,
        "what is the capital of China?",
        "Beijing",
    );
    let ref_neg = gte_score(
        &mut session,
        &tok,
        "how to implement quick sort in python?",
        "The weather is nice today",
    );
    println!("REFERENCE (expect relevant > irrelevant): relevant={ref_pos:.3} irrelevant={ref_neg:.3} -> {}",
        if ref_pos > ref_neg { "OK" } else { "SUSPECT (IO/tokenization bug)" });

    run_cases("gte-reranker-modernbert-base", |q, d| {
        gte_score(&mut session, &tok, q, d)
    });
}

#[test]
#[ignore = "spike: real cross-encoder; run with --ignored --nocapture"]
fn qwen3_reranker_separability_spike() {
    let ort_lib = ort_lib();
    assert!(ort_lib.exists(), "missing ONNX Runtime dylib {ort_lib:?}");
    ort::init_from(ort_lib.to_str().expect("ort lib path utf-8"))
        .commit()
        .expect("ort init");

    println!("=== Qwen3-Reranker-0.6B-seq-cls (instruction-aware) ===");
    println!("  instruct: {QWEN_INSTRUCT}");
    let (mut session, tok) = load(
        "qwen3-reranker-0.6b-seq-cls/model.onnx",
        "qwen3-reranker-0.6b-seq-cls/tokenizer.json",
    );

    // Reference directional check (model card planets example).
    let ref_pos = qwen_score(
        &mut session,
        &tok,
        "Which planet is known as the Red Planet?",
        "Mars, known for its reddish appearance, is often referred to as the Red Planet.",
    );
    let ref_neg = qwen_score(
        &mut session,
        &tok,
        "Which planet is known as the Red Planet?",
        "Venus is often called Earth's twin because of its similar size and proximity.",
    );
    println!("REFERENCE (expect relevant > irrelevant): relevant={ref_pos:.3} irrelevant={ref_neg:.3} -> {}",
        if ref_pos > ref_neg { "OK" } else { "SUSPECT (IO/tokenization bug)" });

    run_cases("Qwen3-Reranker-0.6B-seq-cls", |q, d| {
        qwen_score(&mut session, &tok, q, d)
    });
}

/// Candidate instructions to close the two residual hard cases (the keyboards
/// adjacent-guard and the Lisbon recall miss). The lever is the instruct text.
const QWEN_INSTRUCTIONS: &[(&str, &str)] = &[
    (
        "v1-baseline",
        "Given a conversational question about a user, retrieve the personal fact about the user that answers the question.",
    ),
    (
        "v2-explicit-attribute",
        "Given a question asking about a specific attribute of the user, judge whether the document explicitly states that exact attribute. A document about a related topic that does not state the asked attribute is NOT a match.",
    ),
    (
        "v3-answerable",
        "Decide whether the document contains enough information to directly answer the user's question, including reasonable inference from a clearly-stated fact. Topically related documents that do not actually answer the question are not relevant.",
    ),
    (
        "v4-strict-yesno",
        "You are matching a question about a user to a personal fact. Answer yes only if the fact lets you answer the question with confidence. Same-topic facts that do not contain the answer must be answered no.",
    ),
];

#[test]
#[ignore = "spike: instruction tuning sweep; run with --ignored --nocapture"]
fn qwen3_instruction_tuning_spike() {
    let ort_lib = ort_lib();
    assert!(ort_lib.exists(), "missing ONNX Runtime dylib {ort_lib:?}");
    ort::init_from(ort_lib.to_str().expect("ort lib path utf-8"))
        .commit()
        .expect("ort init");
    let (mut session, tok) = load(
        "qwen3-reranker-0.6b-seq-cls/model.onnx",
        "qwen3-reranker-0.6b-seq-cls/tokenizer.json",
    );

    println!("\n######## QWEN3 INSTRUCTION TUNING (confusion @ logit 0.0) ########");
    println!("Goal: 0 false-answers, max recall. Watch the 2 residual cases:");
    println!(
        "  GUARD H-keyboard-switches-abstain (want LOW) + REAL A7-multisession-far (want HIGH)\n"
    );

    for (name, instr) in QWEN_INSTRUCTIONS {
        let mut rows: Vec<(bool, f32)> = Vec::new();
        let mut wins = 0usize;
        let mut total = 0usize;
        let mut kbd = f32::NAN;
        let mut lisbon = f32::NAN;
        for (id, query, seeds, guard) in cases() {
            let scores: Vec<f32> = seeds
                .iter()
                .map(|s| qwen_score_instr(&mut session, &tok, instr, query, s))
                .collect();
            let top1 = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let argmax = scores
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, _)| i)
                .unwrap_or(0);
            if !guard {
                total += 1;
                if argmax == 0 {
                    wins += 1;
                }
            }
            if id == "H-keyboard-switches-abstain" {
                kbd = top1;
            }
            if id == "A7-multisession-far" {
                lisbon = top1;
            }
            rows.push((guard, top1));
        }
        let (fa, fab, hits, rt, gt) = confusion(&rows, 0.0);
        println!(
            "[{name:<22}] false_answers={fa}/{gt}  recall={hits}/{rt} (false_abstain={fab})  ranking={wins}/{total}  | kbd_guard={kbd:+.3} lisbon_real={lisbon:+.3}"
        );
    }
    println!("#################################################################\n");
}

#[test]
#[ignore = "spike: int8 quality + latency; run with --ignored --nocapture"]
fn qwen3_int8_spike() {
    use std::time::Instant;
    let ort_lib = ort_lib();
    assert!(ort_lib.exists(), "missing ONNX Runtime dylib {ort_lib:?}");
    ort::init_from(ort_lib.to_str().expect("ort lib path utf-8"))
        .commit()
        .expect("ort init");
    println!("=== Qwen3-Reranker-0.6B-seq-cls INT8 (dynamic QInt8) ===");
    let (mut session, tok) = load(
        "qwen3-reranker-0.6b-seq-cls-int8/model.onnx",
        "qwen3-reranker-0.6b-seq-cls-int8/tokenizer.json",
    );

    // Quality: does int8 keep the v4 bar (0 false-answers, 8/8 recall)?
    println!("\n-- QUALITY (confusion @ logit 0.0) — must hold 0 false-answers, max recall --");
    for (name, instr) in &[QWEN_INSTRUCTIONS[0], QWEN_INSTRUCTIONS[3]] {
        let mut rows: Vec<(bool, f32)> = Vec::new();
        let mut real: Vec<f32> = Vec::new();
        let mut guard: Vec<f32> = Vec::new();
        let mut wins = 0usize;
        let mut total = 0usize;
        for (_id, query, seeds, g) in cases() {
            let scores: Vec<f32> = seeds
                .iter()
                .map(|s| qwen_score_instr(&mut session, &tok, instr, query, s))
                .collect();
            let top1 = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let argmax = scores
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(i, _)| i)
                .unwrap_or(0);
            if g {
                guard.push(top1);
            } else {
                real.push(top1);
                total += 1;
                if argmax == 0 {
                    wins += 1;
                }
            }
            rows.push((g, top1));
        }
        let (fa, fab, hits, rt, gt) = confusion(&rows, 0.0);
        let real_min = real.iter().copied().fold(f32::INFINITY, f32::min);
        let guard_max = guard.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        println!(
            "[{name:<22}] false_answers={fa}/{gt}  recall={hits}/{rt} (false_abstain={fab})  ranking={wins}/{total}  | min(real)={real_min:+.3} max(guard)={guard_max:+.3} separable={}",
            real_min > guard_max
        );
    }

    // Latency
    let q = "does the user find bright displays uncomfortable?";
    let d = "The user works primarily in a dark-themed editor and finds light themes straining.";
    for _ in 0..3 {
        let _ = qwen_score(&mut session, &tok, q, d);
    }
    let t = Instant::now();
    for _ in 0..30 {
        let _ = qwen_score(&mut session, &tok, q, d);
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / 30.0;
    println!(
        "\n-- LATENCY: Qwen3-int8 = {per:.1} ms/rerank (serial)  | top-5 read ≈ {:.0} ms --",
        per * 5.0
    );
}

#[test]
#[ignore = "spike: per-rerank latency; run with --ignored --nocapture"]
fn reranker_latency_spike() {
    use std::time::Instant;
    let ort_lib = ort_lib();
    assert!(ort_lib.exists(), "missing ONNX Runtime dylib {ort_lib:?}");
    ort::init_from(ort_lib.to_str().expect("ort lib path utf-8"))
        .commit()
        .expect("ort init");

    let q = "does the user find bright displays uncomfortable?";
    let d = "The user works primarily in a dark-themed editor and finds light themes straining.";
    let iters = 30;

    println!("\n######## PER-RERANK LATENCY (1 query×doc pair, {iters} iters, debug build, ORT Level3) ########");
    println!("NOTE: ORT inference is native C++ (release-grade) regardless of Rust debug profile;");
    println!("the dominant matmul cost is representative. Intra-op threads = ORT default (~physical cores).\n");

    // gte-modernbert (150M, f32)
    {
        let (mut session, tok) = load(
            "gte-reranker-modernbert-base/model.onnx",
            "gte-reranker-modernbert-base/tokenizer.json",
        );
        for _ in 0..3 {
            let _ = gte_score(&mut session, &tok, q, d);
        }
        let t = Instant::now();
        for _ in 0..iters {
            let _ = gte_score(&mut session, &tok, q, d);
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;
        println!("  gte-reranker-modernbert-base (150M f32): {per:.1} ms/rerank");
    }

    // Qwen3-Reranker (600M, f16)
    {
        let (mut session, tok) = load(
            "qwen3-reranker-0.6b-seq-cls/model.onnx",
            "qwen3-reranker-0.6b-seq-cls/tokenizer.json",
        );
        for _ in 0..3 {
            let _ = qwen_score(&mut session, &tok, q, d);
        }
        let t = Instant::now();
        for _ in 0..iters {
            let _ = qwen_score(&mut session, &tok, q, d);
        }
        let per = t.elapsed().as_secs_f64() * 1000.0 / iters as f64;
        println!("  Qwen3-Reranker-0.6B-seq-cls  (600M f16): {per:.1} ms/rerank (serial)");
        println!("  -> top-K=10 read, SERIAL: ≈ {:.0} ms", per * 10.0);

        // Batched: all K candidates in ONE forward pass (the real read shape).
        let docs: Vec<&str> = vec![
            "The user works primarily in a dark-themed editor and finds light themes straining.",
            "The user enjoys trail running in the foothills on weekends.",
            "The user relocated to Lisbon in March 2026 for a fresh start.",
            "The user collects vintage mechanical keyboards.",
            "The user prefers per-action commit approvals and four definition-of-done gates.",
            "The user frequently travels to Berlin and Tokyo for work conferences.",
            "The user recently bought a high-brightness portable monitor for outdoor demos.",
            "The user wrote a long note on how coding agents hand off context between sessions.",
            "The user spent the afternoon verifying that the read-after-write behaviour was fixed.",
            "The user drinks black coffee every morning.",
        ];
        for _ in 0..3 {
            let _ = qwen_score_batch(&mut session, &tok, QWEN_INSTRUCT, q, &docs);
        }
        let bt = Instant::now();
        let batch_iters = 10;
        for _ in 0..batch_iters {
            let _ = qwen_score_batch(&mut session, &tok, QWEN_INSTRUCT, q, &docs);
        }
        let per_batch = bt.elapsed().as_secs_f64() * 1000.0 / batch_iters as f64;
        println!(
            "  -> top-K=10 read, BATCHED (1 pass, K={}): ≈ {:.0} ms ({:.1}× faster than serial)",
            docs.len(),
            per_batch,
            (per * docs.len() as f64) / per_batch
        );
    }
    println!("###############################################################################\n");
}
