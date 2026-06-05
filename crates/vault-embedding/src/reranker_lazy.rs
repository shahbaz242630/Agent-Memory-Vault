//! `LazyQwen3Reranker` — defers the ~1.2 GB Qwen3 reranker model load OFF the
//! MCP server's startup/handshake critical path (ADR-070, 2026-06-05).
//!
//! ## Why
//!
//! [`Qwen3RerankerProvider::open`] reads + SHA-256-verifies a ~1.2 GB ONNX file
//! and builds an `ort` session with Level-3 graph optimisation — ~40 s on CPU.
//! Wired eagerly in [`vault_app`]'s composition root, that cost ran BEFORE the
//! MCP server could answer the `initialize` handshake. Two real consequences:
//! Kimi CLI's connect patience (< 40 s on retries) timed out before the server
//! ever said hello, and Claude Desktop's 60 s init window was uncomfortably
//! close. The reranker is only needed when a read actually happens — never for
//! the handshake — so the load belongs off the critical path.
//!
//! ## How
//!
//! This wrapper implements [`RerankProvider`] but holds only the file paths at
//! construction — **zero disk I/O** in [`LazyQwen3Reranker::new`]. The inner
//! [`Qwen3RerankerProvider`] is loaded exactly once, on first use, through a
//! [`tokio::sync::OnceCell`]; concurrent reads share the single in-flight load.
//! Crucially [`RerankProvider::relevance_floor`] returns the
//! [`RERANK_NO_SIGNAL_FLOOR`] constant WITHOUT loading — so nothing on the
//! handshake path can trigger the model load.
//!
//! [`LazyQwen3Reranker::spawn_warmup`] kicks the load off as a detached
//! background task the moment the server starts serving, so in practice the
//! first read does not pay the full load either (the model is warming while the
//! user reads the tool list and types). The handshake stays sub-second.
//!
//! ## Integrity-timing note (ADR-070)
//!
//! Moving the load defers the model's SHA-256 integrity check (ADR-020) from
//! startup to first-load. The check still runs BEFORE the model is ever used to
//! produce a result — verify-before-use is preserved — only its timing changes
//! from "at server launch" to "at first read / background warm-up". A corrupt or
//! missing model now surfaces at first read instead of at launch; for a
//! local-first single-user tool that is the moment the user would notice anyway.
//! Not a security weakening; recorded as an explicit timing decision.

use crate::reranker::{Qwen3RerankerProvider, RerankProvider, RERANK_NO_SIGNAL_FLOOR};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::OnceCell;
use vault_core::{VaultError, VaultResult};

/// A [`RerankProvider`] that loads [`Qwen3RerankerProvider`] lazily on first
/// use instead of eagerly at construction. See the module docs for the why.
///
/// Cheap to clone via `Arc`; clones share the same [`OnceCell`], so a
/// background warm-up populates the same cell the read path reads.
pub struct LazyQwen3Reranker {
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    ort_lib_path: PathBuf,
    /// Loaded exactly once on first use. `Arc<Qwen3RerankerProvider>` so the
    /// cached value is cheap to hand back from each `rerank` call.
    inner: OnceCell<Arc<Qwen3RerankerProvider>>,
}

impl LazyQwen3Reranker {
    /// Construct the lazy wrapper. **Performs no disk I/O** — the model is not
    /// touched until the first [`RerankProvider::rerank`] (or
    /// [`Self::spawn_warmup`]). Infallible: any load error surfaces later, at
    /// first use, as a [`VaultError`] from `rerank`.
    pub fn new(model_path: &Path, tokenizer_path: &Path, ort_lib_path: &Path) -> Self {
        Self {
            model_path: model_path.to_path_buf(),
            tokenizer_path: tokenizer_path.to_path_buf(),
            ort_lib_path: ort_lib_path.to_path_buf(),
            inner: OnceCell::new(),
        }
    }

    /// Whether the inner model has been loaded yet. Cheap, non-blocking — for
    /// diagnostics / tests (e.g. asserting a warm-up populated the cell).
    pub fn is_loaded(&self) -> bool {
        self.inner.initialized()
    }

    /// Load (or return the already-loaded) inner provider. The blocking
    /// `open` (file read + SHA-256 + ort session build) runs on a blocking
    /// thread so it never stalls the async runtime serving MCP messages.
    ///
    /// On error the [`OnceCell`] stays uninitialised (tokio semantics), so a
    /// transient failure on warm-up is retried by the first real read.
    async fn provider(&self) -> VaultResult<Arc<Qwen3RerankerProvider>> {
        self.inner
            .get_or_try_init(|| async {
                let model = self.model_path.clone();
                let tokenizer = self.tokenizer_path.clone();
                let ort_lib = self.ort_lib_path.clone();
                tracing::info!(
                    target: "vault_embedding::reranker",
                    "lazy reranker: loading model off the handshake path (first use / warm-up)"
                );
                let provider = tokio::task::spawn_blocking(move || {
                    Qwen3RerankerProvider::open(&model, &tokenizer, &ort_lib)
                })
                .await
                .map_err(|e| VaultError::Embedding(format!("reranker load join: {e}")))??;
                Ok::<_, VaultError>(Arc::new(provider))
            })
            .await
            .cloned()
    }

