//! Persistent retry queue for cascading-write partial failures.
//!
//! Per ADR-009 (amended in T0.1.6) and `T0.1.6_PLAN.md` Q2:
//! - 8 attempts max with exponential backoff (1, 2, 4, 8, 16, 30, 60, 120s)
//!   plus ±25% jitter on each schedule.
//! - Permanent-failure classifier ([`is_permanent`]) — `DimensionMismatch`,
//!   `AccessDenied`, schema-mismatch — dead-letters on the very first
//!   failure rather than cycling backoff.
//! - Strict FIFO per `memory_id` by `sequence_id` ASC. The cascade-ordering
//!   invariant locked in plan Q1 reads: concurrent writes to the same
//!   `memory_id` are processed in SQLite-commit order, anchored to the
//!   audit chain's monotonic `seq`.
//!
//! The orchestrator (Phase C) drives the worker loop and decides between
//! reschedule and dead-letter; this module is the data layer plus the pure
//! schedule / classifier helpers.
//!
//! ## Jitter
//!
//! Jitter is sourced via the [`JitterSource`] trait, injected by the
//! caller (BRD §2.3 — no global state). [`SeededJitter`] is the production
//! source (xorshift64 seeded from system time, no external dependency).
//! [`FixedJitter`] is the test fixture.
//!
//! ## Cap behaviour
//!
//! [`RetryQueue::len`] lets the orchestrator enforce the 10k cap before
//! enqueue; over-cap memories register in `pending_sync` instead of this
//! queue. The queue itself does not enforce the cap — separation of
//! concerns: queue persists, orchestrator decides admission.

use std::str::FromStr;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, OptionalExtension};
use tracing::{debug, instrument};
use uuid::Uuid;

use vault_core::{MemoryId, VaultError, VaultResult};

use crate::metadata_store::MetadataStore;

/// Maximum number of attempts before an entry must dead-letter.
/// Per ADR-009 amendment in T0.1.6 (was 5; raised to 8).
pub const MAX_ATTEMPTS: u32 = 8;

/// Schema version for the JSON `payload` BLOB. Bump when the on-disk shape
/// changes; readers dispatch on this number.
pub const PAYLOAD_FORMAT_VERSION: i64 = 1;

/// Truncation threshold for `last_error`. Keeps the column from blowing up
/// if a downstream library returns a multi-MB error. Mirrors the 4 KB limit
/// on `dead_letter.failure_reason` from migration 0002 (Phase A).
pub const LAST_ERROR_MAX_BYTES: usize = 4096;

/// Cascading-write operation kinds. **Per-cascade**, not per-store: a
/// single retry-queue entry covers BOTH the LanceDB and DuckDB sub-ops
/// for the corresponding write. The orchestrator's worker runs both sub-ops
/// idempotently per attempt; either failure → whole entry reschedules.
///
/// Per ADR-016 / ADR-017 (T0.1.6 Phase C), reasoning in
/// `T0.1.6_PLAN_PHASE_C.md` "One-row-per-write retry model": V0.1
/// retrieval is LanceDB-only, so per-store dead-lettering granularity
/// added complexity without a corresponding user benefit. Lockstep
/// success/failure matches the user mental model of "memory write fully
/// cascaded."
///
/// Persisted strings: `"write" / "update" / "delete"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CascadeOperation {
    Write,
    Update,
    Delete,
}

impl CascadeOperation {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

impl FromStr for CascadeOperation {
    type Err = VaultError;

