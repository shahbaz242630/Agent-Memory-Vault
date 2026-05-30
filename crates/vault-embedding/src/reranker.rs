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
//! top retrieved candidates, keeps those scoring ≥ [`RERANK_RELEVANCE_FLOOR`],
//! and re-sorts by reranker score — replacing the ADR-057 cosine floor as the
//! relevance gate. CPU cost ≈ 0.39 s per candidate (f16); GPU sub-second.

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

/// The task instruction handed to the reranker (the "v4 strict-yes/no"
/// variant — the instruction-tuning sweep winner). The instruction is the
/// lever that closes topically-adjacent "wrong-attribute" traps: it tells the
/// model to answer *no* for same-topic facts that don't actually contain the
/// answer. Calibrated 2026-05-29 via the `reranker_spike` sweep.
pub const QWEN3_RERANKER_INSTRUCT: &str =
    "You are matching a question about a user to a personal fact. Answer yes only if the fact lets you answer the question with confidence. Same-topic facts that do not contain the answer must be answered no.";

/// Relevance floor on the reranker logit. A candidate scoring below this is
/// not relevant to the query. **0.0** = sigmoid 0.5 ("more likely yes than
/// no"). Calibrated 2026-05-29 (`reranker_spike`, v4 instruction): on the
/// hardened A7 set every real answer scored above 0 and every guard below it,
/// with a ~3-logit margin. ADR-057 amendment: this supersedes the cosine-0.66
/// floor as the read relevance gate.
pub const RERANK_RELEVANCE_FLOOR: f32 = 0.0;

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

/// Format one `(query, document)` pair into the reranker's chat-template
/// prompt with the baked-in task instruction.
fn format_prompt(query: &str, doc: &str) -> String {
    let doc = if doc.chars().count() > DOC_CHAR_CAP {
        doc.chars().take(DOC_CHAR_CAP).collect()
    } else {
        doc.to_string()
    };
    format!("{QWEN_PREFIX}<Instruct>: {QWEN3_RERANKER_INSTRUCT}\n<Query>: {query}\n<Document>: {doc}{QWEN_SUFFIX}")
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
}

#[async_trait]
impl RerankProvider for Qwen3RerankerProvider {
    #[tracing::instrument(level = "debug", skip(self, query, docs), fields(n_docs = docs.len()))]
    async fn rerank(&self, query: &str, docs: &[String]) -> VaultResult<Vec<f32>> {
        if docs.is_empty() {
            return Ok(Vec::new());
        }

        // Tokenise each formatted prompt on the async runtime (cheap). The
        // chat-template control tokens are explicit, so add_special_tokens=false.
        let mut rows: Vec<Vec<i64>> = Vec::with_capacity(docs.len());
        for doc in docs {
            let prompt = format_prompt(query, doc);
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

    fn relevance_floor(&self) -> f32 {
        RERANK_RELEVANCE_FLOOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevance_floor_is_pinned_at_zero() {
        // ADR-057 amendment: logit 0 (sigmoid 0.5). A future re-calibration
        // MUST break this consciously, not drift.
        assert_eq!(RERANK_RELEVANCE_FLOOR, 0.0);
    }

    #[test]
    fn format_prompt_embeds_instruction_query_and_document() {
        let p = format_prompt("where does the user live?", "The user lives in Lisbon.");
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
        let p = format_prompt("q", &long);
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
            scores[0] >= RERANK_RELEVANCE_FLOOR,
            "the relevant fact MUST clear the relevance floor (got {})",
            scores[0]
        );
    }
}
