//! Dead-letter table — terminal state for cascading-write failures.
//!
//! Per `T0.1.6_PLAN.md` Q4 and migration `0002_cascade_infra.sql`. An
//! entry lands here when:
//! - the retry queue exhausts [`MAX_ATTEMPTS`] backoff retries
//!   (`reason = AttemptsExhausted` from the orchestrator's perspective), or
//! - the orchestrator classifies the first failure as permanent via
//!   [`crate::retry_queue::is_permanent`] (`reason = Permanent`).
//!
//! The orchestrator (Phase C) is responsible for the transfer: on a
//! [`crate::retry_queue::FailureOutcome::DeadLetter`], it copies the
//! `RetryEntry` metadata into a [`NewDeadLetter`] and calls [`DeadLetter::insert`].
//!
//! Resolution lifecycle — resolutions are terminal (a resolved entry never
//! returns to pending). Idempotent re-resolution to the **same** state is
//! a no-op success; re-resolution to a **different** state is rejected
//! with [`VaultError::InvalidInput`].

use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use tracing::{debug, instrument};
use uuid::Uuid;

use vault_core::{MemoryId, VaultError, VaultResult};

use crate::metadata_store::MetadataStore;
use crate::retry_queue::CascadeOperation;

/// Truncation cap for `failure_reason`. Mirrors `LAST_ERROR_MAX_BYTES` on
/// the retry queue side and the schema comment on migration 0002.
pub const FAILURE_REASON_MAX_BYTES: usize = 4096;

/// Schema version for the on-disk JSON `payload` BLOB. Independent of
/// `retry_queue::PAYLOAD_FORMAT_VERSION` (the orchestrator may evolve them
/// at different cadences); the values happen to match for V0.1.
pub const PAYLOAD_FORMAT_VERSION: i64 = 1;

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Terminal resolution states. Persisted strings match the column comment
/// on `dead_letter.resolution` in migration 0002.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// Operator (or auto-recovery) re-ran the operation and it succeeded.
    RetriedSucceeded,
    /// Operator (or auto-recovery) re-ran and it failed again — the entry
    /// stays dead-lettered, this state records that the retry was tried.
    RetriedFailed,
    /// Operator explicitly accepted the loss after reading the entry.
    /// Row stays for audit but no further retries happen.
    Acknowledged,
    /// Divergence detector / consolidator observed the memory in the
    /// downstream store anyway (e.g., a previous orphaned write that
    /// partially succeeded) and is closing the dead-letter without
    /// operator action.
    AutoRecovered,
}

impl Resolution {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RetriedSucceeded => "retried_succeeded",
            Self::RetriedFailed => "retried_failed",
            Self::Acknowledged => "acknowledged",
            Self::AutoRecovered => "auto_recovered",
        }
    }
}

impl FromStr for Resolution {
    type Err = VaultError;

