//! `StdioServer` — wraps rmcp's stdio transport with the four vault tools.
//!
//! ## Phase 1 (T0.1.9, this commit) — scaffold
//!
//! - `StdioServer` struct holds `Arc<dyn Adapter>` + the trusted
//!   `authorized_boundaries: Vec<Boundary>` slice (per ADR-025).
//! - Four `#[tool]`-decorated methods (`search` / `write` / `update` /
//!   `delete`) parse JSON-RPC params, construct domain types using the
//!   TRUSTED authorization slice (never request-body data), and call
//!   `self.adapter.*()`.
//! - Phase 1's stub `Adapter` returns `unimplemented!()` from every method,
//!   so the trust-boundary tests are `#[should_panic]`-marked at the
//!   adapter call — Phase 2 wires a real adapter and the tests turn into
//!   real assertions.
//! - The `initialize` round-trip runtime-confirmation smoke test
//!   (per plan §2 / ADR-026) lives in `tests/initialize_smoke.rs`.
//!
//! ## Param-schema discipline (ADR-025 trust boundary)
//!
//! [`SearchToolParams`] / [`WriteToolParams`] / etc. deliberately do NOT
//! contain an `authorized_boundaries` field. The MCP client may include
//! such a key in its request body — `serde` will silently ignore it
//! (extra fields are deserialised away). Even if it weren't ignored, the
//! handler doesn't read it. The handler ALWAYS uses
//! `self.authorized_boundaries.clone()`.

use std::sync::Arc;
use std::time::Instant;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content, ErrorCode, ServerCapabilities, ServerInfo};
use rmcp::{schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler};
use serde::{Deserialize, Serialize};
use vault_core::{Boundary, MemoryId, MemoryType, NewMemory, VaultError, VaultResult};
use vault_retrieval::{ReadQuery, ReadResponse, RetrievalOptions, RetrievalQuery};

use crate::audit::{ToolInvokeDetails, ToolInvokeError};
use crate::Adapter;

// =============================================================================
// JSON-RPC parameter schemas — typed, schemars-derived for #[tool] macros
// =============================================================================

/// JSON-RPC parameters for the `memory.search` tool.
///
/// **NOTE (ADR-025 trust boundary):** this schema deliberately does NOT
/// contain an `authorized_boundaries` field. Any such key in the
/// JSON-RPC request body is silently ignored by `serde` (extra fields
/// drop). The handler uses `self.authorized_boundaries` (trusted, set at
/// `StdioServer::new` time) — request-body data NEVER influences the
/// auth gate.
///
/// **NOTE (T0.1.9 Phase 2):** the `schemars::JsonSchema` derive is required
/// by rmcp's `#[tool]` macro to generate the JSON Schema 2020-12 input
/// schema published in `tools/list`. `rmcp::schemars` is re-exported via
/// rmcp's `server` feature — no separate workspace `schemars` dep needed.
/// Verified at runtime by `examples/macro_spike.rs`.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SearchToolParams {
    /// Free-text query — embedded by the model and matched via cosine
    /// k-NN over the boundary-filtered vector store.
    pub query: String,
    /// Maximum number of results to return. Defaults to 10 (server side)
    /// if omitted; capped at `vault_retrieval::MAX_RESULTS_CAP` (200).
    #[serde(default)]
    pub max_results: Option<usize>,
    /// Drop results whose cosine similarity is below this threshold.
    /// Defaults to no threshold (return up-to-`max_results` regardless).
    #[serde(default)]
    pub score_threshold: Option<f32>,
    /// Whether to include archived (superseded) memories. Defaults to
    /// `false` (exclude archived).
    #[serde(default)]
    pub include_archived: Option<bool>,
}

/// JSON-RPC parameters for the `memory.read` tool. Added at T0.2.7
/// Phase 4 (2026-05-20) to expose the production read pipeline
/// (`vault_retrieval::ReadPipeline`) over MCP.
///
/// **NOTE (ADR-025 trust boundary):** like `SearchToolParams`, this
/// schema does NOT contain an `authorized_boundaries` field. The
/// handler uses `self.authorized_boundaries` (trusted, set at
/// `StdioServer::new` time); request-body data NEVER influences the
/// auth gate.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ReadToolParams {
    /// Free-text query — handed to the read pipeline's retrieval stage
    /// (hybrid BGE + BM25 + abstain gate) and then synthesised by the
    /// Qwen-7B LLM into a `ReadResponse`.
    pub query: String,
}

