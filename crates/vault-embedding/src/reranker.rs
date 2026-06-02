//! `Qwen3RerankerProvider` — cross-encoder relevance reranker backed by
//! `Qwen3-Reranker-0.6B` (seq-cls form) on ONNX Runtime via `ort` 2.x.
//!
//! ## Why a reranker (the model-fit finding, 2026-05-29)
//!
//! BGE-small's bi-encoder cosine cannot separate relevant from irrelevant on
//! our data shape (conversational question → short first-person fact): real
//! and guard cosines interleave. A bge-reranker/ms-marco cross-encoder also
//! fails (out-of-distribution), and a gte-modernbert cross-encoder separates
//! easy cases but collapses on topically-adjacent "wrong-attribute" traps.
//! Measured against the `reranker_spike` instrument, **Qwen3-Reranker-0.6B**
//! (an *instruction-aware* cross-encoder) given the [`QWEN3_RERANKER_INSTRUCT`]
//! task instruction is the only model that cleanly separates the hardened set
//! (0 false-answers, full recall, perfect ranking) at a logit-0 cutoff.
//!
//! ## Mechanism
//!
//! A cross-encoder reads `(query, document)` TOGETHER in one forward pass and
//! emits a single relevance logit (sigmoid → yes-probability). The seq-cls
//! conversion exposes that as the `logits` output directly (no LM-head / vocab
//! decode). Inputs are `input_ids` + `attention_mask` only (Qwen3 is a decoder
//! — no `token_type_ids`). The instruction is baked into the chat-template
//! prompt; the model emits a higher logit when the document answers the query.
//!
//! ## Integration
//!
//! The read pipeline ([`vault_retrieval::StructuredReadPipeline`]) reranks the
//! top retrieved candidates, keeps those scoring ≥ [`RERANK_NO_SIGNAL_FLOOR`]
//! (recall-first: a coarse no-signal cut, NOT a precision gate — ADR-066), and
//! re-sorts by reranker score. CPU cost ≈ 0.39 s per candidate (f16); GPU
//! sub-second.

use crate::integrity::{
    verify_file_sha256, QWEN3_RERANKER_MODEL_SHA256, QWEN3_RERANKER_TOKENIZER_SHA256,
};
use crate::ort_init::ensure_ort_initialised;
use async_trait::async_trait;
use ort::session::{builder::GraphOptimizationLevel, Session};
use ort::value::Tensor;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokenizers::Tokenizer;
use vault_core::{VaultError, VaultResult};

/// The task instruction handed to the reranker (the **"v5 synonym-aware"**
/// variant). It tells the model to answer *yes* across a synonym/paraphrase gap
/// (food↔cuisine) while still answering *no* for same-topic facts that don't
/// contain the answer.
///
/// History + honest scope (ADR-066, 2026-06-02): the v4 "strict-yes/no" variant
/// (2026-05-29) closed wrong-attribute traps but was TOO strict on
/// zero-token-overlap synonym leaps (real `food↔cuisine` scored +0.07, below a
/// leaking guard). v5 substantially lifts synonym RECALL (food↔cuisine +0.07 →
/// +2.15; allergy→avoid, settled→home recovered on the 4B). BUT measured over the
/// *grown* 16-case fixture, v5 does NOT fully separate real from guard by a
/// single precision floor — the hardest real cases and the most ambiguous guards
/// still interleave (gap −0.33). That is a 0.6B model-capacity ceiling, not an
/// instruction bug. So we do NOT chase a precision floor: the read goes
/// **recall-first** (ADR-066) — the reranker re-orders candidates and applies
/// only the coarse [`RERANK_NO_SIGNAL_FLOOR`] (drop deep no-signal junk), and the
/// calling agent makes the fine relevance call. Reproduce the measurements via
/// `reranker_fun_diagnostic.rs::{conformal_calibrate_reranker_floor,synonym_gap_fix_sweep}`.
pub const QWEN3_RERANKER_INSTRUCT: &str =
    "You are matching a question about a user to a personal fact. Answer yes if the fact answers the question, even when the question and the fact use different words for the same idea (synonyms or paraphrases). Answer no only when the fact does not actually contain the answer, even if it is on the same topic.";

