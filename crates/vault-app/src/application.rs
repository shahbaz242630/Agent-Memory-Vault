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

use std::sync::Arc;

use vault_core::{Boundary, VaultError, VaultResult};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_mcp::{Adapter, StdioServer};
use vault_retrieval::{Retriever, SemanticRetriever};
use vault_storage::{MetadataStore, RetryWorker, StorageBackend};

use crate::process_exit::{LiveProcessExit, ProcessExit};
use crate::{AppConfig, VaultAdapter};

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
    /// # Configuration
    ///
    /// Takes the [`AppConfig`] composition-root configuration by
    /// reference. See [`AppConfig`]'s module docs for the migration-
    /// anchor history (T0.1.10 Phase 2b migrated the seven Phase 1
    /// inline parameters to AppConfig fields with verbatim names per
    /// rename-prohibition discipline).
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
    /// the caller (Phase 2 `Application::start_with_mcp`) can pattern-
    /// match for startup-fatal vs degraded reporting.
    ///
    /// [`VaultError`]: vault_core::VaultError
    #[tracing::instrument(skip_all, fields(
        metadata_path = %config.metadata_path.display(),
        vector_dir = %config.vector_dir.display(),
        graph_path = %config.graph_path.display(),
    ))]
    pub async fn new(config: &AppConfig) -> VaultResult<Self> {
        // 1. StorageBackend — owns its own MetadataStore + LanceDB + DuckDB.
        //    SqlCipherKey clone is cheap (clones inner String); cloning
        //    inside the body is the canonical pattern for by-reference
        //    config (per AppConfig module docs).
        let storage = StorageBackend::open(
            &config.metadata_path,
            &config.vector_dir,
            &config.graph_path,
            config.key.clone(),
            EMBEDDING_DIM,
        )
        .await?;

        // 2. Second MetadataStore handle for VaultAdapter's audit appends.
        let adapter_metadata =
            MetadataStore::open(&config.metadata_path, config.key.clone()).await?;

        // 3. Third MetadataStore handle, Arc-shared for SemanticRetriever.
        let retriever_metadata =
            Arc::new(MetadataStore::open(&config.metadata_path, config.key.clone()).await?);

        // 4. BgeSmallProvider — sync open (verifies SHA-256 model+tokenizer
        //    integrity, idempotent ort init, loads ONNX session +
        //    tokenizer). Sync at startup is acceptable per the existing
        //    vault-embedding test pattern; CPU-heavy work after this
        //    point goes through `EmbeddingProvider::embed` which itself
        //    handles `spawn_blocking` correctly.
        let provider = BgeSmallProvider::open(
            &config.model_path,
            &config.tokenizer_path,
            &config.ort_lib_path,
        )?;
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

    /// **Test-focused entry point.** Spawn the cascading retry worker only;
    /// return the [`tokio::sync::watch::Sender<bool>`] that signals
    /// shutdown when dropped or when `send(true)` is called.
    ///
    /// Production callers should use [`Self::start_with_mcp`] instead —
    /// it composes `start()`'s worker spawn with MCP server bind + signal
    /// handlers + the await-aware [`ApplicationHandle::shutdown`] path.
    ///
    /// # Phase 1b scope (kept stable in Phase 2 per Path α decision)
    ///
    /// This is the **minimum lifecycle** needed for write→search round-trips
    /// through the cascading orchestrator. `StorageBackend::write_memory`
    /// writes to SQLite + `retry_queue` only; the vector store is updated
    /// asynchronously by the worker draining `retry_queue` → `vector.upsert`.
    /// Without `start()` (or `start_with_mcp()`) called, writes never
    /// propagate to the vector store and `SemanticRetriever` queries return
    /// empty (Phase 1 spike surfaced this — triggers (b) and (d) failed
    /// deterministically until Phase 1b added this method).
    pub fn start(&self) -> tokio::sync::watch::Sender<bool> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        let worker = RetryWorker::new(self.storage.clone());
        tokio::spawn(worker.run(rx));
        tx
    }

    /// **Production lifecycle entry point.** Spawn the cascading retry
    /// worker, bind the MCP `StdioServer` against `self.adapter`, and
    /// register signal handlers (Ctrl-C → graceful shutdown; second
    /// Ctrl-C → forced exit per locked semantics). Returns an
    /// [`ApplicationHandle`] that owns the spawned task `JoinHandle`s and
    /// exposes [`ApplicationHandle::shutdown`] for await-aware cleanup.
    ///
    /// # Path α discipline (T0.1.10 Phase 2)
    ///
    /// This method is **separate from** [`Self::start`] (which stays
    /// worker-only for tests). The two methods diverge at the API
    /// surface — explicitly named, no bool flag — so caller intent is
    /// clear from the call site. See HANDOFF.md Phase 2 plan paragraph
    /// for the Path α reasoning.
    ///
    /// # Errors
    ///
    /// - [`VaultError::McpBindFailed`] — `rmcp::ServiceExt::serve` failed
    ///   to bind the stdio transport (rare in practice; possible if
    ///   another process holds stdin or rmcp's transport layer hits an
    ///   I/O error during initial setup).
    /// - [`VaultError::WorkerSpawnFailed`] is reserved as a future-proof
    ///   variant for fallible worker startup paths (e.g., when worker
    ///   construction grows config-validation or initial-state inspection
    ///   that can fail). Phase 2's `RetryWorker::new` + `tokio::spawn`
    ///   are both infallible, so this variant is **technically dead code
    ///   at Phase 2 landing** — kept defined per session-open pre-flag
    ///   #5 awaiting user (a)/(b) decision on whether to retain as
    ///   future-proof or remove until a concrete consumer surfaces.
    #[tracing::instrument(skip_all, fields(boundary_count = authorized_boundaries.len()))]
    pub async fn start_with_mcp(
        &self,
        authorized_boundaries: Vec<Boundary>,
    ) -> VaultResult<ApplicationHandle> {
        use rmcp::ServiceExt;

        // 1. Spawn cascading retry worker — same as Self::start().
        let (shutdown_signal, rx) = tokio::sync::watch::channel(false);
        let worker = RetryWorker::new(self.storage.clone());
        let worker_handle = tokio::spawn(worker.run(rx));

        // 2. Build StdioServer (infallible) against the wired adapter.
        //    `Arc<VaultAdapter>` coerces to `Arc<dyn Adapter>` at the
        //    let-binding via DST coercion since `VaultAdapter: Adapter`.
        let adapter_dyn: Arc<dyn Adapter> = self.adapter.clone();
        let server = StdioServer::new(adapter_dyn, authorized_boundaries);

        // 3. Bind stdio transport synchronously — McpBindFailed propagates
        //    here if rmcp's serve() setup errs. Awaiting serve() returns
        //    a `RunningService` (concrete generic type, not named because
        //    inference handles the storage); the waiting() loop then runs
        //    until the transport closes.
        let running = ServiceExt::serve(server, rmcp::transport::stdio())
            .await
            .map_err(|e| VaultError::McpBindFailed(format!("rmcp serve: {e}")))?;
        let server_handle = tokio::spawn(async move {
            // waiting() blocks until the transport closes (stdin EOF or
            // process termination). We discard its Result; benign errors
            // already surface as the server task's exit, and panics
            // become JoinError on the handle.
            let _ = running.waiting().await;
        });

        // 4. Spawn signal handler — first Ctrl-C → graceful shutdown
        //    signal; second Ctrl-C → forced exit per locked semantics.
        //    Production wires `LiveProcessExit` per ADR-locked
        //    semantics (Phase 2a). Tests of `handle_signals` directly
        //    construct `CapturingProcessExit` per
        //    `feedback_inline_architectural_decisions_produce_adr_in_same_commit.md`
        //    + Phase 4a Clarification 1 (testability of ADR-locked
        //    force-exit-130 path).
        let signal_tx = shutdown_signal.clone();
        let exit_impl: Arc<dyn ProcessExit> = Arc::new(LiveProcessExit);
        let signal_handle = tokio::spawn(handle_signals(signal_tx, exit_impl));

        Ok(ApplicationHandle {
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
        })
    }
}