    fn from_str(s: &str) -> VaultResult<Self> {
        match s {
            "write" => Ok(Self::Write),
            "update" => Ok(Self::Update),
            "delete" => Ok(Self::Delete),
            other => Err(VaultError::Storage(format!(
                "unknown cascade operation: {other}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Jitter
// ---------------------------------------------------------------------------

/// Source of jitter factors in the range `[-1.0, 1.0]`. Effective jitter
/// applied by [`compute_next_attempt`] is ±25% (the factor is scaled
/// internally).
///
/// Inject per BRD §2.3 — never module-private global state. Production
/// code passes a [`SeededJitter`]; tests pass [`FixedJitter`] for
/// deterministic schedule arithmetic, or a `SeededJitter::from_seed(N)`
/// for reproducible randomised behaviour.
///
/// `Send` is required so callers can hold an `&mut dyn JitterSource`
/// across `tokio::spawn` boundaries (the orchestrator's worker loop in
/// Phase C does this).
pub trait JitterSource: Send {
    /// Return the next factor. Out-of-range values are tolerated and
    /// clamped at use-site, but well-behaved sources should respect the
    /// `[-1.0, 1.0]` contract.
    fn next_factor(&mut self) -> f64;
}

/// xorshift64-based jitter. ~5 lines of PRNG, no `rand` dep added to the
/// workspace just for this module.
///
/// xorshift64 is not cryptographically strong — that's intentional. We
/// only need uncoordinated spread across retry waves; an attacker who
/// can predict our jitter does not gain anything they couldn't already
/// gain by reading the schedule.
#[derive(Clone, Debug)]
pub struct SeededJitter {
    state: u64,
}

impl SeededJitter {
    /// Construct a deterministic jitter source from a fixed seed. `0` is
    /// a degenerate state for xorshift64 — coerced to a non-zero sentinel
    /// rather than rejected, since callers may pass system-derived seeds
    /// that legitimately come out zero.
    pub fn from_seed(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0xdead_beef_dead_beef
            } else {
                seed
            },
        }
    }

    /// Construct from current system time (nanoseconds since epoch). For
    /// production use; tests should prefer `from_seed` for reproducibility.
    pub fn from_system_time() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0xdead_beef_dead_beef);
        // The `| 1` makes sure the seed is never even-zero in practice.
        Self::from_seed(nanos | 1)
    }
}

impl JitterSource for SeededJitter {
    fn next_factor(&mut self) -> f64 {
        // xorshift64 step.
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        // Map u64 → f64 in [0.0, 1.0) using the top 53 bits (matches f64
        // mantissa width, avoids precision loss), then shift to [-1.0, 1.0).
        let unit = ((x >> 11) as f64) / ((1_u64 << 53) as f64);
        unit * 2.0 - 1.0
    }
}

/// Test fixture: every call returns the same factor. Public so tests in
/// other modules / Phase C orchestrator's tests can reuse it.
#[derive(Clone, Copy, Debug)]
pub struct FixedJitter(pub f64);

impl JitterSource for FixedJitter {
    fn next_factor(&mut self) -> f64 {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Schedule + classifier (pure, no I/O)
// ---------------------------------------------------------------------------

/// Base wait (seconds) before the next attempt, given prior `attempts_made`.
/// Returns `None` when no further attempt is allowed (attempts exhausted).
///
/// Schedule (per ADR-009 amendment): `1, 2, 4, 8, 16, 30, 60, 120` seconds.
pub const fn base_backoff_secs(attempts_made: u32) -> Option<u32> {
    match attempts_made {
        0 => Some(1),
        1 => Some(2),
        2 => Some(4),
        3 => Some(8),
        4 => Some(16),
        5 => Some(30),
        6 => Some(60),
        7 => Some(120),
        _ => None,
    }
}

/// Compute the absolute time of the next attempt.
///
/// - `attempts_made` — how many attempts have already been made (0 at enqueue).
/// - `now` — reference time (caller's `Utc::now()` typically).
/// - `jitter_factor` — value in `[-1.0, 1.0]`; effective jitter is ±25%.
///
/// Returns `None` when `attempts_made >= MAX_ATTEMPTS` (entry must dead-letter).
pub fn compute_next_attempt(
    attempts_made: u32,
    now: DateTime<Utc>,
    jitter_factor: f64,
) -> Option<DateTime<Utc>> {
    let base_secs = base_backoff_secs(attempts_made)? as f64;
    let factor = jitter_factor.clamp(-1.0, 1.0);
    let scaled = base_secs * (1.0 + 0.25 * factor);
    let delay_ms = (scaled * 1000.0).max(0.0) as i64;
    Some(now + Duration::milliseconds(delay_ms))
}

/// Permanent-failure classifier. Per ADR-009 amendment.
///
/// Returns `true` for errors that retrying cannot fix:
/// - [`VaultError::DimensionMismatch`] — vector dimension contract is
///   broken; subsequent retries with the same payload will fail forever.
/// - [`VaultError::AccessDenied`] — boundary / authorisation failure; not
///   a transient I/O issue.
/// - [`VaultError::Storage`] containing the literal substring `"schema"`
///   — schema-shape mismatch (e.g., a future migration not yet applied
///   locally). String-matching is a temporary workaround; the future ADR
///   that splits `Storage(String)` into structured variants will turn
///   this into an exhaustive `match`. Logged in HANDOFF tech debt.
///
/// Conservative default for any other error: `false` (retry). False
/// negatives are recoverable via the backoff schedule; false positives
/// land in a queryable dead-letter row for the founder.
pub fn is_permanent(err: &VaultError) -> bool {
    match err {
        VaultError::DimensionMismatch { .. } => true,
        VaultError::AccessDenied(_) => true,
        VaultError::Storage(msg) if msg.contains("schema") => true,
        _ => false,
    }
}

/// Truncate a UTF-8 error message at a code-point boundary, never inside
/// a multi-byte sequence. Used by [`RetryQueue::record_failure`] before
/// persisting.
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
// Persistence types
// ---------------------------------------------------------------------------

/// New retry-queue insertion. The queue assigns `id`, `created_at`, and the
/// initial `next_attempt_at` (computed from `attempts_made = 0` plus jitter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewRetry {
    pub memory_id: MemoryId,
    pub operation: CascadeOperation,
    /// Audit-chain-derived ordering anchor. Cascade-ordering invariant
    /// (plan Q1) requires concurrent writes to the same `memory_id` to be
    /// processed in SQLite-commit order; the audit chain's monotonic `seq`
    /// already provides that ordering.
    pub sequence_id: i64,
    /// Operation-specific JSON payload (the orchestrator decides shape).
    pub payload: serde_json::Value,
}

/// A retry-queue entry as persisted.
#[derive(Clone, Debug, PartialEq)]
pub struct RetryEntry {
    pub id: Uuid,
    pub memory_id: MemoryId,
    pub operation: CascadeOperation,
    pub payload_format_version: i64,
    pub payload: serde_json::Value,
    pub sequence_id: i64,
    pub attempts_made: u32,
    pub next_attempt_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

/// Outcome of [`RetryQueue::record_failure`].
#[derive(Clone, Debug, PartialEq)]
pub enum FailureOutcome {
    /// Entry rescheduled with a bumped attempt counter.
    Rescheduled {
        attempts_made: u32,
        next_attempt_at: DateTime<Utc>,
    },
    /// Entry removed from the queue. Orchestrator MUST insert a corresponding
    /// `dead_letter` row using the returned entry's metadata.
    DeadLetter {
        /// The entry as it was when the failure was recorded — caller copies
        /// `id` / `memory_id` / `operation` / `payload` / `created_at` into
        /// `dead_letter`.
        entry: RetryEntry,
        /// The terminal error message (truncated to `LAST_ERROR_MAX_BYTES`).
        last_error: String,
        /// Reason for dead-letter — distinguishes attempts-exhausted from
        /// permanent-classification for audit logging.
        reason: DeadLetterReason,
    },
}

/// Why an entry dead-lettered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeadLetterReason {
    /// All [`MAX_ATTEMPTS`] backoff attempts have been used.
    AttemptsExhausted,
    /// [`is_permanent`] returned true on the failure — no retries useful.
    Permanent,
}

// ---------------------------------------------------------------------------
// RetryQueue (data-layer persistence)
// ---------------------------------------------------------------------------

/// Persistent retry queue. Cheap to clone (holds a [`MetadataStore`] which
/// is itself an `Arc` internally).
#[derive(Clone)]
pub struct RetryQueue {
    store: MetadataStore,
}

impl RetryQueue {
    /// Construct a new queue handle backed by an open `MetadataStore`.
    pub fn new(store: MetadataStore) -> Self {
        Self { store }
    }

    /// Insert a new retry entry. Returns the assigned entry `id`.
    ///
    /// `created_at` is set to `Utc::now()`; `next_attempt_at` is computed
    /// via [`compute_next_attempt`] from `attempts_made = 0` and the
    /// supplied jitter factor.
    #[instrument(skip(self, new, jitter), fields(memory_id = %new.memory_id, sequence_id = new.sequence_id))]
    pub async fn enqueue(&self, new: NewRetry, jitter: &mut dyn JitterSource) -> VaultResult<Uuid> {
        let entry_id = Uuid::now_v7();
        let now = Utc::now();
        // safe: base_backoff_secs(0) = Some(1)
        let next_at = compute_next_attempt(0, now, jitter.next_factor())
            .expect("base_backoff_secs(0) must yield Some");
        let payload_bytes = serde_json::to_vec(&new.payload)?;

        self.store
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "INSERT INTO retry_queue (
                        id, memory_id, operation, payload_format_version,
                        payload, sequence_id, attempts_made,
                        next_attempt_at, created_at, last_error
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, NULL)",
                    params![
                        entry_id.as_bytes().to_vec(),
                        new.memory_id.0.as_bytes().to_vec(),
                        new.operation.as_str(),
                        PAYLOAD_FORMAT_VERSION,
                        payload_bytes,
                        new.sequence_id,
                        next_at.to_rfc3339(),
                        now.to_rfc3339(),
                    ],
                )
                .map_err(|e| VaultError::Storage(format!("enqueue retry: {e}")))?;
                Ok(())
            })
            .await?;