    fn from_str(s: &str) -> VaultResult<Self> {
        match s {
            "retried_succeeded" => Ok(Self::RetriedSucceeded),
            "retried_failed" => Ok(Self::RetriedFailed),
            "acknowledged" => Ok(Self::Acknowledged),
            "auto_recovered" => Ok(Self::AutoRecovered),
            other => Err(VaultError::Storage(format!(
                "unknown dead_letter resolution: {other}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence types
// ---------------------------------------------------------------------------

/// New dead-letter insertion. Caller (orchestrator) supplies all metadata
/// from the originating retry entry; this module assigns the dead-letter
/// row's own UUID v7 id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewDeadLetter {
    pub memory_id: MemoryId,
    pub failed_operation: CascadeOperation,
    pub failure_reason: String,
    pub attempts_made: u32,
    pub first_failed_at: DateTime<Utc>,
    pub last_attempted_at: DateTime<Utc>,
    pub payload_format_version: i64,
    pub payload: serde_json::Value,
}

/// A dead-letter row as persisted.
#[derive(Clone, Debug, PartialEq)]
pub struct DeadLetterEntry {
    pub id: Uuid,
    pub memory_id: MemoryId,
    pub failed_operation: CascadeOperation,
    pub failure_reason: String,
    pub attempts_made: u32,
    pub first_failed_at: DateTime<Utc>,
    pub last_attempted_at: DateTime<Utc>,
    pub payload_format_version: i64,
    pub payload: serde_json::Value,
    pub resolution: Option<Resolution>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Truncate a UTF-8 string at a code-point boundary. Mirrors the helper in
/// `retry_queue` — kept private here to avoid leaking an internal helper
/// across module boundaries.
fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    s[..end].to_string()
}

// ---------------------------------------------------------------------------
// DeadLetter (data-layer persistence)
// ---------------------------------------------------------------------------

/// Persistent dead-letter table. Cheap to clone.
#[derive(Clone)]
pub struct DeadLetter {
    store: MetadataStore,
}

impl DeadLetter {
    /// Construct a new handle backed by an open `MetadataStore`.
    pub fn new(store: MetadataStore) -> Self {
        Self { store }
    }

    /// Insert a new dead-letter row in the `pending` state. Returns the
    /// assigned id.
    #[instrument(skip(self, new), fields(memory_id = %new.memory_id, op = new.failed_operation.as_str()))]
    pub async fn insert(&self, new: NewDeadLetter) -> VaultResult<Uuid> {
        let id = Uuid::now_v7();
        let reason = truncate_utf8(&new.failure_reason, FAILURE_REASON_MAX_BYTES);
        let payload_bytes = serde_json::to_vec(&new.payload)?;

        self.store
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "INSERT INTO dead_letter (
                        id, memory_id, failed_operation, failure_reason,
                        attempts_made, first_failed_at, last_attempted_at,
                        payload_format_version, payload, resolution, resolved_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL, NULL)",
                    params![
                        id.as_bytes().to_vec(),
                        new.memory_id.0.as_bytes().to_vec(),
                        new.failed_operation.as_str(),
                        reason,
                        new.attempts_made,
                        new.first_failed_at.to_rfc3339(),
                        new.last_attempted_at.to_rfc3339(),
                        new.payload_format_version,
                        payload_bytes,
                    ],
                )
                .map_err(|e| VaultError::Storage(format!("insert dead_letter: {e}")))?;
                Ok(())
            })
            .await?;

        debug!(%id, "dead-letter inserted");
        Ok(id)
    }

    /// Return unresolved entries (`resolution IS NULL`), oldest first by
    /// `id` (UUID v7 is time-ordered, so this is insert-time order).
    /// Backed by the partial index `idx_dead_letter_unresolved`.
    pub async fn list_unresolved(&self, limit: usize) -> VaultResult<Vec<DeadLetterEntry>> {
        self.store
            .with_conn_blocking(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, memory_id, failed_operation, failure_reason,
                                attempts_made, first_failed_at, last_attempted_at,
                                payload_format_version, payload, resolution, resolved_at
                           FROM dead_letter
                          WHERE resolution IS NULL
                          ORDER BY id ASC
                          LIMIT ?1",
                    )
                    .map_err(|e| VaultError::Storage(format!("prepare list_unresolved: {e}")))?;
                let rows = stmt
                    .query_map(params![limit as i64], row_to_dead_letter)
                    .map_err(|e| VaultError::Storage(format!("query list_unresolved: {e}")))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(
                        r.map_err(|e| VaultError::Storage(format!("read dead_letter row: {e}")))?,
                    );
                }
                Ok(out)
            })
            .await
    }

    /// Look up by id.
    pub async fn get(&self, id: Uuid) -> VaultResult<Option<DeadLetterEntry>> {
        self.store
            .with_conn_blocking(move |conn| {
                conn.query_row(
                    "SELECT id, memory_id, failed_operation, failure_reason,
                            attempts_made, first_failed_at, last_attempted_at,
                            payload_format_version, payload, resolution, resolved_at
                       FROM dead_letter WHERE id = ?1",
                    params![id.as_bytes().to_vec()],
                    row_to_dead_letter,
                )
                .optional()
                .map_err(|e| VaultError::Storage(format!("get dead_letter: {e}")))
            })
            .await
    }

    /// Transition pending → terminal resolution.
    ///
    /// - First call on a pending row: resolution + `resolved_at = now()`.
    /// - Re-call with the same resolution: no-op success (idempotent).
    /// - Re-call with a different resolution: `VaultError::InvalidInput`.
    /// - Missing id: `VaultError::NotFound`.
    #[instrument(skip(self), fields(%id, resolution = resolution.as_str()))]
    pub async fn resolve(&self, id: Uuid, resolution: Resolution) -> VaultResult<()> {
        self.store
            .with_conn_blocking(move |conn| {
                let tx = conn
                    .transaction()
                    .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;

                // Inspect current state.
                let current: Option<Option<String>> = tx
                    .query_row(
                        "SELECT resolution FROM dead_letter WHERE id = ?1",
                        params![id.as_bytes().to_vec()],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| {
                        VaultError::Storage(format!("read dead_letter resolution: {e}"))
                    })?;

                match current {
                    None => {
                        return Err(VaultError::NotFound(format!(
                            "dead_letter entry {id} not found"
                        )))
                    }
                    Some(Some(existing)) => {
                        // Already resolved. Idempotent only on identity.
                        if existing == resolution.as_str() {
                            // No-op.
                            return Ok(());
                        }
                        return Err(VaultError::InvalidInput(format!(
                            "dead_letter {id} already resolved as {existing}; cannot transition to {}",
                            resolution.as_str()
                        )));
                    }
                    Some(None) => { /* pending — proceed */ }
                }

                tx.execute(
                    "UPDATE dead_letter
                        SET resolution = ?2, resolved_at = ?3
                      WHERE id = ?1
                        AND resolution IS NULL",
                    params![
                        id.as_bytes().to_vec(),
                        resolution.as_str(),
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(|e| VaultError::Storage(format!("update dead_letter resolution: {e}")))?;

                tx.commit()
                    .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
                Ok(())
            })
            .await
    }

    /// Total rows (resolved + pending).
    pub async fn len(&self) -> VaultResult<usize> {
        self.store
            .with_conn_blocking(|conn| {
                let n: i64 = conn
                    .query_row("SELECT COUNT(*) FROM dead_letter", [], |row| row.get(0))
                    .map_err(|e| VaultError::Storage(format!("count dead_letter: {e}")))?;
                Ok(n as usize)
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Row decoder (private)
// ---------------------------------------------------------------------------

fn row_to_dead_letter(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeadLetterEntry> {
    let id_bytes: Vec<u8> = row.get(0)?;
    let id = decode_uuid(&id_bytes, 0, "dead_letter.id")?;

    let mem_bytes: Vec<u8> = row.get(1)?;
    let memory_id = MemoryId(decode_uuid(&mem_bytes, 1, "dead_letter.memory_id")?);

    let op_s: String = row.get(2)?;
    let failed_operation = CascadeOperation::from_str(&op_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("dead_letter.failed_operation: {e}"),
            )),
        )
    })?;

    let failure_reason: String = row.get(3)?;
    let attempts_made_i: i64 = row.get(4)?;
    let attempts_made: u32 = u32::try_from(attempts_made_i).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("dead_letter.attempts_made out of u32 range: {attempts_made_i}"),
            )),
        )
    })?;

    let first_failed_at: DateTime<Utc> = row.get(5)?;
    let last_attempted_at: DateTime<Utc> = row.get(6)?;

    let payload_format_version: i64 = row.get(7)?;
    let payload_bytes: Vec<u8> = row.get(8)?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Blob, Box::new(e))
    })?;

    let resolution_s: Option<String> = row.get(9)?;
    let resolution = match resolution_s {
        None => None,
        Some(s) => Some(Resolution::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("dead_letter.resolution: {e}"),
                )),
            )
        })?),
    };
    let resolved_at: Option<DateTime<Utc>> = row.get(10)?;

    Ok(DeadLetterEntry {
        id,
        memory_id,
        failed_operation,
        failure_reason,
        attempts_made,
        first_failed_at,
        last_attempted_at,
        payload_format_version,
        payload,
        resolution,
        resolved_at,
    })
}

