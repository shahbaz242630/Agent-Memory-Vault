//! `VaultAdapter` — production impl of [`vault_mcp::Adapter`].
//!
//! Wires four trait deps into the concrete vault operations the MCP
//! tool layer dispatches:
//!
//! - **[`Retriever`]** — `Adapter::search` delegates here. Trust-
//!   boundary auth-gating already enforced at the StdioServer layer
//!   (Step 4); VaultAdapter passes through.
//! - **[`EmbeddingProvider`]** — `Adapter::write` / `::update` embed
//!   `content` before calling [`StorageBackend`]'s cascade entry
//!   points (which take a pre-computed embedding).
//! - **[`StorageBackend`]** — write / update / delete cascade across
//!   SQLCipher + LanceDB + DuckDB per ADR-009.
//! - **[`MetadataStore`]** — separate handle (sharing the
//!   `Arc<Inner>` SQLCipher connection at construction time) used
//!   by `append_tool_invoke_audit` for the `mcp.tool_invoke` audit
//!   chain row. Holding a separate handle (rather than calling
//!   `StorageBackend::metadata()`) avoids widening StorageBackend's
//!   public API surface for one consumer; the caller (T0.1.10
//!   `Application::start`) wires both at startup.
//!
//! ## Trust boundary (ADR-025)
//!
//! `authorized_boundaries` enforcement lives at
//! [`vault_mcp::StdioServer`] (Step 4). Every method on `VaultAdapter`
//! receives already-trusted shapes — `RetrievalQuery` with the
//! application-supplied boundary slice, `NewMemory` with a boundary
//! the handler has verified against the trusted slice. **VaultAdapter
//! MUST NOT** re-derive boundaries from request data; the handler is
//! the single auth-gate site per ADR-025.
//!
//! `append_tool_invoke_audit` records `actor_kind = ActorKind::Agent`
//! per the ADR-025 Step 6 application: the MCP client is an
//! untrusted agent per the trust-boundary contract; user attribution
//! lives in the boundary scope (`authorized_boundaries`), not the
//! audit-row actor field.
//!
//! ## Update semantics (ADR-028)
//!
//! `Adapter::update` does **read-before-write** to preserve
//! provenance and lineage. The full per-field classification of
//! preserved vs overwritten lives in **ADR-028** (HANDOFF.md). The
//! invariant the implementation upholds:
//!
//! - **OVERWRITE** only fields exposed by the MCP write/update wire
//!   schema (per ADR-024 tool-param contract: `content`,
//!   `memory_type`, `boundary`, `confidence`) PLUS system-managed
//!   fields update is expected to advance (`last_accessed = now`,
//!   `embedding` re-computed from new content).
//! - **PRESERVE** everything else: `id`, `source_agent`,
//!   `created_at`, `valid_from`, `valid_until`, `access_count`,
//!   `superseded_by`, `metadata`.
//!
//! `metadata.get_memory(id)` returning `None` produces
//! `VaultError::NotFound` — surfaces to the MCP client as `-32602
//! "not found"` per ADR-024's mapping.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use vault_core::{Boundary, Memory, MemoryId, NewMemory, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_mcp::{Adapter, ToolInvokeDetails};
use vault_retrieval::{
    ReadPipeline, ReadQuery, ReadResponse, RetrievalQuery, RetrievedMemory, Retriever,
};
use vault_storage::{
    ActorKind, AuditEventType, AuditResult, MetadataStore, PendingAuditEvent, StorageBackend,
};

