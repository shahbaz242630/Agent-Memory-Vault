//! Shared test fixtures for `vault-mcp` integration tests.
//!
//! ## Phase 1 — `StubAdapter`
//!
//! Panics on every CRUD adapter call with `unimplemented!()`. Trust-boundary
//! tests are `#[should_panic]`-marked at the panic site. Step 7
//! lands [`MockAdapter`] (see `mock_adapter.rs`) which captures
//! call arguments so the trust-boundary invariant can be asserted
//! positively (the trusted slice was used, NOT the malicious body field).
//! Step 8 swaps the `should_panic` markers for those positive assertions.
//!
//! ## Phase 2 Step 3 — `DimMismatchAdapter`
//!
//! Returns `Err(VaultError::DimensionMismatch { expected: 384, actual: 256 })`
//! from `search()`. Step 5 extends `delete()` to return `NotFound` so
//! `tool_delete`'s adapter-error path has a fixture (the boundary check
//! for write/update happens at the handler layer, but delete has no
//! handler-level check — adapter-level error is the only path). Used by
//! `tests/error_mapping.rs` (Step 3 + Step 4 + Step 5).
//!
//! ## Phase 2 Step 4 — `append_tool_invoke_audit` recording
//!
//! Every adapter implements `append_tool_invoke_audit` by pushing the
//! typed [`ToolInvokeDetails`] onto an internal `Mutex<Vec<_>>`.
//! `recorded_audits()` returns a snapshot for assertion.
//!
//! ## Phase 2 Step 5 — `SuccessAdapter`
//!
//! Returns `Ok(...)` from every CRUD method. Used by Step 5's success
//! integration tests (search + write + update + delete) and by the
//! tool_write/update error tests (where `AccessDenied` fires at the
//! handler before the adapter is reached, so a non-erroring adapter
//! is fine).

#![allow(dead_code)]

mod mock_adapter;

// `MockAdapter` + `UpdateCall` are exposed for Step 8's positive-assertion
// trust-boundary tests; intermediate state at Step 7 is green-but-unexercised
// by design — see the scaffold-ahead-of-user note in `mock_adapter.rs`.
#[allow(unused_imports)]
pub use mock_adapter::{MockAdapter, UpdateCall};

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory, VaultError, VaultResult};
use vault_mcp::{Adapter, StdioServer, ToolInvokeDetails};
use vault_retrieval::{
    HealthInfo, HealthStatus, ReadQuery, RetrievalQuery, RetrievedMemory, StructuredReadResponse,
};

/// Phase 1 stub adapter — every CRUD method panics with `unimplemented!()`.
/// Trust-boundary tests catch the panic via `#[should_panic]`; the
/// stub's role at Phase 1 is just to verify the handler reached the
/// adapter call site (i.e. param parsing + auth-gate validation
/// succeeded). The audit method records into `audits` so future
/// trust-boundary audit-shape tests can assert on captured rows.
#[derive(Default)]
pub struct StubAdapter {
    audits: Mutex<Vec<ToolInvokeDetails>>,
}

impl StubAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of audit events recorded so far. Cloned out of the
    /// internal `Mutex<Vec<_>>` so callers can assert without holding
    /// the lock.
    pub fn recorded_audits(&self) -> Vec<ToolInvokeDetails> {
        self.audits
            .lock()
            .expect("StubAdapter audit mutex poisoned")
            .clone()
    }
}

#[async_trait]
impl Adapter for StubAdapter {
    async fn search(&self, _query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        unimplemented!("T0.1.9 Phase 2: wire SemanticRetriever via Application")
    }

    async fn read(&self, _query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        unimplemented!("Commit 6 (ADR-052): wire StructuredReadPipeline via Application")
    }

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        unimplemented!("T0.1.9 Phase 2: wire StorageBackend::write_memory via Application")
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        unimplemented!("T0.1.9 Phase 2: wire StorageBackend::update_memory via Application")
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        unimplemented!("T0.1.9 Phase 2: wire StorageBackend::delete_memory via Application")
    }

    /// ADR-025 amendment 2026-05-05: StubAdapter returns Ok(None) — no
    /// memories exist in the stub. handle_delete will surface as NotFound
    /// before reaching delete(); StubAdapter's delete() panic stays
    /// unreachable.
    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(None)
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.audits
            .lock()
            .expect("StubAdapter audit mutex poisoned")
            .push(details);
        Ok(())
    }
}