/// JSON-RPC parameters for the `memory.write` tool.
///
/// **NOTE (ADR-025):** the `boundary` field IS user-controlled — the
/// agent specifies which boundary to write to. The handler validates
/// this field appears in `self.authorized_boundaries` BEFORE calling
/// the adapter; if not, returns `VaultError::AccessDenied` (mapped to
/// JSON-RPC `-32602 InvalidParams` with a generic message per ADR-024).
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct WriteToolParams {
    pub content: String,
    pub boundary: String,
    #[serde(default)]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// JSON-RPC parameters for the `memory.update` tool — combines the target
/// memory id with the full replacement payload (mirrors `WriteToolParams`).
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct UpdateToolParams {
    /// UUID v7 of the existing memory to replace.
    pub id: String,
    pub content: String,
    pub boundary: String,
    #[serde(default)]
    pub memory_type: Option<String>,
    #[serde(default)]
    pub source_agent: Option<String>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// JSON-RPC parameters for the `memory.delete` tool — id-only.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct DeleteToolParams {
    /// UUID v7 of the memory to delete.
    pub id: String,
}

// =============================================================================
// StdioServer — owns the adapter + trusted auth slice
// =============================================================================

/// MCP stdio server. Constructed by `Application` at startup with the
/// adapter (which routes to vault-retrieval / vault-storage) and the
/// trusted `authorized_boundaries` slice (from the unlocked vault).
///
/// **Trust boundary (ADR-025):** `authorized_boundaries` is the SOLE
/// auth-gate input for every tool dispatch. The struct is `Clone` so
/// rmcp's request-handler can hand instances across the request boundary
/// without locks; clones share the inner `Arc<dyn Adapter>` and a
/// cloned-but-equal `authorized_boundaries` vector.
#[derive(Clone)]
pub struct StdioServer {
    adapter: Arc<dyn Adapter>,
    authorized_boundaries: Vec<Boundary>,
    /// **Load-bearing — DO NOT remove as "dead code."** This field is
    /// populated by the `#[tool_router]` macro on `impl StdioServer`
    /// (which generates `Self::tool_router()`) and read at request
    /// dispatch time by the `#[tool_handler]` macro on
    /// `impl ServerHandler for StdioServer`. The macros connect through
    /// this field; removing it would silently break tool routing.
    ///
    /// Dead-code analysis cannot see through the macro expansion (it
    /// only sees the field declaration, not the macro-generated code
    /// that uses it), so `#[allow(dead_code)]` is required. Same
    /// suppression rmcp's own `tests/test_tool_macros.rs` applies; see
    /// `examples/macro_spike.rs` (spike finding C) for runtime
    /// confirmation that the chain works.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl StdioServer {
    /// Construct a new server. Both arguments are application-supplied
    /// at startup and form the trust boundary per ADR-025.
    pub fn new(adapter: Arc<dyn Adapter>, authorized_boundaries: Vec<Boundary>) -> Self {
        Self {
            adapter,
            authorized_boundaries,
            tool_router: Self::tool_router(),
        }
    }

    /// Returns a clone of the trusted authorized-boundaries slice.
    /// Test-only helper — production code uses the field directly.
    #[doc(hidden)]
    pub fn authorized_boundaries(&self) -> &[Boundary] {
        &self.authorized_boundaries
    }

    // -------------------------------------------------------------------------
    // Phase 1 stub handlers — Phase 2 wires #[tool_router(server_handler)] +
    // #[tool] decorators on the impl block once the rmcp 1.5.0 macro shape
    // is verified end-to-end by the initialize smoke test.
    //
    // For Phase 1, these are plain async methods callable from tests. They
    // demonstrate the trust-boundary discipline (request body NEVER
    // contributes to the auth slice) and the param-validation flow that
    // Phase 2 will wrap with the macro layer.
    // -------------------------------------------------------------------------

    /// `memory.search` Phase 1 stub. Constructs the `RetrievalQuery`
    /// using the TRUSTED `self.authorized_boundaries` (NEVER the request
    /// body), then calls `self.adapter.search()`. Phase 1's stub adapter
    /// panics with `unimplemented!()` — the trust-boundary tests assert
    /// the trusted slice was used by inspecting that panic site (or, in
    /// Phase 2, by replacing the adapter with a recording one).
    pub async fn handle_search(
        &self,
        params: SearchToolParams,
    ) -> VaultResult<Vec<vault_retrieval::RetrievedMemory>> {
        let options = RetrievalOptions {
            score_threshold: params.score_threshold,
            include_archived: params.include_archived.unwrap_or(false),
        };
        let query = RetrievalQuery {
            query_text: params.query,
            // Trust boundary (ADR-025): the trusted slice goes here,
            // NOT anything from the request body.
            authorized_boundaries: self.authorized_boundaries.clone(),
            max_results: params.max_results.unwrap_or(10),
            options,
        };
        self.adapter.search(query).await
    }

    /// `memory.read` handler. Constructs the [`ReadQuery`] using the
    /// TRUSTED `self.authorized_boundaries` (NEVER the request body),
    /// then dispatches to `self.adapter.read()`.
    ///
    /// Added at T0.2.7 Phase 4 (2026-05-20). The read pipeline returns
    /// a structured [`ReadResponse`] with the load-bearing
    /// `contradictions_flagged` field — see the `tool_read` tool
    /// description for the agent-consumption contract.
    pub async fn handle_read(&self, params: ReadToolParams) -> VaultResult<ReadResponse> {
        let query = ReadQuery {
            query_text: params.query,
            // Trust boundary (ADR-025): the trusted slice goes here,
            // NOT anything from the request body.
            authorized_boundaries: self.authorized_boundaries.clone(),
        };
        self.adapter.read(query).await
    }

    /// `memory.write` Phase 1 stub. Validates that `params.boundary` is
    /// in the trusted slice — request data is ALLOWED to specify which
    /// boundary the memory goes to (it's a write target, not an auth
    /// override), but only for boundaries the application has already
    /// authorized.
    pub async fn handle_write(&self, params: WriteToolParams) -> VaultResult<MemoryId> {
        let boundary = Boundary::new(&params.boundary)
            .map_err(|e| VaultError::InvalidInput(format!("boundary: {e}")))?;
        if !self.authorized_boundaries.contains(&boundary) {
            return Err(VaultError::AccessDenied(format!(
                "boundary '{}' not in authorized set",
                params.boundary
            )));
        }
        let memory_type = match params.memory_type.as_deref() {
            None | Some("semantic") => MemoryType::Semantic,
            Some("episodic") => MemoryType::Episodic,
            Some("procedural") => MemoryType::Procedural,
            Some(other) => {
                return Err(VaultError::InvalidInput(format!(
                    "unknown memory_type: {other}"
                )));
            }
        };
        let new_memory = NewMemory {
            content: params.content,
            memory_type,
            boundary,
            source_agent: params.source_agent,
            confidence: params.confidence.unwrap_or(0.9),
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        let memory = vault_core::Memory::try_new(new_memory.clone())?;
        // Ignore the validated `memory` here (Phase 2's adapter does the
        // full write); the validation pass surfaces malformed inputs at
        // the MCP boundary so the adapter sees only well-formed data.
        let _ = memory;
        self.adapter.write(new_memory).await
    }

    /// `memory.update` Phase 1 stub.
    pub async fn handle_update(&self, id: MemoryId, params: WriteToolParams) -> VaultResult<()> {
        // Same boundary-validation as write: the boundary the agent
        // names MUST be one we've authorized.
        let boundary = Boundary::new(&params.boundary)
            .map_err(|e| VaultError::InvalidInput(format!("boundary: {e}")))?;
        if !self.authorized_boundaries.contains(&boundary) {
            return Err(VaultError::AccessDenied(format!(
                "boundary '{}' not in authorized set",
                params.boundary
            )));
        }
        let memory_type = match params.memory_type.as_deref() {
            None | Some("semantic") => MemoryType::Semantic,
            Some("episodic") => MemoryType::Episodic,
            Some("procedural") => MemoryType::Procedural,
            Some(other) => {
                return Err(VaultError::InvalidInput(format!(
                    "unknown memory_type: {other}"
                )));
            }
        };
        let new_memory = NewMemory {
            content: params.content,
            memory_type,
            boundary,
            source_agent: params.source_agent,
            confidence: params.confidence.unwrap_or(0.9),
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        self.adapter.update(id, new_memory).await
    }

    /// `memory.delete` handler. ADR-025 amendment 2026-05-05 (T0.1.11
    /// Phase 4a): handler auth-gates against the trusted
    /// `authorized_boundaries` slice using the memory's stored boundary
    /// looked up via [`Adapter::lookup_boundary`]. The original
    /// 2026-05-01 ADR-025 named all four tools but only specified HOW
    /// to gate (use the trusted slice), not WHEN — multi-agent code
    /// review at T0.1.11 Phase 4 plan time caught that `tool_delete`
    /// shipped with no auth gate at all (CRITICAL finding, conf 97).
    /// This handler enforces BRD §11.4.3 mandatory-access-control on
    /// the delete path.
    ///
    /// Surfacing semantics:
    /// - lookup returns `Ok(None)` → memory does not exist → `NotFound`
    ///   (maps to `-32602 "not found"` per ADR-024)
    /// - lookup returns `Ok(Some(b))` and `b ∉ authorized_boundaries`
    ///   → `AccessDenied` (maps to `-32001 "access denied"` per ADR-024)
    /// - lookup returns `Ok(Some(b))` and `b ∈ authorized_boundaries`
    ///   → dispatch through to `Adapter::delete`
    pub async fn handle_delete(&self, id: MemoryId) -> VaultResult<()> {
        let stored_boundary = self
            .adapter
            .lookup_boundary(id)
            .await?
            .ok_or_else(|| VaultError::NotFound(format!("memory {id} not found")))?;

        if !self.authorized_boundaries.contains(&stored_boundary) {
            return Err(VaultError::AccessDenied(format!(
                "memory {id} stored in boundary '{}' which is not in the authorized set",
                stored_boundary.as_str()
            )));
        }

        self.adapter.delete(id).await
    }

    // -------------------------------------------------------------------------
    // Phase 2 — `#[tool]`-decorated MCP tool surface
    //
    // These wrappers translate between the MCP wire layer (`Parameters<T>` +
    // `Result<CallToolResult, McpError>`) and the existing internal
    // `handle_*` methods (`VaultResult<...>`). The `handle_*` methods own
    // the trust-boundary discipline + parameter validation; these wrappers
    // own success-path JSON serialisation + error-path mapping per ADR-024.
    //
    // **Audit append + tracing emission lands in Step 4** (per
    // T0.1.9_PLAN.md v1.1). Step 3's `dimension_mismatch_returns_*` test
    // pins the protocol-level error contract today; Step 4 extends the
    // test to assert the audit row shape once the audit append is wired.
    // -------------------------------------------------------------------------

    /// `memory.search` MCP tool — the agent-facing surface for the
    /// `SemanticRetriever`-backed search pipeline.
    ///
    /// **Step 4 (Phase 2): audit + tracing wired.** Both success and
    /// error paths emit one `tracing::info!(target: "vault_mcp::tool_invoke", ...)`
    /// event AND one `mcp.tool_invoke` audit row via
    /// [`Adapter::append_tool_invoke_audit`] per ADR-024 + Q7 (a)
    /// handler-mediated audit. Tracing emits BEFORE audit-append so an
    /// audit-storage failure still leaves the operational log; the
    /// audit chain is the authoritative record, tracing is operational.
    #[tool(
        name = "memory.search",
        description = "Search the user's memory vault by free-text query. \
                       Returns relevant memories ranked by cosine similarity. \
                       Authorization is mediated by the host application, \
                       not by this tool's parameters."
    )]
    pub async fn tool_search(
        &self,
        params: Parameters<SearchToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(p) = params;
        // Snapshot the typed-param fields BEFORE handler dispatch so
        // the audit/tracing record uses the agent-supplied values
        // exactly (the handler moves `p` into a `RetrievalQuery`).
        let max_results_recorded: u32 = p.max_results.unwrap_or(10) as u32;
        let score_threshold_recorded: Option<f32> = p.score_threshold;
        let include_archived_recorded: bool = p.include_archived.unwrap_or(false);
        let query_length_recorded: u32 = p.query.len() as u32;
        let boundary_count_recorded: u32 = self.authorized_boundaries.len() as u32;

        let start = Instant::now();
        let dispatch_result = self.handle_search(p).await;
        let duration_ms: u64 = start.elapsed().as_millis() as u64;

        let (result_count, error_for_audit) = match &dispatch_result {
            Ok(memories) => (memories.len() as u32, None),
            Err(e) => (0_u32, Some(ToolInvokeError::from_vault_error(e))),
        };

        let details = ToolInvokeDetails {
            tool: "memory.search",
            duration_ms,
            result_count,
            boundary_count: boundary_count_recorded,
            max_results: Some(max_results_recorded),
            score_threshold: score_threshold_recorded,
            include_archived: Some(include_archived_recorded),
            query_length: Some(query_length_recorded),
            error: error_for_audit,
        };

        // Tracing first — always fires, independent of audit-store
        // health. Q6: fields are audit details_json minus content
        // (no query_text, no boundary names — only counts and
        // metadata).
        tracing::info!(
            target: "vault_mcp::tool_invoke",
            tool = details.tool,
            duration_ms = details.duration_ms,
            result_count = details.result_count,
            boundary_count = details.boundary_count,
            max_results = ?details.max_results,
            score_threshold = ?details.score_threshold,
            include_archived = ?details.include_archived,
            query_length = ?details.query_length,
            error = ?details.error,
            "memory.search tool invocation completed"
        );

        // Audit append — authoritative record, propagates failures
        // as MCP errors. Audit-storage failure is treated as a hard
        // error on V0.1 (single-user local SQLite — failure is rare
        // and signals a serious storage problem the user should know
        // about). May revisit at V0.2.
        self.adapter
            .append_tool_invoke_audit(details)
            .await
            .map_err(vault_error_to_mcp)?;

        let memories = dispatch_result.map_err(vault_error_to_mcp)?;
        success_json_result(&memories)
    }

    /// `memory.read` MCP tool — the agent-facing surface for the
    /// production [`vault_retrieval::ReadPipeline`] (hybrid BGE+BM25
    /// retrieval + Qwen-7B synthesis).
    ///
    /// **Agent consumption contract (T0.2.7 Phase 4 lock,
    /// 2026-05-20):** the tool description below tells calling agents
    /// they MUST consume the `contradictions_flagged` structured field
    /// as the authoritative contradiction signal (the
    /// `synthesis_markdown` field is a convenience summary). This is
    /// the lock from the 2026-05-20 HANDOFF Phase 0 amendment #3 ---
    /// without this language in the tool description, agents that only
    /// read the synthesis prose would miss contradictions when the LLM
    /// elides the prior value in evolution-with-reason phrasing
    /// ([[structured-contract-user-sees-via-agent]]).
    ///
    /// Audit + tracing wired with the same handler-mediated pattern as
    /// `tool_search`: timer brackets the handler dispatch,
    /// `ToolInvokeDetails` records `boundary_count` from the trusted
    /// slice + `result_count = 1` on success (single
    /// `ReadResponse`) / `0` on error.
    #[tool(
        name = "memory.read",
        description = "Read-time pipeline: retrieves relevant memories AND \
                       synthesises a coherent narrative answer in ONE call. \
                       Returns a JSON object with three fields: \
                       `synthesis_markdown` (convenience narrative summary), \
                       `contradictions_flagged` (structured list of \
                       contradictions detected across the retrieved set, with \
                       memory_ids + literal positions), and \
                       `vault_has_no_relevant_content` (true when the vault \
                       has no memory matching the query). \
                       \n\n\
                       CRITICAL — agent contract: when \
                       `contradictions_flagged` is non-empty you MUST \
                       surface every contradiction in your response to the \
                       user. The `synthesis_markdown` field is a \
                       convenience summary; `contradictions_flagged` is the \
                       authoritative contradiction signal. Treating \
                       synthesis_markdown alone as ground truth may cause \
                       you to miss contradictions when the LLM elides a \
                       prior value in evolution-with-reason phrasing. \
                       \n\n\
                       Authorization is mediated by the host application, \
                       not by this tool's parameters."
    )]
    pub async fn tool_read(
        &self,
        params: Parameters<ReadToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(p) = params;
        let query_length_recorded: u32 = p.query.len() as u32;
        let boundary_count_recorded: u32 = self.authorized_boundaries.len() as u32;

        let start = Instant::now();
        let dispatch_result = self.handle_read(p).await;
        let duration_ms: u64 = start.elapsed().as_millis() as u64;

        let (result_count, error_for_audit) = match &dispatch_result {
            Ok(_) => (1_u32, None),
            Err(e) => (0_u32, Some(ToolInvokeError::from_vault_error(e))),
        };

        let details = ToolInvokeDetails {
            tool: "memory.read",
            duration_ms,
            result_count,
            boundary_count: boundary_count_recorded,
            // Search-only fields: query_length applies here too; the
            // other three (max_results, score_threshold, include_archived)
            // are search-specific and stay ABSENT per Q1.
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: Some(query_length_recorded),
            error: error_for_audit,
        };

        tracing::info!(
            target: "vault_mcp::tool_invoke",
            tool = details.tool,
            duration_ms = details.duration_ms,
            result_count = details.result_count,
            boundary_count = details.boundary_count,
            query_length = ?details.query_length,
            error = ?details.error,
            "memory.read tool invocation completed"
        );

        self.adapter
            .append_tool_invoke_audit(details)
            .await
            .map_err(vault_error_to_mcp)?;

        let response = dispatch_result.map_err(vault_error_to_mcp)?;
        success_json_result(&response)
    }

    /// `memory.write` MCP tool — create a new memory in a boundary the
    /// host application has authorized.
    ///
    /// **Step 5 (Phase 2): audit + tracing wired** per Q7(a)
    /// handler-mediated audit. Same shape as Step 4's `tool_search`:
    /// timer brackets the handler dispatch, ToolInvokeDetails records
    /// `boundary_count` from the trusted slice + `result_count = 1`
    /// on success / `0` on error. Search-only fields (`max_results`,
    /// `score_threshold`, `include_archived`, `query_length`) stay
    /// `None` so the canonical-JSON serialisation OMITS them per Q1
    /// (ABSENT, not `null`). Tracing emits before audit-append so the
    /// operational log fires regardless of audit-store health.
    #[tool(
        name = "memory.write",
        description = "Create a new memory in the user's vault. \
                       The `boundary` field must name a boundary the host \
                       application has authorized for this MCP session."
    )]
    pub async fn tool_write(
        &self,
        params: Parameters<WriteToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(p) = params;
        let boundary_count_recorded: u32 = self.authorized_boundaries.len() as u32;

        let start = Instant::now();
        let dispatch_result = self.handle_write(p).await;
        let duration_ms: u64 = start.elapsed().as_millis() as u64;

        let (result_count, error_for_audit) = match &dispatch_result {
            Ok(_) => (1_u32, None),
            Err(e) => (0_u32, Some(ToolInvokeError::from_vault_error(e))),
        };

        let details = ToolInvokeDetails {
            tool: "memory.write",
            duration_ms,
            result_count,
            boundary_count: boundary_count_recorded,
            // Q1: search-only fields ABSENT (not null) on write.
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        };

        // Tracing first, audit second — same ordering as tool_search
        // (operational log independent of audit-store health). Q6:
        // tracing fields = audit fields minus content; for write that
        // means tool / duration_ms / result_count / boundary_count /
        // error?. Search-only fields are absent here too.
        tracing::info!(
            target: "vault_mcp::tool_invoke",
            tool = details.tool,
            duration_ms = details.duration_ms,
            result_count = details.result_count,
            boundary_count = details.boundary_count,
            error = ?details.error,
            "memory.write tool invocation completed"
        );

        self.adapter
            .append_tool_invoke_audit(details)
            .await
            .map_err(vault_error_to_mcp)?;

        let id = dispatch_result.map_err(vault_error_to_mcp)?;
        success_json_result(&serde_json::json!({ "id": id.to_string() }))
    }

    /// `memory.update` MCP tool — replace an existing memory's content.
    ///
    /// **Step 5 (Phase 2): audit + tracing wired** with the same
    /// handler-mediated pattern as `tool_write` / `tool_search`.
    ///
    /// **`parse_memory_id_traced` early-return contract:** if the agent
    /// supplies a malformed UUID, this returns `McpError` BEFORE the
    /// audit/tracing timer starts. Rationale: the audit chain records
    /// vault dispatches (Q7 a), and a malformed-id request never
    /// reaches the vault. Pre-dispatch validation is analogous to
    /// JSON deserialisation, which is not audited either. Operational
    /// visibility for malformed requests lives at the
    /// `tracing::warn!(target: "vault_mcp::request_validation", ...)`
    /// emission inside `parse_memory_id_traced` — different tracing
    /// target (`vault_mcp::request_validation` vs
    /// `vault_mcp::tool_invoke`) so operators can filter parse-level
    /// errors from tool-dispatch events cleanly.
    #[tool(
        name = "memory.update",
        description = "Replace an existing memory's content. \
                       The `id` field selects the target; the remaining \
                       fields are the full replacement payload (same \
                       shape as `memory.write`)."
    )]
    pub async fn tool_update(
        &self,
        params: Parameters<UpdateToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(p) = params;
        // Pre-dispatch parse: not audited (handler-mediated audit
        // contract per Q7 a). Tracing-level visibility only.
        let id = parse_memory_id_traced(&p.id, "memory.update")?;

        let write_params = WriteToolParams {
            content: p.content,
            boundary: p.boundary,
            memory_type: p.memory_type,
            source_agent: p.source_agent,
            confidence: p.confidence,
        };

        let boundary_count_recorded: u32 = self.authorized_boundaries.len() as u32;
        let start = Instant::now();
        let dispatch_result = self.handle_update(id, write_params).await;
        let duration_ms: u64 = start.elapsed().as_millis() as u64;

        let (result_count, error_for_audit) = match &dispatch_result {
            Ok(()) => (1_u32, None),
            Err(e) => (0_u32, Some(ToolInvokeError::from_vault_error(e))),
        };

        let details = ToolInvokeDetails {
            tool: "memory.update",
            duration_ms,
            result_count,
            boundary_count: boundary_count_recorded,
            // Q1: search-only fields ABSENT on update.
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        };

        tracing::info!(
            target: "vault_mcp::tool_invoke",
            tool = details.tool,
            duration_ms = details.duration_ms,
            result_count = details.result_count,
            boundary_count = details.boundary_count,
            error = ?details.error,
            "memory.update tool invocation completed"
        );

        self.adapter
            .append_tool_invoke_audit(details)
            .await
            .map_err(vault_error_to_mcp)?;

        dispatch_result.map_err(vault_error_to_mcp)?;
        success_json_result(&serde_json::json!({ "updated": p.id }))
    }

    /// `memory.delete` MCP tool — remove a memory by id.
    ///
    /// **Step 5 (Phase 2): audit + tracing wired** with the same
    /// handler-mediated pattern. `parse_memory_id` early-return
    /// contract is identical to `tool_update` (see that doc comment).
    #[tool(
        name = "memory.delete",
        description = "Delete a memory by id. The vault verifies the \
                       memory's stored boundary against the authorized \
                       set before deletion."
    )]
    pub async fn tool_delete(
        &self,
        params: Parameters<DeleteToolParams>,
    ) -> Result<CallToolResult, McpError> {
        let Parameters(p) = params;
        let id = parse_memory_id_traced(&p.id, "memory.delete")?;

        let boundary_count_recorded: u32 = self.authorized_boundaries.len() as u32;
        let start = Instant::now();
        let dispatch_result = self.handle_delete(id).await;
        let duration_ms: u64 = start.elapsed().as_millis() as u64;

        let (result_count, error_for_audit) = match &dispatch_result {
            Ok(()) => (1_u32, None),
            Err(e) => (0_u32, Some(ToolInvokeError::from_vault_error(e))),
        };

        let details = ToolInvokeDetails {
            tool: "memory.delete",
            duration_ms,
            result_count,
            boundary_count: boundary_count_recorded,
            // Q1: search-only fields ABSENT on delete.
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        };

        tracing::info!(
            target: "vault_mcp::tool_invoke",
            tool = details.tool,
            duration_ms = details.duration_ms,
            result_count = details.result_count,
            boundary_count = details.boundary_count,
            error = ?details.error,
            "memory.delete tool invocation completed"
        );

        self.adapter
            .append_tool_invoke_audit(details)
            .await
            .map_err(vault_error_to_mcp)?;

        dispatch_result.map_err(vault_error_to_mcp)?;
        success_json_result(&serde_json::json!({ "deleted": p.id }))
    }
}

