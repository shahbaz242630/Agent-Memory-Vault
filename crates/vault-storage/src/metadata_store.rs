//! [`MetadataStore`] — the SQLite/SQLCipher-backed durable record-of-truth
//! for memory metadata, audit events, and migration state (BRD §5.2).
//!
//! Vector embeddings live in LanceDB (T0.1.4). Graph entities and
//! relationships live in DuckDB (T0.1.5). This store is the *metadata*
//! authority — when those other stores disagree with this one, this one
//! wins (cascading writes in T0.1.6 will reconcile).
//!
//! ## Concurrency
//!
//! `rusqlite::Connection` is single-threaded by design. We wrap a single
//! connection in `std::sync::Mutex` and run all DB work inside
//! [`tokio::task::spawn_blocking`] so async callers don't block the
//! runtime. For V0.1's expected throughput (handfuls of writes per
//! second, perhaps tens) this is enough; a real connection pool can
//! land later if profiling demands it.
//!
//! ## Atomicity
//!
//! Every CRUD operation runs inside a transaction that *also* appends
//! the corresponding audit event. Either both happen or neither — there
//! is no observable state where a memory was written but its audit event
//! was lost.

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, Row, Transaction};
use tracing::{debug, instrument};
use uuid::Uuid;

use vault_core::{Boundary, Memory, MemoryId, MemoryType, VaultError, VaultResult};

use crate::audit::{
    seal, ActorKind, AuditEvent, AuditEventType, AuditResult, PendingAuditEvent, AUDIT_GENESIS_HASH,
};
use crate::key::SqlCipherKey;
use crate::migrations;

/// Filter for [`MetadataStore::list_memories`]. All fields default to "no filter."
#[derive(Clone, Debug, Default)]
pub struct MemoryFilter {
    /// If set, only return memories in this boundary.
    pub boundary: Option<Boundary>,
    /// If set, only return memories of this type.
    pub memory_type: Option<MemoryType>,
    /// If `false` (default), exclude memories that have been superseded by a
    /// consolidator merge. Set to `true` to include them (used by audit /
    /// debugging tooling, never by retrieval).
    pub include_superseded: bool,
}

/// Async, encrypted SQLite-backed metadata store. Cheap to clone (it holds
/// an `Arc` internally), share freely across tasks.
///
/// Intentionally does **not** implement `Debug`: it owns a live SQLCipher
/// connection holding key-derived state, and we don't want a habit of
/// stubbing `Debug` on types that hold sensitive handles. See ADR-007.
#[derive(Clone)]
pub struct MetadataStore {
    inner: Arc<Inner>,
}

struct Inner {
    conn: Mutex<Connection>,
}

impl Inner {
    fn lock(&self) -> VaultResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| VaultError::Storage(format!("connection mutex poisoned: {e}")))
    }
}