        debug!(%entry_id, "retry enqueued");
        Ok(entry_id)
    }

    /// Mark a successful attempt — entry removed from the queue.
    /// Returns `true` if a row was deleted, `false` if the id was already
    /// gone (idempotent).
    #[instrument(skip(self), fields(%id))]
    pub async fn record_success(&self, id: Uuid) -> VaultResult<bool> {
        self.store
            .with_conn_blocking(move |conn| {
                let rows = conn
                    .execute(
                        "DELETE FROM retry_queue WHERE id = ?1",
                        params![id.as_bytes().to_vec()],
                    )
                    .map_err(|e| VaultError::Storage(format!("delete retry on success: {e}")))?;
                Ok(rows > 0)
            })
            .await
    }

    /// Mark a failed attempt. The orchestrator pre-classifies via
    /// [`is_permanent`] and passes the result as `permanent`.
    ///
    /// Outcome:
    /// - `permanent == true` → DeadLetter (regardless of attempt count).
    /// - `attempts_made + 1 >= MAX_ATTEMPTS` → DeadLetter (exhausted).
    /// - else → Rescheduled with bumped attempt counter.
    ///
    /// Returns `Err(VaultError::NotFound)` if the id is no longer in the queue.
    #[instrument(skip(self, error, jitter), fields(%id, permanent))]
    pub async fn record_failure(
        &self,
        id: Uuid,
        error: &str,
        permanent: bool,
        jitter: &mut dyn JitterSource,
    ) -> VaultResult<FailureOutcome> {
        let truncated = truncate_utf8(error, LAST_ERROR_MAX_BYTES);
        // Pre-compute the jitter factor on the async caller's thread so the
        // closure passed to spawn_blocking doesn't need the JitterSource.
        let factor = jitter.next_factor();

        self.store
            .with_conn_blocking(move |conn| {
                let tx = conn
                    .transaction()
                    .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;

                // Read the current row (we need its full state to either
                // reschedule or dead-letter).
                let entry: RetryEntry = match select_entry_by_id(&tx, id)? {
                    Some(e) => e,
                    None => {
                        return Err(VaultError::NotFound(format!(
                            "retry_queue entry {id} not found"
                        )))
                    }
                };

                let new_attempts = entry.attempts_made.saturating_add(1);
                let now = Utc::now();

                // Decide outcome.
                let outcome = if permanent || new_attempts >= MAX_ATTEMPTS {
                    let reason = if permanent {
                        DeadLetterReason::Permanent
                    } else {
                        DeadLetterReason::AttemptsExhausted
                    };
                    // Attach the latest attempt to the returned entry so the
                    // caller can persist coherent dead-letter metadata.
                    let mut entry_for_caller = entry.clone();
                    entry_for_caller.attempts_made = new_attempts;
                    entry_for_caller.last_error = Some(truncated.clone());
                    tx.execute(
                        "DELETE FROM retry_queue WHERE id = ?1",
                        params![id.as_bytes().to_vec()],
                    )
                    .map_err(|e| VaultError::Storage(format!("delete on dead-letter: {e}")))?;
                    FailureOutcome::DeadLetter {
                        entry: entry_for_caller,
                        last_error: truncated,
                        reason,
                    }
                } else {
                    // Reschedule. compute_next_attempt MUST return Some here
                    // because new_attempts < MAX_ATTEMPTS.
                    let next_at = compute_next_attempt(new_attempts, now, factor).expect(
                        "compute_next_attempt for new_attempts < MAX_ATTEMPTS must return Some",
                    );
                    tx.execute(
                        "UPDATE retry_queue
                            SET attempts_made = ?2,
                                next_attempt_at = ?3,
                                last_error = ?4
                            WHERE id = ?1",
                        params![
                            id.as_bytes().to_vec(),
                            new_attempts,
                            next_at.to_rfc3339(),
                            truncated,
                        ],
                    )
                    .map_err(|e| VaultError::Storage(format!("reschedule retry: {e}")))?;
                    FailureOutcome::Rescheduled {
                        attempts_made: new_attempts,
                        next_attempt_at: next_at,
                    }
                };

                tx.commit()
                    .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
                Ok(outcome)
            })
            .await
    }