/// Production `vault_mcp::Adapter` impl. Constructed by
/// `Application::start` at startup (T0.1.10) with concrete trait deps.
///
/// Cheap to clone — the Retriever and EmbeddingProvider are
/// `Arc`-shared, StorageBackend and MetadataStore both clone via
/// `Arc<Inner>` internals. Multiple StdioServer instances (V0.2+)
/// can hold clones without locking.
pub struct VaultAdapter {
    retriever: Arc<dyn Retriever>,
    /// Optional read pipeline for the `memory.read` MCP tool. When
    /// `None`, `Adapter::read` returns
    /// `VaultError::Config("read pipeline not configured")`. The field
    /// is `Option` because:
    /// - Integration tests don't have the 4.36 GB Qwen GGUF on disk;
    ///   they wire `qwen_model_path: None` and skip read-pipeline
    ///   testing.
    /// - Future deployments may opt out of local LLM inference (cloud
    ///   tier V0.3+).
    ///
    /// Added at T0.2.7 Phase 4 (2026-05-20).
    read_pipeline: Option<ReadPipeline>,
    embedding: Arc<dyn EmbeddingProvider>,
    storage: StorageBackend,
    metadata: MetadataStore,
}

impl VaultAdapter {
    /// Construct from the four trait deps + an optional read pipeline.
    /// Caller (T0.1.10 `Application::start`) is responsible for wiring
    /// concrete implementations and passing a `MetadataStore` handle
    /// that points at the same encrypted SQLite file used in the
    /// `StorageBackend` open.
    ///
    /// **T0.2.7 Phase 4 addition (2026-05-20):** the `read_pipeline`
    /// parameter accepts `None` for absent-LLM deployments (tests,
    /// cloud tier). When `None`, `memory.read` MCP calls return
    /// `VaultError::Config("read pipeline not configured")`.
    pub fn new(
        retriever: Arc<dyn Retriever>,
        read_pipeline: Option<ReadPipeline>,
        embedding: Arc<dyn EmbeddingProvider>,
        storage: StorageBackend,
        metadata: MetadataStore,
    ) -> Self {
        Self {
            retriever,
            read_pipeline,
            embedding,
            storage,
            metadata,
        }
    }
}

#[async_trait]
impl Adapter for VaultAdapter {
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // Trust-boundary auth-gating already done at the StdioServer
        // layer (Step 4). Pass through to the Retriever.
        self.retriever.retrieve(query).await
    }

    async fn read(&self, query: ReadQuery) -> VaultResult<ReadResponse> {
        // Trust-boundary auth-gating already done at the StdioServer
        // layer per ADR-025 (handle_read populates query.authorized_
        // boundaries from the trusted slice). Pass through to the
        // wired ReadPipeline; if no pipeline was configured (absent
        // GGUF in tests / opted-out deployments), return Config error.
        match &self.read_pipeline {
            Some(pipeline) => pipeline.read(query).await,
            None => Err(VaultError::Config(
                "read pipeline not configured (AppConfig.qwen_model_path was None at \
                 Application::new)"
                    .into(),
            )),
        }
    }

    async fn write(&self, new_memory: NewMemory) -> VaultResult<MemoryId> {
        // Memory::try_new applies validation + generates a fresh
        // MemoryId. Embedding is computed from validated content.
        let memory = Memory::try_new(new_memory)?;
        let embedding = self.embedding.embed(&memory.content).await?;
        self.storage.write_memory(&memory, &embedding).await?;
        Ok(memory.id)
    }

    async fn update(&self, id: MemoryId, new_memory: NewMemory) -> VaultResult<()> {
        // ADR-028 read-before-write contract: read existing → patch
        // preserved fields → re-compute embedding → cascade update.

        // Read existing. NotFound surfaces as VaultError::NotFound,
        // which maps to `-32602 "not found"` at the MCP wire layer
        // per ADR-024.
        let existing = self
            .metadata
            .get_memory(&id)
            .await?
            .ok_or_else(|| VaultError::NotFound(format!("memory {id} not found")))?;

        // Build the candidate updated Memory through try_new_with_id
        // so MCP-supplied fields go through the canonical validation
        // path. Then patch the ADR-028 PRESERVED fields from the
        // existing row.
        let mut updated = Memory::try_new_with_id(id, new_memory)?;

        // ADR-028 PRESERVED — keep from existing row.
        updated.source_agent = existing.source_agent;
        updated.created_at = existing.created_at;
        updated.valid_from = existing.valid_from;
        updated.valid_until = existing.valid_until;
        updated.access_count = existing.access_count;
        updated.superseded_by = existing.superseded_by;
        updated.metadata = existing.metadata;

        // ADR-028 OVERWRITE → now.
        updated.last_accessed = Utc::now();

        // Re-validate after the patches. try_new_with_id validated
        // the new fields (content / memory_type / boundary /
        // confidence) but the patched temporal fields could in
        // theory introduce a `valid_until < valid_from` violation if
        // the existing row violates the invariant (a regression
        // somewhere upstream). Treat that as a hard failure rather
        // than silently shipping invalid data.
        updated.validate()?;

        // Re-compute embedding from the new content. ADR-028:
        // stale embedding is a correctness bug.
        let embedding = self.embedding.embed(&updated.content).await?;

        self.storage.update_memory(&updated, &embedding).await?;
        Ok(())
    }

    async fn delete(&self, id: MemoryId) -> VaultResult<()> {
        // StorageBackend::delete_memory is idempotent at the
        // cascade layer per cascading.rs:323-329 — deleting a
        // non-existent id still returns Ok with details.deleted =
        // false. Pass through.
        self.storage.delete_memory(&id).await?;
        Ok(())
    }

    /// ADR-025 amendment 2026-05-05: returns the memory's stored
    /// boundary so the StdioServer handler can auth-gate `memory.delete`
    /// before dispatching to `delete`. Reads through the same
    /// `MetadataStore` handle used by `update`'s read-before-write path.
    async fn lookup_boundary(&self, id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(self.metadata.get_memory(&id).await?.map(|m| m.boundary))
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.append_audit_with_event_type(AuditEventType::McpToolInvoke, details, ActorKind::Agent)
            .await
    }
}

