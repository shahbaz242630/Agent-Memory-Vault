//! Tauri command surface — V0.1 BRD §5.11 (5 commands).
//!
//! Five commands dispatch through `Application::adapter()` (the wired
//! `VaultAdapter` from Phase 4a) per ADR-030 outcome (a) single-process
//! MCP architecture. Plus one banner-ack command writes directly to the
//! metadata-store audit chain via
//! `VaultAdapter::append_alpha_banner_acknowledged_audit` (UI state, not
//! vault-state CRUD; per ADR-024 amendment 2026-05-05).
//!
//! ## ADR-024 amendment 2026-05-05 (Decision 5(γ)) — TauriCommandInvoke audit
//!
//! Each Tauri CRUD command writes a `TauriCommandInvoke` audit row via
//! `VaultAdapter::append_tauri_command_audit` after the operation. The
//! row carries the same `ToolInvokeDetails` shape as `mcp.tool_invoke`
//! but the event_type discriminator distinguishes UI-origin from
//! MCP-origin. Per ADR-024: "audit chain is the authoritative record of
//! vault-state changes" — Tauri commands ARE vault-state changes.
//!
//! ## Auth-gating posture vs MCP commands
//!
//! Tauri commands operate as `actor_kind = User` (founder is the actor),
//! NOT as `Agent` (which is the ADR-025-locked actor for MCP commands).
//! For V0.1 founder-only dogfood, the founder owns all vault state and
//! auth-gating doesn't apply at the Tauri layer. The ADR-025 amendment
//! auth-gate from Phase 4a remains in place at the MCP/StdioServer layer
//! to protect against untrusted MCP agents — different trust contexts,
//! different auth needs.
//!
//! ## Testability pattern
//!
//! Each `#[tauri::command]` wrapper delegates to a sibling `*_inner`
//! async fn that takes `&Application` directly (not wrapped in
//! `State<'_, Application>`). Tests exercise the inner function with a
//! real Application from a test fixture; the `#[tauri::command]`
//! wrapper is a thin glue that converts errors to user-friendly Strings
//! and cannot be tested without the full Tauri runtime.

use std::time::Instant;

use tauri::State;
use vault_app::Application;
use vault_core::{Boundary, MemoryId, MemoryType, NewMemory};
use vault_mcp::{Adapter, ToolInvokeDetails};
use vault_retrieval::{RetrievalOptions, RetrievalQuery};

