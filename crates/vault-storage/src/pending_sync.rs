//! Pending-sync table — cap-overflow catch-up path.
//!
//! Per `T0.1.6_PLAN.md` Q2 ("Cap behaviour at 10k entries") and migration
//! `0002_cascade_infra.sql`. When the retry queue is at its 10,000-entry
//! cap, new cascading writes still succeed against SQLite (audit + memory
//! durably committed), but the LanceDB / DuckDB cascade can't be enqueued
//! immediately. Instead, the orchestrator registers the memory here. The
//! divergence detector (Phase C) sweeps oldest-first from this table and
//! re-enqueues into `retry_queue` once capacity returns.
//!
//! ## Semantics
//!
//! - One pending entry per `memory_id` (PRIMARY KEY). UPSERT replaces the
//!   `operation` and `queued_at` if the row already exists — a later op
//!   supersedes an earlier one because the *latest* state is what the
//!   downstream stores need to converge to. This matches the cascade
//!   ordering invariant in plan Q1: SQLite is the source of truth, the
//!   downstream stores converge to its latest state.
//!
//! - The orchestrator removes a row via [`PendingSync::remove`] right
//!   after re-enqueueing into `retry_queue`. Two-step (upsert ↔ remove)
//!   rather than a transactional move because the two tables are
//!   independently consistent and the cap-overflow path is a soft signal,
//!   not a hard ordering constraint — even if a crash leaves a row in
//!   `pending_sync` after it was already re-enqueued, the next sweep just
//!   re-issues an idempotent UPSERT into `retry_queue` (which the UNIQUE
//!   constraint on `(memory_id, sequence_id)` rejects cleanly).

use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use tracing::{debug, instrument};

use vault_core::{MemoryId, VaultError, VaultResult};

use crate::metadata_store::MetadataStore;
use crate::retry_queue::CascadeOperation;

// ---------------------------------------------------------------------------
// Persistence types
// ---------------------------------------------------------------------------

/// One pending entry as persisted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSyncEntry {
    pub memory_id: MemoryId,
    pub operation: CascadeOperation,
    pub queued_at: DateTime<Utc>,
    /// Audit-chain sequence anchor captured when the cascade overflowed
    /// (migration 0003). The cascade-ordering invariant (plan Q1) reuses it
    /// when the sweep re-enqueues into `retry_queue`. `0` for legacy/payload-
    /// less rows.
    pub sequence_id: i64,
    /// The cascade `payload` bytes (a serialised `CascadePayloadV1` carrying the
    /// embedding and boundary), captured at overflow so the sweep can re-enqueue
    /// a faithful `retry_queue` row (migration 0003). `None` for pre-0003 legacy
    /// rows; the sweep skips those rather than re-enqueueing a broken cascade.
    pub payload: Option<Vec<u8>>,
}

// ---------------------------------------------------------------------------
// PendingSync (data-layer persistence)
// ---------------------------------------------------------------------------

/// Persistent pending-sync table. Cheap to clone.
#[derive(Clone)]
pub struct PendingSync {
    store: MetadataStore,
}

impl PendingSync {
    /// Construct a new handle backed by an open `MetadataStore`.
    pub fn new(store: MetadataStore) -> Self {
        Self { store }
    }