/// Handle returned by [`Application::start_with_mcp`]. Owns the task
/// `JoinHandle`s for the retry worker, MCP server, and signal handler;
/// provides await-aware [`Self::shutdown`] for graceful production
/// cleanup.
///
/// # Lifecycle
///
/// - **Drop without `shutdown`**: the `shutdown_signal` `Sender` drops,
///   the worker exits cleanly via `cancel.changed()` Err arm, but the
///   server + signal tasks keep running until process exit. Acceptable
///   for tests / abnormal exit paths but NOT for production graceful
///   shutdown.
/// - **`shutdown().await`**: signals the worker, awaits its drain,
///   aborts the server + signal tasks (in-flight MCP requests dropped),
///   awaits all task `JoinHandle`s. Returns when all tasks have exited.
///
/// # Why `shutdown` consumes self by value
///
/// Terminal lifecycle methods consume by value to enforce single-call
/// semantics at compile time. Calling `shutdown` twice on the same
/// handle would attempt to re-await already-consumed `JoinHandle`s
/// (which panics). Consuming `self` prevents this entirely; the
/// borrow-checker rejects double-shutdown at compile time.
pub struct ApplicationHandle {
    shutdown_signal: tokio::sync::watch::Sender<bool>,
    worker_handle: tokio::task::JoinHandle<()>,
    server_handle: tokio::task::JoinHandle<()>,
    signal_handle: tokio::task::JoinHandle<()>,
}