fn decode_uuid(bytes: &[u8], col: usize, label: &'static str) -> rusqlite::Result<Uuid> {
    let arr: [u8; 16] = bytes.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            col,
            rusqlite::types::Type::Blob,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{label}: expected 16 bytes, got {}", bytes.len()),
            )),
        )
    })?;
    Ok(Uuid::from_bytes(arr))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::key::SqlCipherKey;
    use chrono::Duration;
    use tempfile::TempDir;

    async fn make_dead_letter() -> (TempDir, DeadLetter) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let key = SqlCipherKey::new("dead-letter-test-key");
        let store = MetadataStore::open(&path, key).await.unwrap();
        (tmp, DeadLetter::new(store))
    }

    fn sample_new(reason: &str) -> NewDeadLetter {
        let now = Utc::now();
        NewDeadLetter {
            memory_id: MemoryId::new(),
            failed_operation: CascadeOperation::Write,
            failure_reason: reason.to_string(),
            attempts_made: 8,
            first_failed_at: now - Duration::seconds(241),
            last_attempted_at: now,
            payload_format_version: PAYLOAD_FORMAT_VERSION,
            payload: serde_json::json!({"k": "v"}),
        }
    }

    // ---------- Pure helpers ----------

    #[test]
    fn resolution_string_round_trips() {
        for r in [
            Resolution::RetriedSucceeded,
            Resolution::RetriedFailed,
            Resolution::Acknowledged,
            Resolution::AutoRecovered,
        ] {
            let s = r.as_str();
            let back = Resolution::from_str(s).unwrap();
            assert_eq!(r, back, "round-trip for {s}");
        }
    }

    #[test]
    fn resolution_unknown_string_rejected() {
        let err = Resolution::from_str("retried").unwrap_err();
        assert!(matches!(err, VaultError::Storage(_)));
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        let s: String = "🦀".repeat(2000);
        let t = truncate_utf8(&s, FAILURE_REASON_MAX_BYTES);
        assert!(t.len() <= FAILURE_REASON_MAX_BYTES);
        // Each crab is 4 bytes; 4096/4 = 1024 crabs.
        assert_eq!(t.chars().count(), 1024);
    }

    // ---------- Insert + get ----------

    #[tokio::test]
    async fn insert_creates_pending_row_with_full_metadata() {
        let (_tmp, dl) = make_dead_letter().await;
        let new = sample_new("dimension mismatch 384/256");
        let id = dl.insert(new.clone()).await.unwrap();

        let entry = dl.get(id).await.unwrap().expect("row must exist");
        assert_eq!(entry.id, id);
        assert_eq!(entry.memory_id, new.memory_id);
        assert_eq!(entry.failed_operation, new.failed_operation);
        assert_eq!(entry.failure_reason, new.failure_reason);
        assert_eq!(entry.attempts_made, new.attempts_made);
        assert_eq!(entry.payload, new.payload);
        assert!(entry.resolution.is_none());
        assert!(entry.resolved_at.is_none());
    }

    #[tokio::test]
    async fn insert_truncates_failure_reason_at_4kb() {
        let (_tmp, dl) = make_dead_letter().await;
        let huge = "X".repeat(20_000);
        let id = dl.insert(sample_new(&huge)).await.unwrap();
        let entry = dl.get(id).await.unwrap().unwrap();
        assert!(entry.failure_reason.len() <= FAILURE_REASON_MAX_BYTES);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_tmp, dl) = make_dead_letter().await;
        assert!(dl.get(Uuid::now_v7()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn payload_round_trips_through_blob() {
        let (_tmp, dl) = make_dead_letter().await;
        let mut new = sample_new("x");
        new.payload = serde_json::json!({
            "embedding": [0.1, 0.2, 0.3],
            "boundary": "work",
            "nested": {"k": [1, 2, 3]},
        });
        let id = dl.insert(new.clone()).await.unwrap();
        let entry = dl.get(id).await.unwrap().unwrap();
        assert_eq!(entry.payload, new.payload);
    }

    // ---------- list_unresolved ----------

    #[tokio::test]
    async fn list_unresolved_returns_only_pending_rows() {
        let (_tmp, dl) = make_dead_letter().await;
        let id_pending = dl.insert(sample_new("pending")).await.unwrap();
        let id_resolved = dl.insert(sample_new("resolved")).await.unwrap();

        dl.resolve(id_resolved, Resolution::Acknowledged)
            .await
            .unwrap();

        let pending = dl.list_unresolved(10).await.unwrap();
        let ids: Vec<Uuid> = pending.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![id_pending]);
    }

    #[tokio::test]
    async fn list_unresolved_orders_oldest_first_by_id() {
        // UUID v7 is monotonically time-ordered; insert order = id-ASC order.
        let (_tmp, dl) = make_dead_letter().await;
        let mut ids = Vec::new();
        for i in 0..5 {
            ids.push(dl.insert(sample_new(&format!("err-{i}"))).await.unwrap());
            // Yield so consecutive UUID v7s have distinct timestamps.
            tokio::task::yield_now().await;
        }

        let listed = dl.list_unresolved(100).await.unwrap();
        let listed_ids: Vec<Uuid> = listed.iter().map(|e| e.id).collect();
        assert_eq!(listed_ids, ids);
    }

    #[tokio::test]
    async fn list_unresolved_respects_limit() {
        let (_tmp, dl) = make_dead_letter().await;
        for i in 0..7 {
            dl.insert(sample_new(&format!("err-{i}"))).await.unwrap();
        }
        let limited = dl.list_unresolved(3).await.unwrap();
        assert_eq!(limited.len(), 3);
    }

    // ---------- resolve: terminal transitions ----------

    #[tokio::test]
    async fn resolve_sets_resolution_and_resolved_at() {
        let (_tmp, dl) = make_dead_letter().await;
        let id = dl.insert(sample_new("err")).await.unwrap();

        let before = Utc::now();
        dl.resolve(id, Resolution::RetriedSucceeded).await.unwrap();
        let after = Utc::now();

        let entry = dl.get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::RetriedSucceeded));
        let resolved_at = entry.resolved_at.unwrap();
        // Tolerate ~1s clock slop.
        assert!(
            resolved_at >= before - Duration::seconds(1)
                && resolved_at <= after + Duration::seconds(1),
            "resolved_at {resolved_at:?} outside expected window"
        );
    }

    #[tokio::test]
    async fn resolve_each_resolution_kind_round_trips() {
        let kinds = [
            Resolution::RetriedSucceeded,
            Resolution::RetriedFailed,
            Resolution::Acknowledged,
            Resolution::AutoRecovered,
        ];
        for r in kinds {
            let (_tmp, dl) = make_dead_letter().await;
            let id = dl.insert(sample_new("err")).await.unwrap();
            dl.resolve(id, r).await.unwrap();
            let entry = dl.get(id).await.unwrap().unwrap();
            assert_eq!(entry.resolution, Some(r), "round-trip for {}", r.as_str());
        }
    }

    // ---------- resolve: idempotency + invalid transitions ----------

    #[tokio::test]
    async fn resolve_idempotent_when_resolution_matches() {
        let (_tmp, dl) = make_dead_letter().await;
        let id = dl.insert(sample_new("err")).await.unwrap();

        dl.resolve(id, Resolution::Acknowledged).await.unwrap();
        let first_resolved_at = dl.get(id).await.unwrap().unwrap().resolved_at.unwrap();

        // Sleep a hair so a second update would produce a visibly later timestamp.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        // Same resolution again — must succeed without rewriting resolved_at.
        dl.resolve(id, Resolution::Acknowledged).await.unwrap();
        let second_resolved_at = dl.get(id).await.unwrap().unwrap().resolved_at.unwrap();

        assert_eq!(
            first_resolved_at, second_resolved_at,
            "idempotent resolve must NOT touch resolved_at"
        );
    }

    #[tokio::test]
    async fn resolve_to_different_resolution_after_resolved_rejected() {
        let (_tmp, dl) = make_dead_letter().await;
        let id = dl.insert(sample_new("err")).await.unwrap();
        dl.resolve(id, Resolution::Acknowledged).await.unwrap();

        let err = dl
            .resolve(id, Resolution::RetriedSucceeded)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, VaultError::InvalidInput(s) if s.contains("acknowledged")),
            "expected InvalidInput naming the existing state, got {err:?}"
        );

        // State unchanged.
        let entry = dl.get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::Acknowledged));
    }

    #[tokio::test]
    async fn resolve_missing_id_returns_not_found() {
        let (_tmp, dl) = make_dead_letter().await;
        let phantom = Uuid::now_v7();
        let err = dl
            .resolve(phantom, Resolution::Acknowledged)
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
    }

    // ---------- len ----------

    #[tokio::test]
    async fn len_counts_resolved_and_unresolved() {
        let (_tmp, dl) = make_dead_letter().await;
        for i in 0..4 {
            dl.insert(sample_new(&format!("e-{i}"))).await.unwrap();
        }
        assert_eq!(dl.len().await.unwrap(), 4);

        // Resolving doesn't decrement total.
        let some = dl.list_unresolved(10).await.unwrap()[0].id;
        dl.resolve(some, Resolution::Acknowledged).await.unwrap();
        assert_eq!(dl.len().await.unwrap(), 4);
        assert_eq!(dl.list_unresolved(10).await.unwrap().len(), 3);
    }
}
