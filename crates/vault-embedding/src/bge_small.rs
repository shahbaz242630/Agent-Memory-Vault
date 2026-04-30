//! `BgeSmallProvider` — `EmbeddingProvider` implementation backed by
//! bge-small-en-v1.5 on ONNX Runtime via the `ort` 2.x crate.
//!
//! Implementation contracts (from `T0.1.7_PLAN.md` v1.2 + Spike findings):
//! - **CLS-token pooling** per BAAI's `1_Pooling/config.json` (Spike 3) —
//!   extract `last_hidden_state[0, 0, :]`, NOT mean-pool. Test 4 enforces.
//! - **`u32 → i64` cast** at the tokenizer / ort boundary (Spike 2) —
//!   `tokenizers::Encoding::get_*` returns `&[u32]`; ONNX BERT expects i64.
//! - **Manual `encoding.truncate(512, 0, Right)`** (Spike 2) — Rust
//!   tokenizers does NOT auto-apply `model_max_length`.
//! - **`std::sync::OnceLock` wrapping `ort::init_from`** — required for
//!   thread-safe initialisation under `load-dynamic`. Concurrent
//!   `BgeSmallProvider::open` calls (e.g. parallel cargo test threads) all
//!   succeed; first call wins, subsequent calls see the cached result.
//! - **Integrity check before any ort/tokenizer setup** (ADR-020) —
//!   model + tokenizer SHA-256 verified against compiled-in canonical
//!   hashes. Mismatch → fatal at startup, no fallback.

use crate::integrity::{
    verify_file_sha256, BGE_SMALL_EN_V1_5_MODEL_SHA256, BGE_SMALL_EN_V1_5_TOKENIZER_SHA256,
};
use crate::provider::{EmbeddingProvider, EMBEDDING_DIM};
use async_trait::async_trait;
use ort::session::{builder::GraphOptimizationLevel, Session};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use tokenizers::Tokenizer;
use vault_core::{VaultError, VaultResult};

/// Process-global ort initialisation gate. First `BgeSmallProvider::open`
/// call wins and runs `ort::init_from(dylib_path).commit()`; subsequent
/// calls (including concurrent ones from parallel cargo test threads) see
/// the cached result without re-initialising. The `Result<(), String>`
/// payload preserves the first-call outcome — error or success — so a
/// degraded process surfaces the original failure on every subsequent
/// open attempt rather than silently retrying.
static ORT_INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// ONNX-Runtime-backed embedding provider for bge-small-en-v1.5.
///
/// Construct via [`BgeSmallProvider::open`]; thereafter use as an
/// [`EmbeddingProvider`].
///
/// `Send + Sync`: the [`Session`] is wrapped in `Arc<Mutex<Session>>`
/// because `ort::session::Session::run` takes `&mut self`. Concurrent
/// `embed` calls serialise through the mutex inside `spawn_blocking` —
/// that matches V0.1's expected throughput (handfuls of embeds per sec)
/// and avoids the per-thread Session pool a higher-throughput design
/// would need.
pub struct BgeSmallProvider {
    // `pub(crate)` so the `testing` module (gated `testing` feature) can
    // access these fields directly to implement `mean_pooled_for` — the
    // CLS-vs-mean comparison needed by test 9. External callers still see
    // the struct as opaque.
    pub(crate) session: Arc<Mutex<Session>>,
    pub(crate) tokenizer: Arc<Tokenizer>,
}

impl BgeSmallProvider {
    /// Open the provider: verify model + tokenizer integrity, initialise
    /// ort under `load-dynamic` with the given dylib path (idempotent
    /// across the process via [`OnceLock`]), load the ONNX session, load
    /// the tokenizer.
    ///
    /// # Errors
    ///
    /// - [`VaultError::ModelIntegrityFailed`] on SHA-256 mismatch
    ///   (model OR tokenizer). **Fatal at startup, no fallback.**
    /// - [`VaultError::Embedding`] on ort init / session-load /
    ///   tokenizer-load failure.
    /// - [`VaultError::Io`] on file-read failure during integrity check.
    #[tracing::instrument(level = "info", skip_all, fields(
        model = %model_path.display(),
        tokenizer = %tokenizer_path.display(),
        ort_lib = %ort_lib_path.display(),
    ))]
    pub fn open(
        model_path: &Path,
        tokenizer_path: &Path,
        ort_lib_path: &Path,
    ) -> VaultResult<Self> {
        // 1) Integrity verification BEFORE any ort/tokenizer setup. Per
        //    ADR-020: integrity failure is fatal at startup, no fallback.
        verify_file_sha256(model_path, BGE_SMALL_EN_V1_5_MODEL_SHA256, "model")?;
        verify_file_sha256(
            tokenizer_path,
            BGE_SMALL_EN_V1_5_TOKENIZER_SHA256,
            "tokenizer",
        )?;

        // 2) Initialise ort with the dylib path. OnceLock guarantees this
        //    runs at most once per process; concurrent open() calls all
        //    see the same cached outcome.
        ensure_ort_initialised(ort_lib_path)?;

        // 3) Load the ONNX session.
        let session = Session::builder()
            .map_err(|e| VaultError::Embedding(format!("ort session builder: {e}")))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| VaultError::Embedding(format!("ort optimization level: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| VaultError::Embedding(format!("ort load model: {e}")))?;

        // 4) Load the tokenizer.
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|e| VaultError::Embedding(format!("tokenizer load: {e}")))?;

        tracing::info!("BgeSmallProvider opened (integrity OK; session + tokenizer loaded)");

        Ok(Self {
            session: Arc::new(Mutex::new(session)),
            tokenizer: Arc::new(tokenizer),
        })
    }
}