    /// Return entries with `next_attempt_at <= now`, ordered by `sequence_id`
    /// ASC then `next_attempt_at` ASC. The strict-FIFO-per-`memory_id`
    /// invariant (plan Q1) is satisfied because `(memory_id, sequence_id)`
    /// is UNIQUE — for any single memory the worker sees its retries in
    /// `sequence_id` order.
    #[instrument(skip(self), fields(now = %now, limit))]
    pub async fn poll_due(&self, now: DateTime<Utc>, limit: usize) -> VaultResult<Vec<RetryEntry>> {
        self.store
            .with_conn_blocking(move |conn| {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, memory_id, operation, payload_format_version,
                                payload, sequence_id, attempts_made,
                                next_attempt_at, created_at, last_error
                           FROM retry_queue
                          WHERE next_attempt_at <= ?1
                          ORDER BY sequence_id ASC, next_attempt_at ASC
                          LIMIT ?2",
                    )
                    .map_err(|e| VaultError::Storage(format!("prepare poll_due: {e}")))?;
                let rows = stmt
                    .query_map(params![now.to_rfc3339(), limit as i64], row_to_retry_entry)
                    .map_err(|e| VaultError::Storage(format!("query poll_due: {e}")))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(
                        r.map_err(|e| VaultError::Storage(format!("read retry_queue row: {e}")))?,
                    );
                }
                Ok(out)
            })
            .await
    }

    /// Total pending entries. Used by the orchestrator to enforce the 10k
    /// cap before calling [`Self::enqueue`].
    pub async fn len(&self) -> VaultResult<usize> {
        self.store
            .with_conn_blocking(|conn| {
                let n: i64 = conn
                    .query_row("SELECT COUNT(*) FROM retry_queue", [], |row| row.get(0))
                    .map_err(|e| VaultError::Storage(format!("count retry_queue: {e}")))?;
                Ok(n as usize)
            })
            .await
    }

    /// Look up an entry by id. Used by tests and by orchestrator inspection
    /// paths.
    pub async fn get(&self, id: Uuid) -> VaultResult<Option<RetryEntry>> {
        self.store
            .with_conn_blocking(move |conn| {
                conn.query_row(
                    "SELECT id, memory_id, operation, payload_format_version,
                            payload, sequence_id, attempts_made,
                            next_attempt_at, created_at, last_error
                       FROM retry_queue WHERE id = ?1",
                    params![id.as_bytes().to_vec()],
                    row_to_retry_entry,
                )
                .optional()
                .map_err(|e| VaultError::Storage(format!("get retry: {e}")))
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Row decoders (private)
// ---------------------------------------------------------------------------

fn select_entry_by_id(tx: &rusqlite::Transaction<'_>, id: Uuid) -> VaultResult<Option<RetryEntry>> {
    tx.query_row(
        "SELECT id, memory_id, operation, payload_format_version,
                payload, sequence_id, attempts_made,
                next_attempt_at, created_at, last_error
           FROM retry_queue WHERE id = ?1",
        params![id.as_bytes().to_vec()],
        row_to_retry_entry,
    )
    .optional()
    .map_err(|e| VaultError::Storage(format!("select retry by id: {e}")))
}

fn row_to_retry_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<RetryEntry> {
    let id_bytes: Vec<u8> = row.get(0)?;
    let id = decode_uuid(&id_bytes, 0, "retry_queue.id")?;

    let mem_bytes: Vec<u8> = row.get(1)?;
    let memory_id = MemoryId(decode_uuid(&mem_bytes, 1, "retry_queue.memory_id")?);

    let op_s: String = row.get(2)?;
    let operation = CascadeOperation::from_str(&op_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("retry_queue.operation: {e}"),
            )),
        )
    })?;

    let payload_format_version: i64 = row.get(3)?;
    let payload_bytes: Vec<u8> = row.get(4)?;
    let payload: serde_json::Value = serde_json::from_slice(&payload_bytes).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Blob, Box::new(e))
    })?;

    let sequence_id: i64 = row.get(5)?;
    let attempts_made_i: i64 = row.get(6)?;
    let attempts_made: u32 = u32::try_from(attempts_made_i).map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Integer,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("retry_queue.attempts_made out of u32 range: {attempts_made_i}"),
            )),
        )
    })?;

    let next_attempt_at: DateTime<Utc> = row.get(7)?;
    let created_at: DateTime<Utc> = row.get(8)?;
    let last_error: Option<String> = row.get(9)?;

    Ok(RetryEntry {
        id,
        memory_id,
        operation,
        payload_format_version,
        payload,
        sequence_id,
        attempts_made,
        next_attempt_at,
        created_at,
        last_error,
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
    use crate::metadata_store::MetadataStore;
    use proptest::prelude::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    async fn make_queue() -> (TempDir, RetryQueue) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let key = SqlCipherKey::new("retry-queue-test-key");
        let store = MetadataStore::open(&path, key).await.unwrap();
        (tmp, RetryQueue::new(store))
    }

    fn sample_new_retry(seq: i64) -> NewRetry {
        NewRetry {
            memory_id: MemoryId::new(),
            operation: CascadeOperation::Write,
            sequence_id: seq,
            payload: serde_json::json!({"embedding": [0.1, 0.2, 0.3]}),
        }
    }

    // ---------- Pure helpers (no I/O) ----------

    #[test]
    fn cascade_operation_string_round_trips() {
        for op in [
            CascadeOperation::Write,
            CascadeOperation::Update,
            CascadeOperation::Delete,
        ] {
            let s = op.as_str();
            let back = CascadeOperation::from_str(s).unwrap();
            assert_eq!(op, back, "round-trip for {s}");
        }
    }

    #[test]
    fn cascade_operation_unknown_string_rejected() {
        let err = CascadeOperation::from_str("redis_write").unwrap_err();
        assert!(matches!(err, VaultError::Storage(_)));
    }

    #[test]
    fn base_backoff_schedule_matches_plan() {
        // Per ADR-009 amendment / T0.1.6_PLAN Q2.
        assert_eq!(base_backoff_secs(0), Some(1));
        assert_eq!(base_backoff_secs(1), Some(2));
        assert_eq!(base_backoff_secs(2), Some(4));
        assert_eq!(base_backoff_secs(3), Some(8));
        assert_eq!(base_backoff_secs(4), Some(16));
        assert_eq!(base_backoff_secs(5), Some(30));
        assert_eq!(base_backoff_secs(6), Some(60));
        assert_eq!(base_backoff_secs(7), Some(120));
        assert_eq!(base_backoff_secs(8), None);
        assert_eq!(base_backoff_secs(99), None);
        // Total span = 4 min worst case.
        let total: u32 = (0..MAX_ATTEMPTS)
            .map(|n| base_backoff_secs(n).unwrap())
            .sum();
        assert_eq!(total, 241);
    }

    #[test]
    fn compute_next_attempt_zero_jitter_matches_base() {
        let now = Utc::now();
        for attempts in 0..MAX_ATTEMPTS {
            let base = base_backoff_secs(attempts).unwrap();
            let next = compute_next_attempt(attempts, now, 0.0).unwrap();
            let delta = (next - now).num_milliseconds();
            assert_eq!(
                delta,
                (base as i64) * 1000,
                "attempts_made={attempts} expected exact base wait with zero jitter"
            );
        }
    }

    #[test]
    fn compute_next_attempt_max_positive_jitter_is_125pct_base() {
        let now = Utc::now();
        let next = compute_next_attempt(0, now, 1.0).unwrap();
        let delta_ms = (next - now).num_milliseconds();
        assert_eq!(delta_ms, 1250, "1s base * (1 + 0.25) = 1.25s = 1250ms");

        let next = compute_next_attempt(7, now, 1.0).unwrap();
        let delta_ms = (next - now).num_milliseconds();
        assert_eq!(
            delta_ms, 150_000,
            "120s base * 1.25 = 150s = 150_000ms (worst-case attempt-8 wait)"
        );
    }

    #[test]
    fn compute_next_attempt_max_negative_jitter_is_75pct_base() {
        let now = Utc::now();
        let next = compute_next_attempt(0, now, -1.0).unwrap();
        let delta_ms = (next - now).num_milliseconds();
        assert_eq!(delta_ms, 750, "1s base * (1 - 0.25) = 0.75s = 750ms");
    }

    #[test]
    fn compute_next_attempt_clamps_out_of_range_jitter() {
        let now = Utc::now();
        // Jitter values outside [-1, 1] are clamped, never extrapolated.
        let high = compute_next_attempt(0, now, 5.0).unwrap();
        let cap = compute_next_attempt(0, now, 1.0).unwrap();
        assert_eq!(high, cap);

        let low = compute_next_attempt(0, now, -5.0).unwrap();
        let floor = compute_next_attempt(0, now, -1.0).unwrap();
        assert_eq!(low, floor);
    }

    #[test]
    fn compute_next_attempt_returns_none_after_max() {
        assert!(compute_next_attempt(MAX_ATTEMPTS, Utc::now(), 0.0).is_none());
        assert!(compute_next_attempt(MAX_ATTEMPTS + 5, Utc::now(), 0.0).is_none());
    }

    #[test]
    fn is_permanent_classifier_per_adr_009_amendment() {
        // True cases.
        assert!(is_permanent(&VaultError::DimensionMismatch {
            expected: 384,
            actual: 256
        }));
        assert!(is_permanent(&VaultError::AccessDenied("boundary".into())));
        assert!(is_permanent(&VaultError::Storage(
            "schema mismatch in column foo".into()
        )));

        // False cases (transient — retry).
        assert!(!is_permanent(&VaultError::Storage("disk full, EIO".into())));
        assert!(!is_permanent(&VaultError::Io(std::io::Error::other(
            "transient io"
        ))));
        assert!(!is_permanent(&VaultError::InvalidInput(
            "caller passed bad data — but the contract is grey enough that we retry".into()
        )));
        assert!(!is_permanent(&VaultError::NotFound("memory".into())));
    }

    #[test]
    fn truncate_utf8_respects_char_boundaries() {
        // 1KB of 4-byte chars: 256 chars total. Truncate at 100 bytes → 25 chars.
        let s: String = "🦀".repeat(256);
        let t = truncate_utf8(&s, 100);
        assert!(t.len() <= 100);
        // Each crab is 4 bytes; 100/4 = 25 crabs = 100 bytes exactly.
        assert_eq!(t.chars().count(), 25);

        // Awkward boundary: 99 bytes is mid-char; we truncate to 96 (24 crabs).
        let t = truncate_utf8(&s, 99);
        assert_eq!(t.len(), 96);
        assert_eq!(t.chars().count(), 24);

        // ASCII path: truncates exactly.
        let s = "a".repeat(200);
        let t = truncate_utf8(&s, 50);
        assert_eq!(t.len(), 50);
    }

    // ---------- Jitter sources ----------

    #[test]
    fn fixed_jitter_returns_constant() {
        let mut j = FixedJitter(0.5);
        for _ in 0..10 {
            assert!((j.next_factor() - 0.5).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn seeded_jitter_is_deterministic_per_seed() {
        let mut a = SeededJitter::from_seed(42);
        let mut b = SeededJitter::from_seed(42);
        for _ in 0..16 {
            assert!((a.next_factor() - b.next_factor()).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn seeded_jitter_factors_in_range() {
        // 1000 draws — every value must lie in [-1.0, 1.0).
        let mut j = SeededJitter::from_seed(0xCAFEBABE);
        for _ in 0..1000 {
            let f = j.next_factor();
            assert!(
                (-1.0..1.0).contains(&f),
                "seeded jitter produced out-of-range factor: {f}"
            );
        }
    }

    #[test]
    fn seeded_jitter_zero_seed_does_not_yield_constant_zero() {
        // Degenerate xorshift state at 0 → coerce to non-zero sentinel.
        let mut j = SeededJitter::from_seed(0);
        let first = j.next_factor();
        let second = j.next_factor();
        assert!(
            (first - second).abs() > f64::EPSILON,
            "seed=0 should not produce a constant stream"
        );
    }

    // ---------- Persistence: enqueue + get ----------

    #[tokio::test]
    async fn enqueue_creates_persisted_row_with_attempts_zero() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let new = sample_new_retry(7);
        let id = q.enqueue(new.clone(), &mut j).await.unwrap();

        let entry = q.get(id).await.unwrap().expect("entry must exist");
        assert_eq!(entry.id, id);
        assert_eq!(entry.memory_id, new.memory_id);
        assert_eq!(entry.operation, new.operation);
        assert_eq!(entry.sequence_id, 7);
        assert_eq!(entry.attempts_made, 0);
        assert_eq!(entry.payload_format_version, PAYLOAD_FORMAT_VERSION);
        assert_eq!(entry.payload, new.payload);
        assert!(entry.last_error.is_none());
    }

    #[tokio::test]
    async fn enqueue_initial_next_attempt_at_one_second_with_zero_jitter() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let id = q.enqueue(sample_new_retry(1), &mut j).await.unwrap();

        let entry = q.get(id).await.unwrap().unwrap();
        let delta_ms = (entry.next_attempt_at - entry.created_at).num_milliseconds();
        // base[0] = 1s = 1000ms with zero jitter; allow ±5ms slop for clock drift
        // between Utc::now() calls inside enqueue.
        assert!(
            (995..=1005).contains(&delta_ms),
            "expected next_attempt_at ≈ created_at + 1s (delta {delta_ms}ms)"
        );
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_tmp, q) = make_queue().await;
        let phantom = Uuid::now_v7();
        assert!(q.get(phantom).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn enqueue_distinct_sequences_for_same_memory() {
        // (memory_id, sequence_id) UNIQUE allows multiple entries for one
        // memory as long as they have different sequence_ids.
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let mem = MemoryId::new();

        for seq in [10, 5, 20] {
            q.enqueue(
                NewRetry {
                    memory_id: mem,
                    operation: CascadeOperation::Write,
                    sequence_id: seq,
                    payload: serde_json::json!({"seq": seq}),
                },
                &mut j,
            )
            .await
            .unwrap();
        }

        assert_eq!(q.len().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn enqueue_collision_on_same_memory_and_sequence_rejected() {
        // SQL-level UNIQUE constraint must reject (memory_id, sequence_id) collisions.
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let mem = MemoryId::new();

        q.enqueue(
            NewRetry {
                memory_id: mem,
                operation: CascadeOperation::Write,
                sequence_id: 42,
                payload: serde_json::json!({}),
            },
            &mut j,
        )
        .await
        .unwrap();

        let err = q
            .enqueue(
                NewRetry {
                    memory_id: mem,
                    operation: CascadeOperation::Update,
                    sequence_id: 42,
                    payload: serde_json::json!({}),
                },
                &mut j,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::Storage(_)));
    }

    // ---------- poll_due: FIFO + due-time filter ----------

    #[tokio::test]
    async fn poll_due_returns_only_due_entries() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);

        // Two entries at seq 1, 2 — both will have next_attempt_at ≈ now+1s.
        q.enqueue(sample_new_retry(1), &mut j).await.unwrap();
        q.enqueue(sample_new_retry(2), &mut j).await.unwrap();

        // Polling at "now" returns nothing — none are due yet.
        let early = q.poll_due(Utc::now(), 10).await.unwrap();
        assert!(
            early.is_empty(),
            "no entries should be due immediately, got {} ",
            early.len()
        );

        // Polling at now + 5s returns both.
        let later = q
            .poll_due(Utc::now() + Duration::seconds(5), 10)
            .await
            .unwrap();
        assert_eq!(later.len(), 2);
    }

    #[tokio::test]
    async fn poll_due_respects_limit() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        for seq in 0..5 {
            q.enqueue(sample_new_retry(seq), &mut j).await.unwrap();
        }
        let due = q
            .poll_due(Utc::now() + Duration::seconds(5), 3)
            .await
            .unwrap();
        assert_eq!(due.len(), 3);
    }

    #[tokio::test]
    async fn poll_due_strict_fifo_ordering_by_sequence_id() {
        // Per the cascade-ordering invariant (plan Q1): retries are processed
        // in sequence_id order regardless of insertion order.
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let mem = MemoryId::new();
        for seq in [99, 3, 50, 1, 17] {
            q.enqueue(
                NewRetry {
                    memory_id: mem,
                    operation: CascadeOperation::Write,
                    sequence_id: seq,
                    payload: serde_json::json!({"s": seq}),
                },
                &mut j,
            )
            .await
            .unwrap();
        }

        let due = q
            .poll_due(Utc::now() + Duration::seconds(5), 10)
            .await
            .unwrap();
        let seqs: Vec<i64> = due.iter().map(|e| e.sequence_id).collect();
        assert_eq!(seqs, vec![1, 3, 17, 50, 99], "strict ASC by sequence_id");
    }

    // ---------- record_success ----------

    #[tokio::test]
    async fn record_success_removes_row() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let id = q.enqueue(sample_new_retry(1), &mut j).await.unwrap();
        assert!(q.record_success(id).await.unwrap());
        assert!(q.get(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn record_success_idempotent_on_missing_row() {
        let (_tmp, q) = make_queue().await;
        let phantom = Uuid::now_v7();
        // First call: no row → false (no-op success).
        assert!(!q.record_success(phantom).await.unwrap());
        // Second call: still false.
        assert!(!q.record_success(phantom).await.unwrap());
    }

    // ---------- record_failure: reschedule path ----------

    #[tokio::test]
    async fn record_failure_transient_increments_and_reschedules() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let id = q.enqueue(sample_new_retry(1), &mut j).await.unwrap();

        // attempts_made=0 → after one transient failure: attempts_made=1,
        // next_attempt_at ≈ now + 2s (base[1]).
        let outcome = q
            .record_failure(id, "transient I/O", false, &mut j)
            .await
            .unwrap();
        match outcome {
            FailureOutcome::Rescheduled {
                attempts_made,
                next_attempt_at,
            } => {
                assert_eq!(attempts_made, 1);
                let delta_ms = (next_attempt_at - Utc::now()).num_milliseconds();
                assert!(
                    (1900..=2100).contains(&delta_ms),
                    "expected ≈2s wait for second attempt, got {delta_ms}ms"
                );
            }
            other => panic!("expected Rescheduled, got {other:?}"),
        }

        // Persisted state matches.
        let entry = q.get(id).await.unwrap().unwrap();
        assert_eq!(entry.attempts_made, 1);
        assert_eq!(entry.last_error.as_deref(), Some("transient I/O"));
    }

    #[tokio::test]
    async fn record_failure_walks_full_schedule_through_attempts_one_to_seven() {
        // Drive a single entry through 7 transient failures; confirm each
        // post-failure attempts_made and the next-attempt wait.
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let id = q.enqueue(sample_new_retry(1), &mut j).await.unwrap();

        // Each iteration: attempts_made before → after, expected wait = base[after].
        let waits: [(u32, u32, u32); 7] = [
            (0, 1, 2),
            (1, 2, 4),
            (2, 3, 8),
            (3, 4, 16),
            (4, 5, 30),
            (5, 6, 60),
            (6, 7, 120),
        ];
        for (before, after, base_secs) in waits {
            let outcome = q
                .record_failure(id, "transient", false, &mut j)
                .await
                .unwrap();
            match outcome {
                FailureOutcome::Rescheduled {
                    attempts_made,
                    next_attempt_at,
                } => {
                    assert_eq!(
                        attempts_made, after,
                        "starting from {before}, expected attempts_made={after}"
                    );
                    let delta_secs = (next_attempt_at - Utc::now()).num_seconds();
                    assert!(
                        ((base_secs as i64) - 1..=(base_secs as i64) + 1).contains(&delta_secs),
                        "wait at attempts_made={after}: expected ~{base_secs}s, got {delta_secs}s"
                    );
                }
                FailureOutcome::DeadLetter { .. } => {
                    panic!("unexpected dead-letter at attempts_made={after}")
                }
            }
        }
    }

    // ---------- record_failure: dead-letter paths ----------

    #[tokio::test]
    async fn record_failure_dead_letters_after_eighth_attempt() {
        // Drive through 7 transient failures (attempts_made → 7), then the
        // 8th transient failure must dead-letter with AttemptsExhausted.
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let id = q.enqueue(sample_new_retry(1), &mut j).await.unwrap();

        for _ in 0..7 {
            q.record_failure(id, "transient", false, &mut j)
                .await
                .unwrap();
        }
        // 8th failure.
        let outcome = q
            .record_failure(id, "final transient", false, &mut j)
            .await
            .unwrap();
        match outcome {
            FailureOutcome::DeadLetter {
                entry,
                last_error,
                reason,
            } => {
                assert_eq!(reason, DeadLetterReason::AttemptsExhausted);
                assert_eq!(entry.attempts_made, 8);
                assert_eq!(last_error, "final transient");
            }
            other => panic!("expected DeadLetter on 8th failure, got {other:?}"),
        }

        // Row removed.
        assert!(q.get(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn record_failure_permanent_dead_letters_immediately() {
        // permanent=true on the very first failure (attempts_made=0) must
        // dead-letter without cycling backoff.
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let id = q.enqueue(sample_new_retry(1), &mut j).await.unwrap();

        let outcome = q
            .record_failure(id, "dimension mismatch 384/256", true, &mut j)
            .await
            .unwrap();
        match outcome {
            FailureOutcome::DeadLetter {
                entry,
                last_error,
                reason,
            } => {
                assert_eq!(reason, DeadLetterReason::Permanent);
                assert_eq!(entry.attempts_made, 1, "+1 for the failed attempt");
                assert_eq!(last_error, "dimension mismatch 384/256");
            }
            other => panic!("expected DeadLetter on permanent failure, got {other:?}"),
        }
        assert!(q.get(id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn record_failure_truncates_long_error_message() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let id = q.enqueue(sample_new_retry(1), &mut j).await.unwrap();

        let huge: String = "x".repeat(10_000);
        q.record_failure(id, &huge, false, &mut j).await.unwrap();
        let entry = q.get(id).await.unwrap().unwrap();
        let stored = entry.last_error.as_deref().unwrap();
        assert!(
            stored.len() <= LAST_ERROR_MAX_BYTES,
            "stored error len {} exceeded cap",
            stored.len()
        );
    }

    #[tokio::test]
    async fn record_failure_returns_not_found_for_missing_id() {
        let (_tmp, q) = make_queue().await;
        let mut j = FixedJitter(0.0);
        let phantom = Uuid::now_v7();
        let err = q
            .record_failure(phantom, "anything", false, &mut j)
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
    }

    // ---------- Concurrency ----------

    #[tokio::test]
    async fn concurrent_enqueue_for_distinct_sequences_all_succeed() {
        // 20 tasks enqueue retries for the same memory_id with different
        // sequence_ids — UNIQUE(memory_id, sequence_id) admits all.
        let (_tmp, q) = make_queue().await;
        let mem = MemoryId::new();
        let mut handles = Vec::new();
        for seq in 0..20i64 {
            let q = q.clone();
            let m = mem;
            handles.push(tokio::spawn(async move {
                let mut j = FixedJitter(0.0);
                q.enqueue(
                    NewRetry {
                        memory_id: m,
                        operation: CascadeOperation::Write,
                        sequence_id: seq,
                        payload: serde_json::json!({"s": seq}),
                    },
                    &mut j,
                )
                .await
            }));
        }
        let mut ids = HashSet::new();
        for h in handles {
            ids.insert(h.await.unwrap().unwrap());
        }
        assert_eq!(ids.len(), 20, "all enqueues should produce distinct ids");
        assert_eq!(q.len().await.unwrap(), 20);
    }

    // ---------- Property test: schedule arithmetic ----------

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

        #[test]
        fn compute_next_attempt_within_jitter_band(
            attempts in 0u32..MAX_ATTEMPTS,
            jitter_factor in -1.0f64..=1.0,
        ) {
            let now = Utc::now();
            let base = base_backoff_secs(attempts).unwrap() as f64;
            let next = compute_next_attempt(attempts, now, jitter_factor).unwrap();
            let actual_secs = (next - now).num_milliseconds() as f64 / 1000.0;

            // Effective jitter is ±25%: every result lies in [base*0.75, base*1.25].
            // Slop margin of 0.005s for integer-ms truncation.
            let lower = base * 0.75 - 0.005;
            let upper = base * 1.25 + 0.005;
            prop_assert!(
                actual_secs >= lower && actual_secs <= upper,
                "attempts={attempts} jitter={jitter_factor}: {actual_secs}s outside [{lower}, {upper}]"
            );
        }
    }

    // The helper struct below is unused at the data-layer test level; it
    // exists so the orchestrator's adversarial tests in Phase C can mock
    // a controllable jitter sequence. Keeping it here keeps the contract
    // co-located with the trait definition.
    pub struct ScriptedJitter {
        factors: Vec<f64>,
        idx: Arc<Mutex<usize>>,
    }

    impl ScriptedJitter {
        pub fn new(factors: Vec<f64>) -> Self {
            Self {
                factors,
                idx: Arc::new(Mutex::new(0)),
            }
        }
    }

    impl JitterSource for ScriptedJitter {
        fn next_factor(&mut self) -> f64 {
            let mut i = self.idx.lock().unwrap();
            let v = self.factors[*i % self.factors.len()];
            *i += 1;
            v
        }
    }

    #[test]
    fn scripted_jitter_cycles_through_factors() {
        let mut j = ScriptedJitter::new(vec![0.0, 0.5, -0.5]);
        assert_eq!(j.next_factor(), 0.0);
        assert_eq!(j.next_factor(), 0.5);
        assert_eq!(j.next_factor(), -0.5);
        assert_eq!(j.next_factor(), 0.0);
    }
}
