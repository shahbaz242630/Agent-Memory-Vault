//! `memory_write` settable `as_of` → `valid_from` mapping.
//!
//! Decision locked 2026-05-30 ([[as-of-write-time-blocks-a5-temporal]]):
//! `memory_write` accepts an optional `as_of` (the date a fact became
//! true). It seeds `NewMemory.valid_from`; when absent the vault falls
//! back to the write timestamp (`Memory::try_new` defaults `None` → now).
//! `memory_update` is unaffected — it preserves the original `valid_from`
//! per ADR-028, so `as_of` is a write-only field.
//!
//! These tests assert at the `handle_write` boundary using the recording
//! [`MockAdapter`], whose `write_calls()` capture the `NewMemory` exactly
//! as dispatched (the `valid_from` we set, before any downstream default).
//! `valid_from` is compared via `to_rfc3339()` so no chrono import is
//! needed in the test crate.

#![forbid(unsafe_code)]

mod common;

use common::make_mock_server_with_adapter;
use vault_mcp::WriteToolParams;

/// Build a write-params payload to `testeval`-style boundary "work" with
/// the given optional `as_of`.
fn write_params(content: &str, as_of: Option<&str>) -> WriteToolParams {
    WriteToolParams {
        content: content.to_string(),
        boundary: "work".to_string(),
        memory_type: None,
        source_agent: None,
        confidence: None,
        as_of: as_of.map(str::to_string),
    }
}

#[tokio::test]
async fn as_of_date_only_seeds_valid_from_at_midnight_utc() {
    let (server, adapter) = make_mock_server_with_adapter(vec!["work"]);

    server
        .handle_write(write_params(
            "As of 2026-02-01 the user drives a Tesla Model 3.",
            Some("2026-02-01"),
        ))
        .await
        .expect("write with valid as_of date must succeed");

    let writes = adapter.write_calls();
    assert_eq!(writes.len(), 1, "exactly one write dispatched");
    assert_eq!(
        writes[0].valid_from.map(|d| d.to_rfc3339()),
        Some("2026-02-01T00:00:00+00:00".to_string()),
        "date-only as_of must seed valid_from at midnight UTC"
    );
}

#[tokio::test]
async fn as_of_rfc3339_timestamp_seeds_exact_valid_from() {
    let (server, adapter) = make_mock_server_with_adapter(vec!["work"]);

    server
        .handle_write(write_params(
            "The user joined Atlas.",
            Some("2026-05-01T08:30:00Z"),
        ))
        .await
        .expect("write with valid as_of timestamp must succeed");

    let writes = adapter.write_calls();
    assert_eq!(
        writes[0].valid_from.map(|d| d.to_rfc3339()),
        Some("2026-05-01T08:30:00+00:00".to_string()),
        "RFC-3339 as_of must seed valid_from exactly (normalised to UTC)"
    );
}

#[tokio::test]
async fn as_of_with_offset_normalises_to_utc() {
    let (server, adapter) = make_mock_server_with_adapter(vec!["work"]);

    // 2026-05-01T10:30:00+02:00 == 2026-05-01T08:30:00Z.
    server
        .handle_write(write_params(
            "The user joined Atlas.",
            Some("2026-05-01T10:30:00+02:00"),
        ))
        .await
        .expect("write with offset timestamp must succeed");

    let writes = adapter.write_calls();
    assert_eq!(
        writes[0].valid_from.map(|d| d.to_rfc3339()),
        Some("2026-05-01T08:30:00+00:00".to_string()),
        "offset as_of must be converted to UTC"
    );
}

#[tokio::test]
async fn absent_as_of_leaves_valid_from_none_for_write_time_fallback() {
    let (server, adapter) = make_mock_server_with_adapter(vec!["work"]);

    server
        .handle_write(write_params("The user prefers dark mode.", None))
        .await
        .expect("write without as_of must succeed");

    let writes = adapter.write_calls();
    assert!(
        writes[0].valid_from.is_none(),
        "absent as_of must leave valid_from = None so the write-time \
         fallback applies downstream (Memory::try_new defaults None → now)"
    );
}

#[tokio::test]
async fn malformed_as_of_is_rejected_and_never_dispatched() {
    let (server, adapter) = make_mock_server_with_adapter(vec!["work"]);

    let result = server
        .handle_write(write_params(
            "The user drives a Rivian.",
            Some("not-a-date"),
        ))
        .await;

    assert!(
        result.is_err(),
        "a malformed as_of must be rejected, not silently dropped to write-time"
    );
    assert!(
        adapter.write_calls().is_empty(),
        "rejection must happen before adapter dispatch — nothing written"
    );
}
