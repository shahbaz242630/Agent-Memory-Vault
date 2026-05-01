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

use serde::Deserialize;
use vault_core::{Boundary, MemoryId, MemoryType, NewMemory, VaultError, VaultResult};
use vault_retrieval::{RetrievalOptions, RetrievalQuery};

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
/// **NOTE (chrono pin / schemars deferral):** Phase 1 uses bare
/// `serde::Deserialize` (no `schemars::JsonSchema`) per the chrono pin
/// constraint documented in `vault-mcp/Cargo.toml`. Phase 2 may add
/// schemars when the workspace chrono pin can advance.
#[derive(Debug, Deserialize)]
pub struct SearchToolParams {
    /// Free-text query — embedded by the model and matched via cosine
    /// k-NN over the boundary-filtered vector store.
    pub query: String,
    /// Maximum number of results to return. Defaults to 10 (server side)
    /// if omitted; capped at `vault_retrieval::MAX_RESULTS_CAP` (100).
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

/// JSON-RPC parameters for the `memory.write` tool.
///
/// **NOTE (ADR-025):** the `boundary` field IS user-controlled — the
/// agent specifies which boundary to write to. The handler validates
/// this field appears in `self.authorized_boundaries` BEFORE calling
/// the adapter; if not, returns `VaultError::AccessDenied` (mapped to
/// JSON-RPC `-32001` per ADR-024).
#[derive(Debug, Deserialize)]
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
}

impl StdioServer {
    /// Construct a new server. Both arguments are application-supplied
    /// at startup and form the trust boundary per ADR-025.
    pub fn new(adapter: Arc<dyn Adapter>, authorized_boundaries: Vec<Boundary>) -> Self {
        Self {
            adapter,
            authorized_boundaries,
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

    /// `memory.delete` Phase 1 stub. The adapter is responsible for
    /// boundary verification on the existing memory's stored boundary
    /// (the agent supplies only the id); Phase 2 wires this against
    /// `vault-storage::StorageBackend::delete_memory`.
    pub async fn handle_delete(&self, id: MemoryId) -> VaultResult<()> {
        self.adapter.delete(id).await
    }
}
