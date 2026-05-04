//! [`DuckDbGraphStore`] — DuckDB-backed knowledge-graph store for entities
//! and bi-temporal relationships (BRD §5.2; HANDOFF.md ADR-015).
//!
//! ## Boundary scoping (ADR-015)
//!
//! Entities and relationships are boundary-scoped at the schema layer.
//! `entities.boundary` is `NOT NULL` and part of a composite UNIQUE
//! constraint with `(name, entity_type)` — the same name in two different
//! boundaries is two distinct entities. Relationships carry a denormalised
//! `boundary` column for fast traversal-time SQL filtering.
//!
//! All traversal queries take a mandatory `authorized_boundaries: &[Boundary]`
//! parameter (non-`Option`, mirroring `LanceVectorStore::search` from T0.1.4
//! — empty slice returns empty result, not error: compile-time impossible
//! to "forget to filter").
//!
//! Cross-boundary relationships are forbidden except `relation_type IN
//! ('same_as', 'alias_for')`. The invariant is **app-layer enforced**
//! inside [`DuckDbGraphStore::create_relationship`] — DuckDB 1.x supports
//! neither subquery-CHECK nor triggers, and CHECK constraints are per-row
//! only. The property test in `mod tests` is the SQL-layer backstop's
//! substitute. See HANDOFF.md ADR-015 for the full reasoning.
//!
//! ## V0.1 scope
//!
//! `same_as` / `alias_for` rows: schema permits them (the within-boundary
//! invariant exempts these relation types), no V0.1 API path creates them.
//! [`GraphStore::traverse`] takes a `follow_aliases: bool` (in
//! [`TraversalOptions`]) for forward compatibility — for V0.1 callers
//! always pass `false`; T0.2.x consolidator + UI light up the `true` path.
//!
//! ## Concurrency
//!
//! `duckdb::Connection: Send + !Sync`. We wrap a single connection in
//! `std::sync::Mutex` and run all DB work inside `tokio::task::spawn_blocking`
//! — same pattern as [`crate::MetadataStore`].
//!
//! ## Encryption-at-rest
//!
//! V0.1 stores plaintext on disk. Same posture and compensating controls as
//! `LanceVectorStore` per ADR-010. ADR-010 + T0.2.0 must extend to cover
//! DuckDB before V0.2 ships (tracked in HANDOFF.md In Progress).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use duckdb::{params, Connection, OptionalExt};
use tracing::{instrument, warn};
use uuid::Uuid;

use vault_core::{
    Boundary, Entity, EntityId, EntityType, Relationship, RelationshipId, VaultError, VaultResult,
};

use crate::migrations_graph;
use crate::vector_store::{build_boundary_filter, quote_sql_string};

/// Relation types whose endpoints may legally span boundaries (ADR-015).
/// V0.1: schema permits these rows but no API path creates them.
pub const CROSS_BOUNDARY_RELATION_TYPES: &[&str] = &["same_as", "alias_for"];

/// Configurable knobs for [`GraphStore::traverse`].
///
/// Mandatory parameters (`from`, `authorized_boundaries`) stay positional on
/// the trait method — they are security-critical and should be hard to
/// forget. Everything else is grouped here so the trait signature stays
/// stable across V0.1 → V0.2+ as new options land (e.g., `include_archived`,
/// time-range filters). Add fields here, not parameters to `traverse`.
///
/// Intentionally NOT `Default` — every field is meaningful and callers
/// should be explicit. Cheap to construct.
#[derive(Clone, Debug)]
pub struct TraversalOptions {
    /// Walk 1..=`max_hops` outgoing hops. Per BRD §6 V0.1, supported range
    /// is 1–3 for V0.1; the implementation does not impose a hard upper
    /// bound but performance degrades quadratically.
    pub max_hops: usize,

    /// If `Some`, restrict traversal to relationships whose `relation_type`
    /// matches. Exact match, not pattern.
    pub relation_filter: Option<String>,

    /// Forward-compat for `same_as` / `alias_for` (ADR-015). For V0.1
    /// pass `false` always — `same_as` rows aren't created in V0.1, but
    /// the parameter is on the trait so T0.2.x consolidator + UI can
    /// light it up without a trait change. When `true`, `same_as` /
    /// `alias_for` edges are followed; the destination entity must still
    /// be inside `authorized_boundaries` (alias is not privilege escalation).
    pub follow_aliases: bool,
}

/// Trait abstraction over the graph store (BRD §2.2 — depend on traits, not
/// implementations). Retrieval, consolidator, and the cascading
/// [`crate::StorageBackend`] (T0.1.6) consume this trait.
#[async_trait]
pub trait GraphStore: Send + Sync {
    /// Insert a new entity. Returns
    /// [`VaultError::Storage`] (with a duplicate-key message) if an entity
    /// with the same `(name, entity_type, boundary)` already exists.
    ///
    /// Validates `entity` at the API boundary (BRD §11.7.1).
    async fn create_entity(&self, entity: &Entity) -> VaultResult<()>;

    /// Insert a relationship.
    ///
    /// Returns [`VaultError::AccessDenied`] (with a CRR-violation message)
    /// when both endpoints are not in the same boundary AND `relation_type`
    /// is not in [`CROSS_BOUNDARY_RELATION_TYPES`] (ADR-015).
    ///
    /// Returns [`VaultError::NotFound`] when either endpoint does not exist.
    async fn create_relationship(&self, rel: &Relationship) -> VaultResult<()>;

    /// Walk outgoing hops from `from`, returning every reachable entity
    /// together with the relationship sequence that led there (one path
    /// per reachable entity — the **shortest** path, with cycles broken).
    ///
    /// Boundary access control is mandatory: only entities with a
    /// `boundary` in `authorized_boundaries` are returned, and only
    /// relationships whose `boundary` is in `authorized_boundaries` are
    /// followed. Empty `authorized_boundaries` returns an empty result.
    ///
    /// See [`TraversalOptions`] for the configurable knobs.
    async fn traverse(
        &self,
        from: &EntityId,
        authorized_boundaries: &[Boundary],
        options: TraversalOptions,
    ) -> VaultResult<Vec<(Entity, Vec<Relationship>)>>;

    /// Replace an existing relationship: set `old_id`'s `valid_until` to
    /// `new_rel.valid_from`, then insert `new_rel`. Atomic.
    ///
    /// Returns [`VaultError::NotFound`] if `old_id` does not exist.
    /// Returns [`VaultError::AccessDenied`] for the same cross-boundary
    /// rule applied to `new_rel` as in `create_relationship`.
    async fn supersede_relationship(
        &self,
        old_id: &RelationshipId,
        new_rel: &Relationship,
    ) -> VaultResult<()>;

    /// Eager validation that the store is readable end-to-end (not merely
    /// open-able). Used by `StorageBackend::open` (T0.1.6 Phase C, ADR-018)
    /// to surface hard fragment corruption immediately.
    ///
    /// **Same load-bearing contract as
    /// [`crate::VectorStore::validate_readable`]:** minimum-cost end-to-end
    /// read that exercises data-decode. **Not** metadata-only. The
    /// recommended shape is `SELECT id FROM entities ORDER BY id ASC LIMIT 1`
    /// — deterministic, cheap, exercises the full row decode path. Empty
    /// store validates vacuously (no rows, nothing to decode).
    ///
    /// Returns `Ok(())` on a clean / empty store; `Err` on corruption with
    /// the underlying decode error. The orchestrator translates `Err` into
    /// a CRITICAL audit event + degraded-mode flag, not a hard `open()`
    /// failure (ADR-010 / Phase A Change 1).
    async fn validate_readable(&self) -> VaultResult<()>;
}