    /// Mark a memory as needing async cascade. UPSERT semantics: if a row
    /// with this `memory_id` already exists, its `operation` and
    /// `queued_at` are overwritten with the new values.
    ///
    /// Idempotent under concurrent calls (the underlying SQLite mutex
    /// serialises writes; ON CONFLICT replaces in-place; final state is
    /// "exactly one row"). See the property test
    /// `concurrent_upserts_on_same_memory_leave_exactly_one_row`.
    #[instrument(skip(self), fields(memory_id = %memory_id, op = operation.as_str()))]
    pub async fn upsert(
        &self,
        memory_id: MemoryId,
        operation: CascadeOperation,
        queued_at: DateTime<Utc>,
    ) -> VaultResult<()> {
        self.store
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "INSERT INTO pending_sync (memory_id, operation, queued_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(memory_id) DO UPDATE SET
                        operation = excluded.operation,
                        queued_at = excluded.queued_at",
                    params![
                        memory_id.0.as_bytes().to_vec(),
                        operation.as_str(),
                        queued_at.to_rfc3339(),
                    ],
                )
                .map_err(|e| VaultError::Storage(format!("upsert pending_sync: {e}")))?;
                Ok(())
            })
            .await?;
        debug!("pending_sync upserted");
        Ok(())
    }

    /// Like [`Self::upsert`] but also persists the cascade `sequence_id` +
    /// `payload` (migration 0003) so the divergence sweep can re-enqueue a
    /// faithful `retry_queue` row. The orchestrator's overflow path uses the
    /// in-transaction equivalent (`tx_upsert_pending_sync`); this async form is
    /// for out-of-transaction callers + tests. ON CONFLICT, the latest state
    /// (operation / queued_at / sequence_id / payload) wins — matching the
    /// "downstream converges to SQLite's latest state" invariant.
    #[instrument(skip(self, payload), fields(memory_id = %memory_id, op = operation.as_str()))]
    pub async fn upsert_with_payload(
        &self,
        memory_id: MemoryId,
        operation: CascadeOperation,
        queued_at: DateTime<Utc>,
        sequence_id: i64,
        payload: Vec<u8>,
    ) -> VaultResult<()> {
        self.store
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "INSERT INTO pending_sync (memory_id, operation, queued_at, sequence_id, payload)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(memory_id) DO UPDATE SET
                        operation = excluded.operation,
                        queued_at = excluded.queued_at,
                        sequence_id = excluded.sequence_id,
                        payload = excluded.payload",
                    params![
                        memory_id.0.as_bytes().to_vec(),
                        operation.as_str(),
                        queued_at.to_rfc3339(),
                        sequence_id,
                        payload,
                    ],
                )
                .map_err(|e| VaultError::Storage(format!("upsert pending_sync (payload): {e}")))?;
                Ok(())
            })
            .await?;
        debug!("pending_sync upserted with payload");
        Ok(())
    }

    /// Drain candidates oldest-first by `queued_at`. The divergence detector
    /// uses this to refill `retry_queue` when capacity returns; entries are
    /// removed by [`Self::remove`] after successful re-enqueue.
    pub async fn oldest_first(&self, limit: usize) -> VaultResult<Vec<PendingSyncEntry>> {
        self.store
            .with_conn_blocking(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT memory_id, operation, queued_at, sequence_id, payload
                           FROM pending_sync
                          ORDER BY queued_at ASC, memory_id ASC
                          LIMIT ?1",
                    )
                    .map_err(|e| {
                        VaultError::Storage(format!("prepare pending_sync oldest_first: {e}"))
                    })?;
                let rows = stmt
                    .query_map(params![limit as i64], row_to_pending_sync)
                    .map_err(|e| {
                        VaultError::Storage(format!("query pending_sync oldest_first: {e}"))
                    })?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(
                        r.map_err(|e| VaultError::Storage(format!("read pending_sync row: {e}")))?,
                    );
                }
                Ok(out)
            })
            .await
    }

    /// Remove a pending entry by `memory_id`. Returns `true` if a row was
    /// deleted, `false` if no such row existed (idempotent).
    pub async fn remove(&self, memory_id: MemoryId) -> VaultResult<bool> {
        self.store
            .with_conn_blocking(move |conn| {
                let rows = conn
                    .execute(
                        "DELETE FROM pending_sync WHERE memory_id = ?1",
                        params![memory_id.0.as_bytes().to_vec()],
                    )
                    .map_err(|e| VaultError::Storage(format!("delete pending_sync: {e}")))?;
                Ok(rows > 0)
            })
            .await
    }

    /// Look up a single entry. Used by tests + orchestrator inspection.
    pub async fn get(&self, memory_id: MemoryId) -> VaultResult<Option<PendingSyncEntry>> {
        self.store
            .with_conn_blocking(move |conn| {
                conn.query_row(
                    "SELECT memory_id, operation, queued_at, sequence_id, payload
                       FROM pending_sync WHERE memory_id = ?1",
                    params![memory_id.0.as_bytes().to_vec()],
                    row_to_pending_sync,
                )
                .optional()
                .map_err(|e| VaultError::Storage(format!("get pending_sync: {e}")))
            })
            .await
    }

    /// Total pending entries.
    pub async fn len(&self) -> VaultResult<usize> {
        self.store
            .with_conn_blocking(|conn| {
                let n: i64 = conn
                    .query_row("SELECT COUNT(*) FROM pending_sync", [], |row| row.get(0))
                    .map_err(|e| VaultError::Storage(format!("count pending_sync: {e}")))?;
                Ok(n as usize)
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Row decoder (private)
// ---------------------------------------------------------------------------

fn row_to_pending_sync(row: &rusqlite::Row<'_>) -> rusqlite::Result<PendingSyncEntry> {
    let mem_bytes: Vec<u8> = row.get(0)?;
    let memory_id = MemoryId(decode_uuid(&mem_bytes, 0, "pending_sync.memory_id")?);

    let op_s: String = row.get(1)?;
    let operation = CascadeOperation::from_str(&op_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("pending_sync.operation: {e}"),
            )),
        )
    })?;

    let queued_at: DateTime<Utc> = row.get(2)?;
    let sequence_id: i64 = row.get(3)?;
    let payload: Option<Vec<u8>> = row.get(4)?;

    Ok(PendingSyncEntry {
        memory_id,
        operation,
        queued_at,
        sequence_id,
        payload,
    })
}