impl MetadataStore {
    /// Open or create an encrypted SQLite database at `path`.
    ///
    /// On a fresh database, schema migrations are applied automatically
    /// (idempotent — safe to call repeatedly).
    ///
    /// # Errors
    ///
    /// - [`VaultError::Storage`] if the path is unreachable, the key is
    ///   wrong (the verification query fails), or migrations fail.
    pub async fn open(path: impl AsRef<Path>, key: SqlCipherKey) -> VaultResult<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(&path, &key))
            .await
            .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    fn open_blocking(path: &Path, key: &SqlCipherKey) -> VaultResult<Self> {
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|e| VaultError::Storage(format!("open {}: {e}", path.display())))?;

        // SQLCipher: set the key BEFORE any other PRAGMA / query.
        // pragma_update with `key` accepts the raw passphrase; SQLCipher
        // derives the AES key internally via PBKDF2.
        conn.pragma_update(None, "key", key.as_str())
            .map_err(|e| VaultError::Storage(format!("set sqlcipher key: {e}")))?;

        // Verify the key by issuing a query that must succeed when the
        // database is correctly decrypted. Without a valid key this
        // returns "file is not a database" (or similar).
        conn.query_row("SELECT count(*) FROM sqlite_master", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|e| VaultError::Storage(format!("sqlcipher key verification failed: {e}")))?;

        // Sensible defaults: WAL for concurrent readers, foreign keys on,
        // synchronous = NORMAL (durable enough for a personal device DB).
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| VaultError::Storage(format!("enable WAL: {e}")))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| VaultError::Storage(format!("enable foreign_keys: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| VaultError::Storage(format!("set synchronous: {e}")))?;

        migrations::run(&mut conn)?;

        Ok(Self {
            inner: Arc::new(Inner {
                conn: Mutex::new(conn),
            }),
        })
    }

    /// Insert a new memory. Atomic with the corresponding audit event.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] if `memory.validate()` rejects
    /// - [`VaultError::Storage`] if the row already exists or insertion fails
    #[instrument(skip(self, memory), fields(memory_id = %memory.id))]
    pub async fn create_memory(&self, memory: &Memory) -> VaultResult<()> {
        memory.validate()?;
        let memory = memory.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            tx_insert_memory(&tx, &memory)?;
            tx_append_audit(
                &tx,
                PendingAuditEvent::success(AuditEventType::MemoryCreate, ActorKind::System)
                    .with_resource("memory", memory.id.to_string())
                    .with_boundary(memory.boundary.clone()),
            )?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Look up a memory by ID. Atomic with a `memory.read` audit event.
    ///
    /// Returns `Ok(None)` for a missing ID (still records a `Success` audit
    /// event with `result.found = false` so the access pattern remains visible).
    #[instrument(skip(self), fields(memory_id = %id))]
    pub async fn get_memory(&self, id: &MemoryId) -> VaultResult<Option<Memory>> {
        let id = *id;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let memory = tx_get_memory(&tx, &id)?;
            let mut pending =
                PendingAuditEvent::success(AuditEventType::MemoryRead, ActorKind::System)
                    .with_resource("memory", id.to_string());
            pending.details_json = format!(r#"{{"found":{}}}"#, memory.is_some());
            tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
            Ok(memory)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Replace an existing memory. Atomic with the corresponding audit event.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] if `memory.validate()` rejects
    /// - [`VaultError::NotFound`] if no memory with that ID exists
    /// - [`VaultError::Storage`] on transaction or row-update failure
    #[instrument(skip(self, memory), fields(memory_id = %memory.id))]
    pub async fn update_memory(&self, memory: &Memory) -> VaultResult<()> {
        memory.validate()?;
        let memory = memory.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let rows = tx_update_memory(&tx, &memory)?;
            if rows == 0 {
                return Err(VaultError::NotFound(format!(
                    "memory {} does not exist",
                    memory.id
                )));
            }
            tx_append_audit(
                &tx,
                PendingAuditEvent::success(AuditEventType::MemoryUpdate, ActorKind::System)
                    .with_resource("memory", memory.id.to_string())
                    .with_boundary(memory.boundary.clone()),
            )?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Delete a memory by ID. Atomic with the corresponding audit event.
    ///
    /// Idempotent: deleting a non-existent ID is a no-op success. The
    /// audit log records every call with `details.deleted = true|false`.
    #[instrument(skip(self), fields(memory_id = %id))]
    pub async fn delete_memory(&self, id: &MemoryId) -> VaultResult<()> {
        let id = *id;
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let rows = tx
                .execute(
                    "DELETE FROM memories WHERE id = ?1",
                    params![id.to_string()],
                )
                .map_err(|e| VaultError::Storage(format!("delete memory: {e}")))?;
            let mut pending =
                PendingAuditEvent::success(AuditEventType::MemoryDelete, ActorKind::System)
                    .with_resource("memory", id.to_string());
            pending.details_json = format!(r#"{{"deleted":{}}}"#, rows > 0);
            tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// List memories matching `filter`, capped at `limit` results.
    /// Always emits exactly one `memory.list` audit event regardless of
    /// how many rows match.
    #[instrument(skip(self), fields(limit = %limit))]
    pub async fn list_memories(
        &self,
        filter: MemoryFilter,
        limit: usize,
    ) -> VaultResult<Vec<Memory>> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let memories = tx_list_memories(&tx, &filter, limit)?;
            let mut pending =
                PendingAuditEvent::success(AuditEventType::MemoryList, ActorKind::System);
            if let Some(b) = filter.boundary.clone() {
                pending = pending.with_boundary(b);
            }
            pending.details_json = format!(r#"{{"count":{}}}"#, memories.len());
            tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
            Ok(memories)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Append a caller-supplied audit event. Used by other crates
    /// (vault-mcp, vault-sync, etc.) when they need to record their own
    /// security-relevant events.
    pub async fn append_audit_event(&self, pending: PendingAuditEvent) -> VaultResult<AuditEvent> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
            let event = tx_append_audit(&tx, pending)?;
            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))?;
            Ok(event)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Read the entire audit log up to `limit` entries, ordered by `seq`
    /// ascending (oldest first).
    pub async fn list_audit_events(&self, limit: usize) -> VaultResult<Vec<AuditEvent>> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock()?;
            let mut stmt = conn
                .prepare(AUDIT_SELECT_SQL)
                .map_err(|e| VaultError::Storage(format!("prepare audit select: {e}")))?;
            let rows = stmt
                .query_map(params![limit as i64], row_to_audit_event)
                .map_err(|e| VaultError::Storage(format!("query audit events: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| VaultError::Storage(format!("read audit row: {e}")))?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Walk the entire audit log and verify the BLAKE3 hash chain.
    /// Returns `Ok(())` for a healthy chain, or a [`VaultError::Storage`]
    /// pinpointing the first inconsistency.
    pub async fn verify_audit_chain(&self) -> VaultResult<()> {
        // Implementation note: we read the chain in pages so a giant log
        // doesn't have to fit in memory. For V0.1 a single fetch is fine.
        let events = self.list_audit_events(usize::MAX).await?;
        crate::audit::verify_chain(&events)
    }
}

// ---------------------------------------------------------------------------
// Transaction-bound helpers (sync). Centralised so the test harness can
// exercise them too if needed and so all SQL lives in one place.
// ---------------------------------------------------------------------------

fn tx_insert_memory(tx: &Transaction<'_>, m: &Memory) -> VaultResult<()> {
    tx.execute(
        "INSERT INTO memories (
            id, content, memory_type, source_agent, boundary,
            created_at, valid_from, valid_until,
            confidence, access_count, last_accessed,
            superseded_by, metadata_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            m.id.to_string(),
            m.content,
            memory_type_to_str(m.memory_type),
            m.source_agent,
            m.boundary.as_str(),
            m.created_at.to_rfc3339(),
            m.valid_from.to_rfc3339(),
            m.valid_until.map(|d| d.to_rfc3339()),
            m.confidence as f64,
            m.access_count as i64,
            m.last_accessed.to_rfc3339(),
            m.superseded_by.map(|id| id.to_string()),
            m.metadata.to_string(),
        ],
    )
    .map(|_| ())
    .map_err(|e| VaultError::Storage(format!("insert memory {}: {e}", m.id)))
}

fn tx_get_memory(tx: &Transaction<'_>, id: &MemoryId) -> VaultResult<Option<Memory>> {
    tx.query_row(MEMORY_SELECT_SQL, params![id.to_string()], row_to_memory)
        .optional()
        .map_err(|e| VaultError::Storage(format!("get memory {id}: {e}")))
}

fn tx_update_memory(tx: &Transaction<'_>, m: &Memory) -> VaultResult<usize> {
    tx.execute(
        "UPDATE memories SET
            content = ?2, memory_type = ?3, source_agent = ?4, boundary = ?5,
            created_at = ?6, valid_from = ?7, valid_until = ?8,
            confidence = ?9, access_count = ?10, last_accessed = ?11,
            superseded_by = ?12, metadata_json = ?13
         WHERE id = ?1",
        params![
            m.id.to_string(),
            m.content,
            memory_type_to_str(m.memory_type),
            m.source_agent,
            m.boundary.as_str(),
            m.created_at.to_rfc3339(),
            m.valid_from.to_rfc3339(),
            m.valid_until.map(|d| d.to_rfc3339()),
            m.confidence as f64,
            m.access_count as i64,
            m.last_accessed.to_rfc3339(),
            m.superseded_by.map(|id| id.to_string()),
            m.metadata.to_string(),
        ],
    )
    .map_err(|e| VaultError::Storage(format!("update memory {}: {e}", m.id)))
}

fn tx_list_memories(
    tx: &Transaction<'_>,
    filter: &MemoryFilter,
    limit: usize,
) -> VaultResult<Vec<Memory>> {
    // Build SQL dynamically — but keep WHERE pieces parameterised. We never
    // splice user data into the query string.
    let mut sql = String::from(
        "SELECT id, content, memory_type, source_agent, boundary,
                created_at, valid_from, valid_until,
                confidence, access_count, last_accessed,
                superseded_by, metadata_json
         FROM memories WHERE 1 = 1",
    );

    let mut bindings: Vec<rusqlite::types::Value> = Vec::new();

    if let Some(b) = &filter.boundary {
        sql.push_str(&format!(" AND boundary = ?{}", bindings.len() + 1));
        bindings.push(b.as_str().to_string().into());
    }
    if let Some(mt) = filter.memory_type {
        sql.push_str(&format!(" AND memory_type = ?{}", bindings.len() + 1));
        bindings.push(memory_type_to_str(mt).to_string().into());
    }
    if !filter.include_superseded {
        sql.push_str(" AND superseded_by IS NULL");
    }

    sql.push_str(&format!(
        " ORDER BY created_at DESC LIMIT ?{}",
        bindings.len() + 1
    ));
    bindings.push((limit as i64).into());

    let mut stmt = tx
        .prepare(&sql)
        .map_err(|e| VaultError::Storage(format!("prepare list memories: {e}")))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bindings.iter()), row_to_memory)
        .map_err(|e| VaultError::Storage(format!("query list memories: {e}")))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| VaultError::Storage(format!("read memory row: {e}")))?);
    }
    Ok(out)
}

fn tx_append_audit(tx: &Transaction<'_>, pending: PendingAuditEvent) -> VaultResult<AuditEvent> {
    let prev_hash: String = tx
        .query_row(
            "SELECT event_hash FROM audit_log ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| VaultError::Storage(format!("read chain tip: {e}")))?
        .unwrap_or_else(|| AUDIT_GENESIS_HASH.to_string());

    let event_id = Uuid::now_v7();
    let timestamp = Utc::now();
    let (_canonical, event_hash) = seal(event_id, timestamp, &prev_hash, &pending);

    tx.execute(
        "INSERT INTO audit_log (
            event_id, timestamp, user_id, device_id,
            event_type, resource_type, resource_id, boundary,
            actor_kind, actor_name, result, details_json,
            prev_event_hash, event_hash
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            event_id.hyphenated().to_string(),
            timestamp.to_rfc3339(),
            pending.user_id,
            pending.device_id,
            pending.event_type.as_str(),
            pending.resource_type,
            pending.resource_id,
            pending.boundary.as_ref().map(|b| b.as_str().to_string()),
            pending.actor_kind.as_str(),
            pending.actor_name,
            pending.result.as_str(),
            &pending.details_json,
            &prev_hash,
            &event_hash,
        ],
    )
    .map_err(|e| VaultError::Storage(format!("insert audit event: {e}")))?;

    let seq: i64 = tx.last_insert_rowid();
    debug!(seq, %event_id, "audit event recorded");

    Ok(AuditEvent {
        seq,
        event_id,
        timestamp,
        user_id: pending.user_id,
        device_id: pending.device_id,
        event_type: pending.event_type,
        resource_type: pending.resource_type,
        resource_id: pending.resource_id,
        boundary: pending.boundary.map(|b| b.into_inner()),
        actor_kind: pending.actor_kind,
        actor_name: pending.actor_name,
        result: pending.result,
        details_json: pending.details_json,
        prev_event_hash: prev_hash,
        event_hash,
    })
}

// ---------------------------------------------------------------------------
// Row decoders + enum/string helpers
// ---------------------------------------------------------------------------

const MEMORY_SELECT_SQL: &str = "SELECT id, content, memory_type, source_agent, boundary,
            created_at, valid_from, valid_until,
            confidence, access_count, last_accessed,
            superseded_by, metadata_json
         FROM memories WHERE id = ?1";

const AUDIT_SELECT_SQL: &str = "SELECT seq, event_id, timestamp, user_id, device_id,
            event_type, resource_type, resource_id, boundary,
            actor_kind, actor_name, result, details_json,
            prev_event_hash, event_hash
         FROM audit_log ORDER BY seq ASC LIMIT ?1";

fn row_to_memory(row: &Row<'_>) -> rusqlite::Result<Memory> {
    let id_s: String = row.get(0)?;
    let id = MemoryId(parse_uuid_field(&id_s, "memories.id")?);
    let memory_type_s: String = row.get(2)?;
    let memory_type = parse_memory_type(&memory_type_s)?;
    let boundary_s: String = row.get(4)?;
    let boundary = Boundary::new(boundary_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("memories.boundary: {e}"),
            )),
        )
    })?;
    let metadata_s: String = row.get(12)?;
    let metadata: serde_json::Value = serde_json::from_str(&metadata_s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(12, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let superseded_s: Option<String> = row.get(11)?;
    let superseded_by = superseded_s
        .map(|s| parse_uuid_field(&s, "memories.superseded_by").map(MemoryId))
        .transpose()?;

    let confidence: f64 = row.get(8)?;
    let access_count: i64 = row.get(9)?;

    Ok(Memory {
        id,
        content: row.get(1)?,
        memory_type,
        source_agent: row.get(3)?,
        boundary,
        created_at: row.get(5)?,
        valid_from: row.get(6)?,
        valid_until: row.get(7)?,
        confidence: confidence as f32,
        access_count: access_count as u32,
        last_accessed: row.get(10)?,
        superseded_by,
        embedding: None, // lives in LanceDB, not SQLite
        metadata,
    })
}

fn row_to_audit_event(row: &Row<'_>) -> rusqlite::Result<AuditEvent> {
    let event_id_s: String = row.get(1)?;
    let event_id = parse_uuid_field(&event_id_s, "audit_log.event_id")?;

    let event_type_s: String = row.get(5)?;
    let event_type = AuditEventType::parse(&event_type_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            5,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown audit event_type: {event_type_s}"),
            )),
        )
    })?;

    let actor_kind_s: String = row.get(9)?;
    let actor_kind = ActorKind::parse(&actor_kind_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            9,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown actor_kind: {actor_kind_s}"),
            )),
        )
    })?;

    let result_s: String = row.get(11)?;
    let result = AuditResult::parse(&result_s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            11,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown audit result: {result_s}"),
            )),
        )
    })?;

    Ok(AuditEvent {
        seq: row.get(0)?,
        event_id,
        timestamp: row.get(2)?,
        user_id: row.get(3)?,
        device_id: row.get(4)?,
        event_type,
        resource_type: row.get(6)?,
        resource_id: row.get(7)?,
        boundary: row.get(8)?,
        actor_kind,
        actor_name: row.get(10)?,
        result,
        details_json: row.get(12)?,
        prev_event_hash: row.get(13)?,
        event_hash: row.get(14)?,
    })
}