// =============================================================================
// ServerHandler impl — auto-routes `tools/list` + `tools/call` via #[tool_handler]
// =============================================================================

#[tool_handler]
impl ServerHandler for StdioServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "vault-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Memory Vault — a user-owned, cross-agent persistent memory layer. \
                 Tools: memory.search, memory.write, memory.update, memory.delete. \
                 Authorization is host-mediated; tool args never override boundaries.",
            )
    }
}

// =============================================================================
// Error mapping — VaultError → McpError per ADR-024
// =============================================================================

/// JSON-RPC code -32001 — implementation-defined "access denied" per
/// ADR-024 (HANDOFF.md lines 764, 766). Used for `AccessDenied` and
/// `ModelIntegrityFailed`; the latter intentionally collapses to the
/// same wire shape so an attacker can't fingerprint which model file
/// failed integrity (ADR-024 reasoning §793).
const ERROR_CODE_ACCESS_DENIED: ErrorCode = ErrorCode(-32001);

/// Map a `VaultError` into the JSON-RPC error shape ADR-024 specifies.
///
/// **No-info-leak invariant:** error messages are static / generic; no
/// internal state (paths, dimensions, audit ids) leaks into the wire
/// response. The `data` field is always `None` so a future "helpful"
/// detail can't slip through. Detailed diagnostics live in the audit
/// log + tracing emissions, never in the JSON-RPC error.
///
/// Step 3 (`dimension_mismatch_returns_generic_invalid_params_*`) pins
/// this contract for `DimensionMismatch`. Step 4 (this function)
/// converts the match to exhaustive AND aligns `AccessDenied` /
/// `ModelIntegrityFailed` to ADR-024's `-32001` mapping (previously
/// they routed via the `-32602` invalid_params arm and the catch-all
/// internal_error arm respectively — both diverged from ADR-024).
///
/// **Exhaustive by design.** Adding a new `VaultError` variant becomes
/// a compile error here. Decide deliberately whether the new variant
/// belongs in an existing arm (and update ADR-024's mapping table) or
/// gets its own row.
///
/// **Wording (Step 5 reconciliation):** the message string is `"invalid
/// params"` exactly — matches ADR-024 line 765 + the JSON-RPC 2.0 spec
/// literal for code `-32602` ("Invalid params" lower-cased). Step 3's
/// test was originally written against `"invalid parameters"`; Step 5
/// reverted that drift in the same commit as the tool_write/update/
/// delete wiring. Two-against-one to the locked artefacts (ADR-024 +
/// JSON-RPC 2.0 spec) wins over one shipped test.
fn vault_error_to_mcp(err: VaultError) -> McpError {
    match err {
        // ADR-024 line 765: -32602, "invalid params".
        VaultError::DimensionMismatch { .. } | VaultError::InvalidInput(_) => {
            McpError::invalid_params("invalid params", None)
        }
        // ADR-024 line 764: -32001, "access denied". Was -32602 before
        // Step 4 — Step 3's test pins DimensionMismatch only, so the
        // AccessDenied behaviour change is safe.
        VaultError::AccessDenied(_) => {
            McpError::new(ERROR_CODE_ACCESS_DENIED, "access denied", None)
        }
        // ADR-024 line 766: same wire shape as AccessDenied — denies
        // attacker fingerprinting of which model file failed.
        VaultError::ModelIntegrityFailed { .. } => {
            McpError::new(ERROR_CODE_ACCESS_DENIED, "access denied", None)
        }
        // ADR-024 line 767: Storage / Embedding / Retrieval → -32603.
        VaultError::Storage(_) | VaultError::Embedding(_) | VaultError::Retrieval(_) => {
            McpError::internal_error("internal error", None)
        }
        // ADR-024 silent on NotFound — preserves prior behaviour
        // (`-32602 invalid_params, "not found"`). MCP spec offers
        // `RESOURCE_NOT_FOUND = -32002` which may be a better fit;
        // out of Step 4 scope (no shipped test pins this), tracked
        // for ADR-024 amendment when memory.update / memory.delete
        // grow their own pin tests in Step 5.
        VaultError::NotFound(_) => McpError::invalid_params("not found", None),
        // ADR-024 silent on the remaining variants — preserves prior
        // catch-all behaviour (`-32603 internal_error`). The grouping
        // is deliberate and privacy-preserving: any of these leaking
        // structural detail would be a regression. Audit row carries
        // full per-variant detail via `ToolInvokeError::Internal`
        // (see `audit::ToolInvokeError::from_vault_error`).
        VaultError::Llm(_)
        | VaultError::Consolidation(_)
        | VaultError::Mcp(_)
        | VaultError::Sync(_)
        | VaultError::Connector(_)
        | VaultError::Auth(_)
        | VaultError::Crypto(_)
        | VaultError::Config(_)
        | VaultError::Io(_)
        | VaultError::Serde(_)
        // T0.1.10 Phase 2: WorkerSpawnFailed / McpBindFailed are startup
        // errors that surface during Application::start before any MCP
        // tool dispatch. They should never reach this function in
        // practice — startup failure aborts the process before MCP
        // accepts requests. Mapped to internal_error defensively to
        // preserve privacy posture if they ever do leak.
        //
        // T0.2.0 Phase 1 (2026-05-09): KeychainProvenance follows the same
        // discipline — keychain failures surface in vault-tauri's setup()
        // hook BEFORE Application::new is reached, never via MCP dispatch.
        // Defensive mapping preserves the same privacy posture (don't leak
        // namespace / vault_id / per-OS keychain state to an MCP client).
        // Per ADR-040 + ADR-040 amendment.
        | VaultError::WorkerSpawnFailed(_)
        | VaultError::McpBindFailed(_)
        | VaultError::KeychainProvenance(_) => McpError::internal_error("internal error", None),
    }
}