    /// Kick off the model load as a detached background task. Returns
    /// immediately — call right after the MCP transport binds so the model
    /// warms while the handshake completes and the user types their first
    /// query. A warm-up failure is logged (not fatal); the first real read
    /// retries the load and surfaces any genuine error to the caller.
    pub fn spawn_warmup(self: &Arc<Self>) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            match this.provider().await {
                Ok(_) => tracing::info!(
                    target: "vault_embedding::reranker",
                    "lazy reranker: background warm-up complete (first read will be fast)"
                ),
                Err(e) => tracing::warn!(
                    target: "vault_embedding::reranker",
                    error = %e,
                    "lazy reranker: background warm-up failed; the first read will retry the load"
                ),
            }
        });
    }
}

#[async_trait]
impl RerankProvider for LazyQwen3Reranker {
    async fn rerank(&self, query: &str, docs: &[String]) -> VaultResult<Vec<f32>> {
        // Never load the 1.2 GB model just to rerank nothing — an empty pool
        // returns empty (matching the inner provider's own empty-batch guard).
        if docs.is_empty() {
            return Ok(Vec::new());
        }
        self.provider().await?.rerank(query, docs).await
    }

    fn relevance_floor(&self) -> f32 {
        // Constant — independent of the loaded model. Returning it without a
        // load is the property that keeps the `initialize` handshake fast: the
        // read pipeline can read the floor at startup without touching the file.
        RERANK_NO_SIGNAL_FLOOR
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bogus() -> LazyQwen3Reranker {
        // Paths that do not exist — proving these methods never touch disk.
        LazyQwen3Reranker::new(
            Path::new("/nonexistent/model.onnx"),
            Path::new("/nonexistent/tokenizer.json"),
            Path::new("/nonexistent/onnxruntime"),
        )
    }

    #[test]
    fn relevance_floor_does_not_load_the_model() {
        // THE handshake-safety property: the floor is readable with no model on
        // disk and the cell stays cold. If this ever loads, the MCP handshake
        // regresses back to ~40 s.
        let lazy = bogus();
        assert_eq!(lazy.relevance_floor(), RERANK_NO_SIGNAL_FLOOR);
        assert!(
            !lazy.is_loaded(),
            "reading the floor must NOT load the model"
        );
    }

    #[test]
    fn construction_touches_no_disk() {
        // `new` is infallible and does no I/O — bogus paths construct fine and
        // leave the model cold.
        let lazy = bogus();
        assert!(!lazy.is_loaded());
    }

    #[tokio::test]
    async fn empty_docs_returns_empty_without_loading() {
        // An empty candidate pool must short-circuit BEFORE the load — so even
        // pointed at nonexistent files it succeeds and stays cold.
        let lazy = bogus();
        let scores = lazy.rerank("anything", &[]).await.expect("empty rerank ok");
        assert!(scores.is_empty());
        assert!(
            !lazy.is_loaded(),
            "empty docs must not trigger a model load"
        );
    }

    #[tokio::test]
    async fn rerank_with_docs_attempts_the_load_and_surfaces_errors() {
        // A non-empty pool DOES attempt the load; with bogus paths that load
        // fails and the error surfaces (proving the deferral is real, not a
        // silent no-op). The cell stays uninitialised so a later real read
        // could still succeed.
        let lazy = bogus();
        let err = lazy
            .rerank("q", &["a candidate".to_string()])
            .await
            .expect_err("bogus model path must fail the deferred load");
        assert!(
            matches!(
                err,
                VaultError::Embedding(_)
                    | VaultError::ModelIntegrityFailed { .. }
                    | VaultError::Io(_)
            ),
            "expected a load failure, got {err:?}"
        );
        assert!(
            !lazy.is_loaded(),
            "a failed load must leave the cell cold for retry"
        );
    }

    // Real-model behavioural parity check: the lazy path loads on first use and
    // scores identically to the eager `Qwen3RerankerProvider`. Gated `#[ignore]`
    // like the eager reranker's real-model test — needs the f16 model +
    // tokenizer + ORT dylib on disk.
    #[tokio::test]
    #[ignore = "real-model: needs the Qwen3 reranker fixture + ORT dylib on disk"]
    async fn lazy_loads_on_first_use_and_scores_relevant_above_irrelevant() {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-fixtures");
        let model = base.join("qwen3-reranker-0.6b-seq-cls/model.onnx");
        let tok = base.join("qwen3-reranker-0.6b-seq-cls/tokenizer.json");
        #[cfg(target_os = "windows")]
        let ort_lib = base.join("bge-small-en-v1.5/onnxruntime.dll");
        #[cfg(target_os = "linux")]
        let ort_lib = base.join("bge-small-en-v1.5/libonnxruntime.so");
        #[cfg(target_os = "macos")]
        let ort_lib = base.join("bge-small-en-v1.5/libonnxruntime.dylib");

        let lazy = LazyQwen3Reranker::new(&model, &tok, &ort_lib);
        assert!(!lazy.is_loaded(), "must start cold");

        let docs = [
            "The user works primarily in a dark-themed editor and finds light themes straining."
                .to_string(),
            "The user enjoys trail running in the foothills on weekends.".to_string(),
        ];
        let scores = lazy
            .rerank("is the user bothered by bright screens?", &docs)
            .await
            .expect("first-use rerank loads + scores");
        assert!(lazy.is_loaded(), "first rerank must have loaded the model");
        assert_eq!(scores.len(), 2);
        assert!(
            scores[0] > scores[1],
            "relevant fact must outscore irrelevant (got {scores:?})"
        );

        // Second call reuses the cached session (no reload).
        let again = lazy
            .rerank("is the user bothered by bright screens?", &docs)
            .await
            .expect("cached rerank");
        assert_eq!(again.len(), 2);
    }
}
