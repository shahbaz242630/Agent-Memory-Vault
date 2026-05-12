//! [`StorageBackend`] — the cascading-write orchestrator (BRD §5.2, T0.1.6
//! Phase C1b).
//!
//! ## Responsibilities
//!
//! - User-write entry points (`write_memory` / `update_memory` /
//!   `delete_memory`) commit the **SQLite-side authoritative state** —
//!   `memories` row + audit chain entry + retry-queue (or pending-sync)
//!   bookkeeping — atomically in **one** SQLite transaction.
//! - Returns [`Ack`] as soon as that commit lands. Cascading writes to
//!   LanceDB and DuckDB run **asynchronously** through the retry worker
//!   (see `retry_worker.rs`).
//! - Eager `validate_readable` probe on `open()` surfaces hard fragment
//!   corruption immediately as a CRITICAL audit event + [`DegradedMode`]
//!   flag. Per ADR-018 + Phase A Change 1, the backend stays open in
//!   degraded mode so vault-cli triage still works.
//!
//! ## Lockstep + idempotency contract (load-bearing — ADR-017)
//!
//! Every cascading write produces **one** retry-queue row carrying both
//! the LanceDB and DuckDB sub-ops as a single unit (Phase C plan v2 Issue
//! 1). The worker re-runs both sub-ops idempotently per attempt; either
//! failure → whole entry reschedules. Retrieval-quality during the
//! divergence window is unaffected for V0.1 because retrieval is
//! LanceDB-only.
//!
//! ## Eager validation rejects the permanent-failure classes (ADR-009 amendment)
//!
//! `is_permanent` covers `DimensionMismatch` / `AccessDenied` /
//! `Storage(msg).contains("schema")`. The orchestrator validates dim +
//! boundary at `write_memory` / `update_memory` entry **before** the
//! SQLite write — those classes shouldn't reach the queue by
//! construction. If they do (because the worker observed a concurrent
//! schema drift), the worker dead-letters loudly with
//! [`crate::retry_queue::DeadLetterReason::Permanent`].
//!
//! ## V0.1 graph-cascade scope
//!
//! V0.1 does not extract entities at write time — that ships at T0.2.2
//! (consolidator). The orchestrator's "graph-side" cascade is therefore
//! a no-op in production for memory writes. The worker still consults
//! the test-only `FaultInjector::graph_decision()` so adversarial tests
//! can drive graph-side failure scenarios; V0.2 wires the actual
//! consolidator-driven graph writes through the same retry plumbing.

#![allow(dead_code)] // RetryWorker lands in retry_worker.rs; some accessors are consumed there.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};

use vault_core::{Memory, MemoryId, VaultError, VaultResult};

use crate::audit::{ActorKind, AuditEventType, PendingAuditEvent};
use crate::dead_letter::DeadLetter;
use crate::graph_store::{DuckDbGraphStore, GraphStore};
use crate::key::SqlCipherKey;
use crate::metadata_store::{
    tx_append_audit, tx_get_memory, tx_insert_memory, tx_update_memory, MetadataStore,
};
use crate::pending_sync::PendingSync;
use crate::retry_queue::{CascadeOperation, RetryQueue, PAYLOAD_FORMAT_VERSION};
use crate::vector_store::{LanceVectorStore, VectorStore};

/// Hard cap on `retry_queue` entries before new cascading writes spill
/// into `pending_sync` for catch-up. Per ADR-009 amendment / Phase C plan
/// Q2.
pub const MAX_RETRY_QUEUE_DEPTH: usize = 10_000;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Returned from [`StorageBackend::write_memory`] et al. The cascading
/// downstream writes (LanceDB + DuckDB) have **not** completed by the time
/// this is returned — they are async via the retry worker. The SQLite-side
/// state (memory row + audit chain entry + retry-queue / pending-sync row)
/// IS durably committed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub memory_id: MemoryId,
    pub sqlite_committed_at: DateTime<Utc>,
}

/// Reported readability state of the downstream stores. Set on `open()`
/// after `validate_readable` runs. UI banner + vault-cli surface this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DegradedMode {
    /// Both downstream stores validated readable on open.
    Healthy,
    /// LanceDB's `validate_readable` failed — vector search will not work
    /// until repair. SQLite + DuckDB still operational.
    LanceUnreadable,
    /// DuckDB's `validate_readable` failed — graph traversal will not work.
    /// SQLite + LanceDB still operational.
    GraphUnreadable,
    /// Both downstream stores failed `validate_readable`.
    BothUnreadable,
}

impl DegradedMode {
    /// True when at least one downstream store is unreadable.
    pub fn is_degraded(self) -> bool {
        !matches!(self, Self::Healthy)
    }
}

// ---------------------------------------------------------------------------
// Internal payload shape
// ---------------------------------------------------------------------------

/// On-disk shape of `retry_queue.payload` for V0.1. Schema-versioned via
/// `retry_queue.payload_format_version`. The worker dispatches on
/// `retry_queue.operation` (Write / Update / Delete) and reads only the
/// fields relevant for that op.
///
/// For [`CascadeOperation::Delete`], the `embedding` field is an empty
/// `Vec<f32>` and `boundary` carries the boundary of the deleted memory
/// for audit-logging purposes (not used by `VectorStore::delete` itself).
///
/// Public so `vault-cli` (Phase C1b operator binary) can decode dead-letter
/// payloads when running an operator-driven retry. Format-version
/// dispatch on `retry_queue.payload_format_version` /
/// `dead_letter.payload_format_version` is the long-term shape;
/// V0.1 has only `V1`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CascadePayloadV1 {
    /// Embedding for LanceDB upsert. Empty for `Delete` cascades.
    pub embedding: Vec<f32>,
    /// Boundary the memory belongs to (defense-in-depth — also stored on
    /// the memory row + LanceDB row).
    pub boundary: String,
}