/// Serialise a value to a `CallToolResult` with a single JSON content
/// block — the canonical success shape for every vault tool.
fn success_json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let content = Content::json(value).map_err(|e| {
        // Content::json failures only happen on serialise errors, which
        // shouldn't occur for our domain types. Map to a generic internal
        // error rather than leaking the serde message.
        let _ = e;
        McpError::internal_error("response serialisation failed", None)
    })?;
    Ok(CallToolResult::success(vec![content]))
}

/// Parse a UUID-string into `MemoryId`, emitting a
/// `tracing::warn!(target: "vault_mcp::request_validation", ...)` event
/// on parse failure for operational visibility.
///
/// **Audit contract (Step 5 design decision):** parse failures here
/// do NOT append to the audit chain. The audit chain records vault
/// dispatches (Q7 a handler-mediated audit) and a malformed-id
/// request never reaches the vault. This is analogous to JSON
/// deserialisation, which is also not audited. Tracing-level
/// visibility on a separate target (`vault_mcp::request_validation`
/// vs `vault_mcp::tool_invoke`) keeps the operational log filterable
/// — operators can grep one target for tool dispatches and the other
/// for malformed-request rejections.
///
/// `tool_name` is included in the warn event so operators can tell
/// which tool received the malformed id (`memory.update` vs
/// `memory.delete`).
fn parse_memory_id_traced(id: &str, tool_name: &'static str) -> Result<MemoryId, McpError> {
    match id.parse::<uuid::Uuid>() {
        Ok(uuid) => Ok(MemoryId(uuid)),
        Err(_) => {
            // No `id` value or other content goes into the tracing
            // event — only metadata. Same content-redaction discipline
            // as the tool_invoke target (Q6).
            tracing::warn!(
                target: "vault_mcp::request_validation",
                tool = tool_name,
                reason = "uuid_parse_failed",
                "malformed id in tool request"
            );
            Err(McpError::invalid_params("invalid params", None))
        }
    }
}