/// Build a `StdioServer` with a fresh `StubAdapter` and a fixed trusted
/// boundary slice. Used by trust-boundary tests + the initialize smoke.
pub fn make_test_server(trusted: Vec<&str>) -> StdioServer {
    let trusted_boundaries: Vec<Boundary> = trusted
        .into_iter()
        .map(|s| Boundary::new(s).expect("valid trusted boundary"))
        .collect();
    StdioServer::new(Arc::new(StubAdapter::new()), trusted_boundaries)
}

// =============================================================================
// Phase 2 Step 3 — DimMismatchAdapter
// =============================================================================

/// Test fixture: returns `VaultError::DimensionMismatch { expected: 384,
/// actual: 256 }` from `search()`. The two non-search methods stay
/// `unimplemented!()` because Step 3's only test exercises the search
/// pipeline. Step 4 may extend this fixture (or add siblings) to cover
/// write/update/delete error paths.
///
/// Step 4: `append_tool_invoke_audit` records into `audits` so the
/// `dimension_mismatch_audit_row_pins_full_detail` test can assert on
/// the captured row shape per ADR-024 schema.
#[derive(Default)]
pub struct DimMismatchAdapter {
    audits: Mutex<Vec<ToolInvokeDetails>>,
}

impl DimMismatchAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of audit events recorded so far.
    pub fn recorded_audits(&self) -> Vec<ToolInvokeDetails> {
        self.audits
            .lock()
            .expect("DimMismatchAdapter audit mutex poisoned")
            .clone()
    }
}

#[async_trait]
impl Adapter for DimMismatchAdapter {
    async fn search(&self, _query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        Err(VaultError::DimensionMismatch {
            expected: 384,
            actual: 256,
        })
    }

    async fn read(&self, _query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        unimplemented!("DimMismatchAdapter: read() is not exercised by any current test")
    }

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        unimplemented!("DimMismatchAdapter: write() is not exercised by any current test")
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        unimplemented!("DimMismatchAdapter: update() is not exercised by any current test")
    }

    /// Step 5 extension: returns `NotFound` so `tool_delete`'s error
    /// path has an adapter-level error fixture. NotFound is the
    /// natural delete-by-id error and exercises the ADR-024-silent
    /// Internal-collapse default in `ToolInvokeError::from_vault_error`
    /// (audit row records `error.type = "Internal"`,
    /// `error.detail.category = "NotFound"`).
    async fn delete(&self, id: MemoryId) -> VaultResult<()> {
        Err(VaultError::NotFound(format!("memory {id} not found")))
    }

    /// ADR-025 amendment 2026-05-05: DimMismatchAdapter returns
    /// Ok(None) — handle_delete surfaces NotFound at the lookup layer
    /// before reaching delete(). Existing
    /// `tool_delete_not_found_pins_wire_message_and_internal_collapse_audit`
    /// test still passes because the wire shape (NotFound + Internal-
    /// collapse audit row) is identical whether NotFound originates at
    /// lookup or at delete.
    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(None)
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.audits
            .lock()
            .expect("DimMismatchAdapter audit mutex poisoned")
            .push(details);
        Ok(())
    }
}

// =============================================================================
// Phase 2 Step 5 — SuccessAdapter
// =============================================================================

/// Test fixture: returns `Ok(...)` from every CRUD method. Used by:
///
/// - Step 5 success integration tests (search + write + update + delete)
///   — exercise the full success-path response shape + audit-row
///   contract end-to-end.
/// - Step 5 error integration tests for `tool_write` / `tool_update`
///   where `AccessDenied` fires at the handler BEFORE the adapter is
///   reached (boundary check in `handle_write` / `handle_update`).
///   Pairing an unauthorized boundary with `SuccessAdapter` exercises
///   the handler-layer error path while keeping the fixture simple.
///
/// Search returns a single deterministic [`RetrievedMemory`] so the
/// integration test can assert on the wire shape. Write / update /
/// delete return their natural success types.
#[derive(Default)]
pub struct SuccessAdapter {
    audits: Mutex<Vec<ToolInvokeDetails>>,
}

impl SuccessAdapter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn recorded_audits(&self) -> Vec<ToolInvokeDetails> {
        self.audits
            .lock()
            .expect("SuccessAdapter audit mutex poisoned")
            .clone()
    }
}

