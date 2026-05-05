//! Phase 2 Step 7 — `MockAdapter` recording variant.
//!
//! Captures every adapter call argument verbatim so Step 8's
//! trust-boundary tests can assert positively that
//! `RetrievalQuery::authorized_boundaries` matches the trusted slice
//! (NOT a malicious body field). Sibling to `StubAdapter` (panics) /
//! `DimMismatchAdapter` (deterministic-error fixture) / `SuccessAdapter`
//! (deterministic-success fixture).
//!
//! Per Step 7 plan-time decisions:
//! - **(a) Recording shape:** per-method typed structs. No canonical-JSON
//!   serialisation — that is a wire-format concern, not a recording one.
//! - **(b) Per-method:** 5 separate `Mutex<Vec<T>>` recording fields,
//!   matching the existing fixture convention.
//! - **(c) Thread-safety:** `std::sync::Mutex` (V0.1 strict-serial;
//!   no async-mutex justification — recordings are short synchronous
//!   pushes, no `.await` while held).
//! - **(d) Returns:** deterministic Ok values, NOT `unimplemented!()`.
//!   `MockAdapter::WRITE_RETURNS_ID` is a const so Step 8 assertions
//!   can pin the data flow `handler → tool wrapper → response shape`
//!   via `assert_eq!(response.id, MockAdapter::WRITE_RETURNS_ID)` —
//!   no extract-and-compare dance.
//!
//! MockAdapter is **defined here; first exercised in Step 8** when the
//! trust-boundary `should_panic` markers convert to positive assertions.
//! Intermediate state is green-but-unexercised by design — same
//! scaffold-ahead-of-user pattern as Step 3's pinned-TODO ignored test.

use std::sync::Mutex;

use async_trait::async_trait;
use vault_core::{Boundary, MemoryId, NewMemory, VaultResult};
use vault_mcp::{Adapter, ToolInvokeDetails};
use vault_retrieval::{RetrievalQuery, RetrievedMemory};

/// One captured `Adapter::update` call. Named struct (not tuple) so
/// Step 8 assertions read `mock.update_calls()[0].new_memory.boundary`
/// rather than `mock.update_calls()[0].1.boundary`. Tuple positional
/// access compounds across multiple trust-boundary assertion lines.
#[derive(Clone, Debug)]
pub struct UpdateCall {
    pub id: MemoryId,
    pub new_memory: NewMemory,
}

/// Recording test-fixture adapter — captures every CRUD + audit call
/// argument verbatim into per-method `Mutex<Vec<T>>` slots. CRUD methods
/// return deterministic `Ok` values so Step 8's positive assertions can
/// reach the recording-inspection step (panicking adapter would pre-empt
/// the assertion).
#[derive(Default)]
pub struct MockAdapter {
    searches: Mutex<Vec<RetrievalQuery>>,
    writes: Mutex<Vec<NewMemory>>,
    updates: Mutex<Vec<UpdateCall>>,
    deletes: Mutex<Vec<MemoryId>>,
    audits: Mutex<Vec<ToolInvokeDetails>>,
    /// Configurable result for `lookup_boundary` calls. Default `None`
    /// means handle_delete surfaces NotFound at the lookup layer.
    /// ADR-025 amendment 2026-05-05 trust-boundary delete pinning
    /// test sets this to `Some(Boundary("personal"))` while the server
    /// is constructed with `trusted: vec!["work"]` to pin the
    /// AccessDenied path.
    lookup_result: Mutex<Option<Boundary>>,
}

impl MockAdapter {
    /// Deterministic id returned by [`MockAdapter::write`]. Step 8
    /// assertions pin the response payload's id via
    /// `assert_eq!(response.id, MockAdapter::WRITE_RETURNS_ID)`.
    ///
    /// The constant value `deadbeef-deadbeef-deadbeef-deadbeefdeadbeef`
    /// is a sentinel pattern chosen for grep-distinctiveness; UUID
    /// version bits are not load-bearing for fixture use.
    pub const WRITE_RETURNS_ID: MemoryId = MemoryId(uuid::Uuid::from_u128(
        0xDEADBEEF_DEADBEEF_DEADBEEF_DEADBEEF_u128,
    ));

    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of `search()` calls. Cloned out of the internal
    /// `Mutex<Vec<_>>` so callers can assert without holding the lock.
    pub fn search_calls(&self) -> Vec<RetrievalQuery> {
        self.searches
            .lock()
            .expect("MockAdapter searches mutex poisoned")
            .clone()
    }

    pub fn write_calls(&self) -> Vec<NewMemory> {
        self.writes
            .lock()
            .expect("MockAdapter writes mutex poisoned")
            .clone()
    }

    pub fn update_calls(&self) -> Vec<UpdateCall> {
        self.updates
            .lock()
            .expect("MockAdapter updates mutex poisoned")
            .clone()
    }

    pub fn delete_calls(&self) -> Vec<MemoryId> {
        self.deletes
            .lock()
            .expect("MockAdapter deletes mutex poisoned")
            .clone()
    }

    pub fn recorded_audits(&self) -> Vec<ToolInvokeDetails> {
        self.audits
            .lock()
            .expect("MockAdapter audits mutex poisoned")
            .clone()
    }

    /// Configure the result returned by [`MockAdapter::lookup_boundary`].
    /// Used by the ADR-025-amendment trust-boundary delete pinning test
    /// to inject a memory-exists-but-in-unauthorized-boundary scenario.
    pub fn set_lookup_boundary(&self, boundary: Option<Boundary>) {
        *self
            .lookup_result
            .lock()
            .expect("MockAdapter lookup_result mutex poisoned") = boundary;
    }
}

#[async_trait]
impl Adapter for MockAdapter {
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        self.searches
            .lock()
            .expect("MockAdapter searches mutex poisoned")
            .push(query);
        Ok(Vec::new())
    }

    async fn write(&self, new_memory: NewMemory) -> VaultResult<MemoryId> {
        self.writes
            .lock()
            .expect("MockAdapter writes mutex poisoned")
            .push(new_memory);
        Ok(Self::WRITE_RETURNS_ID)
    }

    async fn update(&self, id: MemoryId, new_memory: NewMemory) -> VaultResult<()> {
        self.updates
            .lock()
            .expect("MockAdapter updates mutex poisoned")
            .push(UpdateCall { id, new_memory });
        Ok(())
    }

    async fn delete(&self, id: MemoryId) -> VaultResult<()> {
        self.deletes
            .lock()
            .expect("MockAdapter deletes mutex poisoned")
            .push(id);
        Ok(())
    }

    /// ADR-025 amendment 2026-05-05: MockAdapter returns the configured
    /// [`MockAdapter::lookup_result`], default `None`. The trust-boundary
    /// delete pinning test sets this to `Some(Boundary("personal"))`
    /// against a `vec!["work"]` trusted slice to pin AccessDenied.
    async fn lookup_boundary(&self, _id: MemoryId) -> VaultResult<Option<Boundary>> {
        Ok(self
            .lookup_result
            .lock()
            .expect("MockAdapter lookup_result mutex poisoned")
            .clone())
    }

    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()> {
        self.audits
            .lock()
            .expect("MockAdapter audits mutex poisoned")
            .push(details);
        Ok(())
    }
}