// ---------------------------------------------------------------------------
// StorageBackend
// ---------------------------------------------------------------------------

/// The cascading orchestrator. Cheap to clone (it holds `Arc`s of every
/// component internally); share freely across tasks.
///
/// Intentionally does **not** implement `Debug`: it owns the
/// `MetadataStore` (which holds the SQLCipher connection per ADR-007).
#[derive(Clone)]
pub struct StorageBackend {
    metadata: MetadataStore,
    vector: Arc<dyn VectorStore>,
    graph: Arc<dyn GraphStore>,
    retry_queue: RetryQueue,
    dead_letter: DeadLetter,
    pending_sync: PendingSync,
    degraded: DegradedMode,
    /// Tracks whether the most recent enqueue attempt found the queue at
    /// cap. When the state transitions Healthy→Overflow we fire one
    /// CRITICAL `cascade.queue_overflow` audit event; Overflow→Healthy
    /// fires nothing (the next write succeeds normally and the audit log
    /// shows the gap). Per Phase C plan Q2 ("debounced, not on every
    /// overflow write — so logs don't get drowned").
    in_cap_overflow: Arc<AtomicBool>,
}

impl StorageBackend {
    /// Open all three stores at the given paths and run `validate_readable`
    /// on the downstream stores. Returns `Ok(Self)` even on validation
    /// failure — the backend is reported via [`Self::degraded`] so
    /// vault-cli triage can still run (per ADR-018 / Phase A Change 1).
    ///
    /// Hard failures (couldn't open SQLCipher, couldn't open LanceDB /
    /// DuckDB at all) still return `Err` — those mean the vault-cli
    /// can't even read the SQLite metadata, so degraded mode wouldn't
    /// help.
    ///
    /// **Visibility (T0.2.0 Phase 3 sub-task (b)+(c), 2026-05-11):** gated to
    /// `#[cfg(any(test, feature = "v0_1_migration"))]` per HANDOFF.md iteration 4
    /// §4 amendment cascade scope expansion (option α). Calls the gated plaintext
    /// [`LanceVectorStore::open`] at line 194; the cascade gate keeps cascading.rs
    /// compiling when the feature is off (the gated symbol it references doesn't
    /// exist in those builds, so this function must also not exist).
    #[cfg(any(test, feature = "v0_1_migration"))]
    #[instrument(
        skip(metadata_path, vector_data_dir, graph_path, key),
        fields(
            metadata_path = %metadata_path.display(),
            vector_data_dir = %vector_data_dir.display(),
            graph_path = %graph_path.display(),
            dimension,
        )
    )]
    pub async fn open(
        metadata_path: &Path,
        vector_data_dir: &Path,
        graph_path: &Path,
        key: SqlCipherKey,
        dimension: usize,
    ) -> VaultResult<Self> {
        let metadata = MetadataStore::open(metadata_path, key).await?;
        let vector = LanceVectorStore::open(vector_data_dir, dimension).await?;
        let graph = DuckDbGraphStore::open(graph_path).await?;
        Self::assemble(metadata, vector, graph).await
    }

    /// Sealed companion to [`Self::open`] — opens the LanceDB store via
    /// [`LanceVectorStore::open_with_at_rest_key`] (T0.2.0 sealed at-rest
    /// path) instead of the V0.1 plaintext [`LanceVectorStore::open`].
    /// MetadataStore (SQLCipher) and DuckDbGraphStore are unchanged at
    /// Phase 2 — sealed metadata + sealed graph land at later T0.2.x
    /// hardening phases per BRD §11.5.1. Same `Ok(Self)`-on-validation-
    /// failure semantics as [`Self::open`] (degraded mode preserved).
    ///
    /// **Caller MUST pass the already-derived at-rest key** (`K3(master_key)`
    /// per ADR-008 amendment K3 KDF). Canonical production derivation
    /// site: [`vault_app::keychain::derive_at_rest_key`] per ADR-040
    /// amendment.
    ///
    /// Plaintext [`Self::open`] is retained for the V0.1 → V0.2
    /// migration source path; Phase 3 deletes both plaintext
    /// constructors after the formal at-rest gate close.
    #[instrument(
        skip(metadata_path, vector_data_dir, graph_path, key, at_rest_key),
        fields(
            metadata_path = %metadata_path.display(),
            vector_data_dir = %vector_data_dir.display(),
            graph_path = %graph_path.display(),
            dimension,
        )
    )]
    pub async fn open_with_at_rest_key(
        metadata_path: &Path,
        vector_data_dir: &Path,
        graph_path: &Path,
        key: SqlCipherKey,
        dimension: usize,
        at_rest_key: &[u8; 32],
    ) -> VaultResult<Self> {
        let metadata = MetadataStore::open(metadata_path, key).await?;
        let vector =
            LanceVectorStore::open_with_at_rest_key(vector_data_dir, dimension, at_rest_key)
                .await?;
        let graph = DuckDbGraphStore::open(graph_path).await?;
        Self::assemble(metadata, vector, graph).await
    }

    /// Shared assembly path for both [`Self::open`] (plaintext) and
    /// [`Self::open_with_at_rest_key`] (sealed). Runs `validate_readable`
    /// on the downstream stores, computes [`DegradedMode`], emits the
    /// per-store `store.corruption` audit events on failure, and builds
    /// the [`StorageBackend`] struct with shared metadata-store clones.
    async fn assemble(
        metadata: MetadataStore,
        vector: LanceVectorStore,
        graph: DuckDbGraphStore,
    ) -> VaultResult<Self> {
        let vector: Arc<dyn VectorStore> = Arc::new(vector);
        let graph: Arc<dyn GraphStore> = Arc::new(graph);

        let lance_ok = vector.validate_readable().await;
        let graph_ok = graph.validate_readable().await;

        let degraded = match (lance_ok.is_ok(), graph_ok.is_ok()) {
            (true, true) => DegradedMode::Healthy,
            (false, true) => DegradedMode::LanceUnreadable,
            (true, false) => DegradedMode::GraphUnreadable,
            (false, false) => DegradedMode::BothUnreadable,
        };

        // Emit a single `store.corruption` audit event per failed store,
        // before construction returns — so the chain captures the open
        // outcome even if the caller never reads the degraded flag.
        if let Err(e) = &lance_ok {
            warn!(error = %e, "LanceDB validate_readable failed at open — entering degraded mode (see ADR-018)");
            metadata
                .append_audit_event(
                    PendingAuditEvent::success(AuditEventType::StoreCorruption, ActorKind::System)
                        .error()
                        .with_resource("store", "lancedb")
                        .with_details_json(format!(
                            r#"{{"tag":"lancedb_corruption_at_open","error":{}}}"#,
                            json_string(&e.to_string()),
                        )),
                )
                .await?;
        }
        if let Err(e) = &graph_ok {
            warn!(error = %e, "DuckDB validate_readable failed at open — entering degraded mode (see ADR-018)");
            metadata
                .append_audit_event(
                    PendingAuditEvent::success(AuditEventType::StoreCorruption, ActorKind::System)
                        .error()
                        .with_resource("store", "duckdb")
                        .with_details_json(format!(
                            r#"{{"tag":"duckdb_corruption_at_open","error":{}}}"#,
                            json_string(&e.to_string()),
                        )),
                )
                .await?;
        }

        Ok(Self {
            retry_queue: RetryQueue::new(metadata.clone()),
            dead_letter: DeadLetter::new(metadata.clone()),
            pending_sync: PendingSync::new(metadata.clone()),
            metadata,
            vector,
            graph,
            degraded,
            in_cap_overflow: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Current degraded-mode state captured at `open()`. Stable for the
    /// lifetime of this backend instance.
    pub fn degraded(&self) -> DegradedMode {
        self.degraded
    }

    /// Metadata store handle. Crate-private — the cascading orchestrator
    /// owns all SQLite-side audit + memory writes; outside callers go
    /// through `write_memory` / `update_memory` / `delete_memory`.
    pub(crate) fn metadata(&self) -> &MetadataStore {
        &self.metadata
    }

    /// LanceDB-backed vector store handle. Public because `vault-cli`
    /// drives operator-initiated cascade retries directly against the
    /// vector store (production cascades go through the worker).
    pub fn vector_store(&self) -> &Arc<dyn VectorStore> {
        &self.vector
    }

    /// DuckDB-backed graph store handle. Public for symmetry with
    /// `vector_store()` — V0.2 consolidator + divergence-check use this
    /// for traversal validation.
    pub fn graph_store(&self) -> &Arc<dyn GraphStore> {
        &self.graph
    }

    /// Retry-queue handle. Crate-private — the cascading orchestrator
    /// owns enqueue; the retry worker owns drain. External callers
    /// observe outcomes via the `dead_letter` table.
    pub(crate) fn retry_queue(&self) -> &RetryQueue {
        &self.retry_queue
    }

    /// Dead-letter table handle. Public so vault-cli can list / inspect
    /// / resolve unresolved rows.
    pub fn dead_letter(&self) -> &DeadLetter {
        &self.dead_letter
    }

    /// Pending-sync table handle. Crate-private — V0.2 divergence-check
    /// owns the drain path.
    pub(crate) fn pending_sync(&self) -> &PendingSync {
        &self.pending_sync
    }

    /// Cascading user write. Atomic on the SQLite side: `memories` row +
    /// audit event + retry-queue (or pending-sync) row commit together.
    ///
    /// Eager validation rejects [`VaultError::DimensionMismatch`] and
    /// invalid-memory failures **before** any SQLite write — those classes
    /// would otherwise be permanent-failure dead-letters in the worker
    /// (wasted work + a confusing dead-letter row).
    #[instrument(skip(self, memory, embedding), fields(memory_id = %memory.id, dim = embedding.len()))]
    pub async fn write_memory(&self, memory: &Memory, embedding: &[f32]) -> VaultResult<Ack> {
        self.eager_validate(memory, embedding)?;
        self.cascading_write(memory.clone(), embedding.to_vec(), CascadeOperation::Write)
            .await
    }

    /// Cascading update. Same atomicity contract as [`Self::write_memory`].
    /// Returns [`VaultError::NotFound`] if the memory id doesn't exist.
    #[instrument(skip(self, memory, embedding), fields(memory_id = %memory.id, dim = embedding.len()))]
    pub async fn update_memory(&self, memory: &Memory, embedding: &[f32]) -> VaultResult<Ack> {
        self.eager_validate(memory, embedding)?;
        self.cascading_write(memory.clone(), embedding.to_vec(), CascadeOperation::Update)
            .await
    }

    /// Cascading delete. Idempotent: deleting a non-existent id still
    /// returns `Ok` and records an audit event with `details.deleted =
    /// false` (matches the [`MetadataStore::delete_memory`] contract). For
    /// non-existent ids, NO retry-queue / pending-sync row is enqueued
    /// (nothing to cascade).
    #[instrument(skip(self), fields(memory_id = %id))]
    pub async fn delete_memory(&self, id: &MemoryId) -> VaultResult<Ack> {
        let id_owned = *id;
        let metadata = self.metadata.clone();
        let in_cap = self.in_cap_overflow.clone();
        let id_for_closure = id_owned;

        let (committed_at, audit_seq, deleted_boundary): (DateTime<Utc>, i64, Option<String>) =
            metadata
                .with_transaction(move |tx| {
                    // Look up the existing memory so we know its boundary for
                    // the audit event AND so the cascade payload can carry it
                    // forward to the worker. The boundary read happens inside
                    // the same transaction as the delete — atomic.
                    let existing = tx_get_memory(tx, &id_for_closure)?;
                    let boundary_owned = existing.as_ref().map(|m| m.boundary.clone());

                    let rows = tx
                        .execute(
                            "DELETE FROM memories WHERE id = ?1",
                            params![id_for_closure.to_string()],
                        )
                        .map_err(|e| VaultError::Storage(format!("delete memory: {e}")))?;

                    let mut pending =
                        PendingAuditEvent::success(AuditEventType::MemoryDelete, ActorKind::System)
                            .with_resource("memory", id_for_closure.to_string());
                    if let Some(b) = &boundary_owned {
                        pending = pending.with_boundary(b.clone());
                    }
                    pending.details_json = format!(r#"{{"deleted":{}}}"#, rows > 0);
                    let event = tx_append_audit(tx, pending)?;
                    let committed_at = event.timestamp;
                    let audit_seq = event.seq;

                    // Only enqueue the cascade if a row was actually deleted.
                    // No row → nothing to cascade downstream.
                    if rows > 0 {
                        if let Some(boundary) = &boundary_owned {
                            let payload = CascadePayloadV1 {
                                embedding: Vec::new(),
                                boundary: boundary.as_str().to_string(),
                            };
                            let payload_bytes = serde_json::to_vec(&payload)?;

                            let queue_len = tx_count_retry_queue(tx)?;
                            if queue_len < MAX_RETRY_QUEUE_DEPTH {
                                tx_insert_retry_queue(
                                    tx,
                                    id_for_closure,
                                    CascadeOperation::Delete,
                                    audit_seq,
                                    &payload_bytes,
                                )?;
                                // Falling out of overflow if we were in it.
                                if in_cap.swap(false, Ordering::AcqRel) {
                                    // No audit event on transition out — the
                                    // gap in `cascade.queue_overflow` events
                                    // is the signal.
                                }
                            } else {
                                tx_upsert_pending_sync(
                                    tx,
                                    id_for_closure,
                                    CascadeOperation::Delete,
                                    committed_at,
                                )?;
                                // Transition into overflow: emit one audit event.
                                if !in_cap.swap(true, Ordering::AcqRel) {
                                    tx_append_audit(
                                        tx,
                                        PendingAuditEvent::success(
                                            AuditEventType::CascadeQueueOverflow,
                                            ActorKind::System,
                                        )
                                        .error()
                                        .with_details_json(
                                            r#"{"cap":10000,"action":"pending_sync_fallback"}"#
                                                .to_string(),
                                        ),
                                    )?;
                                }
                            }
                        }
                    }

                    Ok::<_, VaultError>((
                        committed_at,
                        audit_seq,
                        boundary_owned.map(|b| b.as_str().to_string()),
                    ))
                })
                .await?;

        let _ = audit_seq;
        let _ = deleted_boundary;
        Ok(Ack {
            memory_id: id_owned,
            sqlite_committed_at: committed_at,
        })
    }

    /// Eager-validate memory + embedding before any SQLite write. Drops
    /// [`VaultError::DimensionMismatch`] / invalid-memory cases on the
    /// floor so they never reach the cascading queue.
    fn eager_validate(&self, memory: &Memory, embedding: &[f32]) -> VaultResult<()> {
        memory.validate()?; // content / confidence / etc.
        let expected = self.vector.dimension();
        if embedding.len() != expected {
            return Err(VaultError::DimensionMismatch {
                expected,
                actual: embedding.len(),
            });
        }
        if embedding.is_empty() {
            return Err(VaultError::InvalidInput("embedding is empty".into()));
        }
        if embedding.iter().any(|x| !x.is_finite()) {
            return Err(VaultError::InvalidInput(
                "embedding contains non-finite values".into(),
            ));
        }
        Ok(())
    }

    /// Shared SQLite-side body for write + update. The only difference is
    /// (a) the SQL (INSERT vs UPDATE) and (b) the `CascadeOperation` written
    /// onto the retry-queue row.
    async fn cascading_write(
        &self,
        memory: Memory,
        embedding: Vec<f32>,
        op: CascadeOperation,
    ) -> VaultResult<Ack> {
        debug_assert!(matches!(
            op,
            CascadeOperation::Write | CascadeOperation::Update
        ));

        let memory_id = memory.id;
        let boundary_str = memory.boundary.as_str().to_string();
        let payload = CascadePayloadV1 {
            embedding,
            boundary: boundary_str,
        };
        let payload_bytes = serde_json::to_vec(&payload)?;
        let in_cap = self.in_cap_overflow.clone();

        let committed_at: DateTime<Utc> = self
            .metadata
            .with_transaction(move |tx| {
                let event_kind = match op {
                    CascadeOperation::Write => {
                        tx_insert_memory(tx, &memory)?;
                        AuditEventType::MemoryCreate
                    }
                    CascadeOperation::Update => {
                        let rows = tx_update_memory(tx, &memory)?;
                        if rows == 0 {
                            return Err(VaultError::NotFound(format!(
                                "memory {memory_id} does not exist",
                            )));
                        }
                        AuditEventType::MemoryUpdate
                    }
                    CascadeOperation::Delete => unreachable!("delete uses delete_memory"),
                };

                let event = tx_append_audit(
                    tx,
                    PendingAuditEvent::success(event_kind, ActorKind::System)
                        .with_resource("memory", memory_id.to_string())
                        .with_boundary(memory.boundary.clone()),
                )?;
                let committed_at = event.timestamp;
                let audit_seq = event.seq;

                let queue_len = tx_count_retry_queue(tx)?;
                if queue_len < MAX_RETRY_QUEUE_DEPTH {
                    tx_insert_retry_queue(tx, memory_id, op, audit_seq, &payload_bytes)?;
                    in_cap.store(false, Ordering::Release);
                } else {
                    tx_upsert_pending_sync(tx, memory_id, op, committed_at)?;
                    if !in_cap.swap(true, Ordering::AcqRel) {
                        tx_append_audit(
                            tx,
                            PendingAuditEvent::success(
                                AuditEventType::CascadeQueueOverflow,
                                ActorKind::System,
                            )
                            .error()
                            .with_details_json(
                                r#"{"cap":10000,"action":"pending_sync_fallback"}"#.to_string(),
                            ),
                        )?;
                    }
                }
                Ok::<DateTime<Utc>, VaultError>(committed_at)
            })
            .await?;

        Ok(Ack {
            memory_id,
            sqlite_committed_at: committed_at,
        })
    }
}

// ---------------------------------------------------------------------------
// SQL helpers (private to this module — all run inside a caller-supplied
// `&Transaction<'_>`)
// ---------------------------------------------------------------------------

fn tx_count_retry_queue(tx: &rusqlite::Transaction<'_>) -> VaultResult<usize> {
    let n: i64 = tx
        .query_row("SELECT COUNT(*) FROM retry_queue", [], |row| row.get(0))
        .map_err(|e| VaultError::Storage(format!("count retry_queue (in tx): {e}")))?;
    Ok(n as usize)
}

fn tx_insert_retry_queue(
    tx: &rusqlite::Transaction<'_>,
    memory_id: MemoryId,
    operation: CascadeOperation,
    sequence_id: i64,
    payload_bytes: &[u8],
) -> VaultResult<()> {
    use crate::retry_queue::{base_backoff_secs, compute_next_attempt};
    let now = Utc::now();
    let next_at = compute_next_attempt(0, now, 0.0)
        .expect("base_backoff_secs(0) must yield Some — invariant of retry_queue schedule");
    let entry_id = uuid::Uuid::now_v7();
    let _ = base_backoff_secs(0); // sanity reference for the audit reader

    tx.execute(
        "INSERT INTO retry_queue (
            id, memory_id, operation, payload_format_version,
            payload, sequence_id, attempts_made,
            next_attempt_at, created_at, last_error
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, NULL)",
        params![
            entry_id.as_bytes().to_vec(),
            memory_id.0.as_bytes().to_vec(),
            operation.as_str(),
            PAYLOAD_FORMAT_VERSION,
            payload_bytes,
            sequence_id,
            next_at.to_rfc3339(),
            now.to_rfc3339(),
        ],
    )
    .map_err(|e| VaultError::Storage(format!("enqueue retry (in tx): {e}")))?;
    Ok(())
}

fn tx_upsert_pending_sync(
    tx: &rusqlite::Transaction<'_>,
    memory_id: MemoryId,
    operation: CascadeOperation,
    queued_at: DateTime<Utc>,
) -> VaultResult<()> {
    tx.execute(
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
    .map_err(|e| VaultError::Storage(format!("upsert pending_sync (in tx): {e}")))?;
    Ok(())
}

/// Encode `s` as a JSON string literal (with surrounding quotes, embedded
/// quotes / backslashes / control chars escaped). Used for the
/// `details_json` field — keeps the audit body well-formed JSON without
/// pulling `serde_json::Value` round-trips.
fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"<unserialisable>\"".to_string())
}

// ---------------------------------------------------------------------------
// PendingAuditEvent builder extension — small convenience for the
// orchestrator's audit emissions.
// ---------------------------------------------------------------------------

/// Trait extension so we can chain `.with_details_json("...")` like the
/// other builder methods. Kept private to avoid leaking the helper outside
/// this module.
trait PendingAuditEventExt {
    fn with_details_json(self, json: String) -> Self;
}

impl PendingAuditEventExt for PendingAuditEvent {
    fn with_details_json(mut self, json: String) -> Self {
        self.details_json = json;
        self
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom, Write};

    use tempfile::TempDir;

    use vault_core::{Boundary, MemoryType, NewMemory};

    use crate::audit::{AuditEvent, AuditEventType};

    const DIM: usize = 4;

    /// Test-only at-rest key (32 bytes, fixed pattern). Per-mod local
    /// const per HANDOFF sub-task (d) §"Const placement" decision lock;
    /// matches the convention in `tests/migration_v0_1_to_sealed.rs:96`.
    const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

    fn embedding(fill: f32) -> Vec<f32> {
        vec![fill; DIM]
    }

    fn sample_memory(boundary: &str, content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(boundary).unwrap(),
            source_agent: Some("test".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .unwrap()
    }

    /// Open a backend rooted at a fresh tempdir. Returns the tempdir guard
    /// (must be kept alive — it owns the directory) plus the backend.
    async fn make_backend() -> (TempDir, StorageBackend) {
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("cascading-test-key");
        let backend = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();
        (tmp, backend)
    }

    // ------------------------------------------------------------------
    // open() degraded-mode reporting
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn open_on_clean_store_is_healthy() {
        let (_tmp, backend) = make_backend().await;
        assert_eq!(backend.degraded(), DegradedMode::Healthy);
    }

    #[tokio::test]
    async fn open_on_corrupted_lance_fragments_returns_lance_unreadable() {
        // Per ADR-018 / Phase A Q5 test 3b: open MUST return Ok with
        // degraded == LanceUnreadable, audit log MUST contain a
        // `store.corruption` event tagged `lancedb_corruption_at_open`,
        // and SQLite-side state MUST remain operational.
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("corruption-test-key");

        // First open: write 5 memories so there's data to corrupt.
        {
            let backend = StorageBackend::open_with_at_rest_key(
                &metadata_path,
                &vector_dir,
                &graph_path,
                key.clone(),
                DIM,
                &TEST_AT_REST_KEY,
            )
            .await
            .unwrap();
            for i in 0..5 {
                let m = sample_memory("work", &format!("memory-{i}"));
                backend
                    .write_memory(&m, &embedding(0.1 * i as f32))
                    .await
                    .unwrap();
            }
            // Force the cascade through manually so LanceDB has fragments to corrupt.
            // We do this by upserting directly into the vector store on this path.
            for i in 0..5 {
                let id = MemoryId::new();
                let b = Boundary::new("work").unwrap();
                backend
                    .vector
                    .upsert(&id, &embedding(0.2 * i as f32), &b)
                    .await
                    .unwrap();
            }
        }
        // Drop the backend (releases the LanceDB connection's file locks).

        // Find a Lance fragment file under the vector_dir and clobber its
        // last 64 bytes with 0xAB. Mirrors the spike's corruption shape.
        //
        // Why the FOOTER, not the header: lance v1 (0.8 era) used a
        // header-based file format — the magic + version bytes were at
        // offset 0, so corrupting the first 64 bytes tripped magic-check
        // fast-fail. Lance v2 (4.0+) moved to a footer-based format —
        // the magic ("LANC") + length-prefixed metadata live at the END
        // of the file. Phase 0a-fix (2026-05-07) discovered that
        // header-corruption on a lance v2 file does NOT fail fast: the
        // first 64 bytes are interpreted as data, and a corrupted size
        // field downstream triggers a 32 GB allocation attempt that
        // OOM-aborts the test process. Footer-corruption fails
        // lance's magic-check immediately, no allocation. Same intent
        // (file is unreadable), different offset for the format change.
        // ADR-018's `validate_readable` path still surfaces the
        // corruption regardless.
        let lance_path = find_first_lance_fragment(&vector_dir)
            .expect("expected at least one .lance fragment under the vector data dir");
        {
            let mut f = OpenOptions::new().write(true).open(&lance_path).unwrap();
            f.seek(SeekFrom::End(-64)).unwrap();
            f.write_all(&[0xAB; 64]).unwrap();
            f.sync_all().unwrap();
        }

        // Reopen — must succeed with degraded mode.
        let backend2 = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();
        assert_eq!(
            backend2.degraded(),
            DegradedMode::LanceUnreadable,
            "open on corrupted Lance must report LanceUnreadable, not fail"
        );

        // SQLite side must still work: list memories returns the rows we
        // wrote on the first open.
        let memories = backend2
            .metadata
            .list_memories(Default::default(), 100)
            .await
            .unwrap();
        assert_eq!(memories.len(), 5);

        // Audit log includes the corruption event, tagged lancedb.
        let events = backend2
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let corruption: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::StoreCorruption)
            .collect();
        assert_eq!(
            corruption.len(),
            1,
            "exactly one store.corruption event expected on second open"
        );
        let detail = &corruption[0].details_json;
        assert!(
            detail.contains("lancedb_corruption_at_open"),
            "details_json should tag the failing store: {detail}"
        );
        assert_eq!(corruption[0].resource_type.as_deref(), Some("store"));
        assert_eq!(corruption[0].resource_id.as_deref(), Some("lancedb"));
    }

    fn find_first_lance_fragment(root: &Path) -> Option<std::path::PathBuf> {
        for entry in walkdir_min(root) {
            let p = entry;
            if p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s == "lance")
                    .unwrap_or(false)
            {
                return Some(p);
            }
        }
        None
    }

    /// Tiny walkdir replacement so we don't pull a workspace dep just for tests.
    fn walkdir_min(root: &Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            let Ok(read) = std::fs::read_dir(&p) else {
                continue;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------
    // Eager validation — DimensionMismatch / invalid-memory rejection
    // (Shahbaz observation #2: rejection-path test)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn write_memory_rejects_dimension_mismatch_with_no_rows_anywhere() {
        // The load-bearing rejection-path test. Per Phase C plan v2
        // observation #2: a wrong-dim write must NOT leave any state on
        // disk — no `memories` row, no `audit_log` entry, no `retry_queue`
        // entry. The lockstep contract (Issue 1) breaks if a permanent
        // failure can sneak past the orchestrator into the worker.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "wrong-dim memory");

        let wrong_dim = [0.0_f32; DIM + 3];
        let err = backend.write_memory(&m, &wrong_dim).await.unwrap_err();
        match err {
            VaultError::DimensionMismatch { expected, actual } => {
                assert_eq!(expected, DIM);
                assert_eq!(actual, DIM + 3);
            }
            other => panic!("expected DimensionMismatch, got {other:?}"),
        }

        // No memory row.
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());
        // No retry_queue row.
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
        // No pending_sync row.
        assert_eq!(backend.pending_sync.len().await.unwrap(), 0);
        // Audit log only contains the eager `get_memory` read above —
        // anything else means the orchestrator wrote.
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        assert!(
            !events
                .iter()
                .any(|e| e.event_type == AuditEventType::MemoryCreate),
            "no memory.create event should be present after a rejected write"
        );
    }

    #[tokio::test]
    async fn write_memory_rejects_invalid_memory_with_no_rows_anywhere() {
        let (_tmp, backend) = make_backend().await;
        let mut m = sample_memory("work", "bad confidence");
        m.confidence = 5.0; // outside [0, 1]

        let err = backend.write_memory(&m, &embedding(0.1)).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));

        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
        assert_eq!(backend.pending_sync.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn write_memory_rejects_empty_embedding() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "empty embedding");
        // Length 0 takes the dimension-mismatch path (0 != DIM).
        let err = backend.write_memory(&m, &[]).await.unwrap_err();
        assert!(matches!(err, VaultError::DimensionMismatch { .. }));
    }

    #[tokio::test]
    async fn write_memory_rejects_non_finite_embedding() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "nan embedding");
        let mut e = embedding(0.1);
        e[1] = f32::NAN;
        let err = backend.write_memory(&m, &e).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    // ------------------------------------------------------------------
    // Standard CRUD paths — write / update / delete
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn write_memory_round_trips_via_metadata_get() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "hello");
        let ack = backend.write_memory(&m, &embedding(0.1)).await.unwrap();
        assert_eq!(ack.memory_id, m.id);

        let back = backend
            .metadata
            .get_memory(&m.id)
            .await
            .unwrap()
            .expect("memory must be persisted");
        // embedding lives in LanceDB on the cascade — the metadata row's
        // embedding stays None.
        let mut expected = m.clone();
        expected.embedding = None;
        assert_eq!(back, expected);
    }

    #[tokio::test]
    async fn write_memory_emits_memory_create_audit_event() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "audited write");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();

        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let creates: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::MemoryCreate)
            .collect();
        assert_eq!(creates.len(), 1);
        assert_eq!(
            creates[0].resource_id.as_deref(),
            Some(m.id.to_string().as_str())
        );
        assert_eq!(creates[0].boundary.as_deref(), Some("work"));
    }

    #[tokio::test]
    async fn write_memory_enqueues_one_retry_queue_row_with_audit_seq_as_sequence_id() {
        // Cascade ordering invariant (ADR-017): retry_queue.sequence_id
        // equals the audit_log.seq of the corresponding memory.create
        // event. Concurrent writes serialise by SQLite commit order.
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "ordered write");
        backend.write_memory(&m, &embedding(0.3)).await.unwrap();

        assert_eq!(backend.retry_queue.len().await.unwrap(), 1);
        let due = backend
            .retry_queue
            .poll_due(Utc::now() + chrono::Duration::seconds(60), 100)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
        let entry = &due[0];
        assert_eq!(entry.memory_id, m.id);
        assert_eq!(entry.operation, CascadeOperation::Write);

        // Cross-check against the audit chain: the latest MemoryCreate
        // event's seq must equal the entry's sequence_id.
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let create_event = events
            .iter()
            .find(|e| e.event_type == AuditEventType::MemoryCreate)
            .expect("memory.create event must exist");
        assert_eq!(entry.sequence_id, create_event.seq);
    }

    #[tokio::test]
    async fn update_memory_round_trip() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "v1 content");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let mut updated = m.clone();
        updated.content = "v2 content".into();
        backend
            .update_memory(&updated, &embedding(0.2))
            .await
            .unwrap();

        let back = backend.metadata.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.content, "v2 content");

        // Two retry-queue entries: one Write, one Update. The Update's
        // sequence_id is greater than the Write's (audit seq is monotonic).
        let due = backend
            .retry_queue
            .poll_due(Utc::now() + chrono::Duration::seconds(60), 100)
            .await
            .unwrap();
        assert_eq!(due.len(), 2);
        let ops: Vec<CascadeOperation> = due.iter().map(|e| e.operation).collect();
        assert!(ops.contains(&CascadeOperation::Write));
        assert!(ops.contains(&CascadeOperation::Update));
        let mut seqs: Vec<i64> = due.iter().map(|e| e.sequence_id).collect();
        seqs.sort();
        assert!(seqs[0] < seqs[1]);
    }

    #[tokio::test]
    async fn update_missing_memory_returns_not_found_with_no_state_changes() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "ghost");
        let err = backend
            .update_memory(&m, &embedding(0.4))
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn delete_memory_idempotent_on_missing_id() {
        let (_tmp, backend) = make_backend().await;
        let id = MemoryId::new();
        // First delete on a missing id: no error.
        let ack = backend.delete_memory(&id).await.unwrap();
        assert_eq!(ack.memory_id, id);
        // No retry-queue entry — nothing to cascade.
        assert_eq!(backend.retry_queue.len().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn delete_memory_removes_row_and_enqueues_delete_cascade() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "doomed");
        backend.write_memory(&m, &embedding(0.5)).await.unwrap();
        // Confirm preconditions.
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_some());
        let pre_count = backend.retry_queue.len().await.unwrap();
        assert_eq!(pre_count, 1);

        backend.delete_memory(&m.id).await.unwrap();
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_none());

        // Two entries now (Write + Delete).
        assert_eq!(backend.retry_queue.len().await.unwrap(), 2);
    }

    // ------------------------------------------------------------------
    // Cap-overflow → pending_sync (Phase A Q2 cap-overflow refinement)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn write_when_queue_at_cap_routes_to_pending_sync_and_emits_overflow_audit() {
        // Pre-fill retry_queue beyond the cap by inserting raw rows
        // (avoids 10k cascade calls in the test). We need exactly
        // MAX_RETRY_QUEUE_DEPTH rows.
        let (_tmp, backend) = make_backend().await;

        // Backdoor: fill retry_queue to cap by direct UPSERT inside a tx.
        backend
            .metadata
            .with_transaction(|tx| {
                use uuid::Uuid;
                let now = Utc::now();
                for i in 0..MAX_RETRY_QUEUE_DEPTH {
                    let id = Uuid::now_v7();
                    let mem = MemoryId::new();
                    let payload = serde_json::json!({"embedding": [], "boundary": "work"});
                    tx.execute(
                        "INSERT INTO retry_queue (
                            id, memory_id, operation, payload_format_version,
                            payload, sequence_id, attempts_made,
                            next_attempt_at, created_at, last_error
                        ) VALUES (?1, ?2, 'write', 1, ?3, ?4, 0, ?5, ?5, NULL)",
                        params![
                            id.as_bytes().to_vec(),
                            mem.0.as_bytes().to_vec(),
                            serde_json::to_vec(&payload).unwrap(),
                            i as i64 + 100, // arbitrary monotonic sequence_id
                            now.to_rfc3339(),
                        ],
                    )
                    .map_err(|e| VaultError::Storage(e.to_string()))?;
                }
                Ok::<_, VaultError>(())
            })
            .await
            .unwrap();
        assert_eq!(
            backend.retry_queue.len().await.unwrap(),
            MAX_RETRY_QUEUE_DEPTH
        );

        // Now write — the retry_queue is at cap, so the cascade row
        // should land in pending_sync and a CascadeQueueOverflow audit
        // event should fire (first overflow in this session).
        let m = sample_memory("work", "overflowed");
        backend.write_memory(&m, &embedding(0.7)).await.unwrap();

        // SQLite-side state: memory committed.
        assert!(backend.metadata.get_memory(&m.id).await.unwrap().is_some());
        // Retry queue length unchanged (still at cap).
        assert_eq!(
            backend.retry_queue.len().await.unwrap(),
            MAX_RETRY_QUEUE_DEPTH
        );
        // pending_sync now carries this memory_id.
        let entry = backend.pending_sync.get(m.id).await.unwrap().unwrap();
        assert_eq!(entry.operation, CascadeOperation::Write);

        // Audit log contains exactly one CascadeQueueOverflow event.
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let overflows: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::CascadeQueueOverflow)
            .collect();
        assert_eq!(
            overflows.len(),
            1,
            "exactly one cascade.queue_overflow event expected on first overflow"
        );

        // Second overflow within the same session: NO additional overflow event
        // (debounced).
        let m2 = sample_memory("work", "overflowed-again");
        backend.write_memory(&m2, &embedding(0.8)).await.unwrap();
        let events = backend
            .metadata
            .list_audit_events(usize::MAX)
            .await
            .unwrap();
        let overflows_after_second: Vec<&AuditEvent> = events
            .iter()
            .filter(|e| e.event_type == AuditEventType::CascadeQueueOverflow)
            .collect();
        assert_eq!(
            overflows_after_second.len(),
            1,
            "second overflowing write should not emit another overflow event (debounced)"
        );
        assert!(backend.pending_sync.get(m2.id).await.unwrap().is_some());
    }

    // ------------------------------------------------------------------
    // Mid-cascade abort recovery (Phase A Q5 test 1)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn mid_cascade_abort_recovers_via_retry_queue() {
        // Simulates process exit between SQLite ack and the LanceDB-side
        // cascade. After the orchestrator drops, a fresh orchestrator on
        // the same files must see the retry_queue row and be able to
        // re-drive it. We don't wire the worker here — that's
        // retry_worker.rs's territory — we just assert the persistence
        // surface that the worker consumes.
        let tmp = TempDir::new().unwrap();
        let metadata_path = tmp.path().join("vault.db");
        let vector_dir = tmp.path().join("lance");
        let graph_path = tmp.path().join("graph.duckdb");
        let key = SqlCipherKey::new("recover-test-key");

        let m = sample_memory("work", "recover me");
        let id = m.id;
        {
            let backend1 = StorageBackend::open_with_at_rest_key(
                &metadata_path,
                &vector_dir,
                &graph_path,
                key.clone(),
                DIM,
                &TEST_AT_REST_KEY,
            )
            .await
            .unwrap();
            backend1.write_memory(&m, &embedding(0.9)).await.unwrap();
        }
        // Simulated process exit: backend1 dropped here. Files persist.

        let backend2 = StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();

        // SQLite has the memory.
        assert!(backend2.metadata.get_memory(&id).await.unwrap().is_some());
        // Retry queue still has the cascade entry waiting to be drained.
        let due = backend2
            .retry_queue
            .poll_due(Utc::now() + chrono::Duration::seconds(60), 100)
            .await
            .unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].memory_id, id);
        assert_eq!(due[0].operation, CascadeOperation::Write);
        let payload: CascadePayloadV1 = serde_json::from_value(due[0].payload.clone()).unwrap();
        assert_eq!(payload.embedding.len(), DIM);
        assert_eq!(payload.boundary, "work");
    }

    // ------------------------------------------------------------------
    // Ack contents
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn ack_carries_memory_id_and_recent_committed_at() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "ack content");
        let before = Utc::now() - chrono::Duration::seconds(1);
        let ack = backend.write_memory(&m, &embedding(0.1)).await.unwrap();
        let after = Utc::now() + chrono::Duration::seconds(1);
        assert_eq!(ack.memory_id, m.id);
        assert!(ack.sqlite_committed_at >= before);
        assert!(ack.sqlite_committed_at <= after);
    }

    // ------------------------------------------------------------------
    // DegradedMode helper
    // ------------------------------------------------------------------

    #[test]
    fn degraded_mode_is_degraded_helper() {
        assert!(!DegradedMode::Healthy.is_degraded());
        assert!(DegradedMode::LanceUnreadable.is_degraded());
        assert!(DegradedMode::GraphUnreadable.is_degraded());
        assert!(DegradedMode::BothUnreadable.is_degraded());
    }
}
