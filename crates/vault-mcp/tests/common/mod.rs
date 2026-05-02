//! Shared test fixtures for `vault-mcp` integration tests.
//!
//! ## Phase 1 — `StubAdapter`
//!
//! Panics on every CRUD adapter call with `unimplemented!()`. Trust-boundary
//! tests are `#[should_panic]`-marked at the panic site. Step 5 / Step 7
//! replace this with a `RecordingAdapter` (mock variant) that captures
//! call arguments so the trust-boundary invariant can be asserted
//! positively (the trusted slice was used, NOT the malicious body field).
//!
//! ## Phase 2 Step 3 — `DimMismatchAdapter`
//!
//! Returns `Err(VaultError::DimensionMismatch { expected: 384, actual: 256 })`
//! from `search()`. Used by `tests/error_mapping.rs` to pin the
//! `VaultError::DimensionMismatch → JSON-RPC InvalidParams` contract
//! (ADR-024 + plan v1.1 Step 3). Single-purpose; not the recording
//! mock that Step 5 / Step 7 need.
//!
//! ## Phase 2 Step 4 — `append_tool_invoke_audit` recording
//!
//! Both adapters now implement `append_tool_invoke_audit` by pushing
//! the typed [`ToolInvokeDetails`] onto an internal `Mutex<Vec<_>>`.
//! `recorded_audits()` returns a snapshot for assertion. The audit
//! method does NOT panic on either adapter — Step 4's
//! `dimension_mismatch_audit_row_pins_full_detail` test (DimMismatch
//! adapter) and any future trust-boundary audit-shape tests (Stub
//! adapter, Step 7+) need it to actually record.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use vault_core::{Boundary, MemoryId, NewMemory, VaultError, VaultResult};
use vault_mcp::{Adapter, StdioServer, ToolInvokeDetails};
use vault_retrieval::{RetrievalQuery, RetrievedMemory};

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

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        unimplemented!("T0.1.9 Phase 2: wire StorageBackend::write_memory via Application")
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        unimplemented!("T0.1.9 Phase 2: wire StorageBackend::update_memory via Application")
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        unimplemented!("T0.1.9 Phase 2: wire StorageBackend::delete_memory via Application")
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

    async fn write(&self, _new_memory: NewMemory) -> VaultResult<MemoryId> {
        unimplemented!("DimMismatchAdapter: only search() is exercised by Step 3/4 tests")
    }

    async fn update(&self, _id: MemoryId, _new_memory: NewMemory) -> VaultResult<()> {
        unimplemented!("DimMismatchAdapter: only search() is exercised by Step 3/4 tests")
    }

    async fn delete(&self, _id: MemoryId) -> VaultResult<()> {
        unimplemented!("DimMismatchAdapter: only search() is exercised by Step 3/4 tests")
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.audits
            .lock()
            .expect("DimMismatchAdapter audit mutex poisoned")
            .push(details);
        Ok(())
    }
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
