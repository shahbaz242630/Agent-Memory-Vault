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

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;
use vault_consolidator::{
    write_report_atomic, ConsolidationReport, Consolidator, ConsolidatorConfig,
};
use vault_core::{Boundary, VaultError, VaultResult};
use vault_embedding::{
    BgeSmallProvider, EmbeddingProvider, Qwen3RerankerProvider, RerankProvider, EMBEDDING_DIM,
};
use vault_llm::{LlmProvider, Phi4MiniConfig, Phi4MiniProvider};
use vault_mcp::{Adapter, StdioServer};
use vault_retrieval::{
    AbstainingRetriever, FilesystemReportLoader, HybridRetriever, KeywordIndex, KeywordRetriever,
    Retriever, SemanticRetriever, StructuredReadPipeline,
};
use vault_storage::{MemoryFilter, MetadataStore, RetryWorker, StorageBackend};

use crate::consolidator_lock::ConsolidatorLock;
use crate::process_exit::{LiveProcessExit, ProcessExit};
use crate::signal_source::{LiveSignalSource, SignalSource};
use crate::{AppConfig, VaultAdapter};

/// Hard upper bound on a single consolidation run per the locked-next-arc
/// Step 4 operational-safety contract (2026-05-26): 30 minutes. Past this,
/// the run is cancelled and [`VaultError::ConsolidatorTimeout`] returned.
/// Per-merge transactions already committed remain committed (ADR-046
/// atomic supersession); uncommitted work rolls back via storage primitives'
/// transaction wrappers; atomic REPORT artifact writes (`.tmp + rename` at
/// Commit 4) preserve the previous artifact intact under cancellation.
pub(crate) const CONSOLIDATOR_HARD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Wrap an inner consolidation future in a hard timeout. If the inner
/// future completes before `timeout_dur`, returns its [`VaultResult`]
/// verbatim. If the timeout fires first, drops the inner future and
/// returns [`VaultError::ConsolidatorTimeout`] with the elapsed-budget
/// seconds.
///
/// Factored out from [`Application::run_consolidation_with_safety`] so
/// the timeout semantics are independently testable with sub-second
/// budgets (the production const is 30 min — untestable end-to-end).
async fn timeout_or_consolidator_timeout<F, T>(timeout_dur: Duration, inner: F) -> VaultResult<T>
where
    F: std::future::Future<Output = VaultResult<T>>,
{
    match tokio::time::timeout(timeout_dur, inner).await {
        Ok(inner_result) => inner_result,
        Err(_elapsed) => Err(VaultError::ConsolidatorTimeout(timeout_dur.as_secs())),
    }
}

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
    /// Consolidator wired when [`AppConfig::phi4_model_path`] is `Some` at
    /// construction. Cloned out by [`Self::run_consolidation_with_safety`]
    /// for each invocation. `None` at integration-test time (no Phi-4
    /// GGUF on disk); in that case the safety wrapper returns
    /// [`VaultError::Config`] — graceful degradation per the locked-next-arc
    /// Thread 3 enterprise practice (fail-open on quality-degrading
    /// dependencies, signal it loudly, do not block startup).
    ///
    /// Added at T0.3.x Batch A (2026-05-26) per the architectural lock
    /// (Phi-4-mini stays at consolidation, Qwen-7B exits the read path).
    consolidator: Option<Arc<Consolidator>>,
    /// Vault root directory — derived from `AppConfig::metadata_path.parent()`
    /// at construction. Used by [`Self::run_consolidation_with_safety`] to
    /// place the cross-process [`ConsolidatorLock`] file. Captured here so
    /// the safety wrapper doesn't require `AppConfig` to be threaded
    /// through the lifecycle.
    vault_root: PathBuf,
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
        //
        //    T0.2.0 Phase 2 (2026-05-11): flipped from plaintext
        //    `StorageBackend::open` to sealed `open_with_at_rest_key`
        //    per ADR-040 amendment ("at_rest_key flows from keychain
        //    through AppConfig to migration consumer"). LanceDB is now
        //    AEAD-sealed at-rest via SealedFileStoreProvider; SQLCipher
        //    metadata + DuckDB graph remain unchanged at Phase 2.
        //    Plaintext `StorageBackend::open` is retained for the V0.1
        //    → V0.2 migration source path (see vault_storage::migration);
        //    Phase 3 deletes both plaintext constructors.
        let storage = StorageBackend::open_with_at_rest_key(
            &config.metadata_path,
            &config.vector_dir,
            &config.graph_path,
            config.key.clone(),
            EMBEDDING_DIM,
            &config.at_rest_key,
        )
        .await?;

        // 1b. Derive vault_root from metadata_path's parent. Moved up
        //     from former-step-12 at Commit 6 (locked-next-arc, 2026-05-26)
        //     because the new StructuredReadPipeline (step 9) needs it to
        //     wire FilesystemReportLoader; the Consolidator lockfile
        //     (step 11) also consumes it. By this line StorageBackend::open
        //     above has already used metadata_path, so its parent is
        //     guaranteed to exist on disk — the parent() check guards
        //     against the edge case where metadata_path has no parent
        //     component (e.g., a bare filename without a directory).
        let vault_root = config
            .metadata_path
            .parent()
            .ok_or_else(|| {
                VaultError::Config(
                    "AppConfig.metadata_path must have a parent directory for \
                     consolidator lockfile placement + structured read pipeline \
                     REPORT-loader root"
                        .into(),
                )
            })?
            .to_path_buf();

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
        //    DO NOT open a second `LanceVectorStore::open_with_at_rest_key(vector_dir, …)`
        //    handle here. LanceDB does not officially support concurrent
        //    dataset handles to the same data directory; the `Arc<dyn
        //    VectorStore>` already in `StorageBackend` is the correct
        //    sharing primitive. Future refactors that "helpfully" open a
        //    second handle will surface as fragmentation / write-races
        //    under load — see the integration spike at
        //    `tests/integration_smoke.rs` trigger (b)/(c).
        let vector_store = storage.vector_store().clone();
        let semantic =
            SemanticRetriever::new(retriever_metadata.clone(), embedder.clone(), vector_store);
        let semantic: Arc<dyn Retriever> = Arc::new(semantic);

        // 6. KeywordIndex (T0.2.7 Phase 1) — in-RAM BM25 over all
        //    memory content. Bulk-loaded from the encrypted SQLite
        //    metadata store at startup; subsequent writes/updates/
        //    deletes maintain the index incrementally (vault-app's
        //    write path is wired in a follow-on phase — Phase 1 left
        //    a documented gap that lands when the read-path validation
        //    proves the architecture).
        //
        //    Per [[run-cargo-gates-in-background]] memory: the bulk-
        //    load completes in ~1 sec at 10K memories, ~10 sec at 100K
        //    — fine for V0.2 beta scale. Future on-disk sealed-sidecar
        //    persistence is deferred until startup-rebuild cost
        //    matters in practice.
        let keyword_index = Arc::new(KeywordIndex::new()?);
        let all_memories = retriever_metadata
            .list_memories(MemoryFilter::default(), None)
            .await?;
        keyword_index.bulk_insert(&all_memories).await?;
        drop(all_memories);
        let keyword = KeywordRetriever::new(keyword_index.clone(), retriever_metadata);
        let keyword: Arc<dyn Retriever> = Arc::new(keyword);

        // 7. HybridRetriever — fuses semantic + keyword via Reciprocal
        //    Rank Fusion (k=60, top_n_each=200) per T0.2.7 Phase 2.
        let hybrid: Arc<dyn Retriever> =
            Arc::new(HybridRetriever::new(semantic.clone(), keyword.clone()));

        // 8. AbstainingRetriever — gates on top-1 BM25 score; below
        //    threshold (default 6.0) returns empty result so the LLM
        //    isn't asked to synthesise from a hard-negative corpus
        //    (T0.2.7 Phase 3). Wraps the hybrid; probes the keyword
        //    channel directly for the threshold check.
        let retriever: Arc<dyn Retriever> = Arc::new(AbstainingRetriever::new(hybrid, keyword));

        // 9. StructuredReadPipeline — deterministic filter+pack for the
        //    `memory_read` MCP tool per ADR-052 + ADR-054 (Commit 6 of
        //    the locked-next-arc, 2026-05-26). Replaces the V0.2-era
        //    Qwen-7B single-call synthesis pipeline (ADR-048 + ADR-049,
        //    formally retired by ADR-052) with code that:
        //
        //    - loads the per-boundary REPORT artifact via
        //      [`FilesystemReportLoader`] from
        //      `<vault_root>/reports/<boundary>.report.json`,
        //    - enriches each retrieved candidate with its
        //      consolidator-discovered topic label, and
        //    - emits the six ADR-054 Contract 2 health-warnings
        //      (REPORT_MISSING, REPORT_STALE_*, TOPIC_NAMES_UNAVAILABLE,
        //      CLOCK_SKEW_DETECTED). DELTA_LOG_UNAVAILABLE was retired by
        //      ADR-054 Amendment 2 (Commit 7) when Plan Iteration 3
        //      Contract 4 was falsified by the shipped Commit 6 shape.
        //
        //    No LLM in this stage. The pipeline is always constructed
        //    (no Option) — no model loading, no fallible setup. The
        //    `AppConfig.qwen_model_path` field is now dead (kept with
        //    #[allow(dead_code)] until Commit 8 removes it).
        let report_loader = Arc::new(FilesystemReportLoader::new(vault_root.clone()));
        // Relevance gate. Production (ADR-057 amendment, 2026-05-29): the
        // cross-encoder reranker (Qwen3-Reranker-0.6B) is the relevance gate —
        // it separates topically-adjacent-but-wrong facts that the cosine floor
        // could not. When both rerank paths are configured, open the reranker
        // and wire `with_reranker`; otherwise fall back to the cosine
        // `with_relevance_gate(semantic)` so a deployment without the ~1.2 GB
        // model still abstains on no-signal queries (graceful degradation).
        let base_pipeline = StructuredReadPipeline::new(retriever.clone(), report_loader);
        let read_pipeline = match (&config.rerank_model_path, &config.rerank_tokenizer_path) {
            (Some(rerank_model), Some(rerank_tokenizer)) => {
                let reranker: Arc<dyn RerankProvider> = Arc::new(Qwen3RerankerProvider::open(
                    rerank_model,
                    rerank_tokenizer,
                    &config.ort_lib_path,
                )?);
                tracing::info!(
                    target: "vault_app::startup",
                    "read relevance gate: cross-encoder reranker (Qwen3-Reranker-0.6B, ADR-057 amendment)"
                );
                base_pipeline.with_reranker(reranker)
            }
            _ => {
                tracing::info!(
                    target: "vault_app::startup",
                    "read relevance gate: cosine floor (no reranker model configured — graceful fallback)"
                );
                base_pipeline.with_relevance_gate(semantic)
            }
        };
        tracing::info!(
            target: "vault_app::startup",
            vault_root = %vault_root.display(),
            "structured read pipeline wired (deterministic filter+pack, no LLM)"
        );

        // 10. VaultAdapter — composes the trait deps + optional read
        //    pipeline into the MCP Adapter surface. Clone the
        //    StorageBackend so Application retains a handle for
        //    `start()` to construct the worker against. The
        //    `#[derive(Clone)]` on StorageBackend is `Arc<Inner>`-
        //    shallow per cascading.rs:149 — both clones share the
        //    same retry_queue so writes via the adapter are drained
        //    by the worker constructed from Application's clone.
        let adapter_storage = storage.clone();
        let adapter = VaultAdapter::new(
            retriever,
            read_pipeline,
            embedder.clone(),
            adapter_storage,
            adapter_metadata,
            // Same Arc the retriever's keyword channel holds — inline
            // upsert/delete here keeps a fresh write searchable in the
            // same process (read-after-write fix, 2026-05-28).
            keyword_index.clone(),
        );

        // 11. Optional Consolidator (T0.3.x Batch A, 2026-05-26).
        //
        //    When `phi4_model_path` is `Some`, load Phi-4-mini-instruct
        //    at startup so the nightly consolidation workload doesn't
        //    pay model-load cost per run, then construct a
        //    `vault_consolidator::Consolidator` with the shared storage
        //    + embedder + a default `ConsolidatorConfig` (BRD §5.6
        //    defaults: 3 AM, 0.92 similarity, 180-day decay, 365-day
        //    archive, 1000 memories/run).
        //
        //    When `None`, the consolidator is unwired and
        //    `run_consolidation_with_safety` surfaces `VaultError::Config`
        //    — graceful degradation per the locked-next-arc Thread 3
        //    enterprise practice. Write + read paths remain fully
        //    functional; only nightly consolidation is unavailable.
        //
        //    Per the architectural lock (2026-05-26): Phi-4-mini stays
        //    at consolidation (cheap, offline, real quality contribution
        //    on the binary merge-classifier role); Qwen-7B exits the
        //    read path entirely. Read still uses Qwen via the existing
        //    `ReadPipeline` wiring above at step 9; Commit 6 of the
        //    locked-next-arc removes that and replaces it with a
        //    deterministic structured-fact pipeline.
        let consolidator = match &config.phi4_model_path {
            Some(path) => {
                tracing::info!(
                    target: "vault_app::startup",
                    phi4_model_path = %path.display(),
                    "loading Phi-4-mini for consolidator"
                );
                let model_dir = path
                    .parent()
                    .ok_or_else(|| {
                        VaultError::Config(
                            "AppConfig.phi4_model_path must have a parent directory".into(),
                        )
                    })?
                    .to_path_buf();
                let model_filename = path
                    .file_name()
                    .ok_or_else(|| {
                        VaultError::Config(
                            "AppConfig.phi4_model_path must have a filename component".into(),
                        )
                    })?
                    .to_string_lossy()
                    .into_owned();
                let mut phi4_config = Phi4MiniConfig::v0_2_default(model_dir);
                phi4_config.model_filename = model_filename;
                let phi4_provider = Phi4MiniProvider::new(phi4_config).await.map_err(|e| {
                    VaultError::Llm(format!("Phi-4-mini load failed at startup: {e}"))
                })?;
                let llm: Arc<dyn LlmProvider> = Arc::new(phi4_provider);
                let cons = Consolidator::new(
                    Arc::new(storage.clone()),
                    llm,
                    embedder,
                    ConsolidatorConfig::default(),
                );
                Some(Arc::new(cons))
            }
            None => {
                tracing::info!(
                    target: "vault_app::startup",
                    "phi4_model_path is None; consolidator not wired (graceful degradation \
                     per locked-next-arc Thread 3 — write/read remain functional, \
                     `vault-cli consolidate run` returns VaultError::Config)"
                );
                None
            }
        };

        // vault_root was derived at step 1b (moved up at Commit 6 so
        // step 9's StructuredReadPipeline could use it). The Consolidator
        // lockfile in `run_consolidation_with_safety` continues to consume
        // the same value via `self.vault_root`.

        Ok(Self {
            adapter: Arc::new(adapter),
            storage,
            consolidator,
            vault_root,
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
        //    Production wires `LiveProcessExit` (Phase 4a) +
        //    `LiveSignalSource` (Phase 4b). Tests construct
        //    `CapturingProcessExit` + `MockSignalSource` to drive the
        //    handler through both Ctrl-C paths without OS signals.
        let signal_tx = shutdown_signal.clone();
        let exit_impl: Arc<dyn ProcessExit> = Arc::new(LiveProcessExit);
        let signal_impl: Arc<dyn SignalSource> = Arc::new(LiveSignalSource);
        let signal_handle = tokio::spawn(handle_signals(signal_tx, exit_impl, signal_impl));

        Ok(ApplicationHandle {
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
        })
    }

    /// Run one consolidation cycle under cross-process lockfile +
    /// [`CONSOLIDATOR_HARD_TIMEOUT`] (30 min). Returns the underlying
    /// [`vault_consolidator::ConsolidationReport`] on success.
    ///
    /// # Operational safety (locked-next-arc Step 4, 2026-05-26)
    ///
    /// - **Cross-process lockfile** at `<vault_root>/.consolidator.lock` —
    ///   refuses with [`VaultError::ConsolidatorBusy`] if held. Released
    ///   on drop (RAII guard via [`ConsolidatorLock`]) including under
    ///   panic unwind. Stale lockfiles (holder crashed without cleanup)
    ///   require manual removal — explicit operator action per the
    ///   `consolidator_lock` module docs.
    /// - **30-min hard timeout** — past this, the run is cancelled and
    ///   [`VaultError::ConsolidatorTimeout`] returned. Per-merge
    ///   transactions already committed remain committed (ADR-046);
    ///   uncommitted work rolls back via storage primitives' tx wrappers.
    /// - **Tracing span** tagged with `run_id = Uuid::new_v4()` propagates
    ///   to every consolidator phase log line for end-to-end correlation.
    ///
    /// # Errors
    ///
    /// - [`VaultError::Config`] — consolidator not wired
    ///   ([`AppConfig::phi4_model_path`] was `None` at construction).
    /// - [`VaultError::ConsolidatorBusy`] — another run holds the lockfile.
    /// - [`VaultError::ConsolidatorTimeout`] — exceeded the 30-min budget.
    /// - Any [`VaultError`] propagated by
    ///   [`vault_consolidator::Consolidator::run_consolidation`].
    #[tracing::instrument(skip_all)]
    pub async fn run_consolidation_with_safety(&self) -> VaultResult<ConsolidationReport> {
        let consolidator = self.consolidator.as_ref().ok_or_else(|| {
            VaultError::Config(
                "consolidator not configured (AppConfig.phi4_model_path was None at \
                 Application::new); set phi4_model_path to enable nightly consolidation"
                    .into(),
            )
        })?;

        let run_id = Uuid::new_v4();
        tracing::info!(
            target: "vault_app::consolidator",
            run_id = %run_id,
            "consolidation run starting under safety wrapper"
        );

        // Acquire the cross-process lockfile. The guard is held for the
        // entire run; dropped on function exit (success / error / panic
        // unwind) which removes the lockfile.
        let _lock = ConsolidatorLock::try_acquire(&self.vault_root)?;

        // Wrap the consolidator's run_consolidation + per-boundary REPORT
        // generation in one hard timeout — both phases call the LLM and
        // re-embed, so both belong under the same cancellation budget. We
        // .clone() the Arc<Consolidator> so the future is 'static-friendly
        // (no borrow on self threaded through tokio::timeout's internal
        // future polling). generate_reports runs AFTER run_consolidation so
        // the topics + facts reflect the post-merge / post-invalidate state
        // (ADR-058).
        let consolidator = consolidator.clone();
        let inner = async move {
            let report = consolidator.run_consolidation().await?;
            let reports = consolidator.generate_reports(run_id).await?;
            Ok::<_, VaultError>((report, reports))
        };
        let (report, reports) =
            timeout_or_consolidator_timeout(CONSOLIDATOR_HARD_TIMEOUT, inner).await?;

        // Persist each per-boundary REPORT atomically to the vault root.
        // The filesystem write lives in this app layer (it owns
        // `vault_root`); the consolidator stays filesystem-agnostic. A
        // single REPORT write failure is logged-and-continued rather than
        // aborting the whole run — mirrors the contradiction-invalidate
        // philosophy (a transient failure is retried next cycle, and a
        // missing REPORT surfaces as REPORT_MISSING at read time, which is
        // the correct degraded signal). The merge work already committed to
        // storage above is durable regardless.
        for report_artifact in &reports {
            match write_report_atomic(report_artifact, &self.vault_root) {
                Ok(path) => tracing::info!(
                    target: "vault_app::consolidator",
                    run_id = %run_id,
                    boundary = %report_artifact.boundary.as_str(),
                    topics = report_artifact.facts_by_topic.len(),
                    path = %path.display(),
                    "per-boundary REPORT written"
                ),
                Err(e) => tracing::warn!(
                    target: "vault_app::consolidator",
                    run_id = %run_id,
                    boundary = %report_artifact.boundary.as_str(),
                    error = %e,
                    "REPORT write failed; REPORT_MISSING will surface at read until the next run succeeds"
                ),
            }
        }

        tracing::info!(
            target: "vault_app::consolidator",
            run_id = %run_id,
            memories_processed = report.memories_processed,
            memories_merged = report.memories_merged,
            contradictions_resolved = report.contradictions_resolved,
            reports_written = reports.len(),
            "consolidation run completed under safety wrapper"
        );

        Ok(report)
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
    /// Test-only constructor. Allows lifecycle tests to assert
    /// `shutdown` semantics without constructing a full `Application`
    /// (which requires SqlCipher + LanceDB + DuckDB + ORT). Caller
    /// passes pre-built `JoinHandle`s; production callers go through
    /// [`Application::start_with_mcp`].
    ///
    /// Phase 4b T0.1.11 — added per multi-agent code-review CRITICAL
    /// finding "vault-app/src/application.rs has zero tests."
    #[cfg(test)]
    pub(crate) fn for_test(
        shutdown_signal: tokio::sync::watch::Sender<bool>,
        worker_handle: tokio::task::JoinHandle<()>,
        server_handle: tokio::task::JoinHandle<()>,
        signal_handle: tokio::task::JoinHandle<()>,
    ) -> Self {
        Self {
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
        }
    }

    /// Borrow the shutdown-signal `Sender`. Useful when an external
    /// supervisor wants to signal cancellation without consuming the
    /// handle (e.g., a parent task that's also tracking other lifecycle
    /// resources).
    pub fn shutdown_signal(&self) -> &tokio::sync::watch::Sender<bool> {
        &self.shutdown_signal
    }

    /// Block until one of the spawned tasks naturally exits, then perform
    /// graceful shutdown. The typical "main loop" entry-point for a CLI
    /// subcommand that runs the vault as a long-lived MCP stdio server
    /// (`vault-cli mcp serve`).
    ///
    /// Selects across:
    /// - **`server_handle`** — completes on stdio EOF (the MCP client,
    ///   typically Claude Desktop, disconnected) or on rmcp-internal task
    ///   panic.
    /// - **`signal_handle`** — completes when the SIGINT handler's future
    ///   resolves (the signal source closed, OR the second-Ctrl-C path
    ///   already called `process_exit` and we never reach here).
    ///
    /// The retry worker is intentionally *not* selected on — under normal
    /// operation it polls indefinitely until [`Self::shutdown_signal`]
    /// flips, which this method does after the select completes. A worker
    /// task exiting on its own is anomalous (panic), surfaced via
    /// [`Self::shutdown`]'s join-error logging.
    ///
    /// Consumes `self` by value to enforce single-call semantics at compile
    /// time (same rationale as [`Self::shutdown`]).
    ///
    /// # Errors
    ///
    /// Propagates [`Self::shutdown`]'s error surface. Currently
    /// [`Self::shutdown`] always returns `Ok(())`, so this is reserved for
    /// future shutdown-fallibility surfacing.
    pub async fn wait(mut self) -> VaultResult<()> {
        tokio::select! {
            _ = &mut self.server_handle => {
                // stdio EOF — client disconnected, or rmcp server task
                // returned. Graceful shutdown of remaining tasks below.
            }
            _ = &mut self.signal_handle => {
                // Signal handler resolved — typically the signal stream
                // broke (rare) or the second-Ctrl-C path called
                // `process_exit` and we never observed the resolution.
            }
        }
        self.shutdown().await
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
        //
        //    `is_finished()` guard (2026-05-28, Codex dogfood): when reached
        //    via `wait()`, the `select!` already polled one of these handles
        //    to completion by `&mut` (stdio EOF completes `server_handle`).
        //    Re-awaiting an already-finished `JoinHandle` panics ("JoinHandle
        //    polled after completion"). Skip the await when the task is already
        //    finished; on the direct-`shutdown()` path the freshly-aborted
        //    handles are not yet finished, so they're awaited to confirm
        //    cancellation exactly as before.
        if !self.server_handle.is_finished() {
            let _ = self.server_handle.await;
        }
        if !self.signal_handle.is_finished() {
            let _ = self.signal_handle.await;
        }

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
    signals: Arc<dyn SignalSource>,
) {
    // First Ctrl-C — graceful shutdown request.
    if signals.next_signal().await.is_err() {
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
    if signals.next_signal().await.is_err() {
        return;
    }
    eprintln!(
        "[vault-app] forced exit triggered (second SIGINT received before graceful shutdown completed). \
         Exit code 130 (128 + SIGINT)."
    );
    exit.exit(130);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process_exit::CapturingProcessExit;
    use crate::signal_source::MockSignalSource;

    // =========================================================================
    // Consolidator safety wrapper — timeout helper unit tests (T0.3.x Batch A)
    //
    // These tests pin `timeout_or_consolidator_timeout`'s contract independently
    // of the full `Application::run_consolidation_with_safety` path so we can
    // exercise the timeout behaviour with sub-second budgets (the production
    // const is 30 min — untestable end-to-end). The lockfile contract is pinned
    // in `consolidator_lock::tests`. End-to-end wiring is exercised at Batch A
    // Commit 2 (vault-cli consolidate run subcommand) where a real Application
    // is constructed against a tempdir backend.
    // =========================================================================

    #[tokio::test]
    async fn timeout_or_returns_inner_value_when_inner_completes_before_budget() {
        let fast_inner = async { Ok::<u32, VaultError>(42) };
        let result = timeout_or_consolidator_timeout(Duration::from_secs(60), fast_inner).await;
        assert_eq!(
            result.unwrap(),
            42,
            "inner future completing within budget MUST return its value verbatim"
        );
    }

    #[tokio::test]
    async fn timeout_or_returns_consolidator_timeout_when_inner_exceeds_budget() {
        let slow_inner = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok::<(), VaultError>(())
        };
        let result = timeout_or_consolidator_timeout(Duration::from_millis(50), slow_inner).await;
        match result {
            Err(VaultError::ConsolidatorTimeout(secs)) => {
                // 50ms rounds to 0 seconds under `as_secs()`. The point of the
                // assertion is the variant + that the value is what we passed
                // in, not the exact ms-vs-secs precision.
                assert_eq!(
                    secs, 0,
                    "ConsolidatorTimeout payload MUST be the budget's as_secs() value"
                );
            }
            other => panic!("expected VaultError::ConsolidatorTimeout, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_or_propagates_inner_error_verbatim_when_inner_errs_before_timeout() {
        let inner = async { Err::<u32, _>(VaultError::Storage("simulated".into())) };
        let result = timeout_or_consolidator_timeout(Duration::from_secs(60), inner).await;
        match result {
            Err(VaultError::Storage(msg)) => assert_eq!(
                msg, "simulated",
                "inner error MUST propagate verbatim when it fires before timeout"
            ),
            other => panic!("expected VaultError::Storage, got: {other:?}"),
        }
    }

    // =========================================================================
    // Lifecycle test 1 (v2 test 10) — `start_with_mcp` McpBindFailed path
    //
    // PHASE 4B SCOPE DEFERRAL per Shahbaz approval at v2-greenlit-step-expansion
    // review (2026-05-05): rmcp's `ServiceExt::serve` transport-error mock would
    // require either (a) implementing a mock transport against rmcp's Layer/
    // Service trait surface (research-spike scope, not implementation), or
    // (b) closing stdin to force serve failure (test-environment-fragile across
    // CI runners), or (c) refactoring `start_with_mcp` to take a transport-
    // builder closure (contract-establishing scope, out of bounds for
    // consume-existing-contracts depth). Per `feedback_forward_compat_concrete_vs_hypothetical.md`,
    // V0.2 alpha-distribution task IS the named concrete consumer where
    // transport-hardening scope is touched; deferral preserves intent without
    // paying the implementation cost now.
    // =========================================================================

    /// **Phase 4b ignored placeholder.** Pin McpBindFailed wiring at
    /// V0.2 alpha-distribution task time when transport-mock infra
    /// is appropriately scoped. See module-level deferral note above.
    #[tokio::test]
    #[ignore = "Phase 4b deferred — needs rmcp transport mock; lands at V0.2 alpha-distribution task"]
    async fn start_with_mcp_returns_mcp_bind_failed_when_serve_errs() {
        unimplemented!(
            "Phase 4b ignored placeholder — V0.2 alpha-distribution task lands the rmcp \
             transport mock. Per ADR-024 + ADR-026 cross-link: McpBindFailed surfaces \
             from rmcp::ServiceExt::serve setup errors; testing requires a swappable \
             transport at start_with_mcp boundary."
        );
    }

    // =========================================================================
    // Lifecycle test 2 (v2 test 11) — `ApplicationHandle::shutdown` drain
    // =========================================================================

    /// Verifies `ApplicationHandle::shutdown` cleanly awaits all three
    /// task handles + sends the shutdown signal. Uses test-only
    /// `for_test` constructor so the test doesn't need a full
    /// Application (SqlCipher + LanceDB + DuckDB + ORT — heavy).
    ///
    /// Mock handles are `tokio::spawn(async { ... })` futures that
    /// observe the shutdown signal and exit cleanly when received,
    /// mirroring the production worker / server / signal-handler
    /// behaviour at the JoinHandle level.
    #[tokio::test]
    async fn application_handle_shutdown_drains_worker() {
        let (shutdown_signal, mut rx) = tokio::sync::watch::channel(false);

        // Mock worker: spawned task that waits for shutdown signal,
        // then exits. Mirrors production RetryWorker::run shape.
        let mut rx_worker = rx.clone();
        let worker_handle = tokio::spawn(async move {
            // Wait for the first true signal.
            while !*rx_worker.borrow_and_update() {
                if rx_worker.changed().await.is_err() {
                    break;
                }
            }
        });

        // Mock server + signal handlers: trivial spawned tasks. In
        // production these are aborted by `shutdown` rather than
        // awaiting cleanly; we use simple pending tasks here so
        // `shutdown`'s abort+await sequence has something to abort.
        let server_handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });
        let signal_handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        let handle = ApplicationHandle::for_test(
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
        );

        // Snapshot the shutdown_signal state pre-shutdown.
        rx.mark_unchanged();
        let pre_state = *rx.borrow();
        assert!(
            !pre_state,
            "Pre-shutdown the channel must be `false`; got {pre_state}"
        );

        // Bound the test wait — if shutdown hangs, fail the test
        // rather than hanging the test runner.
        let shutdown_result =
            tokio::time::timeout(std::time::Duration::from_secs(5), handle.shutdown()).await;

        assert!(
            shutdown_result.is_ok(),
            "ApplicationHandle::shutdown MUST complete within 5s for the \
             happy-path mock-worker scenario; timed out (potential drain \
             regression — worker_handle.await may have hung)."
        );
        assert!(
            shutdown_result.unwrap().is_ok(),
            "ApplicationHandle::shutdown's inner Result MUST be Ok for the \
             clean-exit mock-worker path."
        );

        // Verify the shutdown signal was sent.
        let post_state = *rx.borrow();
        assert!(
            post_state,
            "ApplicationHandle::shutdown MUST have sent `true` over \
             shutdown_signal so the worker observed the drain request; \
             post-shutdown channel state is `false` (regression)."
        );
    }

    /// Regression (Codex dogfood 2026-05-28): `wait()`'s `select!` drives
    /// `server_handle` to completion on stdio EOF; the subsequent
    /// `shutdown()` MUST NOT re-await that already-completed handle — doing so
    /// panics with "JoinHandle polled after completion". Pins the
    /// `is_finished()` guard in `shutdown()`. Pre-fix this test panics.
    #[tokio::test]
    async fn wait_does_not_panic_when_server_handle_completes_first() {
        // `_rx` kept alive so `shutdown_signal.send` has a live receiver.
        let (shutdown_signal, _rx) = tokio::sync::watch::channel(false);

        // worker exits immediately (drain trivially complete).
        let worker_handle = tokio::spawn(async {});
        // server_handle completes immediately == stdio EOF: wait()'s select!
        // drives it to completion via `&mut`.
        let server_handle = tokio::spawn(async {});
        // signal_handle stays pending (the SIGINT path never fires here).
        let signal_handle = tokio::spawn(async {
            std::future::pending::<()>().await;
        });

        let handle = ApplicationHandle::for_test(
            shutdown_signal,
            worker_handle,
            server_handle,
            signal_handle,
        );

        // wait() → select! fires on the completed server_handle → shutdown().
        // MUST return Ok within the bound, never panic on a re-awaited handle.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), handle.wait()).await;
        assert!(
            result.is_ok(),
            "wait() MUST complete (not hang) after server_handle EOF"
        );
        assert!(
            result.unwrap().is_ok(),
            "wait() MUST return Ok after graceful shutdown, not panic re-awaiting \
             the already-completed server_handle"
        );
    }

    // =========================================================================
    // Lifecycle test 3 (v2 test 12) — `handle_signals` double-Ctrl-C path
    // =========================================================================

    /// Pin both Ctrl-C paths in one consolidated test (per v2 test
    /// consolidation per Shahbaz greenlight): first Ctrl-C → shutdown
    /// signal sent; second Ctrl-C → ProcessExit::exit(130) called.
    ///
    /// Uses MockSignalSource + CapturingProcessExit to drive the
    /// handler without OS signals. CapturingProcessExit panics inside
    /// the spawned task on the second Ctrl-C; JoinHandle::await
    /// returns Err(JoinError::panic) which is the expected shape.
    #[tokio::test]
    async fn handle_signals_first_ctrl_c_signals_shutdown_then_second_ctrl_c_force_exits_with_130()
    {
        let (shutdown_signal, mut rx) = tokio::sync::watch::channel(false);
        let exit = CapturingProcessExit::new();
        let captured_handle = exit.captured_handle();

        // Pre-load the queue with two Ok(()) events — first triggers
        // shutdown signal; second triggers force exit.
        let signals: Arc<dyn SignalSource> =
            Arc::new(MockSignalSource::with_queue(vec![Ok(()), Ok(())]));
        let exit_arc: Arc<dyn ProcessExit> = Arc::new(exit);

        // Spawn handle_signals as the production callsite would. The
        // task panics when CapturingProcessExit::exit fires (second
        // Ctrl-C path); the panic is the expected shape.
        let handle = tokio::spawn(handle_signals(shutdown_signal, exit_arc, signals));

        // Wait for the task to complete (panic). Bound the wait so a
        // hung handle_signals fails the test rather than hanging.
        let join_result = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("handle_signals MUST complete within 5s; timeout indicates hang");

        // The spawned task panicked (expected — CapturingProcessExit::exit
        // panics by design to convert the divergence into a JoinError).
        assert!(
            join_result.is_err(),
            "handle_signals task MUST have panicked from CapturingProcessExit::exit \
             on the second Ctrl-C path; instead got Ok(()) — force-exit-130 \
             regression."
        );
        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "JoinError MUST be a panic (CapturingProcessExit::exit panics by design); \
             got: {join_err:?}"
        );

        // First Ctrl-C: shutdown_signal received `true`.
        rx.mark_unchanged();
        let signal_state = *rx.borrow();
        assert!(
            signal_state,
            "ADR-locked first-Ctrl-C path: handle_signals MUST send `true` over \
             shutdown_signal after the first signal event; got `false` (regression)."
        );

        // Second Ctrl-C: ProcessExit::exit(130) was called.
        let captured = *captured_handle.lock().expect("CapturingProcessExit mutex");
        assert_eq!(
            captured,
            Some(130),
            "ADR-locked second-Ctrl-C-130 path (T0.1.10 Phase 2a): handle_signals MUST \
             call ProcessExit::exit(130) after the second signal event; \
             CapturingProcessExit captured {captured:?} instead. Per the operational \
             contract, exit code 130 (128 + SIGINT) distinguishes user-requested forced \
             exit from general failure (1) — wrapper scripts and CI rely on this code."
        );
    }
}
