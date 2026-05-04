//! `Application` — composition root for V0.1. Owns the full dependency
//! graph and exposes the wired [`VaultAdapter`] for the MCP server to
//! dispatch through.
//!
//! ## T0.1.10 Phase 1 scope (this commit)
//!
//! Phase 1 lands [`Application::new`] — the **minimal construction
//! surface** that instantiates every concrete dep the V0.1 stack needs
//! and wires them into a [`VaultAdapter`]. No lifecycle, no MCP server
//! bind, no cascading-retry-worker spawn — those land in Phase 2.
//!
//! Per session-open Decision 2 (HANDOFF.md), T0.1.10 is consume-existing-
//! contracts work — every type used here was locked in T0.1.5–T0.1.9.
//! Phase 1's job is purely to confirm the composed dep graph runs
//! end-to-end against real LanceDB / SQLCipher / ort backends and to
//! exercise the four pre-declared stop-and-escalate triggers (Decision
//! 3) via `tests/integration_smoke.rs`.
//!
//! ## Wiring contract
//!
//! - **`StorageBackend`** owns its own internal `MetadataStore` +
//!   `LanceVectorStore` + `DuckDbGraphStore` per [`StorageBackend::open`].
//! - **`SemanticRetriever`** receives a third `Arc<MetadataStore>` handle
//!   (separate connection to the same SQLCipher file) plus the shared
//!   `Arc<dyn VectorStore>` extracted from `StorageBackend::vector_store`.
//!   Sharing the vector-store `Arc` (not opening a second LanceDB handle)
//!   is the correct pattern — LanceDB does not officially support
//!   concurrent handles to the same dataset directory, and the `Arc`
//!   already provides the necessary sharing.
//! - **`VaultAdapter`** receives a fourth `MetadataStore` handle for its
//!   `append_tool_invoke_audit` path per the existing adapter contract
//!   (sibling docstring at `adapter.rs`).
//!
//! Three separate `MetadataStore` handles to the same SQLCipher file are
//! deliberate — each is its own connection. SQLCipher with WAL mode
//! supports this; the audit-chain BLAKE3 hash links remain consistent
//! across interleaved writes from multiple connections (verified by
//! `trigger_b_audit_chain_consistent_across_composition` in
//! `tests/integration_smoke.rs`).

use std::path::Path;
use std::sync::Arc;

use vault_core::VaultResult;
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_retrieval::{Retriever, SemanticRetriever};
use vault_storage::{MetadataStore, RetryWorker, SqlCipherKey, StorageBackend};

use crate::VaultAdapter;

/// Composition root. Phase 1 wires the dep graph; Phase 1b adds the
/// minimum lifecycle (retry-worker spawn) needed for write→search
/// round-trips through the cascading orchestrator. Phase 2 adds full
/// lifecycle (shutdown handling, MCP server bind, signal handlers).
pub struct Application {
    adapter: Arc<VaultAdapter>,
    /// Held for [`Self::start`] to clone into the spawned [`RetryWorker`].
    /// `StorageBackend` is `#[derive(Clone)]` with `Arc<Inner>` semantics
    /// (per `cascading.rs:149`), so this clone is cheap and shares state
    /// with the [`VaultAdapter`]'s clone — both see the same retry_queue.
    storage: StorageBackend,
}