impl ApplicationHandle {
    /// Borrow the shutdown-signal `Sender`. Useful when an external
    /// supervisor wants to signal cancellation without consuming the
    /// handle (e.g., a parent task that's also tracking other lifecycle
    /// resources).
    pub fn shutdown_signal(&self) -> &tokio::sync::watch::Sender<bool> {
        &self.shutdown_signal
    }

    /// Graceful shutdown. Signals the worker to drain, aborts the server
    /// and signal tasks, awaits all `JoinHandle`s. Consumes `self` (see
    /// type-level docstring for why).
    ///
    /// # V0.1 known limitation
    ///
    /// MCP server shutdown aborts the running task rather than closing
    /// the stdio transport gracefully — in-flight tool calls are
    /// dropped. Closing stdin from inside the process is not directly
    /// supported by rmcp's stdio transport; a future-proof graceful-MCP
    /// shutdown would require either a transport-level close API or a
    /// supervisor pattern that closes stdio externally. Acceptable for
    /// V0.1 internal alpha (single-user, single-agent); revisit at V0.2
    /// multi-agent task if concrete consumer surfaces.
    pub async fn shutdown(self) -> VaultResult<()> {
        // 1. Signal the retry worker to stop polling. Worker will finish
        //    its current step (drain in-flight cascade entry) and exit.
        let _ = self.shutdown_signal.send(true);

        // 2. Abort the signal handler (it's blocked on Ctrl-C waiting).
        //    Aborting drops the future; the underlying ctrl_c handler
        //    is unregistered when the future is dropped.
        self.signal_handle.abort();

        // 3. Abort the MCP server task (see V0.1 known limitation above).
        self.server_handle.abort();

        // 4. Await the worker — graceful drain. JoinError = panic;
        //    log but don't return an error (shutdown is best-effort
        //    cleanup; a panicked worker is a correctness bug surfaced
        //    elsewhere via tracing).
        if let Err(e) = self.worker_handle.await {
            if !e.is_cancelled() {
                tracing::error!(error = %e, "retry worker join error during shutdown");
            }
        }

        // 5. Await the aborted handles to confirm cleanup. JoinError on
        //    aborted tasks is expected (cancellation), so swallow.
        let _ = self.server_handle.await;
        let _ = self.signal_handle.await;

        Ok(())
    }
}

/// Signal handler task: first Ctrl-C → flip shutdown signal + stderr
/// announce; second Ctrl-C → forced exit per locked semantics
/// (`std::process::exit(130)` + stderr message).
///
/// # Locked semantics (T0.1.10 Phase 2a pre-declaration)
///
/// - Exit code 130 = 128 + SIGINT(2), the SIGINT-conventional shell
///   convention (bash, zsh) for "process killed by Ctrl-C." Tools
///   monitoring exit codes (CI systems, supervisors) can distinguish
///   "graceful shutdown didn't complete in time" from a clean exit (0)
///   or a panic (101).
/// - Stderr messages document why exit happened. NOT logged via
///   `tracing` because the tracing subsystem may itself be torn down by
///   the time the second SIGINT fires; raw `eprintln!` is the
///   most-reliable signal.
///
/// # Cross-platform support
///
/// `tokio::signal::ctrl_c` works on **both Unix and Windows** under the
/// `tokio` `signal` feature, which is enabled via the workspace `tokio`
/// dep's `["full"]` feature set (`Cargo.toml` line 40). Verified
/// 2026-05-04 directly against `docs.rs/tokio/1.52.1/tokio/signal/fn.ctrl_c.html`,
/// which states verbatim: *"While signals are handled very differently
/// between Unix and Windows, both platforms support receiving a signal
/// on 'ctrl-c'. This function provides a portable API for receiving this
/// notification."* No `cfg(unix)` / `cfg(windows)` gating needed.
async fn handle_signals(
    shutdown_signal: tokio::sync::watch::Sender<bool>,
    exit: Arc<dyn ProcessExit>,
) {
    // First Ctrl-C — graceful shutdown request.
    if tokio::signal::ctrl_c().await.is_err() {
        // Signal stream broken (rare; signal handler couldn't install
        // on this platform). Exit silently — process will rely on
        // explicit `ApplicationHandle::shutdown` for cleanup.
        return;
    }
    eprintln!(
        "[vault-app] graceful shutdown requested (SIGINT received); awaiting in-flight cascade drain. \
         Press Ctrl-C again to force exit."
    );
    let _ = shutdown_signal.send(true);

    // Second Ctrl-C — forced exit because graceful shutdown didn't
    // complete fast enough (or the user is in a hurry).
    if tokio::signal::ctrl_c().await.is_err() {
        return;
    }
    eprintln!(
        "[vault-app] forced exit triggered (second SIGINT received before graceful shutdown completed). \
         Exit code 130 (128 + SIGINT)."
    );
    exit.exit(130);
}