/// DuckDB-backed [`GraphStore`] implementation.
///
/// Cheap to clone (it holds an `Arc` internally); share freely across tasks.
///
/// Intentionally does **not** implement `Debug`: same posture as
/// [`crate::MetadataStore`] (ADR-007) — types holding live DB connections
/// don't get a stub `Debug` impl.
#[derive(Clone)]
pub struct DuckDbGraphStore {
    inner: Arc<Inner>,
}

struct Inner {
    conn: Mutex<Connection>,
}

impl Inner {
    fn lock(&self) -> VaultResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|e| VaultError::Storage(format!("graph connection mutex poisoned: {e}")))
    }
}

impl DuckDbGraphStore {
    /// Open or create a DuckDB graph database at `path`.
    ///
    /// On first open, schema migrations are applied automatically
    /// (idempotent — safe to call repeatedly). The startup WARN log fires
    /// unconditionally per the ADR-010 plaintext-on-disk compensating
    /// control extended to DuckDB.
    ///
    /// # Errors
    ///
    /// - [`VaultError::Storage`] if the path is unreachable or migrations fail.
    pub async fn open(path: impl AsRef<Path>) -> VaultResult<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(&path))
            .await
            .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    fn open_blocking(path: &Path) -> VaultResult<Self> {
        let mut conn = Connection::open(path)
            .map_err(|e| VaultError::Storage(format!("open duckdb {}: {e}", path.display())))?;

        migrations_graph::run(&mut conn)?;

        // ADR-010 (extended to DuckDB per HANDOFF.md In Progress): unconditional
        // startup WARN that the graph data dir is plaintext until T0.2.0 ships.
        warn!(
            data_dir = %path.display(),
            "DuckDB graph store data is PLAINTEXT on disk (V0.1 alpha — see ADR-010). \
             Encryption layer ships in T0.2.0."
        );

        Ok(Self {
            inner: Arc::new(Inner {
                conn: Mutex::new(conn),
            }),
        })
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Encode an [`EntityType`] as a JSON string for the `entities.entity_type`
/// column. Round-trips via [`entity_type_from_text`].
fn entity_type_to_text(et: &EntityType) -> VaultResult<String> {
    serde_json::to_string(et)
        .map_err(|e| VaultError::Storage(format!("serialise entity_type: {e}")))
}

fn entity_type_from_text(s: &str) -> VaultResult<EntityType> {
    serde_json::from_str(s)
        .map_err(|e| VaultError::Storage(format!("deserialise entity_type {s:?}: {e}")))
}

fn uuid_from_blob(bytes: &[u8]) -> VaultResult<Uuid> {
    Uuid::from_slice(bytes)
        .map_err(|e| VaultError::Storage(format!("invalid UUID bytes (len {}): {e}", bytes.len())))
}

fn datetime_from_text(s: &str) -> VaultResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| VaultError::Storage(format!("invalid RFC3339 timestamp {s:?}: {e}")))
}

fn boundary_from_text(s: &str) -> VaultResult<Boundary> {
    Boundary::new(s).map_err(|e| {
        VaultError::Storage(format!(
            "stored boundary {s:?} fails Boundary::new validation: {e}"
        ))
    })
}

struct RawEntity {
    id: Vec<u8>,
    name: String,
    entity_type: String,
    boundary: String,
    created_at: String,
}

impl RawEntity {
    fn try_into_entity(self) -> VaultResult<Entity> {
        Ok(Entity {
            id: EntityId(uuid_from_blob(&self.id)?),
            name: self.name,
            entity_type: entity_type_from_text(&self.entity_type)?,
            boundary: boundary_from_text(&self.boundary)?,
            created_at: datetime_from_text(&self.created_at)?,
        })
    }
}

/// Reconstruct a [`Relationship`] from the canonical column order:
/// `(id, from_entity_id, to_entity_id, relation_type, valid_from, valid_until, confidence)`.
/// Note: the `boundary` column is NOT part of the [`Relationship`] domain
/// type (it's a denormalised storage detail — see migrations_graph/0001).
fn row_to_relationship(row: &duckdb::Row<'_>) -> duckdb::Result<RawRelationship> {
    Ok(RawRelationship {
        id: row.get::<_, Vec<u8>>(0)?,
        from_entity_id: row.get::<_, Vec<u8>>(1)?,
        to_entity_id: row.get::<_, Vec<u8>>(2)?,
        relation_type: row.get(3)?,
        valid_from: row.get(4)?,
        valid_until: row.get(5)?,
        confidence: row.get(6)?,
    })
}

struct RawRelationship {
    id: Vec<u8>,
    from_entity_id: Vec<u8>,
    to_entity_id: Vec<u8>,
    relation_type: String,
    valid_from: String,
    valid_until: Option<String>,
    confidence: f64,
}

impl RawRelationship {
    fn try_into_relationship(self) -> VaultResult<Relationship> {
        Ok(Relationship {
            id: RelationshipId(uuid_from_blob(&self.id)?),
            from_entity: EntityId(uuid_from_blob(&self.from_entity_id)?),
            to_entity: EntityId(uuid_from_blob(&self.to_entity_id)?),
            relation_type: self.relation_type,
            valid_from: datetime_from_text(&self.valid_from)?,
            valid_until: self
                .valid_until
                .map(|s| datetime_from_text(&s))
                .transpose()?,
            confidence: self.confidence as f32,
        })
    }
}

/// Detect a duplicate-key error from DuckDB's stringly-typed error messages.
/// DuckDB does not expose a structured error code for unique violations in
/// the duckdb-rs 1.x bindings; pattern-match on the message instead.
fn is_duplicate_key_error(e: &duckdb::Error) -> bool {
    let msg = e.to_string();
    msg.contains("Duplicate")
        || msg.contains("UNIQUE")
        || msg.contains("violates unique constraint")
}

// =============================================================================
// GraphStore impl
// =============================================================================

