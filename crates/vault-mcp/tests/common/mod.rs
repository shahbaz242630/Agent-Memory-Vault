//! Shared test fixtures for `vault-mcp` integration tests.
//!
//! Phase 1 stub: a `StubAdapter` that panics on every adapter call with
//! `unimplemented!()`. Trust-boundary tests are `#[should_panic]`-marked
//! at the panic site — Phase 2 will replace this with a `RecordingAdapter`
//! that captures call arguments so the trust-boundary invariant can be
//! asserted positively (the trusted slice was used, NOT the malicious
//! body field).

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use vault_core::{Boundary, MemoryId, NewMemory, VaultResult};
use vault_mcp::{Adapter, StdioServer};
use vault_retrieval::{RetrievalQuery, RetrievedMemory};

/// Phase 1 stub adapter — every method panics with `unimplemented!()`.
/// Trust-boundary tests catch the panic via `#[should_panic]`; the
/// stub's role at Phase 1 is just to verify the handler reached the
/// adapter call site (i.e. param parsing + auth-gate validation
/// succeeded).
pub struct StubAdapter;

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
}

/// Build a `StdioServer` with the `StubAdapter` and a fixed trusted
/// boundary slice. Used by trust-boundary tests + the initialize smoke.
pub fn make_test_server(trusted: Vec<&str>) -> StdioServer {
    let trusted_boundaries: Vec<Boundary> = trusted
        .into_iter()
        .map(|s| Boundary::new(s).expect("valid trusted boundary"))
        .collect();
    StdioServer::new(Arc::new(StubAdapter), trusted_boundaries)
}
