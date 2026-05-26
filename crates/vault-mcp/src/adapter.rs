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

use vault_core::{Boundary, MemoryId, NewMemory, VaultResult};
use vault_retrieval::{ReadQuery, RetrievalQuery, RetrievedMemory, StructuredReadResponse};

use crate::audit::ToolInvokeDetails;

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

    /// `memory.read` — read-time pipeline returning a
    /// [`StructuredReadResponse`] (structured `relevant_facts` +
    /// `abstain` + `health.warnings`).
    ///
    /// **Commit 6 retirement of Qwen-7B (locked-next-arc, 2026-05-26 —
    /// ADR-052):** the V0.2-era `ReadResponse` shape (synthesis_markdown
    /// + contradictions_flagged + vault_has_no_relevant_content) is
    /// retired. The new contract returns deterministic structured facts
    /// that the calling agent (Claude / GPT / Codex / Kimi) composes
    /// into its own response. The vault never speaks to the user
    /// directly; it returns facts + health metadata, and the agent
    /// owns voice / framing.
    ///
    /// The `ReadQuery::authorized_boundaries` field carries the trusted
    /// slice — the caller (`StdioServer`) populates it from its own
    /// state, NOT from the JSON-RPC request body, mirroring `search`.
    ///
    /// Implementations always have a wired `StructuredReadPipeline`
    /// (no fallible model load), so this method is never a config-
    /// error surface in practice. Errors propagate from the underlying
    /// retriever + REPORT-loader (`VaultError::Retrieval` /
    /// `VaultError::Io` / `VaultError::Serde` / `VaultError::InvalidInput`).
    async fn read(&self, query: ReadQuery) -> VaultResult<StructuredReadResponse>;

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

    /// Look up the stored boundary of a memory by id, returning `Ok(None)`
    /// if no memory with that id exists.
    ///
    /// Added at T0.1.11 Phase 4a per ADR-025 amendment 2026-05-05 to
    /// enable handler-mediated auth-gating on `memory.delete` (see
    /// HANDOFF.md ADR-025 amendment + multi-agent code review CRITICAL
    /// finding 2026-05-05). The handler MUST verify the returned
    /// boundary against `self.authorized_boundaries` before dispatching
    /// `delete`. Implementations MUST NOT enforce any auth themselves —
    /// the lookup is purely a stored-boundary read.
    ///
    /// Returns the boundary the memory was stored against, or `None` if
    /// the memory does not exist (caller surfaces as
    /// `VaultError::NotFound`).
    async fn lookup_boundary(&self, id: MemoryId) -> VaultResult<Option<Boundary>>;

    /// Append one `mcp.tool_invoke` audit event to the local audit
    /// chain. Called by the `tool_*` handlers in [`crate::server`] at
    /// invocation exit (success and error paths both append) per
    /// Q7 (a) — handler-mediated audit, the adapter is the work-doer.
    ///
    /// Implementations MUST serialise `details` to canonical sorted-key
    /// JSON via [`ToolInvokeDetails::to_canonical_json`] before persisting
    /// — direct `serde_json::to_string` uses struct field declaration
    /// order, which is fine for tracing/debug but would silently break
    /// audit chain hashing (BRD §11.9.2).
    ///
    /// The schema of `details` is locked by ADR-024 (HANDOFF.md +
    /// T0.1.9_PLAN.md §5 / §6.2 rule 2). Search-only fields are
    /// `Option<T>` and absent (not null) on write/update/delete per Q1.
    async fn append_tool_invoke_audit(&self, details: ToolInvokeDetails) -> VaultResult<()>;
}