impl VaultAdapter {
    /// **ADR-024 amendment 2026-05-05 (T0.1.11 Phase 4b — Decision 5(γ)).**
    /// Generic audit-write helper used by both the trait method
    /// `append_tool_invoke_audit` (event_type = McpToolInvoke) AND the
    /// inherent method `append_tauri_command_audit` below. Encapsulates
    /// the PendingAuditEvent construction shape per ADR-024 + BRD §11.9.2
    /// audit-chain hash determinism.
    async fn append_audit_with_event_type(
        &self,
        event_type: AuditEventType,
        details: ToolInvokeDetails,
        actor_kind: ActorKind,
    ) -> VaultResult<()> {
        let result = if details.error.is_some() {
            AuditResult::Error
        } else {
            AuditResult::Success
        };
        let details_json = details.to_canonical_json()?;
        let pending = PendingAuditEvent {
            event_type,
            resource_type: None,
            resource_id: None,
            boundary: None,
            actor_kind,
            actor_name: None,
            user_id: None,
            device_id: None,
            result,
            details_json,
        };
        self.metadata.append_audit_event(pending).await?;
        Ok(())
    }

    /// **ADR-024 amendment 2026-05-05 (T0.1.11 Phase 4b — Decision 5(γ)).**
    /// Append a `TauriCommandInvoke` audit row for vault-state-changing
    /// Tauri commands (add_memory / search_memories / update_memory /
    /// delete_memory). Tauri commands aren't MCP — reusing
    /// `mcp.tool_invoke` would create semantic debt at V0.2 cloud sync;
    /// the new variant gives Tauri commands their own discriminator.
    /// `actor_kind = User` (founder is the actor; not an untrusted agent
    /// like ADR-025 specifies for MCP).
    pub async fn append_tauri_command_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.append_audit_with_event_type(
            AuditEventType::TauriCommandInvoke,
            details,
            ActorKind::User,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tempfile::TempDir;
    use vault_core::{Boundary, MemoryType};
    use vault_embedding::EMBEDDING_DIM;
    use vault_storage::SqlCipherKey;

    /// Test-only at-rest key (32 bytes, fixed pattern). Per-mod local
    /// const per HANDOFF sub-task (d) §"Const placement" decision lock;
    /// matches the convention in `vault-storage/tests/migration_v0_1_to_sealed.rs:96`
    /// and `vault-cli/src/main.rs:497`.
    const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

    // -----------------------------------------------------------------
    // Stub trait impls used across tests
    // -----------------------------------------------------------------

    /// Stub retriever returning a caller-supplied response. Records
    /// the queries it received for later assertion.
    struct StubRetriever {
        response: Vec<RetrievedMemory>,
        queries: std::sync::Mutex<Vec<RetrievalQuery>>,
    }

    impl StubRetriever {
        fn with_response(response: Vec<RetrievedMemory>) -> Self {
            Self {
                response,
                queries: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded_queries(&self) -> Vec<RetrievalQuery> {
            self.queries.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Retriever for StubRetriever {
        async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
            self.queries.lock().unwrap().push(query);
            Ok(self.response.clone())
        }
    }

    /// Stub embedder returning a deterministic L2-normalised vector.
    /// First slot is `1.0`, rest zeros — unit norm. Different inputs
    /// produce the same vector (irrelevant for these tests; the
    /// real embedder is exercised in vault-embedding).
    struct StubEmbedder {
        calls: AtomicU64,
    }

    impl StubEmbedder {
        fn new() -> Self {
            Self {
                calls: AtomicU64::new(0),
            }
        }

        fn call_count(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, _text: &str) -> VaultResult<Vec<f32>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let mut v = vec![0.0_f32; EMBEDDING_DIM];
            v[0] = 1.0;
            Ok(v)
        }
    }

    // -----------------------------------------------------------------
    // Test fixture: open fresh tempdir-backed StorageBackend + a
    // SECOND MetadataStore handle for VaultAdapter. The two
    // MetadataStore handles are SEPARATE SQLCipher connections to
    // the same physical DB file (V0.1 single-user serial MCP =
    // never concurrent appends; V0.2+ revisit per ADR-028).
    // -----------------------------------------------------------------

    struct Fixture {
        _tmp: TempDir,
        adapter: VaultAdapter,
        retriever: Arc<StubRetriever>,
        embedder: Arc<StubEmbedder>,
        // Held for direct read-back assertions in tests.
        metadata_for_assert: MetadataStore,
    }

    async fn make_fixture(retriever_response: Vec<RetrievedMemory>) -> Fixture {
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("vault-app-adapter-test-key");

        // StorageBackend opens its own MetadataStore internally. Open
        // a SECOND MetadataStore for VaultAdapter.metadata. Both
        // point at the same SQLCipher file via separate connections.
        let storage = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key.clone(),
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();
        let metadata = MetadataStore::open(&metadata_path, key.clone())
            .await
            .unwrap();
        let metadata_for_assert = MetadataStore::open(&metadata_path, key).await.unwrap();

        let retriever = Arc::new(StubRetriever::with_response(retriever_response));
        let embedder = Arc::new(StubEmbedder::new());

        let adapter = VaultAdapter::new(
            retriever.clone() as Arc<dyn Retriever>,
            // T0.2.7 Phase 4: read_pipeline = None for unit tests
            // (no Qwen GGUF, no need for the read path here). The
            // VaultAdapter::read tests live elsewhere; these inner
            // unit tests cover the cascading + audit paths only.
            None,
            embedder.clone() as Arc<dyn EmbeddingProvider>,
            storage,
            metadata,
        );

        Fixture {
            _tmp: tmp,
            adapter,
            retriever,
            embedder,
            metadata_for_assert,
        }
    }

    fn sample_new_memory(content: &str, boundary: &str) -> NewMemory {
        NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(boundary).unwrap(),
            source_agent: Some("update-agent".to_string()),
            confidence: 0.7,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        }
    }

    fn sample_query(boundary: &str) -> RetrievalQuery {
        RetrievalQuery {
            query_text: "anything".to_string(),
            authorized_boundaries: vec![Boundary::new(boundary).unwrap()],
            max_results: 10,
            options: vault_retrieval::RetrievalOptions {
                score_threshold: None,
                include_archived: false,
            },
        }
    }

    fn sample_search_audit_details(error: bool) -> ToolInvokeDetails {
        ToolInvokeDetails {
            tool: "memory.search",
            duration_ms: 12,
            result_count: if error { 0 } else { 3 },
            boundary_count: 1,
            max_results: Some(10),
            score_threshold: None,
            include_archived: Some(false),
            query_length: Some(8),
            error: if error {
                Some(vault_mcp::ToolInvokeError::DimensionMismatch {
                    expected: 384,
                    actual: 256,
                })
            } else {
                None
            },
        }
    }

    // ==================================================================
    // 1. search → retriever
    // ==================================================================

    #[tokio::test]
    async fn search_dispatches_to_retriever() {
        let f = make_fixture(vec![]).await;
        let q = sample_query("work");
        let results = f.adapter.search(q.clone()).await.unwrap();
        assert_eq!(results.len(), 0, "empty stub response → empty results");

        let recorded = f.retriever.recorded_queries();
        assert_eq!(recorded.len(), 1, "exactly one retriever dispatch per call");
        assert_eq!(recorded[0].query_text, q.query_text);
        assert_eq!(
            recorded[0].authorized_boundaries, q.authorized_boundaries,
            "trusted boundary slice passed through verbatim"
        );
    }

    // ==================================================================
    // 2. write → embed + cascade
    // ==================================================================

    #[tokio::test]
    async fn write_embeds_then_cascades() {
        let f = make_fixture(vec![]).await;
        let id = f
            .adapter
            .write(sample_new_memory("remember the milk", "work"))
            .await
            .unwrap();

        // Embedder was called exactly once.
        assert_eq!(f.embedder.call_count(), 1);

        // Row exists in metadata store.
        let stored = f
            .metadata_for_assert
            .get_memory(&id)
            .await
            .unwrap()
            .expect("memory written by VaultAdapter::write must be readable");
        assert_eq!(stored.content, "remember the milk");
        assert_eq!(stored.boundary.as_str(), "work");
        assert_eq!(stored.access_count, 0);
    }

    // ==================================================================
    // 3. update — ADR-028 preservation invariant
    // ==================================================================

    /// **Pinning test for ADR-028.** Pre-populate a memory with non-
    /// default `created_at`, `access_count > 0`, and
    /// `superseded_by = Some(...)`. Call `Adapter::update` with a
    /// new content + memory_type + boundary + confidence. Assert:
    ///
    /// - PRESERVED fields unchanged (id, source_agent, created_at,
    ///   valid_from, valid_until, access_count, superseded_by,
    ///   metadata)
    /// - OVERWRITTEN fields match input (content, memory_type,
    ///   boundary, confidence)
    /// - last_accessed advanced to ≥ pre-update timestamp
    /// - embedder called once during update (re-computation per
    ///   ADR-028)
    #[tokio::test]
    async fn update_preserves_provenance_per_adr_028() {
        let f = make_fixture(vec![]).await;

        // Pre-populate via VaultAdapter::write.
        let original_id = f
            .adapter
            .write(sample_new_memory("original content", "work"))
            .await
            .unwrap();

        // Mutate the row's fields directly via the metadata store
        // (simulating a memory that's accumulated history through
        // some other path — e.g. consolidator). We need
        // non-default `access_count` and `superseded_by` so the
        // preservation assertions are non-trivial.
        let original_created_at = Utc.with_ymd_and_hms(2025, 1, 15, 10, 30, 0).unwrap();
        let other_id = MemoryId::new();
        let original_metadata = serde_json::json!({"source": "consolidator", "weight": 0.42});
        let original_valid_from = Utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();

        // Read, mutate, write back.
        let mut row = f
            .metadata_for_assert
            .get_memory(&original_id)
            .await
            .unwrap()
            .expect("pre-populated memory must exist");
        row.created_at = original_created_at;
        row.valid_from = original_valid_from;
        row.access_count = 42;
        row.superseded_by = Some(other_id);
        row.metadata = original_metadata.clone();
        row.source_agent = Some("genesis-agent".to_string());

        f.metadata_for_assert.update_memory(&row).await.unwrap();

        // Snapshot pre-update timestamp so we can assert
        // `last_accessed` advanced.
        let pre_update_timestamp = Utc::now();
        // Sleep a millisecond so `last_accessed > pre_update_timestamp`
        // can be a strict comparison even on fast clocks.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;

        // Reset embedder call count so we can pin "update calls
        // embed exactly once."
        let pre_update_embed_calls = f.embedder.call_count();

        // The update payload — note the values that should land
        // (content / memory_type / boundary / confidence) and the
        // values that should NOT (source_agent on the new payload
        // is "update-agent" but should be preserved as
        // "genesis-agent"; metadata on the new payload should be
        // discarded in favor of the existing).
        let update_payload = NewMemory {
            content: "updated content".to_string(),
            memory_type: MemoryType::Procedural,
            boundary: Boundary::new("personal").unwrap(),
            source_agent: Some("update-agent".to_string()),
            confidence: 0.95,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({"this_should_not_appear": true}),
        };

        f.adapter.update(original_id, update_payload).await.unwrap();

        // Read back.
        let after = f
            .metadata_for_assert
            .get_memory(&original_id)
            .await
            .unwrap()
            .expect("memory still exists after update");

        // OVERWRITTEN.
        assert_eq!(after.content, "updated content", "content overwritten");
        assert_eq!(
            after.memory_type,
            MemoryType::Procedural,
            "memory_type overwritten"
        );
        assert_eq!(after.boundary.as_str(), "personal", "boundary overwritten");
        assert_eq!(after.confidence, 0.95, "confidence overwritten");

        // PRESERVED.
        assert_eq!(after.id, original_id, "id preserved (identity)");
        assert_eq!(
            after.source_agent.as_deref(),
            Some("genesis-agent"),
            "ADR-028: source_agent preserved (genesis attribution)"
        );
        assert_eq!(
            after.created_at, original_created_at,
            "ADR-028: created_at preserved (provenance)"
        );
        assert_eq!(
            after.valid_from, original_valid_from,
            "ADR-028: valid_from preserved (bi-temporal)"
        );
        assert_eq!(
            after.access_count, 42,
            "ADR-028: access_count preserved (read-history)"
        );
        assert_eq!(
            after.superseded_by,
            Some(other_id),
            "ADR-028: superseded_by preserved (consolidation lineage)"
        );
        assert_eq!(
            after.metadata, original_metadata,
            "ADR-028: metadata preserved"
        );

        // ADVANCED.
        assert!(
            after.last_accessed > pre_update_timestamp,
            "ADR-028: last_accessed advanced to now (update IS an access)"
        );

        // RE-COMPUTED.
        assert_eq!(
            f.embedder.call_count(),
            pre_update_embed_calls + 1,
            "ADR-028: embedding re-computed exactly once during update"
        );
    }

    // ==================================================================
    // 4. update — NotFound for missing id
    // ==================================================================

    #[tokio::test]
    async fn update_returns_not_found_for_missing_id() {
        let f = make_fixture(vec![]).await;
        let missing = MemoryId::new();
        let err = f
            .adapter
            .update(missing, sample_new_memory("anything", "work"))
            .await
            .expect_err("missing id must surface as Err");
        assert!(
            matches!(err, VaultError::NotFound(_)),
            "missing id MUST surface as VaultError::NotFound (maps to -32602 \"not found\" \
             at MCP wire layer per ADR-024); got {err:?}"
        );
    }

    // ==================================================================
    // 5. delete — cascade idempotent
    // ==================================================================

    #[tokio::test]
    async fn delete_cascades_idempotently() {
        let f = make_fixture(vec![]).await;
        let id = f
            .adapter
            .write(sample_new_memory("doomed memory", "work"))
            .await
            .unwrap();

        // First delete: row removed.
        f.adapter.delete(id).await.unwrap();
        assert!(
            f.metadata_for_assert
                .get_memory(&id)
                .await
                .unwrap()
                .is_none(),
            "first delete removes the row"
        );

        // Second delete (same id): also Ok per cascade idempotency.
        f.adapter.delete(id).await.unwrap();
    }

    // ==================================================================
    // 6. append_tool_invoke_audit — chain row
    // ==================================================================

    #[tokio::test]
    async fn append_tool_invoke_audit_writes_chain_row() {
        let f = make_fixture(vec![]).await;
        let details = sample_search_audit_details(false);

        f.adapter.append_tool_invoke_audit(details).await.unwrap();

        // Read back via the assertion handle. The test just checks
        // a row exists with the expected event_type; the per-field
        // assertions are below.
        let events = f.metadata_for_assert.list_audit_events(10).await.unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.event_type == AuditEventType::McpToolInvoke),
            "audit_log must contain a `mcp.tool_invoke` row after append"
        );
    }