/// **No-signal floor** on the reranker logit (ADR-066, recall-first read,
/// 2026-06-02). A candidate scoring below this is treated as *no signal at all*
/// and dropped; everything at/above is returned to the calling agent, which
/// makes the fine relevance call.
///
/// This is deliberately NOT a precision floor. The prior 0.0 floor (sigmoid 0.5,
/// "more likely yes than no") tried to make the 0.6B reranker the relevance
/// *authority* and caused over-abstention — it discarded real facts on
/// synonym/paraphrase leaps that the reranker scored slightly negative. Measured
/// 2026-06-02 over the grown A7 fixture (v5 instruction): genuine should-surface
/// facts score **≥ −0.35** while true no-signal queries top out at **≤ −4.4** —
/// a clean ~4-logit gap. **−2.5** sits in that gap (≈2.1 below the worst real
/// fact, ≈1.9 above the strongest no-signal), recall-leaning per the locked
/// recall-&gt;precision stance. Calibrated on a small fixture → treated as a
/// starting value to confirm in live dogfood, not a final constant.
pub const RERANK_NO_SIGNAL_FLOOR: f32 = -2.5;

/// Subject frame prepended to every candidate document before the production
/// reranker scores it (Bug-2 fix, 2026-06-01). The 0.6B reranker mis-scores
/// subject-LESS stored facts: a bare "Plays the cello in a community orchestra"
/// is read as not-about-the-user and rejected even for the near-literal "what
/// music does the user play?" (logit −5.2). The agent stores uncontrolled prose,
/// so the fix lives on the read side — prepend an explicit subject so floor 0
/// separates subject-bearing AND subject-less facts. Measured winner of the
/// A/B framing sweep (2026-06-01): "The user — " gave 8/8 relevant above floor,
/// 0 guard leaks, the widest separation gap (+1.69 logits) — beating "The user:
/// " (fragile +0.18 on cello/music), "About the user: " (broke 2 cases), and
/// every instruction-only variant (which leaked a guard). Reproduce via
/// `reranker_fun_diagnostic.rs::framing_variant_sweep`. A change here re-scores
/// every read — re-break the [`doc_subject_frame_is_pinned`] test consciously.
const DOC_SUBJECT_FRAME: &str = "The user — ";

/// Per-document character cap applied BEFORE chat-template wrapping. Keeps the
/// prompt's system prefix + assistant suffix intact (truncating the formatted
/// string would corrupt the last-token-pooled seq-cls signal) and bounds
/// latency. Facts are short; the store-whole 100 KB ceiling is the rare case.
const DOC_CHAR_CAP: usize = 2000;

/// Pad token id (`pad_token_id` from the model's config.json). Used for
/// left-padding a batch to a uniform length — Qwen uses left padding, which
/// keeps the last (pooled) token aligned across rows.
const QWEN_PAD_ID: i64 = 151643;

const QWEN_PREFIX: &str = "<|im_start|>system\nJudge whether the Document meets the requirements based on the Query and the Instruct provided. Note that the answer can only be \"yes\" or \"no\".<|im_end|>\n<|im_start|>user\n";
const QWEN_SUFFIX: &str = "<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n";

/// Format one `(instruct, query, document)` triple into the reranker's
/// chat-template prompt. Production scores via [`RerankProvider::rerank`], which
/// applies the [`DOC_SUBJECT_FRAME`] to `doc` and passes
/// [`QWEN3_RERANKER_INSTRUCT`]; the `testing`-gated
/// [`Qwen3RerankerProvider::rerank_with_instruction`] seam passes `doc` raw so
/// the framing sweep can measure variants.
fn format_prompt_with(instruct: &str, query: &str, doc: &str) -> String {
    let doc = if doc.chars().count() > DOC_CHAR_CAP {
        doc.chars().take(DOC_CHAR_CAP).collect()
    } else {
        doc.to_string()
    };
    format!("{QWEN_PREFIX}<Instruct>: {instruct}\n<Query>: {query}\n<Document>: {doc}{QWEN_SUFFIX}")
}

/// Abstract relevance reranker. Scores `(query, document)` relevance with a
/// cross-encoder; higher score = more relevant. Consumed by the read pipeline
/// to gate + re-rank retrieved candidates.
#[async_trait]
pub trait RerankProvider: Send + Sync {
    /// Score each document against the query in a single batched forward pass.
    /// Returns one score per input document, in input order. An empty `docs`
    /// slice returns an empty vec without running inference.
    ///
    /// # Errors
    ///
    /// [`VaultError::Embedding`] on tokenisation / inference / extraction
    /// failure.
    async fn rerank(&self, query: &str, docs: &[String]) -> VaultResult<Vec<f32>>;