impl Application {
    /// Construct the full V0.1 dependency graph and wire it into a
    /// [`VaultAdapter`].
    ///
    /// # Path arguments
    ///
    /// - `metadata_path` — SQLCipher database file (created if missing).
    /// - `vector_dir` — LanceDB dataset directory (created if missing).
    /// - `graph_path` — DuckDB graph file (created if missing).
    /// - `model_path` — `bge-small-en-v1.5/model.onnx` (verified against
    ///   pinned SHA-256 per ADR-019/020 — startup-fatal on mismatch).
    /// - `tokenizer_path` — `bge-small-en-v1.5/tokenizer.json` (verified).
    /// - `ort_lib_path` — `libonnxruntime.{dll,dylib,so}` for the host
    ///   platform per ADR-019 `load-dynamic` strategy.
    ///
    /// # Errors
    ///
    /// Surfaces the first failure in:
    /// 1. `StorageBackend::open` — SQLCipher / LanceDB / DuckDB open.
    /// 2. Second `MetadataStore::open` — adapter audit handle.
    /// 3. Third `MetadataStore::open` — retriever read handle.
    /// 4. `BgeSmallProvider::open` — model/tokenizer SHA verification +
    ///    ort dynamic load + ONNX session.
    ///
    /// All four failure modes propagate as [`VaultError`] variants
    /// the caller (Phase 2 `Application::start`) can pattern-match for
    /// startup-fatal vs degraded reporting.
    ///
    /// # Phase 2 migration anchor
    ///
    /// The seven inline parameters here (`metadata_path`, `vector_dir`,
    /// `graph_path`, `key`, `model_path`, `tokenizer_path`,
    /// `ort_lib_path`) are deliberate Phase 1 spike-level minimalism.
    /// Phase 2 wraps them in a proper `AppConfig` struct; when that
    /// lands, each Phase 1 parameter MUST be enumerated as an
    /// `AppConfig` field with a doc-comment citing this function's
    /// inline parameter as its migration anchor (so the schema's
    /// provenance is auditable, not a clean-slate redesign).
    ///
    /// [`VaultError`]: vault_core::VaultError
    #[tracing::instrument(skip_all, fields(
        metadata_path = %metadata_path.display(),
        vector_dir = %vector_dir.display(),
        graph_path = %graph_path.display(),
    ))]
    pub async fn new(
        metadata_path: &Path,
        vector_dir: &Path,
        graph_path: &Path,
        key: SqlCipherKey,
        model_path: &Path,
        tokenizer_path: &Path,
        ort_lib_path: &Path,
    ) -> VaultResult<Self> {
        // 1. StorageBackend — owns its own MetadataStore + LanceDB + DuckDB.
        let storage = StorageBackend::open(
            metadata_path,
            vector_dir,
            graph_path,
            key.clone(),
            EMBEDDING_DIM,
        )
        .await?;

        // 2. Second MetadataStore handle for VaultAdapter's audit appends.
        let adapter_metadata = MetadataStore::open(metadata_path, key.clone()).await?;

        // 3. Third MetadataStore handle, Arc-shared for SemanticRetriever.
        let retriever_metadata = Arc::new(MetadataStore::open(metadata_path, key).await?);

        // 4. BgeSmallProvider — sync open (verifies SHA-256 model+tokenizer
        //    integrity, idempotent ort init, loads ONNX session +
        //    tokenizer). Sync at startup is acceptable per the existing
        //    vault-embedding test pattern; CPU-heavy work after this
        //    point goes through `EmbeddingProvider::embed` which itself
        //    handles `spawn_blocking` correctly.
        let provider = BgeSmallProvider::open(model_path, tokenizer_path, ort_lib_path)?;
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(provider);

        // 5. SemanticRetriever — shares storage's vector store Arc.
        //
        //    DO NOT open a second `LanceVectorStore::open(vector_dir, …)`
        //    handle here. LanceDB does not officially support concurrent
        //    dataset handles to the same data directory; the `Arc<dyn
        //    VectorStore>` already in `StorageBackend` is the correct
        //    sharing primitive. Future refactors that "helpfully" open a
        //    second handle will surface as fragmentation / write-races
        //    under load — see the integration spike at
        //    `tests/integration_smoke.rs` trigger (b)/(c).
        let vector_store = storage.vector_store().clone();
        let retriever = SemanticRetriever::new(retriever_metadata, embedder.clone(), vector_store);
        let retriever: Arc<dyn Retriever> = Arc::new(retriever);

        // 6. VaultAdapter — composes the four trait deps into the MCP
        //    Adapter surface. Clone the StorageBackend so Application
        //    retains a handle for `start()` to construct the worker
        //    against. The `#[derive(Clone)]` on StorageBackend is
        //    `Arc<Inner>`-shallow per cascading.rs:149 — both clones
        //    share the same retry_queue so writes via the adapter are
        //    drained by the worker constructed from Application's clone.
        let adapter_storage = storage.clone();
        let adapter = VaultAdapter::new(retriever, embedder, adapter_storage, adapter_metadata);

        Ok(Self {
            adapter: Arc::new(adapter),
            storage,
        })
    }

    /// Borrow the wired adapter. Phase 2 `Application::start` clones
    /// this `Arc` into the `StdioServer`'s constructor; integration
    /// tests in `tests/integration_smoke.rs` use it for direct dispatch.
    pub fn adapter(&self) -> &Arc<VaultAdapter> {
        &self.adapter
    }

    /// Spawn the cascading retry worker; return the [`tokio::sync::watch::Sender<bool>`]
    /// that signals shutdown when dropped or when `send(true)` is called.
    ///
    /// # Phase 1b scope (locked)
    ///
    /// This is the **minimum lifecycle** needed for write→search round-trips
    /// through the cascading orchestrator. `StorageBackend::write_memory`
    /// writes to SQLite + `retry_queue` only; the vector store is updated
    /// asynchronously by the worker draining `retry_queue` → `vector.upsert`.
    /// Without `start()` called, writes never propagate to the vector store
    /// and `SemanticRetriever` queries return empty (Phase 1 spike surfaced
    /// this — triggers (b) and (d) failed deterministically until Phase 1b
    /// added this method).
    ///
    /// **NOT included in Phase 1b** (each Phase 2 scope):
    /// - Shutdown handling logic (caller drops the returned Sender;
    ///   `RetryWorker::run`'s `cancel.changed()` arm at `retry_worker.rs:206`
    ///   breaks the loop on Sender-drop or `send(true)`).
    /// - MCP server bind / signal handler registration / `AppConfig`
    ///   migration / error recovery for worker spawn failure / a
    ///   corresponding `Application::shutdown()` await-aware path.
    ///
    /// Phase 2 wraps this in a proper `start/shutdown` pair with
    /// `JoinHandle` tracking + signal handlers; for Phase 1b the integration
    /// test holds the returned Sender on `TestApp` and lets it drop when
    /// the test ends, signaling clean worker exit.
    pub fn start(&self) -> tokio::sync::watch::Sender<bool> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let worker = RetryWorker::new(self.storage.clone());
        tokio::spawn(worker.run(rx));
        tx
    }
}