fn parse_uuid_field(s: &str, field: &str) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{field}: {e}"),
            )),
        )
    })
}

fn memory_type_to_str(mt: MemoryType) -> &'static str {
    match mt {
        MemoryType::Episodic => "episodic",
        MemoryType::Semantic => "semantic",
        MemoryType::Procedural => "procedural",
    }
}

fn parse_memory_type(s: &str) -> rusqlite::Result<MemoryType> {
    match s {
        "episodic" => Ok(MemoryType::Episodic),
        "semantic" => Ok(MemoryType::Semantic),
        "procedural" => Ok(MemoryType::Procedural),
        other => Err(rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("unknown memory_type: {other}"),
            )),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use tempfile::TempDir;
    use vault_core::NewMemory;

    /// Async test helper. Must be `.await`-ed — we never construct a fresh
    /// tokio Runtime here because that would panic when called from a
    /// `#[tokio::test]` (already inside a runtime) or from `tokio_test::block_on`
    /// (already inside a single-thread runtime).
    async fn make_store() -> (TempDir, MetadataStore) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");
        let key = SqlCipherKey::new("correct-horse-battery-staple-test-key");
        let store = MetadataStore::open(&path, key).await.unwrap();
        (tmp, store)
    }

    fn sample_memory(boundary: &str, mt: MemoryType, content: &str) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: mt,
            boundary: Boundary::new(boundary).unwrap(),
            source_agent: Some("test".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({"k": "v"}),
        })
        .unwrap()
    }

    #[tokio::test]
    async fn create_then_get_round_trips() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "hello");
        store.create_memory(&m).await.unwrap();
        let back = store.get_memory(&m.id).await.unwrap().unwrap();
        // embedding is intentionally None on read (lives in LanceDB)
        let mut expected = m.clone();
        expected.embedding = None;
        assert_eq!(back, expected);
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_tmp, store) = make_store().await;
        let id = MemoryId::new();
        let result = store.get_memory(&id).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn update_modifies_existing_record() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "v1");
        store.create_memory(&m).await.unwrap();

        let mut updated = m.clone();
        updated.content = "v2 — updated".into();
        updated.confidence = 0.5;
        store.update_memory(&updated).await.unwrap();

        let back = store.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.content, "v2 — updated");
        assert!((back.confidence - 0.5).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn update_missing_returns_not_found() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "ghost");
        let err = store.update_memory(&m).await.unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)));
    }

    #[tokio::test]
    async fn delete_removes_record_and_is_idempotent() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "doomed");
        store.create_memory(&m).await.unwrap();

        store.delete_memory(&m.id).await.unwrap();
        assert!(store.get_memory(&m.id).await.unwrap().is_none());

        // Second delete is a no-op success.
        store.delete_memory(&m.id).await.unwrap();
    }

    #[tokio::test]
    async fn invalid_memory_rejected_at_create() {
        let (_tmp, store) = make_store().await;
        let mut m = sample_memory("work", MemoryType::Semantic, "ok");
        m.confidence = 2.0; // invalid
        let err = store.create_memory(&m).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn list_with_no_filter_returns_all_active() {
        let (_tmp, store) = make_store().await;
        for i in 0..5 {
            let m = sample_memory("work", MemoryType::Semantic, &format!("mem-{i}"));
            store.create_memory(&m).await.unwrap();
        }
        let list = store
            .list_memories(MemoryFilter::default(), 100)
            .await
            .unwrap();
        assert_eq!(list.len(), 5);
    }

    #[tokio::test]
    async fn list_filters_by_boundary() {
        let (_tmp, store) = make_store().await;
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "w1"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("personal", MemoryType::Semantic, "p1"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("personal", MemoryType::Semantic, "p2"))
            .await
            .unwrap();

        let only_personal = store
            .list_memories(
                MemoryFilter {
                    boundary: Some(Boundary::new("personal").unwrap()),
                    ..Default::default()
                },
                100,
            )
            .await
            .unwrap();
        assert_eq!(only_personal.len(), 2);
        assert!(only_personal
            .iter()
            .all(|m| m.boundary.as_str() == "personal"));
    }

    #[tokio::test]
    async fn list_filters_by_memory_type() {
        let (_tmp, store) = make_store().await;
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "s"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Episodic, "e"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Procedural, "p"))
            .await
            .unwrap();

        let only_episodic = store
            .list_memories(
                MemoryFilter {
                    memory_type: Some(MemoryType::Episodic),
                    ..Default::default()
                },
                100,
            )
            .await
            .unwrap();
        assert_eq!(only_episodic.len(), 1);
        assert_eq!(only_episodic[0].memory_type, MemoryType::Episodic);
    }

    #[tokio::test]
    async fn list_excludes_superseded_by_default() {
        let (_tmp, store) = make_store().await;
        let parent = sample_memory("work", MemoryType::Semantic, "merged-into");
        store.create_memory(&parent).await.unwrap();

        let mut child = sample_memory("work", MemoryType::Semantic, "old-version");
        store.create_memory(&child).await.unwrap();
        child.superseded_by = Some(parent.id);
        store.update_memory(&child).await.unwrap();

        let default_list = store
            .list_memories(MemoryFilter::default(), 100)
            .await
            .unwrap();
        assert_eq!(default_list.len(), 1);
        assert_eq!(default_list[0].id, parent.id);

        let with_superseded = store
            .list_memories(
                MemoryFilter {
                    include_superseded: true,
                    ..Default::default()
                },
                100,
            )
            .await
            .unwrap();
        assert_eq!(with_superseded.len(), 2);
    }

    #[tokio::test]
    async fn list_respects_limit() {
        let (_tmp, store) = make_store().await;
        for i in 0..10 {
            let m = sample_memory("work", MemoryType::Semantic, &format!("m-{i}"));
            store.create_memory(&m).await.unwrap();
        }
        let limited = store
            .list_memories(MemoryFilter::default(), 3)
            .await
            .unwrap();
        assert_eq!(limited.len(), 3);
    }

    #[tokio::test]
    async fn audit_chain_is_maintained_across_operations() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "audited");
        store.create_memory(&m).await.unwrap();
        store.get_memory(&m.id).await.unwrap();
        store
            .update_memory(&{
                let mut x = m.clone();
                x.content = "v2".into();
                x
            })
            .await
            .unwrap();
        store.delete_memory(&m.id).await.unwrap();

        let events = store.list_audit_events(100).await.unwrap();
        // create, read, update, delete = 4 events
        assert_eq!(events.len(), 4);
        store.verify_audit_chain().await.unwrap();
    }

    #[tokio::test]
    async fn audit_chain_detects_tampering() {
        let (_tmp, store) = make_store().await;
        let m = sample_memory("work", MemoryType::Semantic, "trip-wire");
        store.create_memory(&m).await.unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "second"))
            .await
            .unwrap();

        // Reach into the DB directly and tamper with the boundary on the
        // first event. The chain should refuse to validate.
        {
            let inner = store.inner.clone();
            let conn = inner.lock().unwrap();
            conn.execute(
                "UPDATE audit_log SET boundary = 'tampered' WHERE seq = 1",
                [],
            )
            .unwrap();
        }

        let err = store.verify_audit_chain().await.unwrap_err();
        assert!(
            matches!(&err, VaultError::Storage(s) if s.contains("tampering detected")),
            "expected tampering detection, got {err:?}",
        );
    }

    #[tokio::test]
    async fn opening_with_wrong_key_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");

        // Create with the right key.
        let store = MetadataStore::open(&path, SqlCipherKey::new("right"))
            .await
            .unwrap();
        store
            .create_memory(&sample_memory("work", MemoryType::Semantic, "secret"))
            .await
            .unwrap();
        drop(store);

        // Reopen with the wrong key — must fail at the verification query.
        // Note: `MetadataStore` does not impl Debug (ADR-007), so we cannot
        // print `result` directly; use static descriptions in panic messages.
        let result = MetadataStore::open(&path, SqlCipherKey::new("wrong")).await;
        match result {
            Err(VaultError::Storage(_)) => {}
            Err(_) => panic!("expected VaultError::Storage from wrong-key open, got a different VaultError variant"),
            Ok(_) => panic!("expected wrong-key open to fail, got Ok"),
        }
    }

    #[tokio::test]
    async fn reopening_with_correct_key_preserves_data() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("vault.db");

        let store = MetadataStore::open(&path, SqlCipherKey::new("right"))
            .await
            .unwrap();
        let m = sample_memory("work", MemoryType::Semantic, "persistent");
        store.create_memory(&m).await.unwrap();
        drop(store);

        let store = MetadataStore::open(&path, SqlCipherKey::new("right"))
            .await
            .unwrap();
        let back = store.get_memory(&m.id).await.unwrap().unwrap();
        assert_eq!(back.content, "persistent");
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        #[test]
        fn round_trip_integrity(
            content in "[a-zA-Z0-9 _.-]{1,200}",
            confidence in 0.0f32..=1.0,
            boundary_name in "[a-zA-Z0-9_-]{1,32}",
        ) {
            tokio_test::block_on(async {
                let (_tmp, store) = make_store().await;
                let m = Memory::try_new(NewMemory {
                    content: content.clone(),
                    memory_type: MemoryType::Semantic,
                    boundary: Boundary::new(boundary_name).unwrap(),
                    source_agent: None,
                    confidence,
                    valid_from: None,
                    valid_until: None,
                    metadata: serde_json::json!({}),
                }).unwrap();
                store.create_memory(&m).await.unwrap();
                let back = store.get_memory(&m.id).await.unwrap().unwrap();
                prop_assert_eq!(back.content, content);
                prop_assert!((back.confidence - confidence).abs() < 1e-6);
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }
    }

    proptest! {
        // Adversarial: for any chain of length N, tampering with any single
        // event breaks verify_audit_chain on that prefix; restoring the
        // original value re-validates the chain. This is the property-test
        // form of "the audit chain catches every tamper, every time."
        // Lower case count because each iteration creates a fresh on-disk
        // SQLCipher DB (slow due to PBKDF2).
        #![proptest_config(ProptestConfig { cases: 8, ..ProptestConfig::default() })]

        #[test]
        fn tampering_breaks_chain_at_every_seq(n_events in 2usize..6) {
            tokio_test::block_on(async {
                let (_tmp, store) = make_store().await;

                for i in 0..n_events {
                    let pending = PendingAuditEvent::success(
                        AuditEventType::MemoryRead,
                        ActorKind::System,
                    )
                    .with_resource("memory", format!("res-{i}"))
                    .with_boundary(Boundary::new(format!("b-{i}")).unwrap());
                    store.append_audit_event(pending).await.unwrap();
                }
                store.verify_audit_chain().await.unwrap();

                for tamper_seq in 1..=(n_events as i64) {
                    // Save the current value, tamper, expect verification to fail.
                    let inner = store.inner.clone();
                    let original: Option<String> = {
                        let conn = inner.lock().unwrap();
                        let v: Option<String> = conn
                            .query_row(
                                "SELECT boundary FROM audit_log WHERE seq = ?1",
                                rusqlite::params![tamper_seq],
                                |row| row.get(0),
                            )
                            .unwrap();
                        conn.execute(
                            "UPDATE audit_log SET boundary = 'tampered-by-test' WHERE seq = ?1",
                            rusqlite::params![tamper_seq],
                        )
                        .unwrap();
                        v
                    };

                    let err = store.verify_audit_chain().await.unwrap_err();
                    prop_assert!(
                        matches!(&err, VaultError::Storage(s) if s.contains("tampering detected")),
                        "expected tamper at seq {} to break chain, got {:?}",
                        tamper_seq, err,
                    );

                    // Restore original value, verify chain heals.
                    {
                        let conn = inner.lock().unwrap();
                        conn.execute(
                            "UPDATE audit_log SET boundary = ?2 WHERE seq = ?1",
                            rusqlite::params![tamper_seq, original],
                        )
                        .unwrap();
                    }
                    store.verify_audit_chain().await.unwrap();
                }
                Ok::<(), proptest::test_runner::TestCaseError>(())
            })?;
        }
    }

    #[tokio::test]
    async fn concurrent_writes_all_succeed_and_chain_stays_valid() {
        // 20 tasks concurrently create memories. The Mutex<Connection>
        // serialises them at the storage layer; we want to verify that
        // (a) every task succeeds, (b) every memory is retrievable,
        // (c) the audit chain validates after the dust settles.
        let (_tmp, store) = make_store().await;
        let n: u32 = 20;
        let mut handles = Vec::new();
        for i in 0..n {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                let m = sample_memory("work", MemoryType::Semantic, &format!("c-{i}"));
                store.create_memory(&m).await.map(|()| m.id)
            }));
        }
        let mut ids = Vec::new();
        for h in handles {
            ids.push(h.await.unwrap().unwrap());
        }
        assert_eq!(ids.len(), n as usize);

        // All retrievable.
        for id in &ids {
            assert!(store.get_memory(id).await.unwrap().is_some());
        }

        // Chain still valid (each create + each get appended an audit event).
        store.verify_audit_chain().await.unwrap();

        // Total audit events: n creates + n reads = 2n.
        let events = store.list_audit_events(1000).await.unwrap();
        assert_eq!(events.len() as u32, n * 2);
    }

    /// Honest perf measurement, reported via eprintln for visibility.
    /// Does NOT assert a tight 50ms budget — SQLCipher's default PBKDF2
    /// (256,000 SHA-512 iterations) makes first-open inherently 100-300ms
    /// on a modern CPU. That cost is *intentional* (brute-force resistance).
    /// Steady-state operations should be sub-millisecond.
    /// Asserts only a generous regression bound.
    #[tokio::test]
    async fn perf_budget_open_migrate_first_audit() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("perf.db");
        let key = SqlCipherKey::new("perf-test-key-not-secret");

        let t0 = std::time::Instant::now();
        let store = MetadataStore::open(&path, key.clone()).await.unwrap();
        let after_open = t0.elapsed();

        let pending =
            PendingAuditEvent::success(AuditEventType::SchemaMigration, ActorKind::System);
        store.append_audit_event(pending).await.unwrap();
        let after_first_audit = t0.elapsed();

        // Steady-state — second audit insert should be quick.
        let t_steady_start = std::time::Instant::now();
        store
            .append_audit_event(PendingAuditEvent::success(
                AuditEventType::MemoryRead,
                ActorKind::System,
            ))
            .await
            .unwrap();
        let steady_state_audit = t_steady_start.elapsed();

        eprintln!(
            "[perf] open+migrate={:?}  +first_audit={:?}  steady_audit={:?}",
            after_open, after_first_audit, steady_state_audit,
        );

        // Generous regression bound — anything over 5s is a real bug.
        // The 50ms target Shahbaz set is reported in HANDOFF.md alongside
        // these measured numbers so we can decide together whether to
        // tune kdf_iter (security trade-off) or accept the cost.
        assert!(
            after_first_audit.as_secs() < 5,
            "perf regression: open+migrate+first_audit took {:?}",
            after_first_audit,
        );
        assert!(
            steady_state_audit.as_millis() < 200,
            "steady-state audit insert took {:?} — expected sub-100ms",
            steady_state_audit,
        );
    }
}