    /// The relevance floor: documents scoring below this are not relevant.
    fn relevance_floor(&self) -> f32;
}

/// ONNX-Runtime-backed reranker for Qwen3-Reranker-0.6B (seq-cls).
///
/// Construct via [`Qwen3RerankerProvider::open`]; thereafter use as a
/// [`RerankProvider`]. The [`Session`] is wrapped in `Arc<Mutex<Session>>`
/// (ort's `Session::run` takes `&mut self`); concurrent `rerank` calls
/// serialise through the mutex inside `spawn_blocking` — matching V0.2's
/// handful-of-reads-per-second throughput.
pub struct Qwen3RerankerProvider {
    session: Arc<Mutex<Session>>,
    tokenizer: Arc<Tokenizer>,
}

impl Qwen3RerankerProvider {
    /// Open the provider: verify model + tokenizer integrity (ADR-020 —
    /// fatal on mismatch, no fallback), share the process-global ort init,
    /// load the ONNX session + tokenizer.
    ///
    /// # Errors
    ///
    /// - [`VaultError::ModelIntegrityFailed`] on SHA-256 mismatch.
    /// - [`VaultError::Embedding`] on ort init / session / tokenizer load.
    /// - [`VaultError::Io`] on file-read failure during integrity check.
    #[tracing::instrument(level = "info", skip_all, fields(
        model = %model_path.display(),
        tokenizer = %tokenizer_path.display(),
    ))]
    pub fn open(
        model_path: &Path,
        tokenizer_path: &Path,
        ort_lib_path: &Path,
    ) -> VaultResult<Self> {
        verify_file_sha256(model_path, QWEN3_RERANKER_MODEL_SHA256, "reranker-model")?;
        verify_file_sha256(
            tokenizer_path,
            QWEN3_RERANKER_TOKENIZER_SHA256,
            "reranker-tokenizer",
        )?;

        ensure_ort_initialised(ort_lib_path)?;

        let session = Session::builder()
            .map_err(|e| VaultError::Embedding(format!("ort session builder: {e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| VaultError::Embedding(format!("ort optimization level: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| VaultError::Embedding(format!("ort load reranker model: {e}")))?;

        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VaultError::Embedding(format!("reranker tokenizer load: {e}")))?;

        tracing::info!("Qwen3RerankerProvider opened (integrity OK; session + tokenizer loaded)");

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer: Arc::new(tokenizer),
        })
    }

    /// Test-only seam: rerank with a caller-supplied task instruction so the
    /// Bug-2 framing sweep (2026-06-01) can measure instruction variants
    /// without mutating the production path. Gated on the `testing` feature so
    /// it never reaches the production surface. Production reranking goes through
    /// the [`RerankProvider::rerank`] trait method (fixed
    /// [`QWEN3_RERANKER_INSTRUCT`]).
    ///
    /// # Errors
    ///
    /// As [`RerankProvider::rerank`].
    #[cfg(feature = "testing")]
    pub async fn rerank_with_instruction(
        &self,
        instruct: &str,
        query: &str,
        docs: &[String],
    ) -> VaultResult<Vec<f32>> {
        self.rerank_inner(instruct, query, docs).await
    }

    /// Core rerank: tokenise `(instruct, query, doc)` prompts, left-pad to a
    /// uniform batch, run one forward pass, extract the seq-cls logit per row.
    /// The production trait method delegates here with [`QWEN3_RERANKER_INSTRUCT`].
    #[tracing::instrument(level = "debug", skip(self, instruct, query, docs), fields(n_docs = docs.len()))]
    async fn rerank_inner(
        &self,
        instruct: &str,
        query: &str,
        docs: &[String],
    ) -> VaultResult<Vec<f32>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }

        // Tokenise each formatted prompt on the async runtime (cheap). The
        // chat-template control tokens are explicit, so add_special_tokens=false.
        let mut rows: Vec<Vec<i64>> = Vec::with_capacity(docs.len());
        for doc in docs {
            let prompt = format_prompt_with(instruct, query, doc);
            let enc = self
                .tokenizer
                .encode(prompt.as_str(), false)
                .map_err(|e| VaultError::Embedding(format!("reranker tokenize: {e}")))?;
            rows.push(enc.get_ids().iter().map(|&x| x as i64).collect());
        }

        let batch = rows.len();
        let maxlen = rows.iter().map(Vec::len).max().unwrap_or(0);
        // Left-pad each row to maxlen (Qwen padding_side="left").
        let mut ids = vec![QWEN_PAD_ID; batch * maxlen];
        let mut mask = vec![0_i64; batch * maxlen];
        for (r, row) in rows.iter().enumerate() {
            let pad = maxlen - row.len();
            for (j, &tok) in row.iter().enumerate() {
                ids[r * maxlen + pad + j] = tok;
                mask[r * maxlen + pad + j] = 1;
            }
        }

        let session = Arc::clone(&self.session);
        tokio::task::spawn_blocking(move || -> VaultResult<Vec<f32>> {
            let input_ids = Tensor::from_array(([batch, maxlen], ids))
                .map_err(|e| VaultError::Embedding(format!("input_ids tensor: {e}")))?;
            let attention_mask = Tensor::from_array(([batch, maxlen], mask))
                .map_err(|e| VaultError::Embedding(format!("attention_mask tensor: {e}")))?;

            let mut guard = session
                .lock()
                .map_err(|e| VaultError::Embedding(format!("session lock poisoned: {e}")))?;
            let outputs = guard
                .run(ort::inputs![
                    "input_ids" => input_ids,
                    "attention_mask" => attention_mask,
                ])
                .map_err(|e| VaultError::Embedding(format!("reranker session run: {e}")))?;

            let logits = outputs
                .get("logits")
                .ok_or_else(|| VaultError::Embedding("reranker missing 'logits' output".into()))?;

            // seq-cls head emits one logit per row (`[batch, 1]`). The model is
            // f16; older f32 exports are handled too. Take the first `batch`
            // values in row order.
            let scores: Vec<f32> = if let Ok((_s, data)) = logits.try_extract_tensor::<f32>() {
                data.iter().take(batch).copied().collect()
            } else {
                let (_s, data) = logits
                    .try_extract_tensor::<half::f16>()
                    .map_err(|e| VaultError::Embedding(format!("extract logits: {e}")))?;
                data.iter().take(batch).map(|v| v.to_f32()).collect()
            };

            if scores.len() != batch {
                return Err(VaultError::Embedding(format!(
                    "reranker returned {} scores for {} docs",
                    scores.len(),
                    batch
                )));
            }
            Ok(scores)
        })
        .await
        .map_err(|e| VaultError::Embedding(format!("spawn_blocking join: {e}")))?
    }
}