fn decode_uuid(bytes: &[u8], col: usize, label: &'static str) -> rusqlite::Result<uuid::Uuid> {
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
    Ok(uuid::Uuid::from_bytes(arr))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    use crate::key::SqlCipherKey;
    use chrono::Duration;
    use std::collections::HashSet;
    use tempfile::TempDir;

    async fn make_pending() -> (TempDir, PendingSync) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let key = SqlCipherKey::new("pending-sync-test-key");
        let store = MetadataStore::open(&path, key).await.unwrap();
        (tmp, PendingSync::new(store))
    }

    // ---------- upsert + get ----------

    #[tokio::test]
    async fn upsert_creates_row() {
        let (_tmp, p) = make_pending().await;
        let mem = MemoryId::new();
        let now = Utc::now();
        p.upsert(mem, CascadeOperation::Write, now).await.unwrap();

        let entry = p.get(mem).await.unwrap().unwrap();
        assert_eq!(entry.memory_id, mem);
        assert_eq!(entry.operation, CascadeOperation::Write);
        // RFC3339 round-trip is microsecond-accurate; ensure the timestamp
        // matches at second granularity.
        assert_eq!(entry.queued_at.timestamp(), now.timestamp());
    }

    #[tokio::test]
    async fn upsert_replaces_on_same_memory_id() {
        // Watchpoint #3: latest operation supersedes.
        let (_tmp, p) = make_pending().await;
        let mem = MemoryId::new();
        let earlier = Utc::now();
        p.upsert(mem, CascadeOperation::Write, earlier)
            .await
            .unwrap();

        let later = earlier + Duration::seconds(10);
        p.upsert(mem, CascadeOperation::Delete, later)
            .await
            .unwrap();

        let entry = p.get(mem).await.unwrap().unwrap();
        assert_eq!(
            entry.operation,
            CascadeOperation::Delete,
            "second UPSERT should replace operation"
        );
        assert_eq!(
            entry.queued_at.timestamp(),
            later.timestamp(),
            "second UPSERT should replace queued_at"
        );

        assert_eq!(p.len().await.unwrap(), 1, "still exactly one row");
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_tmp, p) = make_pending().await;
        assert!(p.get(MemoryId::new()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_with_payload_round_trips_sequence_id_and_payload() {
        // Migration 0003: the sweep needs sequence_id + payload to reconstruct
        // a faithful retry_queue row. Verify they persist + read back.
        let (_tmp, p) = make_pending().await;
        let mem = MemoryId::new();
        let payload = vec![9u8, 8, 7, 6];
        p.upsert_with_payload(
            mem,
            CascadeOperation::Write,
            Utc::now(),
            42,
            payload.clone(),
        )
        .await
        .unwrap();

        let entry = p.get(mem).await.unwrap().unwrap();
        assert_eq!(entry.sequence_id, 42);
        assert_eq!(entry.payload.as_deref(), Some(payload.as_slice()));
        assert_eq!(entry.operation, CascadeOperation::Write);
    }

    #[tokio::test]
    async fn plain_upsert_leaves_payload_null() {
        // A plain upsert (no payload) — the pre-0003 shape — must read back
        // with sequence_id 0 (column default) and a NULL payload, which the
        // sweep treats as "skip, can't reconstruct."
        let (_tmp, p) = make_pending().await;
        let mem = MemoryId::new();
        p.upsert(mem, CascadeOperation::Write, Utc::now())
            .await
            .unwrap();

        let entry = p.get(mem).await.unwrap().unwrap();
        assert_eq!(entry.sequence_id, 0);
        assert!(entry.payload.is_none());
    }

    // ---------- oldest_first ordering ----------

    #[tokio::test]
    async fn oldest_first_orders_by_queued_at_asc() {
        let (_tmp, p) = make_pending().await;
        let now = Utc::now();
        let mems: Vec<MemoryId> = (0..5).map(|_| MemoryId::new()).collect();

        // Insert in reverse-queued-at order to make sure ordering is by
        // column value, not insert order.
        for (i, m) in mems.iter().enumerate() {
            let qt = now + Duration::seconds(10 - i as i64);
            p.upsert(*m, CascadeOperation::Write, qt).await.unwrap();
        }

        let listed = p.oldest_first(100).await.unwrap();
        assert_eq!(listed.len(), 5);

        // Listed in ascending queued_at order — index 0 was last inserted
        // (qt = now+10s), index 4 was first inserted (qt = now+6s? wait no:
        // for i=0 qt = now+10, i=1 qt = now+9, ...). So earliest queued_at
        // is i=4 → qt = now+6, latest is i=0 → qt = now+10.
        // Listed ASC: i=4, 3, 2, 1, 0 → mems[4], [3], [2], [1], [0].
        let listed_mems: Vec<MemoryId> = listed.iter().map(|e| e.memory_id).collect();
        let expected: Vec<MemoryId> = mems.iter().rev().copied().collect();
        assert_eq!(listed_mems, expected);
    }

    #[tokio::test]
    async fn oldest_first_respects_limit() {
        let (_tmp, p) = make_pending().await;
        for i in 0..7 {
            let m = MemoryId::new();
            let qt = Utc::now() + Duration::seconds(i);
            p.upsert(m, CascadeOperation::Write, qt).await.unwrap();
        }
        let limited = p.oldest_first(3).await.unwrap();
        assert_eq!(limited.len(), 3);
    }

    // ---------- remove ----------

    #[tokio::test]
    async fn remove_returns_true_for_existing_row() {
        let (_tmp, p) = make_pending().await;
        let mem = MemoryId::new();
        p.upsert(mem, CascadeOperation::Write, Utc::now())
            .await
            .unwrap();

        assert!(p.remove(mem).await.unwrap());
        assert!(p.get(mem).await.unwrap().is_none());
        // Second remove is a no-op false.
        assert!(!p.remove(mem).await.unwrap());
    }

    #[tokio::test]
    async fn remove_returns_false_for_missing_row() {
        let (_tmp, p) = make_pending().await;
        assert!(!p.remove(MemoryId::new()).await.unwrap());
    }

    // ---------- len ----------

    #[tokio::test]
    async fn len_counts_total_rows() {
        let (_tmp, p) = make_pending().await;
        assert_eq!(p.len().await.unwrap(), 0);
        for i in 0..3 {
            let m = MemoryId::new();
            let qt = Utc::now() + Duration::seconds(i);
            p.upsert(m, CascadeOperation::Write, qt).await.unwrap();
        }
        assert_eq!(p.len().await.unwrap(), 3);
    }

    // ---------- Watchpoint #3: concurrent UPSERTs leave exactly one row ----------

    #[tokio::test]
    async fn concurrent_upserts_on_same_memory_leave_exactly_one_row() {
        // Per Watchpoint #3 in the Phase B greenlight. 32 tasks UPSERT
        // the same memory_id concurrently with different operations and
        // timestamps; final state must be exactly one row, and total rows
        // unchanged at 1. Mutex<Connection> serialises writes; ON CONFLICT
        // replaces in place; the property is "no duplicate row, ever."
        let (_tmp, p) = make_pending().await;
        let mem = MemoryId::new();
        let base = Utc::now();

        let ops = [
            CascadeOperation::Write,
            CascadeOperation::Update,
            CascadeOperation::Delete,
        ];

        let mut handles = Vec::new();
        for i in 0..32u32 {
            let p = p.clone();
            let op = ops[(i as usize) % ops.len()];
            let qt = base + Duration::milliseconds(i as i64);
            handles.push(tokio::spawn(async move { p.upsert(mem, op, qt).await }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }

        assert_eq!(
            p.len().await.unwrap(),
            1,
            "32 concurrent UPSERTs on same memory_id must leave exactly one row"
        );
        // The surviving entry's fields are *some* valid (op, qt) pair from
        // one of the 32 calls — we don't assert which one because the
        // serialisation order is non-deterministic. We just verify the
        // entry exists and is internally coherent.
        let entry = p.get(mem).await.unwrap().unwrap();
        assert_eq!(entry.memory_id, mem);
        assert!(ops.contains(&entry.operation));
    }

    #[tokio::test]
    async fn concurrent_upserts_across_distinct_memories_yield_distinct_rows() {
        // Inverse: 20 distinct memories upserted concurrently → 20 rows.
        let (_tmp, p) = make_pending().await;
        let mems: Vec<MemoryId> = (0..20).map(|_| MemoryId::new()).collect();
        let mut handles = Vec::new();
        for (i, m) in mems.iter().enumerate() {
            let p = p.clone();
            let m = *m;
            let qt = Utc::now() + Duration::milliseconds(i as i64);
            handles.push(tokio::spawn(async move {
                p.upsert(m, CascadeOperation::Write, qt).await
            }));
        }
        for h in handles {
            h.await.unwrap().unwrap();
        }

        assert_eq!(p.len().await.unwrap(), 20);
        // Sanity: every distinct memory_id has its own row.
        let listed = p.oldest_first(100).await.unwrap();
        let listed_mems: HashSet<MemoryId> = listed.iter().map(|e| e.memory_id).collect();
        let expected: HashSet<MemoryId> = mems.into_iter().collect();
        assert_eq!(listed_mems, expected);
    }

    // ---------- All operation kinds round-trip ----------

    #[tokio::test]
    async fn all_cascade_operations_round_trip() {
        let (_tmp, p) = make_pending().await;
        let kinds = [
            CascadeOperation::Write,
            CascadeOperation::Update,
            CascadeOperation::Delete,
        ];
        for op in kinds {
            let m = MemoryId::new();
            p.upsert(m, op, Utc::now()).await.unwrap();
            let e = p.get(m).await.unwrap().unwrap();
            assert_eq!(e.operation, op);
        }
    }
}
