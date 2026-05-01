//! `Adapter` trait — domain-level boundary between the MCP wire protocol
//! (JSON-RPC stdio handled by [`crate::server::StdioServer`]) and the vault
//! crates that actually do the work (`vault-retrieval` for search,
//! `vault-storage` for write / update / delete).
//!
//! ## Trust boundary (ADR-025)
//!
//! Every method takes the request shape directly (a `RetrievalQuery` for
//! search, a `NewMemory` for write, etc.). The authorization input — the
//! `authorized_boundaries` field on `RetrievalQuery` and the per-method
//! parameters — is set by [`StdioServer`](crate::server::StdioServer) from
//! its trusted state, never from request-body parsing. The Adapter
//! implementor MUST NOT re-derive boundaries from the request shape.
//!
//! Phase 1 (T0.1.9) lands the trait + a stub implementation that returns
//! `unimplemented!()` for every method. Phase 2 lands the real impl in
//! `vault-app` (or a sibling `VaultAdapter` type — TBD at T0.1.10) that
//! holds `Arc<MetadataStore>` + `Arc<dyn Retriever>` + `Arc<StorageBackend>`
//! and dispatches accordingly.

use async_trait::async_trait;

use vault_core::{MemoryId, NewMemory, VaultResult};
use vault_retrieval::{RetrievalQuery, RetrievedMemory};

/// Domain-level interface for the four MCP tools. Implementations are
/// expected to be cheap-to-clone (`Arc`-shared internal state) so the
/// stdio server can hand them off across the request boundary without
/// locking.
///
/// **Phase 1 contract:** trait is defined; the only implementer in tree
/// at Phase 1 is the stub harness used by tests. Phase 2 wires the
/// production implementation at vault-app (T0.1.10).
#[async_trait]
pub trait Adapter: Send + Sync {
    /// `memory.search` — semantic retrieval over the boundary-filtered
    /// vector store. Returns up to `query.max_results` `RetrievedMemory`
    /// items, sorted score-DESC then created_at-DESC (per T0.1.8 Q9).
    ///
    /// The `RetrievalQuery::authorized_boundaries` field carries the
    /// trusted slice — the caller (`StdioServer`) populates it from its
    /// own state, NOT from the JSON-RPC request body.
    async fn search(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>>;

    /// `memory.write` — create a new memory in the given boundary.
    /// `new_memory.boundary` MUST appear in the trusted authorization
    /// slice that `StdioServer` checks before calling.
    async fn write(&self, new_memory: NewMemory) -> VaultResult<MemoryId>;

    /// `memory.update` — update an existing memory's content / metadata.
    /// Phase 1 stub takes a full `NewMemory` as the patch payload;
    /// Phase 2 may introduce a `MemoryUpdates` partial-update struct
    /// once the Tauri UI's update-flow design is firmer (T0.1.11).
    async fn update(&self, id: MemoryId, new_memory: NewMemory) -> VaultResult<()>;

    /// `memory.delete` — delete a memory by id. The caller (`StdioServer`)
    /// has already verified the memory's boundary against the trusted
    /// authorization slice before this is called.
    async fn delete(&self, id: MemoryId) -> VaultResult<()>;
}