#[async_trait]
impl RerankProvider for Qwen3RerankerProvider {
    async fn rerank(&self, query: &str, docs: &[String]) -> VaultResult<Vec<f32>> {
        // Bug-2 fix (2026-06-01): frame each candidate with an explicit subject
        // before scoring so the reranker scores subject-less stored facts
        // correctly. Measured winner of the A/B framing sweep — see
        // [`DOC_SUBJECT_FRAME`].
        let framed: Vec<String> = docs
            .iter()
            .map(|d| format!("{DOC_SUBJECT_FRAME}{d}"))
            .collect();
        self.rerank_inner(QWEN3_RERANKER_INSTRUCT, query, &framed)
            .await
    }

    fn relevance_floor(&self) -> f32 {
        RERANK_NO_SIGNAL_FLOOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_signal_floor_is_pinned() {
        // ADR-066 (recall-first read, 2026-06-02): the reranker gate is a coarse
        // NO-SIGNAL floor, not a precision gate. -2.5 sits in the measured gap
        // between the worst real should-surface fact (≥ -0.35) and the strongest
        // true no-signal query (≤ -4.4) over the grown A7 fixture under v5. A
        // future re-calibration (esp. after live dogfood) MUST break this
        // consciously, not drift.
        assert_eq!(RERANK_NO_SIGNAL_FLOOR, -2.5);
    }

    #[test]
    fn doc_subject_frame_is_pinned() {
        // Bug-2 fix (2026-06-01): the measured winning subject frame. Every
        // production reranker score depends on it — re-break only after a
        // re-sweep (reranker_fun_diagnostic.rs::framing_variant_sweep).
        assert_eq!(DOC_SUBJECT_FRAME, "The user — ");
    }

    #[test]
    fn rerank_frames_each_doc_with_the_subject() {
        // The production gate: the trait `rerank` path prepends DOC_SUBJECT_FRAME
        // to every candidate (the fix). We can't run the model here, but we pin
        // the framing transform the path applies.
        let docs = [
            "Plays the cello.".to_string(),
            "The user lives in Lisbon.".to_string(),
        ];
        let framed: Vec<String> = docs
            .iter()
            .map(|d| format!("{DOC_SUBJECT_FRAME}{d}"))
            .collect();
        assert_eq!(framed[0], "The user — Plays the cello.");
        assert_eq!(framed[1], "The user — The user lives in Lisbon.");
    }

    #[test]
    fn format_prompt_embeds_instruction_query_and_document() {
        let p = format_prompt_with(
            QWEN3_RERANKER_INSTRUCT,
            "where does the user live?",
            "The user lives in Lisbon.",
        );
        assert!(
            p.contains(QWEN3_RERANKER_INSTRUCT),
            "instruction must be present"
        );
        assert!(p.contains("<Query>: where does the user live?"));
        assert!(p.contains("<Document>: The user lives in Lisbon."));
        assert!(
            p.starts_with("<|im_start|>system"),
            "system prefix must lead"
        );
        assert!(
            p.ends_with(QWEN_SUFFIX),
            "assistant suffix must close (last-token pooling)"
        );
    }

    #[test]
    fn format_prompt_caps_overlong_document_but_keeps_suffix() {
        let long = "x".repeat(DOC_CHAR_CAP + 500);
        let p = format_prompt_with(QWEN3_RERANKER_INSTRUCT, "q", &long);
        assert!(
            p.ends_with(QWEN_SUFFIX),
            "suffix MUST survive doc truncation"
        );
        // doc segment is capped; total prompt minus scaffolding ≤ cap-ish.
        let xs = p.chars().filter(|&c| c == 'x').count();
        assert_eq!(
            xs, DOC_CHAR_CAP,
            "document MUST be truncated to DOC_CHAR_CAP chars"
        );
    }

    // Real-model behavioural check (the spike's reference assertion, promoted).
    // Gated #[ignore] like the BGE real-model tests: needs the f16 model +
    // tokenizer + ORT dylib on disk. Run:
    //   cargo test -p vault-embedding --test ... -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "real-model: needs the Qwen3 reranker fixture + ORT dylib on disk"]
    async fn real_model_scores_relevant_above_irrelevant() {
        use std::path::PathBuf;
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-fixtures");
        let model = base.join("qwen3-reranker-0.6b-seq-cls/model.onnx");
        let tok = base.join("qwen3-reranker-0.6b-seq-cls/tokenizer.json");
        #[cfg(target_os = "windows")]
        let ort_lib = base.join("bge-small-en-v1.5/onnxruntime.dll");
        #[cfg(target_os = "linux")]
        let ort_lib = base.join("bge-small-en-v1.5/libonnxruntime.so");
        // macOS branch is required for `--all-targets` to COMPILE on the CI
        // macos-latest runner even though the test is `#[ignore]`d there (the
        // ONNX Runtime macOS process-exit SIGABRT per ADR-033 keeps it from
        // running). Without it, `ort_lib` is undefined on macOS → E0425, which
        // is exactly what reddened the `87d0b72` reranker push's macOS clippy
        // leg. Local Windows clippy cannot catch this; the CI matrix is the
        // canonical surface.
        #[cfg(target_os = "macos")]
        let ort_lib = base.join("bge-small-en-v1.5/libonnxruntime.dylib");

        let provider =
            Qwen3RerankerProvider::open(&model, &tok, &ort_lib).expect("open reranker provider");
        let scores = provider
            .rerank(
                "is the user bothered by bright screens?",
                &[
                    "The user works primarily in a dark-themed editor and finds light themes straining.".to_string(),
                    "The user enjoys trail running in the foothills on weekends.".to_string(),
                ],
            )
            .await
            .expect("rerank");
        assert_eq!(scores.len(), 2);
        assert!(
            scores[0] > scores[1],
            "the relevant fact MUST outscore the irrelevant one (got {scores:?})"
        );
        assert!(
            scores[0] >= RERANK_NO_SIGNAL_FLOOR,
            "the relevant fact MUST clear the no-signal floor (got {})",
            scores[0]
        );
    }
}