#[async_trait]
impl GraphStore for DuckDbGraphStore {
    #[instrument(
        skip(self, entity),
        fields(entity_id = %entity.id, boundary = entity.boundary.as_str()),
    )]
    async fn create_entity(&self, entity: &Entity) -> VaultResult<()> {
        entity.validate()?;
        let entity = entity.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let entity_type_json = entity_type_to_text(&entity.entity_type)?;

            // Pre-flight UNIQUE check inside an explicit transaction.
            // Empirically, DuckDB 1.2.2's autocommit INSERT can wedge the
            // connection on UNIQUE constraint violation — pre-checking + an
            // explicit tx avoids the wedge and gives us a deterministic
            // duplicate-key error path. The pre-check + insert is atomic
            // because we hold the connection mutex for the whole tx.
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;

            let dup_count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM entities \
                     WHERE name = ? AND entity_type = ? AND boundary = ?",
                    params![&entity.name, &entity_type_json, entity.boundary.as_str()],
                    |row| row.get(0),
                )
                .map_err(|e| VaultError::Storage(format!("create_entity dup-check: {e}")))?;
            if dup_count > 0 {
                return Err(VaultError::Storage(format!(
                    "entity already exists with same (name, entity_type, boundary): \
                     {}/{}/{}",
                    entity.name,
                    entity_type_json,
                    entity.boundary.as_str(),
                )));
            }

            tx.execute(
                "INSERT INTO entities (id, name, entity_type, boundary, created_at) \
                 VALUES (?, ?, ?, ?, ?)",
                params![
                    entity.id.0.as_bytes().to_vec(),
                    entity.name,
                    entity_type_json,
                    entity.boundary.as_str(),
                    entity.created_at.to_rfc3339(),
                ],
            )
            .map_err(|e| {
                if is_duplicate_key_error(&e) {
                    VaultError::Storage(format!(
                        "entity already exists with same (name, entity_type, boundary): {e}"
                    ))
                } else {
                    VaultError::Storage(format!("create_entity: {e}"))
                }
            })?;

            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    #[instrument(skip(self, rel), fields(rel_id = %rel.id))]
    async fn create_relationship(&self, rel: &Relationship) -> VaultResult<()> {
        rel.validate()?;
        let rel = rel.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;

            let from_boundary =
                lookup_entity_boundary(&tx, &rel.from_entity)?.ok_or_else(|| {
                    VaultError::NotFound(format!("from_entity {} does not exist", rel.from_entity))
                })?;
            let to_boundary = lookup_entity_boundary(&tx, &rel.to_entity)?.ok_or_else(|| {
                VaultError::NotFound(format!("to_entity {} does not exist", rel.to_entity))
            })?;

            // ADR-015: cross-boundary forbidden unless same_as / alias_for.
            let is_cross_boundary = from_boundary != to_boundary;
            let is_alias = CROSS_BOUNDARY_RELATION_TYPES.contains(&rel.relation_type.as_str());
            if is_cross_boundary && !is_alias {
                return Err(VaultError::AccessDenied(format!(
                    "cross-boundary relationship with relation_type {:?} is forbidden \
                     — only {:?} may span boundaries (ADR-015)",
                    rel.relation_type, CROSS_BOUNDARY_RELATION_TYPES,
                )));
            }

            // Denormalised boundary: from-side endpoint (asymmetric for
            // alias rows, consistent for within-boundary rows).
            let denorm_boundary = from_boundary.as_str();

            tx.execute(
                "INSERT INTO relationships \
                 (id, from_entity_id, to_entity_id, relation_type, boundary, \
                  valid_from, valid_until, confidence) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    rel.id.0.as_bytes().to_vec(),
                    rel.from_entity.0.as_bytes().to_vec(),
                    rel.to_entity.0.as_bytes().to_vec(),
                    rel.relation_type,
                    denorm_boundary,
                    rel.valid_from.to_rfc3339(),
                    rel.valid_until.map(|d| d.to_rfc3339()),
                    rel.confidence as f64,
                ],
            )
            .map_err(|e| VaultError::Storage(format!("create_relationship insert: {e}")))?;

            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    #[instrument(
        skip(self, authorized_boundaries, options),
        fields(
            from = %from,
            max_hops = options.max_hops,
            follow_aliases = options.follow_aliases,
        ),
    )]
    async fn traverse(
        &self,
        from: &EntityId,
        authorized_boundaries: &[Boundary],
        options: TraversalOptions,
    ) -> VaultResult<Vec<(Entity, Vec<Relationship>)>> {
        // Compile-time-impossible-to-forget access control: empty list short-circuits.
        if authorized_boundaries.is_empty() || options.max_hops == 0 {
            return Ok(Vec::new());
        }

        let from = *from;
        let auth = authorized_boundaries.to_vec();
        let inner = self.inner.clone();

        tokio::task::spawn_blocking(move || traverse_blocking(&inner, &from, &auth, &options))
            .await
            .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    #[instrument(skip(self, new_rel), fields(old_id = %old_id, new_id = %new_rel.id))]
    async fn supersede_relationship(
        &self,
        old_id: &RelationshipId,
        new_rel: &Relationship,
    ) -> VaultResult<()> {
        new_rel.validate()?;
        let old_id = *old_id;
        let new_rel = new_rel.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock()?;
            let tx = conn
                .transaction()
                .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;

            // 1) Verify old exists, capture its current valid_until (must be
            //    NULL — we don't double-supersede).
            let old_exists: Option<Option<String>> = tx
                .query_row(
                    "SELECT valid_until FROM relationships WHERE id = ?",
                    params![old_id.0.as_bytes().to_vec()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| {
                    VaultError::Storage(format!("supersede lookup old relationship: {e}"))
                })?;
            let old_valid_until = match old_exists {
                None => {
                    return Err(VaultError::NotFound(format!(
                        "relationship {old_id} does not exist"
                    )))
                }
                Some(v) => v,
            };
            if old_valid_until.is_some() {
                return Err(VaultError::Storage(format!(
                    "relationship {old_id} is already superseded (valid_until is set)"
                )));
            }

            // 2) Apply the same cross-boundary rule to new_rel.
            let from_boundary =
                lookup_entity_boundary(&tx, &new_rel.from_entity)?.ok_or_else(|| {
                    VaultError::NotFound(format!(
                        "from_entity {} does not exist",
                        new_rel.from_entity
                    ))
                })?;
            let to_boundary =
                lookup_entity_boundary(&tx, &new_rel.to_entity)?.ok_or_else(|| {
                    VaultError::NotFound(format!("to_entity {} does not exist", new_rel.to_entity))
                })?;
            let is_cross_boundary = from_boundary != to_boundary;
            let is_alias = CROSS_BOUNDARY_RELATION_TYPES.contains(&new_rel.relation_type.as_str());
            if is_cross_boundary && !is_alias {
                return Err(VaultError::AccessDenied(format!(
                    "cross-boundary relationship with relation_type {:?} is forbidden \
                     — only {:?} may span boundaries (ADR-015)",
                    new_rel.relation_type, CROSS_BOUNDARY_RELATION_TYPES,
                )));
            }

            // 3) Set old's valid_until = new.valid_from. Clamp to old.valid_from
            //    so we never violate the CHECK (valid_until >= valid_from).
            tx.execute(
                "UPDATE relationships SET valid_until = ? \
                 WHERE id = ? AND (valid_from <= ?)",
                params![
                    new_rel.valid_from.to_rfc3339(),
                    old_id.0.as_bytes().to_vec(),
                    new_rel.valid_from.to_rfc3339(),
                ],
            )
            .map_err(|e| VaultError::Storage(format!("supersede update old: {e}")))?;

            // Verify the update applied (would fail if old.valid_from > new.valid_from).
            let updated_check: Option<String> = tx
                .query_row(
                    "SELECT valid_until FROM relationships WHERE id = ?",
                    params![old_id.0.as_bytes().to_vec()],
                    |row| row.get(0),
                )
                .map_err(|e| VaultError::Storage(format!("supersede verify: {e}")))?;
            if updated_check.is_none() {
                return Err(VaultError::Storage(format!(
                    "supersede refused: new_rel.valid_from {} precedes old's valid_from",
                    new_rel.valid_from.to_rfc3339(),
                )));
            }

            // 4) Insert the new relationship.
            tx.execute(
                "INSERT INTO relationships \
                 (id, from_entity_id, to_entity_id, relation_type, boundary, \
                  valid_from, valid_until, confidence) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    new_rel.id.0.as_bytes().to_vec(),
                    new_rel.from_entity.0.as_bytes().to_vec(),
                    new_rel.to_entity.0.as_bytes().to_vec(),
                    new_rel.relation_type,
                    from_boundary.as_str(),
                    new_rel.valid_from.to_rfc3339(),
                    new_rel.valid_until.map(|d| d.to_rfc3339()),
                    new_rel.confidence as f64,
                ],
            )
            .map_err(|e| VaultError::Storage(format!("supersede insert new: {e}")))?;

            tx.commit()
                .map_err(|e| VaultError::Storage(format!("commit: {e}")))
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }

    /// Per the trait contract: minimum-cost end-to-end read that exercises
    /// the data-decode path (NOT metadata-only). Reads the smallest-id
    /// row's `id` column. Empty tables validate vacuously via
    /// `query_row(...).optional()`.
    ///
    /// Same load-bearing rationale as
    /// [`crate::VectorStore::validate_readable`]: corrupting `entities`
    /// row data on disk must surface here, not silently pass on a
    /// metadata-only check. The `ORDER BY id ASC LIMIT 1` shape forces
    /// an actual row decode (the `id` BLOB column is read into
    /// `Vec<u8>`).
    #[instrument(skip(self))]
    async fn validate_readable(&self) -> VaultResult<()> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> VaultResult<()> {
            let conn = inner.lock()?;
            let _row: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT id FROM entities ORDER BY id ASC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|e| VaultError::Storage(format!("validate_readable read: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| VaultError::Storage(format!("spawn_blocking join: {e}")))?
    }
}

// =============================================================================
// Lookup + traversal helpers (sync / blocking)
// =============================================================================

/// Look up the boundary of an entity by ID. Returns `Ok(None)` if the
/// entity does not exist. Works against both [`duckdb::Connection`] and
/// [`duckdb::Transaction`] because the latter derefs to the former.
fn lookup_entity_boundary(
    conn: &duckdb::Connection,
    id: &EntityId,
) -> VaultResult<Option<Boundary>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT boundary FROM entities WHERE id = ?",
            params![id.0.as_bytes().to_vec()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| VaultError::Storage(format!("lookup entity boundary: {e}")))?;
    raw.map(|s| boundary_from_text(&s)).transpose()
}

/// Run the traversal blocking-style (called from `spawn_blocking`).
fn traverse_blocking(
    inner: &Inner,
    from: &EntityId,
    authorized: &[Boundary],
    options: &TraversalOptions,
) -> VaultResult<Vec<(Entity, Vec<Relationship>)>> {
    let conn = inner.lock()?;

    // Build the SQL fragments. `Boundary` validation guarantees no
    // single-quotes in the values; `quote_sql_string` is the defense-in-
    // depth half (see vector_store ADR commentary).
    let r_boundary_filter = format!("r.{}", build_boundary_filter(authorized));
    let e_boundary_filter = format!("e.{}", build_boundary_filter(authorized));

    let alias_guard = if options.follow_aliases {
        String::new()
    } else {
        format!(
            " AND r.relation_type NOT IN ({}, {})",
            quote_sql_string("same_as"),
            quote_sql_string("alias_for"),
        )
    };

    let relation_filter_sql = if let Some(rt) = &options.relation_filter {
        format!(" AND r.relation_type = {}", quote_sql_string(rt))
    } else {
        String::new()
    };

    // Recursive CTE traversal:
    //   * `entity_path` — the visited-entity list, used for cycle break
    //     via `list_position`. Initialised with the start entity to prevent
    //     immediate self-cycles too.
    //   * `rel_path` — the relationship-id list along this path, returned
    //     so Rust can rehydrate the full Vec<Relationship>.
    //   * `depth` — bounded by `max_hops` in the recursive WHERE clause.
    //
    // Defense in depth (ADR-015 watch-item #1): boundary filter applies on
    // BOTH the anchor and the recursive step, plus on the final entity
    // join. Any one of those three would suffice; all three together is
    // explicit and audit-friendly.
    let sql = format!(
        "WITH RECURSIVE walk AS (
            SELECT
                r.to_entity_id              AS to_id,
                1                           AS depth,
                [r.id]                      AS rel_path,
                [r.from_entity_id, r.to_entity_id] AS entity_path
            FROM relationships r
            WHERE r.from_entity_id = ?
              AND {r_boundary_filter}
              AND (r.valid_until IS NULL){relation_filter_sql}{alias_guard}
            UNION ALL
            SELECT
                r.to_entity_id,
                w.depth + 1,
                list_append(w.rel_path, r.id),
                list_append(w.entity_path, r.to_entity_id)
            FROM relationships r
            JOIN walk w ON r.from_entity_id = w.to_id
            WHERE w.depth < ?
              AND {r_boundary_filter}
              AND (r.valid_until IS NULL){relation_filter_sql}{alias_guard}
              AND list_position(w.entity_path, r.to_entity_id) IS NULL
        )
        SELECT
            w.to_id, w.depth, w.rel_path,
            e.id, e.name, e.entity_type, e.boundary, e.created_at
        FROM walk w
        JOIN entities e ON e.id = w.to_id
        WHERE {e_boundary_filter}
        ORDER BY w.depth ASC",
    );

    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| VaultError::Storage(format!("prepare traversal: {e}")))?;

    let max_hops_i64 = options.max_hops as i64;
    let from_blob = from.0.as_bytes().to_vec();

    // First pass: pull rows out as raw `duckdb::types::Value` for the
    // BLOB[] path column (duckdb-rs 1.2 doesn't impl `FromSql` for
    // `Vec<Vec<u8>>` — only the scalar `Vec<u8>` for BLOB). We unpack the
    // list manually outside the closure so we can return VaultError on a
    // schema-shape mismatch.
    struct WalkRowRaw {
        depth: i64,
        rel_path_value: duckdb::types::Value,
        raw_entity: RawEntity,
    }

    let rows = stmt
        .query_map(params![from_blob, max_hops_i64], |row| {
            Ok(WalkRowRaw {
                depth: row.get::<_, i64>(1)?,
                rel_path_value: row.get::<_, duckdb::types::Value>(2)?,
                raw_entity: RawEntity {
                    id: row.get::<_, Vec<u8>>(3)?,
                    name: row.get(4)?,
                    entity_type: row.get(5)?,
                    boundary: row.get(6)?,
                    created_at: row.get(7)?,
                },
            })
        })
        .map_err(|e| VaultError::Storage(format!("execute traversal: {e}")))?;

    struct WalkRow {
        depth: i64,
        rel_path: Vec<Vec<u8>>,
        raw: RawEntity,
    }

    fn unpack_blob_list(v: duckdb::types::Value) -> VaultResult<Vec<Vec<u8>>> {
        match v {
            duckdb::types::Value::List(items) => items
                .into_iter()
                .map(|item| match item {
                    duckdb::types::Value::Blob(bytes) => Ok(bytes),
                    other => Err(VaultError::Storage(format!(
                        "traversal rel_path element is not a Blob: got {other:?}"
                    ))),
                })
                .collect(),
            duckdb::types::Value::Null => Ok(Vec::new()),
            other => Err(VaultError::Storage(format!(
                "traversal rel_path is not a List: got {other:?}"
            ))),
        }
    }

    // Group by entity-id, keep the row with smallest depth (= shortest path).
    // ORDER BY depth ASC means the first occurrence we see IS the shortest.
    let mut shortest_by_entity: HashMap<Vec<u8>, WalkRow> = HashMap::new();
    let mut wanted_rel_ids: HashSet<Vec<u8>> = HashSet::new();
    for raw_row in rows {
        let raw_row = raw_row.map_err(|e| VaultError::Storage(format!("walk row: {e}")))?;
        let row = WalkRow {
            depth: raw_row.depth,
            rel_path: unpack_blob_list(raw_row.rel_path_value)?,
            raw: raw_row.raw_entity,
        };
        let entity_id = row.raw.id.clone();
        if let std::collections::hash_map::Entry::Vacant(e) = shortest_by_entity.entry(entity_id) {
            for rid in &row.rel_path {
                wanted_rel_ids.insert(rid.clone());
            }
            e.insert(row);
        }
    }

    if shortest_by_entity.is_empty() {
        return Ok(Vec::new());
    }

    // Bulk-fetch the relationships referenced by any kept path.
    // Build a single SELECT with id IN (?, ?, ...) — the count varies per
    // call, so we format it dynamically. The `wanted_rel_ids` are BLOBs
    // we fetched ourselves moments ago; no injection surface.
    let placeholders = (0..wanted_rel_ids.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(", ");
    let rel_sql = format!(
        "SELECT id, from_entity_id, to_entity_id, relation_type, \
                valid_from, valid_until, confidence \
         FROM relationships WHERE id IN ({placeholders})"
    );
    let mut rel_stmt = conn
        .prepare(&rel_sql)
        .map_err(|e| VaultError::Storage(format!("prepare rel hydration: {e}")))?;

    // duckdb-rs accepts a slice of `&dyn ToSql` as Params. Build it here.
    let rel_id_vec: Vec<Vec<u8>> = wanted_rel_ids.into_iter().collect();
    let rel_params: Vec<&dyn duckdb::ToSql> =
        rel_id_vec.iter().map(|v| v as &dyn duckdb::ToSql).collect();

    let rel_rows = rel_stmt
        .query_map(rel_params.as_slice(), row_to_relationship)
        .map_err(|e| VaultError::Storage(format!("execute rel hydration: {e}")))?;

    let mut rel_by_id: HashMap<Vec<u8>, Relationship> = HashMap::new();
    for raw in rel_rows {
        let raw = raw.map_err(|e| VaultError::Storage(format!("rel row: {e}")))?;
        let id_blob = raw.id.clone();
        rel_by_id.insert(id_blob, raw.try_into_relationship()?);
    }

    // Reassemble: for each kept entity, build its Vec<Relationship> in path order.
    // Stable output ordering: by depth ASC, then entity-id ASC for determinism.
    let mut rows_kept: Vec<WalkRow> = shortest_by_entity.into_values().collect();
    rows_kept.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.raw.id.cmp(&b.raw.id)));

    let mut out: Vec<(Entity, Vec<Relationship>)> = Vec::with_capacity(rows_kept.len());
    for row in rows_kept {
        let entity = row.raw.try_into_entity()?;
        let chain: VaultResult<Vec<Relationship>> = row
            .rel_path
            .into_iter()
            .map(|rid| {
                rel_by_id.get(&rid).cloned().ok_or_else(|| {
                    VaultError::Storage(format!(
                        "rel hydration missing id (len {}); \
                         BUG in graph_store traversal",
                        rid.len()
                    ))
                })
            })
            .collect();
        out.push((entity, chain?));
    }

    Ok(out)
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet as StdHashSet;
    use std::sync::Arc as StdArc;
    use tempfile::TempDir;
    use vault_core::{EntityType, NewEntity};

    // -------- Fixtures --------

    fn b(s: &str) -> Boundary {
        Boundary::new(s).expect("valid boundary in test fixture")
    }

    fn ent(name: &str, et: EntityType, boundary: &str) -> Entity {
        Entity::try_new(NewEntity {
            name: name.to_string(),
            entity_type: et,
            boundary: b(boundary),
        })
        .expect("valid entity in test fixture")
    }

    fn rel(from: EntityId, to: EntityId, rt: &str, conf: f32) -> Relationship {
        Relationship::try_new(from, to, rt, conf).expect("valid relationship in test fixture")
    }

    async fn open_tmp() -> (TempDir, DuckDbGraphStore) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("graph.duckdb");
        let store = DuckDbGraphStore::open(&path).await.unwrap();
        (dir, store)
    }

    fn opts(max_hops: usize) -> TraversalOptions {
        TraversalOptions {
            max_hops,
            relation_filter: None,
            follow_aliases: false,
        }
    }

    // -------- Open / migrations --------

    #[tokio::test]
    async fn fresh_open_creates_expected_tables() {
        let (_dir, store) = open_tmp().await;
        // Smoke: can we count rows on each table?
        let inner = store.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().unwrap();
            for table in ["entities", "relationships"] {
                let n: i64 = conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(n, 0, "fresh {table} should be empty");
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("graph.duckdb");
        // First close before reopening — duckdb file lock.
        {
            let _store = DuckDbGraphStore::open(&path).await.unwrap();
        }
        let _store2 = DuckDbGraphStore::open(&path).await.unwrap();
    }

    /// **T0.1.10 Phase 3b — ADR-010 (extended via ADR-014 / module
    /// docstring at line 211-212) compensating-control #3 pin for
    /// DuckDB.**
    ///
    /// Sibling pin to `vector_store.rs::open_emits_adr_010_plaintext_warn_log`.
    /// The WARN itself has been live in `DuckDbGraphStore::open_blocking`
    /// since T0.1.5 (`graph_store.rs:213-217`); this test pins it
    /// against regression. Same regression vectors as the LanceDB
    /// counterpart: a future change that drops the WARN, demotes its
    /// level, or removes the ADR-010 / T0.2.0 references trips CI
    /// immediately.
    ///
    /// Asserts the same three properties as the LanceDB counterpart:
    /// (1) WARN fires on every `DuckDbGraphStore::open`, (2) message
    /// contains "ADR-010", (3) message contains "T0.2.0".
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn open_emits_adr_010_plaintext_warn_log() {
        let _ = open_tmp().await;

        assert!(
            tracing_test::internal::logs_with_scope_contain(
                "vault_storage",
                "DuckDB graph store data is PLAINTEXT",
            ),
            "ADR-010 (extended to DuckDB) compensating-control #3 WARN log MUST fire on \
             every DuckDbGraphStore::open. If this fails, the WARN at graph_store.rs:213-217 \
             has been removed, demoted, or its scope altered."
        );
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_storage", "ADR-010",),
            "ADR-010 reference MUST appear in the DuckDB WARN message"
        );
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_storage", "T0.2.0",),
            "T0.2.0 reference MUST appear in the DuckDB WARN message"
        );
    }

    // -------- create_entity --------

    #[tokio::test]
    async fn create_entity_round_trips_through_traverse() {
        // No direct get_entity API in V0.1; we observe round-trip via
        // a 1-hop traverse from this entity to a neighbor we add.
        let (_dir, store) = open_tmp().await;
        let alice = ent("Alice", EntityType::Person, "work");
        let bob = ent("Bob", EntityType::Person, "work");
        store.create_entity(&alice).await.unwrap();
        store.create_entity(&bob).await.unwrap();
        store
            .create_relationship(&rel(alice.id, bob.id, "works_with", 0.9))
            .await
            .unwrap();

        let result = store
            .traverse(&alice.id, &[b("work")], opts(1))
            .await
            .unwrap();
        assert_eq!(result.len(), 1);
        let (returned_bob, chain) = &result[0];
        assert_eq!(returned_bob, &bob);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].relation_type, "works_with");
    }

    #[tokio::test]
    async fn create_entity_validates_at_api_boundary() {
        let (_dir, store) = open_tmp().await;
        let mut bad = ent("X", EntityType::Person, "work");
        bad.name = String::new(); // bypass try_new validation
        let err = store.create_entity(&bad).await.unwrap_err();
        assert!(
            matches!(err, VaultError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[tokio::test]
    async fn create_entity_duplicate_in_same_boundary_rejected() {
        // Composite UNIQUE on (name, entity_type, boundary) per ADR-015 watch #3.
        let (_dir, store) = open_tmp().await;
        let a = ent("Sara", EntityType::Person, "work");
        let b_ = ent("Sara", EntityType::Person, "work"); // same triple, different ID
        store.create_entity(&a).await.unwrap();
        let err = store.create_entity(&b_).await.unwrap_err();
        match err {
            VaultError::Storage(msg) => {
                assert!(
                    msg.contains("already exists"),
                    "expected duplicate-key message, got {msg:?}",
                );
            }
            other => panic!("expected Storage(_), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_entity_same_name_different_boundaries_succeeds() {
        // ADR-015 watch #3: cross-boundary entity duplication is the privacy default.
        let (_dir, store) = open_tmp().await;
        let work_sara = ent("Sara", EntityType::Person, "work");
        let personal_sara = ent("Sara", EntityType::Person, "personal");
        store.create_entity(&work_sara).await.unwrap();
        store.create_entity(&personal_sara).await.unwrap();
        // Both rows landed; both are reachable via their respective boundaries.
    }

    // -------- create_relationship --------

    #[tokio::test]
    async fn create_relationship_within_boundary_succeeds() {
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        let c = ent("C", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        store.create_entity(&c).await.unwrap();
        store
            .create_relationship(&rel(a.id, c.id, "knows", 0.5))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_relationship_cross_boundary_non_alias_rejected() {
        // ADR-015: cross-boundary works_with is forbidden.
        let (_dir, store) = open_tmp().await;
        let work_a = ent("A", EntityType::Person, "work");
        let personal_a = ent("A", EntityType::Person, "personal");
        store.create_entity(&work_a).await.unwrap();
        store.create_entity(&personal_a).await.unwrap();
        let err = store
            .create_relationship(&rel(work_a.id, personal_a.id, "works_with", 0.7))
            .await
            .unwrap_err();
        match err {
            VaultError::AccessDenied(msg) => {
                assert!(msg.contains("cross-boundary"), "got {msg:?}");
                assert!(msg.contains("ADR-015"), "got {msg:?}");
            }
            other => panic!("expected AccessDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn create_relationship_cross_boundary_same_as_succeeds() {
        // ADR-015 forward-compat: schema permits same_as across boundaries.
        let (_dir, store) = open_tmp().await;
        let work_sara = ent("Sara", EntityType::Person, "work");
        let personal_sara = ent("Sara", EntityType::Person, "personal");
        store.create_entity(&work_sara).await.unwrap();
        store.create_entity(&personal_sara).await.unwrap();
        store
            .create_relationship(&rel(work_sara.id, personal_sara.id, "same_as", 1.0))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_relationship_cross_boundary_alias_for_succeeds() {
        let (_dir, store) = open_tmp().await;
        let work_x = ent("X", EntityType::Person, "work");
        let personal_x = ent("X", EntityType::Person, "personal");
        store.create_entity(&work_x).await.unwrap();
        store.create_entity(&personal_x).await.unwrap();
        store
            .create_relationship(&rel(work_x.id, personal_x.id, "alias_for", 1.0))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn create_relationship_missing_endpoint_returns_not_found() {
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        let phantom = EntityId::new();
        let err = store
            .create_relationship(&rel(a.id, phantom, "knows", 0.5))
            .await
            .unwrap_err();
        match err {
            VaultError::NotFound(msg) => assert!(msg.contains(&phantom.to_string())),
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    // -------- traverse: hop bounding --------

    #[tokio::test]
    async fn traverse_one_hop_returns_direct_neighbors_only() {
        let (_dir, store) = open_tmp().await;
        // Linear chain A -> B -> C (within the same boundary).
        let a = ent("A", EntityType::Person, "work");
        let bb = ent("B", EntityType::Person, "work");
        let cc = ent("C", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        store.create_entity(&bb).await.unwrap();
        store.create_entity(&cc).await.unwrap();
        store
            .create_relationship(&rel(a.id, bb.id, "k", 0.5))
            .await
            .unwrap();
        store
            .create_relationship(&rel(bb.id, cc.id, "k", 0.5))
            .await
            .unwrap();
        let result = store.traverse(&a.id, &[b("work")], opts(1)).await.unwrap();
        assert_eq!(result.len(), 1, "1-hop should reach only B");
        assert_eq!(result[0].0.id, bb.id);
    }

    #[tokio::test]
    async fn traverse_two_hops_returns_two_levels() {
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        let bb = ent("B", EntityType::Person, "work");
        let cc = ent("C", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        store.create_entity(&bb).await.unwrap();
        store.create_entity(&cc).await.unwrap();
        store
            .create_relationship(&rel(a.id, bb.id, "k", 0.5))
            .await
            .unwrap();
        store
            .create_relationship(&rel(bb.id, cc.id, "k", 0.5))
            .await
            .unwrap();
        let result = store.traverse(&a.id, &[b("work")], opts(2)).await.unwrap();
        let ids: StdHashSet<EntityId> = result.iter().map(|(e, _)| e.id).collect();
        assert!(ids.contains(&bb.id) && ids.contains(&cc.id));
        assert_eq!(ids.len(), 2);
        // Path length sanity: B is 1 hop, C is 2 hops.
        for (e, chain) in &result {
            if e.id == bb.id {
                assert_eq!(chain.len(), 1);
            } else {
                assert_eq!(chain.len(), 2);
            }
        }
    }

    #[tokio::test]
    async fn traverse_three_hops_recursive_cte_works() {
        let (_dir, store) = open_tmp().await;
        // A -> B -> C -> D
        let entities: Vec<Entity> = ["A", "B", "C", "D"]
            .iter()
            .map(|n| ent(n, EntityType::Person, "work"))
            .collect();
        for e in &entities {
            store.create_entity(e).await.unwrap();
        }
        for w in entities.windows(2) {
            store
                .create_relationship(&rel(w[0].id, w[1].id, "k", 0.5))
                .await
                .unwrap();
        }
        let result = store
            .traverse(&entities[0].id, &[b("work")], opts(3))
            .await
            .unwrap();
        let ids: StdHashSet<EntityId> = result.iter().map(|(e, _)| e.id).collect();
        assert_eq!(ids.len(), 3);
        for e in &entities[1..] {
            assert!(ids.contains(&e.id));
        }
    }

    #[tokio::test]
    async fn traverse_max_hops_bound_strictly_respected() {
        // Shahbaz-added test: 5-hop graph queried with max_hops=2 returns
        // only the first 2 hops, not 3-5. Easy to get wrong with recursive CTEs.
        let (_dir, store) = open_tmp().await;
        let entities: Vec<Entity> = ["A", "B", "C", "D", "E", "F"]
            .iter()
            .map(|n| ent(n, EntityType::Person, "work"))
            .collect();
        for e in &entities {
            store.create_entity(e).await.unwrap();
        }
        for w in entities.windows(2) {
            store
                .create_relationship(&rel(w[0].id, w[1].id, "k", 0.5))
                .await
                .unwrap();
        }
        let result = store
            .traverse(&entities[0].id, &[b("work")], opts(2))
            .await
            .unwrap();
        let ids: StdHashSet<EntityId> = result.iter().map(|(e, _)| e.id).collect();
        // Only B and C reachable in 2 hops; D, E, F must be excluded.
        assert!(ids.contains(&entities[1].id), "B (1 hop) must be present");
        assert!(ids.contains(&entities[2].id), "C (2 hops) must be present");
        assert!(
            !ids.contains(&entities[3].id),
            "D (3 hops) must NOT be present"
        );
        assert!(
            !ids.contains(&entities[4].id),
            "E (4 hops) must NOT be present"
        );
        assert!(
            !ids.contains(&entities[5].id),
            "F (5 hops) must NOT be present"
        );
    }

    #[tokio::test]
    async fn traverse_zero_max_hops_returns_empty() {
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        let result = store.traverse(&a.id, &[b("work")], opts(0)).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn traverse_empty_authorized_boundaries_returns_empty() {
        // ADR-015: empty authorized list short-circuits to empty result, not error.
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        let result = store.traverse(&a.id, &[], opts(3)).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn traverse_relation_filter_restricts_results() {
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        let bb = ent("B", EntityType::Person, "work");
        let cc = ent("C", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        store.create_entity(&bb).await.unwrap();
        store.create_entity(&cc).await.unwrap();
        store
            .create_relationship(&rel(a.id, bb.id, "works_with", 0.5))
            .await
            .unwrap();
        store
            .create_relationship(&rel(a.id, cc.id, "knows", 0.5))
            .await
            .unwrap();
        let mut o = opts(1);
        o.relation_filter = Some("works_with".into());
        let result = store.traverse(&a.id, &[b("work")], o).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0.id, bb.id);
    }

    // -------- traverse: boundary leak / aliases --------

    #[tokio::test]
    async fn traverse_does_not_return_entities_outside_authorized_boundary() {
        // Watch-item #1: every reachable entity must be inside authorized.
        // Setup: a single same_as link (cross-boundary) between work_x and personal_x.
        // With follow_aliases=false (V0.1 default), we must NOT see personal_x
        // even when authorized_boundaries = [work, personal].
        let (_dir, store) = open_tmp().await;
        let work_x = ent("X", EntityType::Person, "work");
        let personal_x = ent("X", EntityType::Person, "personal");
        store.create_entity(&work_x).await.unwrap();
        store.create_entity(&personal_x).await.unwrap();
        store
            .create_relationship(&rel(work_x.id, personal_x.id, "same_as", 1.0))
            .await
            .unwrap();

        let result = store
            .traverse(
                &work_x.id,
                &[b("work"), b("personal")],
                opts(2), // follow_aliases=false
            )
            .await
            .unwrap();
        assert!(
            result.is_empty(),
            "follow_aliases=false must NOT cross the same_as edge; got {} hits",
            result.len()
        );
    }

    #[tokio::test]
    async fn traverse_with_only_personal_boundary_does_not_see_work_neighbors() {
        // Pure boundary leak guard: even if data exists in `work`, an authorized
        // list of [personal] returns nothing.
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        let bb = ent("B", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        store.create_entity(&bb).await.unwrap();
        store
            .create_relationship(&rel(a.id, bb.id, "knows", 0.5))
            .await
            .unwrap();
        let result = store
            .traverse(&a.id, &[b("personal")], opts(2))
            .await
            .unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn traverse_with_follow_aliases_true_crosses_same_as_into_authorized_boundary() {
        // Forward-compat smoke test: when follow_aliases=true, same_as edges are
        // followed, but the destination still must be in authorized_boundaries.
        let (_dir, store) = open_tmp().await;
        let work_x = ent("X", EntityType::Person, "work");
        let personal_x = ent("X", EntityType::Person, "personal");
        store.create_entity(&work_x).await.unwrap();
        store.create_entity(&personal_x).await.unwrap();
        store
            .create_relationship(&rel(work_x.id, personal_x.id, "same_as", 1.0))
            .await
            .unwrap();

        // authorized = [work, personal] — should reach personal_x.
        let mut o = opts(2);
        o.follow_aliases = true;
        let result = store
            .traverse(&work_x.id, &[b("work"), b("personal")], o.clone())
            .await
            .unwrap();
        let ids: StdHashSet<EntityId> = result.iter().map(|(e, _)| e.id).collect();
        assert!(ids.contains(&personal_x.id));
    }

    #[tokio::test]
    async fn traverse_with_follow_aliases_true_still_respects_authorized_boundaries() {
        // Forward-compat invariant: follow_aliases=true is NOT privilege escalation.
        // Even if a same_as edge exists, the destination must be in authorized.
        let (_dir, store) = open_tmp().await;
        let work_x = ent("X", EntityType::Person, "work");
        let personal_x = ent("X", EntityType::Person, "personal");
        store.create_entity(&work_x).await.unwrap();
        store.create_entity(&personal_x).await.unwrap();
        store
            .create_relationship(&rel(work_x.id, personal_x.id, "same_as", 1.0))
            .await
            .unwrap();

        let mut o = opts(2);
        o.follow_aliases = true;
        // authorized = [work] only — personal_x should NOT appear.
        let result = store.traverse(&work_x.id, &[b("work")], o).await.unwrap();
        assert!(
            result.is_empty(),
            "alias destination outside authorized boundaries leaked: {} hits",
            result.len()
        );
    }

    // -------- supersede + bi-temporal --------

    #[tokio::test]
    async fn supersede_relationship_marks_old_valid_until_and_inserts_new() {
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        let bb = ent("B", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        store.create_entity(&bb).await.unwrap();
        let old = rel(a.id, bb.id, "works_at", 0.7);
        store.create_relationship(&old).await.unwrap();

        // New relationship with valid_from strictly after old.valid_from.
        let mut new_r = rel(a.id, bb.id, "works_at", 0.95);
        new_r.valid_from = old.valid_from + chrono::Duration::seconds(60);
        store.supersede_relationship(&old.id, &new_r).await.unwrap();

        // Inspect raw rows.
        let inner = store.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().unwrap();
            // Old row now has valid_until set.
            let old_until: Option<String> = conn
                .query_row(
                    "SELECT valid_until FROM relationships WHERE id = ?",
                    params![old.id.0.as_bytes().to_vec()],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(old_until.is_some(), "old.valid_until must be set");
            // New row exists.
            let new_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM relationships WHERE id = ?",
                    params![new_r.id.0.as_bytes().to_vec()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(new_count, 1);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn supersede_relationship_returns_not_found_for_missing_old() {
        let (_dir, store) = open_tmp().await;
        let a = ent("A", EntityType::Person, "work");
        let bb = ent("B", EntityType::Person, "work");
        store.create_entity(&a).await.unwrap();
        store.create_entity(&bb).await.unwrap();
        let phantom = RelationshipId::new();
        let new_r = rel(a.id, bb.id, "works_at", 0.5);
        let err = store
            .supersede_relationship(&phantom, &new_r)
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn supersede_rejects_cross_boundary_new_rel() {
        // The cross-boundary rule applies to new_rel as well.
        let (_dir, store) = open_tmp().await;
        let work_a = ent("A", EntityType::Person, "work");
        let personal_a = ent("A", EntityType::Person, "personal");
        let work_b = ent("B", EntityType::Person, "work");
        store.create_entity(&work_a).await.unwrap();
        store.create_entity(&personal_a).await.unwrap();
        store.create_entity(&work_b).await.unwrap();

        // Valid old: within work.
        let old = rel(work_a.id, work_b.id, "works_at", 0.5);
        store.create_relationship(&old).await.unwrap();

        // Invalid new: cross boundary, non-alias.
        let mut new_r = rel(work_a.id, personal_a.id, "works_at", 0.9);
        new_r.valid_from = old.valid_from + chrono::Duration::seconds(60);
        let err = store
            .supersede_relationship(&old.id, &new_r)
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::AccessDenied(_)));
    }

    #[tokio::test]
    async fn schema_permits_manual_same_as_row_for_v01_forward_compat() {
        // ADR-015: schema MUST accept same_as rows even though no V0.1 API
        // creates them via the trait. Insert directly via SQL.
        let (_dir, store) = open_tmp().await;
        let work_x = ent("X", EntityType::Person, "work");
        let personal_x = ent("X", EntityType::Person, "personal");
        store.create_entity(&work_x).await.unwrap();
        store.create_entity(&personal_x).await.unwrap();

        let inner = store.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().unwrap();
            let n_inserted = conn
                .execute(
                    "INSERT INTO relationships \
                     (id, from_entity_id, to_entity_id, relation_type, boundary, \
                      valid_from, valid_until, confidence) \
                     VALUES (?, ?, ?, 'same_as', 'work', ?, NULL, 1.0)",
                    params![
                        Uuid::now_v7().as_bytes().to_vec(),
                        work_x.id.0.as_bytes().to_vec(),
                        personal_x.id.0.as_bytes().to_vec(),
                        Utc::now().to_rfc3339(),
                    ],
                )
                .unwrap();
            assert_eq!(n_inserted, 1);
        })
        .await
        .unwrap();
    }

    // -------- Concurrent writes --------

    #[tokio::test]
    async fn concurrent_creates_dont_corrupt_state() {
        // 20 tasks each create one entity in the same boundary. All must
        // land cleanly; no row count mismatch, no panic.
        let (_dir, store) = open_tmp().await;
        let store = StdArc::new(store);
        let mut handles = Vec::new();
        for i in 0..20 {
            let s = store.clone();
            handles.push(tokio::spawn(async move {
                let e = ent(&format!("E{i:02}"), EntityType::Person, "work");
                s.create_entity(&e).await.unwrap();
                e
            }));
        }
        let mut created = Vec::new();
        for h in handles {
            created.push(h.await.unwrap());
        }
        assert_eq!(created.len(), 20);

        // Spot-check: the count column equals 20.
        let inner = store.inner.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().unwrap();
            let n: i64 = conn
                .query_row("SELECT COUNT(*) FROM entities", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 20);
        })
        .await
        .unwrap();
    }

    // -------- Property test: boundary-leak across arbitrary graphs --------

    use proptest::prelude::*;
    use proptest::test_runner::Config as ProptestConfig;

    proptest! {
        #![proptest_config(ProptestConfig {
            // Modest case count keeps the property test under 5s per the test budget.
            cases: 12, .. ProptestConfig::default()
        })]

        #[test]
        fn traverse_never_leaks_outside_authorized_boundaries(
            // Random small graph: 6 entities, with each entity in one of two boundaries,
            // and 0-8 random edges (from-to in {0..6}, relation_type "k").
            entity_boundary_choices in prop::collection::vec(prop::bool::ANY, 6..=6),
            edges in prop::collection::vec((0usize..6, 0usize..6), 0..=8),
            authorized_choice in prop::bool::ANY,
        ) {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let (_dir, store) = open_tmp().await;
                let mut entities = Vec::new();
                for (i, work_side) in entity_boundary_choices.iter().enumerate() {
                    let bnd = if *work_side { "work" } else { "personal" };
                    let e = ent(&format!("E{i}"), EntityType::Person, bnd);
                    store.create_entity(&e).await.unwrap();
                    entities.push(e);
                }
                for (from_idx, to_idx) in &edges {
                    if from_idx == to_idx { continue; }
                    let from = &entities[*from_idx];
                    let to = &entities[*to_idx];
                    if from.boundary == to.boundary {
                        // Within-boundary relationship — always insert.
                        let _ = store
                            .create_relationship(&rel(from.id, to.id, "k", 0.5))
                            .await;
                    }
                    // Cross-boundary edges are rejected by app-layer enforcement;
                    // skip them so the DB stays consistent for the property check.
                }

                // Pick an authorized set of size 1.
                let auth = if authorized_choice {
                    vec![b("work")]
                } else {
                    vec![b("personal")]
                };
                let target_boundary = auth[0].clone();

                for source in &entities {
                    let result = store.traverse(&source.id, &auth, opts(3)).await.unwrap();
                    for (e, _chain) in &result {
                        prop_assert_eq!(
                            &e.boundary,
                            &target_boundary,
                            "boundary leak: traversal from {} returned entity in {}, \
                             but authorized was [{}]",
                            source.id,
                            e.boundary.as_str(),
                            target_boundary.as_str(),
                        );
                    }
                }
                Ok(())
            })?;
        }
    }

    // ---------- validate_readable (ADR-018) ----------

    #[tokio::test]
    async fn validate_readable_passes_on_empty_graph() {
        // Vacuous pass: empty entities → no rows → no decode → Ok.
        let (_tmp, store) = open_tmp().await;
        store.validate_readable().await.unwrap();
    }

    #[tokio::test]
    async fn validate_readable_passes_on_clean_graph_with_entities() {
        let (_tmp, store) = open_tmp().await;
        let entity = ent("Alice", EntityType::Person, "work");
        store.create_entity(&entity).await.unwrap();
        store.validate_readable().await.unwrap();
    }
}