#[async_trait]
impl Adapter for SuccessAdapter {
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // One deterministic hit pinned to the first authorized
        // boundary so the boundary-leak invariant is preserved
        // trivially. Score is a conventional 0.95 — irrelevant to
        // the response-shape tests but plausible.
        let boundary = query
            .authorized_boundaries
            .first()
            .cloned()
            .unwrap_or_else(|| Boundary::new("test").expect("valid test boundary"));
        let memory = Memory::try_new(NewMemory {
            content: "deterministic test memory content".to_string(),
            memory_type: MemoryType::Semantic,
            boundary,
            source_agent: Some("success-adapter".to_string()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })?;
        Ok(vec![RetrievedMemory {
            memory,
            score: 0.95,
            explanation: "semantic: cosine=0.9500 (rank 1/1)".to_string(),
        }])
    }

    async fn read(&self, _query: ReadQuery) -> VaultResult<StructuredReadResponse> {
        // Commit 6 (ADR-052/054, 2026-05-26): deterministic success fixture
        // in the new structured-fact shape. Empty `relevant_facts` +
        // `abstain=true` + Ok health is the simplest deterministic shape;
        // tests that exercise the read response shape can construct a
        // richer fixture inline.
        Ok(StructuredReadResponse {
            boundary: None,
            query: "deterministic test query".to_string(),
            relevant_facts: Vec::new(),
            abstain: true,
            health: HealthInfo {
                status: HealthStatus::Ok,
                warnings: Vec::new(),
            },
        })
    }

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        Ok(MemoryId::new())
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        Ok(())
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        Ok(())
    }

    /// ADR-025 amendment 2026-05-05: SuccessAdapter returns
    /// Ok(Some(Boundary("work"))) — matches the conventional `vec!["work"]`
    /// trusted slice all SuccessAdapter-using tests use. handle_delete's
    /// new auth gate passes (boundary is in authorized list); delete()
    /// proceeds to its existing Ok(()) success path. Existing
    /// `tool_delete_success_records_audit_and_omits_search_only_keys`
    /// test still passes — wire shape unchanged.
    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(Some(
            Boundary::new("work").expect("'work' is a valid Boundary literal"),
        ))
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.audits
            .lock()
            .expect("SuccessAdapter audit mutex poisoned")
            .push(details);
        Ok(())
    }
}

/// Build a `StdioServer` paired with a shared `Arc<SuccessAdapter>`,
/// returning both so tests can call `recorded_audits()` after invoking
/// the server. Used by Step 5 success integration tests + tool_write /
/// tool_update handler-layer-error tests.
pub fn make_success_server_with_adapter(trusted: Vec<&str>) -> (StdioServer, Arc<SuccessAdapter>) {
    let trusted_boundaries: Vec<Boundary> = trusted
        .into_iter()
        .map(|s| Boundary::new(s).expect("valid trusted boundary"))
        .collect();
    let adapter = Arc::new(SuccessAdapter::new());
    let server = StdioServer::new(adapter.clone(), trusted_boundaries);
    (server, adapter)
}

/// Build a `StdioServer` paired with a shared `Arc<DimMismatchAdapter>`,
/// returning both so tests can call `recorded_audits()` after invoking
/// the server. Used by `tests/error_mapping.rs` (Step 4).
pub fn make_dim_mismatch_server_with_adapter(
    trusted: Vec<&str>,
) -> (StdioServer, Arc<DimMismatchAdapter>) {
    let trusted_boundaries: Vec<Boundary> = trusted
        .into_iter()
        .map(|s| Boundary::new(s).expect("valid trusted boundary"))
        .collect();
    let adapter = Arc::new(DimMismatchAdapter::new());
    let server = StdioServer::new(adapter.clone(), trusted_boundaries);
    (server, adapter)
}

/// Backwards-compatible wrapper for Step 3's `make_dim_mismatch_server`
/// — returns just the server, discarding the adapter handle. Step 3's
/// existing test (`dimension_mismatch_returns_generic_invalid_params_no_data_leak`)
/// only needs the server.
pub fn make_dim_mismatch_server(trusted: Vec<&str>) -> StdioServer {
    let (server, _adapter) = make_dim_mismatch_server_with_adapter(trusted);
    server
}

/// Build a `StdioServer` paired with a shared `Arc<MockAdapter>`,
/// returning both so Step 8 trust-boundary tests can invoke the server
/// AND inspect captured call arguments via the recording snapshots.
/// Defined at Step 7; first exercised in Step 8.
pub fn make_mock_server_with_adapter(trusted: Vec<&str>) -> (StdioServer, Arc<MockAdapter>) {
    let trusted_boundaries: Vec<Boundary> = trusted
        .into_iter()
        .map(|s| Boundary::new(s).expect("valid trusted boundary"))
        .collect();
    let adapter = Arc::new(MockAdapter::new());
    let server = StdioServer::new(adapter.clone(), trusted_boundaries);
    (server, adapter)
}