/// Process-global ort initialisation under `load-dynamic`. First-call
/// wins; subsequent calls see the same cached result without re-running
/// `ort::init_from`. Concurrent callers all observe the same outcome.
fn ensure_ort_initialised(dylib_path: &Path) -> VaultResult<()> {
    let result = ORT_INIT.get_or_init(|| {
        let path_str = dylib_path
            .to_str()
            .ok_or_else(|| "ort dylib path is not valid UTF-8".to_string())?;
        ort::init_from(path_str)
            .commit()
            .map(|_| ())
            .map_err(|e| format!("ort init_from: {e}"))
    });

    match result {
        Ok(()) => Ok(()),
        Err(s) => Err(VaultError::Embedding(format!("ort init: {s}"))),
    }
}

#[async_trait]
impl EmbeddingProvider for BgeSmallProvider {
    /// Embed a single text input → 384-dim L2-normalised `Vec<f32>` via
    /// CLS-token pooling. See module-level docs for the full pipeline.
    #[tracing::instrument(level = "debug", skip(self, text), fields(text_len = text.len()))]
    async fn embed(&self, text: &str) -> VaultResult<Vec<f32>> {
        // Tokenize on the async runtime (cheap; no need for spawn_blocking).
        let mut encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| VaultError::Embedding(format!("tokenize: {e}")))?;

        // Manual truncation to model_max_length=512. Rust tokenizers does
        // NOT auto-apply tokenizer_config.json's model_max_length (Python
        // tokenizers does — see Spike 2 finding). Right-direction truncation
        // matches BERT default; stride 0 = no overlap windows.
        if encoding.len() > 512 {
            encoding.truncate(512, 0, tokenizers::TruncationDirection::Right);
        }

        // u32 → i64 conversion at the tokenizer/ort boundary (Spike 2 finding).
        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&x| x as i64)
            .collect();
        let type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&x| x as i64).collect();
        let seq_len = ids.len();

        let session = Arc::clone(&self.session);

        tokio::task::spawn_blocking(move || -> VaultResult<Vec<f32>> {
            // Construct input tensors. Shape [1, seq_len] for each.
            let input_ids = ort::value::Tensor::from_array(([1_usize, seq_len], ids))
                .map_err(|e| VaultError::Embedding(format!("input_ids tensor: {e}")))?;
            let attention_mask = ort::value::Tensor::from_array(([1_usize, seq_len], mask))
                .map_err(|e| VaultError::Embedding(format!("attention_mask tensor: {e}")))?;
            let token_type_ids = ort::value::Tensor::from_array(([1_usize, seq_len], type_ids))
                .map_err(|e| VaultError::Embedding(format!("token_type_ids tensor: {e}")))?;

            // Lock the session for the inference call (Session::run takes &mut self).
            let mut session_guard = session
                .lock()
                .map_err(|e| VaultError::Embedding(format!("session lock poisoned: {e}")))?;

            let outputs = session_guard
                .run(ort::inputs![
                    "input_ids" => input_ids,
                    "attention_mask" => attention_mask,
                    "token_type_ids" => token_type_ids,
                ])
                .map_err(|e| VaultError::Embedding(format!("session run: {e}")))?;

            // BERT's standard ONNX output name. If bge-small uses a different
            // name we'll find out via the runtime confirmation test loudly.
            let last_hidden_state = outputs.get("last_hidden_state").ok_or_else(|| {
                VaultError::Embedding(
                    "missing 'last_hidden_state' output — ort output names may differ for bge-small"
                        .into(),
                )
            })?;

            let (_shape, data) = last_hidden_state
                .try_extract_tensor::<f32>()
                .map_err(|e| VaultError::Embedding(format!("extract tensor: {e}")))?;

            // CLS-token pool: extract last_hidden_state[0, 0, :] — the [CLS]
            // vector at sequence position 0 of batch position 0. Memory layout
            // is [batch, seq, hidden] = [1, seq_len, 384] in row-major, so the
            // CLS vector is the first 384 floats.
            if data.len() < EMBEDDING_DIM {
                return Err(VaultError::Embedding(format!(
                    "output too small: {} floats < expected {}",
                    data.len(),
                    EMBEDDING_DIM
                )));
            }
            let cls: Vec<f32> = data[..EMBEDDING_DIM].to_vec();

            // L2-normalize.
            let norm: f32 = cls.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm == 0.0 {
                return Err(VaultError::Embedding("zero-norm CLS vector".into()));
            }
            Ok(cls.iter().map(|x| x / norm).collect())
        })
        .await
        .map_err(|e| VaultError::Embedding(format!("spawn_blocking join: {e}")))?
    }
}
