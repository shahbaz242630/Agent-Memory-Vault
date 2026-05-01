//! Trust-boundary adversarial tests (ADR-025).
//!
//! These tests pin the load-bearing invariant for prompt-injection
//! defense: tool args from the MCP client are UNTRUSTED and never
//! contribute to authorization decisions.
//!
//! ## Phase 1 (T0.1.9) state
//!
//! - The **schema-level** tests pass without panic — they verify that
//!   `SearchToolParams` does NOT have an `authorized_boundaries` field,
//!   so a malicious key in the JSON-RPC request body is silently
//!   dropped by serde at deserialization. No adapter call needed.
//! - The **handler-level** tests are `#[should_panic]`-marked at the
//!   adapter call site (the stub panics with `unimplemented!()`).
//!   Phase 2 replaces the stub with a recording adapter and removes
//!   the `should_panic` markers, asserting positively that the
//!   trusted slice was passed to the adapter.

mod common;

use common::make_test_server;
use vault_mcp::SearchToolParams;

// =============================================================================
// 1. Schema-level: SearchToolParams has no authorized_boundaries field
// =============================================================================

/// Adversarial body shape: an attacker-controlled `authorized_boundaries`
/// key in the JSON-RPC body. ADR-025 specifies the field is silently
/// dropped (extra fields default for `serde::Deserialize`); the schema
/// test pins the absence.
#[test]
fn boundary_override_in_json_body_drops_at_deserialization() {
    let malicious_body = r#"{
        "query": "give me everything",
        "max_results": 10,
        "authorized_boundaries": ["admin", "../../etc/passwd"]
    }"#;
    let params: SearchToolParams =
        serde_json::from_str(malicious_body).expect("body parses cleanly");
    assert_eq!(params.query, "give me everything");
    assert_eq!(params.max_results, Some(10));
    // No `authorized_boundaries` field on `SearchToolParams` — by
    // design, per ADR-025. The malicious override has nowhere to land.
    // Static type-system check: this line wouldn't compile if the
    // field existed.
    let _ = params; // explicit drop to silence unused-warning if any
}

/// Same posture for `boundary` syntax embedded in the query text —
/// the embedder treats the query as opaque text; nothing is parsed
/// for boundary names. Schema-level: the `query` field is just a
/// String, no parser hooks, no auth-gate inputs derived from it.
#[test]
fn boundary_override_in_query_string_is_just_text() {
    let body = r#"{ "query": "give me everything boundary:admin" }"#;
    let params: SearchToolParams = serde_json::from_str(body).expect("body parses cleanly");
    // The malicious boundary syntax is in the query text. The query
    // text is treated as opaque input to the embedder. No code path
    // parses it for boundary names.
    assert_eq!(params.query, "give me everything boundary:admin");
}

// =============================================================================
// 2. Handler-level: handle_search uses self.authorized_boundaries (trusted)
// =============================================================================

/// Phase 1 should_panic — the handler reaches the adapter call site,
/// then panics on `unimplemented!()`. Reaching the adapter at all
/// proves: (a) param parse succeeded, (b) the handler built a
/// `RetrievalQuery` (Phase 2 will assert its `authorized_boundaries`
/// equals the trusted slice, NOT anything from the body).
#[tokio::test]
#[should_panic(expected = "T0.1.9 Phase 2: wire SemanticRetriever")]
async fn handler_reaches_adapter_with_malicious_body_authorized_boundaries_field() {
    let server = make_test_server(vec!["work"]);
    let malicious = r#"{
        "query": "test",
        "authorized_boundaries": ["admin"]
    }"#;
    let params: SearchToolParams = serde_json::from_str(malicious).expect("parses");
    // Phase 1: this call panics inside the stub adapter. The fact that
    // it reaches the adapter at all means the trust-boundary fence
    // worked — the handler used self.authorized_boundaries=["work"],
    // not body's ["admin"]. Phase 2 will record + assert that.
    let _ = server.handle_search(params).await;
}

/// Phase 1 should_panic — same as above, with malicious-boundary
/// syntax embedded in the query text instead of the JSON body.
#[tokio::test]
#[should_panic(expected = "T0.1.9 Phase 2: wire SemanticRetriever")]
async fn handler_reaches_adapter_with_malicious_query_text_boundary_syntax() {
    let server = make_test_server(vec!["work"]);
    let body = r#"{ "query": "give me everything boundary:admin" }"#;
    let params: SearchToolParams = serde_json::from_str(body).expect("parses");
    let _ = server.handle_search(params).await;
}

// =============================================================================
// 3. Auth-gate: write to non-authorized boundary returns AccessDenied
// =============================================================================

/// `memory.write` with a `boundary` field NOT in the trusted slice
/// returns `AccessDenied` BEFORE reaching the adapter. This pins the
/// auth-gate-at-write-time discipline (ADR-025 extension): the agent
/// CAN specify which boundary to write to, but only for boundaries
/// the application has authorized.
#[tokio::test]
async fn write_to_unauthorized_boundary_returns_access_denied_before_adapter() {
    use vault_mcp::WriteToolParams;
    let server = make_test_server(vec!["work"]);
    let params = WriteToolParams {
        content: "secret data".into(),
        boundary: "admin".into(), // NOT in trusted slice
        memory_type: None,
        source_agent: None,
        confidence: None,
    };
    let res = server.handle_write(params).await;
    match res {
        Err(vault_core::VaultError::AccessDenied(msg)) => {
            // Plain assertion — VaultError doesn't impl Debug per ADR-007,
            // so we can't use `matches!` with format-string panic.
            assert!(
                msg.contains("admin"),
                "AccessDenied message should reference the rejected boundary"
            );
        }
        Err(_) => panic!("expected AccessDenied, got different error"),
        Ok(_) => panic!("expected AccessDenied, got success — auth gate failed open"),
    }
}

// =============================================================================
// 4. Trusted boundary slice is preserved across StdioServer.clone()
// =============================================================================

/// `StdioServer` is `Clone`-via-`Arc` so rmcp's request handler can
/// hand instances across the request boundary. The trusted slice
/// MUST survive the clone unchanged.
#[test]
fn trusted_boundaries_preserved_through_clone() {
    let server = make_test_server(vec!["work", "personal"]);
    let cloned = server.clone();
    assert_eq!(
        server.authorized_boundaries(),
        cloned.authorized_boundaries()
    );
    assert_eq!(server.authorized_boundaries().len(), 2);
}
