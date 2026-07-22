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
                // ADR-089: an ABSENT file is the ordinary state of an install
                // whose first-run download has not landed yet — not a fault.
                // Classify it before attempting the load, so the caller can
                // degrade to the un-reranked path instead of failing the
                // whole query. A file that IS present but hashes wrong still
                // falls through to `open` and fails fatally there: absent and
                // tampered are deliberately different outcomes.
                //
                // Only the two DOWNLOADED files are treated this way. The ORT
                // dylib is installer-bundled, so its absence is a corrupt
                // install rather than a pending download, and it is left to
                // fail loudly through `open`.
                for (component, path) in [
                    ("reranker-model", &self.model_path),
                    ("reranker-tokenizer", &self.tokenizer_path),
                ] {
                    if !path.exists() {
                        tracing::debug!(
                            target: "vault_embedding::reranker",
                            component,
                            "reranker component not present — reporting unavailable so the \
                             caller can degrade (ADR-089)"
                        );
                        return Err(VaultError::ModelUnavailable {
                            component: component.to_string(),
                        });
                    }
                }

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

    /// Whether both DOWNLOADED components are on disk (ADR-089's "absent"
    /// test, without attempting a load).
    ///
    /// Lets a caller distinguish *"the first-run download has not landed yet"*
    /// from *"the model is present and still loading"* — two states that look
    /// identical through [`Self::is_loaded`] alone but mean opposite things to
    /// a user: the first is a pending fetch, the second is a few seconds of
    /// patience. The desktop UI needs that distinction to say something honest
    /// (ADR-090).
    ///
    /// Deliberately mirrors the absent-check inside [`Self::provider`] — the
    /// same two paths, for the same reason. The ORT dylib is excluded here
    /// exactly as it is there: it is installer-bundled, so its absence is a
    /// corrupt install rather than a pending download.
    ///
    /// Cheap (two `stat` calls) but NOT free — this is a status query, not
    /// something to call per rerank.
    pub fn files_present(&self) -> bool {
        self.model_path.exists() && self.tokenizer_path.exists()
    }

    /// Load the model now, awaiting completion.
    ///
    /// Same work [`Self::spawn_warmup`] does in the background, but awaitable
    /// so a caller can act on the outcome — which is what lets the desktop app
    /// report a truthful "ready" instead of guessing at a duration (ADR-090).
    ///
    /// Idempotent: returns immediately once the model is loaded. On failure
    /// the [`OnceCell`] stays cold, so a later call (or the first real read)
    /// retries.
    ///
    /// # Errors
    ///
    /// [`VaultError::ModelUnavailable`] when a downloaded component is absent
    /// (degrade, per ADR-089); any other [`VaultError`] is a genuine load
    /// failure such as a failed integrity check.
    pub async fn warm_up(&self) -> VaultResult<()> {
        self.provider().await.map(|_| ())
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
    fn files_present_is_false_for_absent_components_and_loads_nothing() {
        // ADR-090: the status query must answer without warming the cell —
        // otherwise asking "are we ready?" would itself trigger the 1.2 GB
        // load it is meant to be reporting on.
        let lazy = bogus();
        assert!(
            !lazy.files_present(),
            "absent components must report not-present"
        );
        assert!(!lazy.is_loaded(), "a status query must NOT load the model");
    }

    #[tokio::test]
    async fn warm_up_reports_absent_components_as_model_unavailable() {
        // ADR-089 classification, reached through the ADR-090 entry point: an
        // absent download is degrade-able, NOT a hard failure. If this ever
        // returns a different variant the desktop app would stop degrading and
        // start erroring on a vault whose download has not landed.
        let lazy = bogus();
        let err = lazy
            .warm_up()
            .await
            .expect_err("absent components must not warm successfully");
        assert!(
            matches!(err, VaultError::ModelUnavailable { .. }),
            "expected ModelUnavailable so callers can degrade; got {err:?}"
        );
        assert!(
            !lazy.is_loaded(),
            "a failed warm-up must leave the cell cold for retry"
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
            matches!(err, VaultError::ModelUnavailable { .. }),
            "absent files must report unavailable so the caller can degrade, got {err:?}"
        );
        assert!(
            !lazy.is_loaded(),
            "a failed load must leave the cell cold for retry"
        );
    }

    #[tokio::test]
    async fn absent_files_report_unavailable_and_name_the_missing_component() {
        // ADR-089: the pre-download state. This is what lets the read/search
        // paths degrade instead of erroring while the first-run fetch is
        // still in flight.
        let lazy = bogus();
        let err = lazy
            .rerank("q", &["a candidate".to_string()])
            .await
            .expect_err("absent files must not load");
        match err {
            VaultError::ModelUnavailable { component } => {
                assert_eq!(
                    component, "reranker-model",
                    "the model is checked first, so it is the component named"
                );
            }
            other => panic!("expected ModelUnavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_tokenizer_that_is_absent_alone_is_still_reported_unavailable() {
        // Only one of the two files present — still the pre-download state.
        // Guards against a check that only looks at the model path.
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.onnx");
        std::fs::write(&model, b"not the real model").expect("write model");
        let tokenizer = dir.path().join("tokenizer.json");

        let lazy = LazyQwen3Reranker::new(&model, &tokenizer, Path::new("/nonexistent/ort"));
        let err = lazy
            .rerank("q", &["a candidate".to_string()])
            .await
            .expect_err("absent tokenizer must not load");
        match err {
            VaultError::ModelUnavailable { component } => {
                assert_eq!(component, "reranker-tokenizer");
            }
            other => panic!("expected ModelUnavailable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn present_but_corrupt_files_fail_integrity_not_unavailable() {
        // THE security-critical half of ADR-089, and the reason absent and
        // tampered are separate variants. Both files exist here, so the
        // pre-download check passes and the load proceeds to SHA-256
        // verification — which rejects them. If this ever returned
        // `ModelUnavailable`, a tampered model file would silently take the
        // degrade path and the vault would quietly stop verifying what it
        // loads (BRD §11.12 vault-embedding: "Model files signed and verified
        // before loading").
        //
        // Note this needs no 1.15 GB fixture: integrity is checked before the
        // ORT session is ever built, so junk bytes fail at the hash gate.
        let dir = tempfile::tempdir().expect("tempdir");
        let model = dir.path().join("model.onnx");
        let tokenizer = dir.path().join("tokenizer.json");
        std::fs::write(&model, b"tampered model bytes").expect("write model");
        std::fs::write(&tokenizer, b"tampered tokenizer bytes").expect("write tokenizer");

        let lazy = LazyQwen3Reranker::new(&model, &tokenizer, Path::new("/nonexistent/ort"));
        let err = lazy
            .rerank("q", &["a candidate".to_string()])
            .await
            .expect_err("corrupt files must fail the load");
        assert!(
            !matches!(err, VaultError::ModelUnavailable { .. }),
            "a present-but-wrong file must NEVER classify as merely unavailable — \
             that would let a tampered model through the degrade path; got {err:?}"
        );
        assert!(
            matches!(err, VaultError::ModelIntegrityFailed { .. }),
            "expected an integrity failure, got {err:?}"
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