    // ==================================================================
    // 7. append_tool_invoke_audit — canonical JSON
    // ==================================================================

    #[tokio::test]
    async fn append_tool_invoke_audit_uses_canonical_json() {
        let f = make_fixture(vec![]).await;
        let details = sample_search_audit_details(false);
        let expected_canonical = details
            .to_canonical_json()
            .expect("canonical JSON serialisation succeeds");

        f.adapter.append_tool_invoke_audit(details).await.unwrap();

        let events = f.metadata_for_assert.list_audit_events(10).await.unwrap();
        let row = events
            .iter()
            .find(|e| e.event_type == AuditEventType::McpToolInvoke)
            .expect("mcp.tool_invoke row present");

        // BRD §11.9.2: details_json byte-string must match the
        // canonical-JSON output verbatim. The audit chain hashes
        // these bytes; any drift from to_canonical_json's output
        // breaks hash determinism.
        assert_eq!(
            row.details_json, expected_canonical,
            "details_json must equal ToolInvokeDetails::to_canonical_json output verbatim \
             (BRD §11.9.2 audit chain hash determinism)"
        );
    }

    // ==================================================================
    // 8. append_tool_invoke_audit — actor_kind = Agent (ADR-025)
    // ==================================================================

    /// **Stand-alone test for the ADR-025 Step 6 application.** Pinned
    /// separately from the chain-row + canonical-JSON tests so a
    /// regression on actor-kind classification fails at its own
    /// assertion line rather than getting tangled with the broader
    /// audit-row assertion.
    #[tokio::test]
    async fn append_tool_invoke_audit_records_actor_as_agent_per_adr_025() {
        let f = make_fixture(vec![]).await;
        f.adapter
            .append_tool_invoke_audit(sample_search_audit_details(true))
            .await
            .unwrap();

        let events = f.metadata_for_assert.list_audit_events(10).await.unwrap();
        let row = events
            .iter()
            .find(|e| e.event_type == AuditEventType::McpToolInvoke)
            .expect("mcp.tool_invoke row present");
        assert_eq!(
            row.actor_kind,
            ActorKind::Agent,
            "ADR-025 Step 6 application: append_tool_invoke_audit MUST record \
             actor_kind = ActorKind::Agent. The MCP client is an untrusted agent per \
             ADR-025; user attribution lives in the boundary scope (authorized_boundaries), \
             not the audit-row actor field."
        );
        // Result reflects the error path (details.error.is_some()).
        assert_eq!(
            row.result,
            AuditResult::Error,
            "details.error.is_some() → AuditResult::Error"
        );
    }
}