/// Inner add_memory implementation. Pure async fn over `&Application`
/// for testability.
pub async fn add_memory_inner(
    app: &Application,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<MemoryId, String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let parsed_memory_type = match memory_type.as_str() {
        "semantic" => MemoryType::Semantic,
        "episodic" => MemoryType::Episodic,
        "procedural" => MemoryType::Procedural,
        other => return Err(format!("invalid memory_type: '{other}'")),
    };
    let parsed_boundary = Boundary::new(&boundary).map_err(|e| format!("invalid boundary: {e}"))?;

    let new_memory = NewMemory {
        content,
        memory_type: parsed_memory_type,
        boundary: parsed_boundary,
        source_agent: Some("vault-tauri".to_string()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    };

    let result = adapter.write(new_memory).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (id, error_for_audit) = match &result {
        Ok(id) => (Some(*id), None),
        Err(e) => (None, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "add_memory",
            duration_ms,
            result_count: if id.is_some() { 1 } else { 0 },
            boundary_count: 1,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn add_memory(
    state: State<'_, Application>,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<String, String> {
    add_memory_inner(state.inner(), content, memory_type, boundary)
        .await
        .map(|id| id.to_string())
}

/// Inner search_memories implementation.
pub async fn search_memories_inner(
    app: &Application,
    query: String,
    limit: usize,
    authorized_boundaries: Vec<Boundary>,
) -> Result<Vec<serde_json::Value>, String> {
    let adapter = app.adapter();
    let start = Instant::now();
    let query_length = query.len();

    let retrieval_query = RetrievalQuery {
        query_text: query,
        authorized_boundaries: authorized_boundaries.clone(),
        max_results: limit,
        options: RetrievalOptions {
            score_threshold: None,
            include_archived: false,
        },
    };

    let result = adapter.search(retrieval_query).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let (count, error_for_audit) = match &result {
        Ok(memories) => (memories.len() as u32, None),
        Err(e) => (0, Some(vault_mcp::ToolInvokeError::from_vault_error(e))),
    };

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "search_memories",
            duration_ms,
            result_count: count,
            boundary_count: authorized_boundaries.len() as u32,
            max_results: Some(limit as u32),
            score_threshold: None,
            include_archived: Some(false),
            query_length: Some(query_length as u32),
            error: error_for_audit,
        })
        .await;

    result
        .map(|memories| {
            memories
                .into_iter()
                .map(|rm| {
                    serde_json::json!({
                        "id": rm.memory.id.to_string(),
                        "content": rm.memory.content,
                        "memory_type": format!("{:?}", rm.memory.memory_type).to_lowercase(),
                        "boundary": rm.memory.boundary.as_str(),
                        "score": rm.score,
                        "explanation": rm.explanation,
                        "created_at": rm.memory.created_at.to_rfc3339(),
                    })
                })
                .collect()
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn search_memories(
    state: State<'_, Application>,
    query: String,
    limit: usize,
) -> Result<Vec<serde_json::Value>, String> {
    // V0.1 founder-only: founder has implicit access to default boundary.
    // Multi-boundary management UI deferred to V0.2 alpha-distribution.
    let boundaries = vec![Boundary::default_name()];
    search_memories_inner(state.inner(), query, limit, boundaries).await
}

/// Inner update_memory implementation.
pub async fn update_memory_inner(
    app: &Application,
    id_str: String,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<(), String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let id: MemoryId = id_str
        .parse()
        .map_err(|e| format!("invalid memory id: {e}"))?;

    let parsed_memory_type = match memory_type.as_str() {
        "semantic" => MemoryType::Semantic,
        "episodic" => MemoryType::Episodic,
        "procedural" => MemoryType::Procedural,
        other => return Err(format!("invalid memory_type: '{other}'")),
    };
    let parsed_boundary = Boundary::new(&boundary).map_err(|e| format!("invalid boundary: {e}"))?;

    let new_memory = NewMemory {
        content,
        memory_type: parsed_memory_type,
        boundary: parsed_boundary,
        source_agent: Some("vault-tauri".to_string()),
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    };

    let result = adapter.update(id, new_memory).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let error_for_audit = result
        .as_ref()
        .err()
        .map(vault_mcp::ToolInvokeError::from_vault_error);

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "update_memory",
            duration_ms,
            result_count: if result.is_ok() { 1 } else { 0 },
            boundary_count: 1,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_memory(
    state: State<'_, Application>,
    id: String,
    content: String,
    memory_type: String,
    boundary: String,
) -> Result<(), String> {
    update_memory_inner(state.inner(), id, content, memory_type, boundary).await
}

/// Inner delete_memory implementation. Note: ADR-025 amendment auth-
/// gate from Phase 4a lives at the MCP/StdioServer layer; Tauri layer
/// operates as founder/User actor and bypasses that gate (V0.1
/// founder-only context). V0.2 alpha-cohort will revisit per
/// ADR-029-implied multi-user trust context.
pub async fn delete_memory_inner(app: &Application, id_str: String) -> Result<(), String> {
    let adapter = app.adapter();
    let start = Instant::now();

    let id: MemoryId = id_str
        .parse()
        .map_err(|e| format!("invalid memory id: {e}"))?;

    let result = adapter.delete(id).await;
    let duration_ms = start.elapsed().as_millis() as u64;

    let error_for_audit = result
        .as_ref()
        .err()
        .map(vault_mcp::ToolInvokeError::from_vault_error);

    let _ = adapter
        .append_tauri_command_audit(ToolInvokeDetails {
            tool: "delete_memory",
            duration_ms,
            result_count: if result.is_ok() { 1 } else { 0 },
            boundary_count: 0,
            max_results: None,
            score_threshold: None,
            include_archived: None,
            query_length: None,
            error: error_for_audit,
        })
        .await;

    result.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_memory(state: State<'_, Application>, id: String) -> Result<(), String> {
    delete_memory_inner(state.inner(), id).await
}

/// Inner acknowledge_alpha_banner implementation.
pub async fn acknowledge_alpha_banner_inner(app: &Application) -> Result<(), String> {
    app.adapter()
        .append_alpha_banner_acknowledged_audit()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn acknowledge_alpha_banner(state: State<'_, Application>) -> Result<(), String> {
    acknowledge_alpha_banner_inner(state.inner()).await
}

#[cfg(test)]
mod tests {
    // Test fixture construction for vault-tauri commands requires a
    // real Application (SqlCipher + LanceDB + DuckDB + ORT). This is
    // the same constraint as vault-app/tests/integration_smoke.rs —
    // those tests are #[ignore]-by-default per session-discipline.
    //
    // Phase 4b commits 5 IPC tests + 1 audit-row test as
    // #[ignore]-by-default placeholders; Phase 5 close OR V0.2 alpha-
    // distribution task lands the real implementations once the test
    // fixture infrastructure is shared across vault-app +
    // vault-tauri (currently each crate would need to duplicate the
    // BgeSmallProvider + StorageBackend setup boilerplate).
    //
    // Per Shahbaz Phase 4b v2 review + step-expansion #[ignore]
    // discipline: each placeholder unimplemented! body fails when run
    // with --ignored. Workspace floor reflects the +6 ignored deltas.

    /// Phase 4b ignored placeholder. ADR-024 amendment Decision 5(γ)
    /// pinning test — landing the real impl needs shared test-fixture
    /// scaffolding for Application construction.
    #[tokio::test]
    #[ignore = "Phase 4b deferred — Application test-fixture needs sharing across vault-app + vault-tauri; lands at V0.2 alpha-distribution"]
    async fn add_memory_command_dispatches_through_adapter_write() {
        unimplemented!("Phase 4b ignored placeholder — V0.2 alpha-distribution lands real impl");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint as add_memory_command test above"]
    async fn search_memories_command_dispatches_through_adapter_search() {
        unimplemented!("Phase 4b ignored placeholder");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint"]
    async fn update_memory_command_dispatches_through_adapter_update_with_full_newmemory_per_adr_028(
    ) {
        unimplemented!("Phase 4b ignored placeholder");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint"]
    async fn delete_memory_command_dispatches_through_adapter_delete_with_auth_gate_inherited_from_phase_4a(
    ) {
        unimplemented!("Phase 4b ignored placeholder");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint"]
    async fn acknowledge_alpha_banner_writes_alphabanneracknowledged_audit_row() {
        unimplemented!("Phase 4b ignored placeholder");
    }

    #[tokio::test]
    #[ignore = "Phase 4b deferred — same fixture-sharing constraint"]
    async fn tauri_command_invoke_audit_row_written_per_adr_024_amendment() {
        unimplemented!("Phase 4b ignored placeholder");
    }
}
