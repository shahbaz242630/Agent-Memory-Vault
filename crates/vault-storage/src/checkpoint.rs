//! `checkpoint.rs` — Checkpoint & Rollback (T0.2.5 / A2, BRD §5.6 line 998).
//!
//! ## What this is
//!
//! Every consolidation run that changes at least one memory records a
//! **checkpoint** — an undo-log of *only what that run touched*, NOT a full
//! vault copy. A user (via vault-cli today, the UI later) can roll the vault
//! back to the state captured by any retained checkpoint: the "undo a bad
//! nightly run" safety net now that the consolidator runs unattended
//! (T0.2.6 scheduler).
//!
//! ## Why it is tractable to restore EXACTLY
//!
//! The consolidator never hard-deletes — supersede / `valid_until` / decay /
//! enrich are all soft mutations on existing rows, plus a handful of brand-new
//! rows (merged + enriched). So a run's effect on the authoritative stores is
//! fully described by two facts per touched memory:
//!
//! - **`Modified`**: the memory existed before the run and was mutated. We
//!   capture the FULL pre-consolidation [`Memory`] **plus its embedding**
//!   (the embedding lives only in LanceDB, never in the SQLite row, so it must
//!   be carried explicitly). Rollback re-applies it via the existing
//!   [`StorageBackend::update_memory`], which restores content / confidence /
//!   `superseded_by` / `valid_until` / embedding in one cascading call.
//! - **`Created`**: the run created the memory (a new merged or enriched row).
//!   Rollback deletes it via [`StorageBackend::delete_memory`].
//!
//! ## Scope (V1, founder-locked 2026-06-15)
//!
//! - Captures the **authoritative answer-driving stores**: memory rows +
//!   embeddings. **Graph (DuckDB) rollback is DEFERRED** until the graph
//!   enters the read path (HANDOFF tech-debt #2 tripwire) — the graph is
//!   write-only / not consumed at read in V0.2.
//! - Captures **only changed** memories' pre-images (scales to 10k — we never
//!   snapshot the whole vault).
//! - Retention **N = [`CHECKPOINT_RETENTION`]**: creating a checkpoint prunes
//!   all but the newest N (cascade-deleting their entries).
//!
//! The checkpoint tables live in the SQLCipher metadata DB, so the pre-image
//! blobs (which hold memory content) inherit the vault's zero-knowledge
//! encryption at rest. Schema: `migrations/0004_consolidation_checkpoints.sql`.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::instrument;
use uuid::Uuid;

use vault_core::{Boundary, Memory, MemoryId, VaultError, VaultResult};

use crate::cascading::StorageBackend;

/// Payload format version for the `checkpoint_entries.pre_image` blob.
/// Mirrors the [`crate::retry_queue::PAYLOAD_FORMAT_VERSION`] /
/// [`crate::dead_letter::PAYLOAD_FORMAT_VERSION`] versioning pattern: every
/// serialized blob is tagged so a future schema change can dispatch on the
/// stored version. V1 is the only shape today.
pub const CHECKPOINT_PAYLOAD_FORMAT_VERSION: i64 = 1;

/// Retention cap: [`StorageBackend::create_checkpoint`] prunes all but the
/// newest N checkpoints on each create. Founder-locked 2026-06-15.
pub const CHECKPOINT_RETENTION: usize = 7;

// ---------------------------------------------------------------------------
// CheckpointId
// ---------------------------------------------------------------------------

/// Strongly-typed checkpoint handle. Wraps a UUID v7 (time-ordered) so the
/// on-disk primary key has good locality and natural creation-order sort —
/// mirrors [`MemoryId`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckpointId(pub Uuid);

impl CheckpointId {
    /// Create a new, unique, time-ordered checkpoint id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for CheckpointId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for CheckpointId {
    type Err = VaultError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|e| VaultError::InvalidInput(format!("invalid checkpoint id: {e}")))
    }
}

// ---------------------------------------------------------------------------
// ChangeType / CheckpointStatus
// ---------------------------------------------------------------------------

/// What a consolidation run did to a memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeType {
    /// The memory existed before the run and was mutated; a pre-image was
    /// captured.
    Modified,
    /// The memory was created by the run; rollback deletes it.
    Created,
}

impl ChangeType {
    /// Stable on-disk discriminant for the `checkpoint_entries.change_type`
    /// column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::Created => "created",
        }
    }

    fn from_db_str(s: &str) -> VaultResult<Self> {
        match s {
            "modified" => Ok(Self::Modified),
            "created" => Ok(Self::Created),
            other => Err(VaultError::Storage(format!(
                "unknown checkpoint change_type {other:?}"
            ))),
        }
    }
}

/// Lifecycle state of a checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckpointStatus {
    /// Capturable target — has not been rolled back.
    Active,
    /// Already rolled back; a second rollback is rejected (no-op guard).
    RolledBack,
}

impl CheckpointStatus {
    /// Stable on-disk discriminant for the `consolidation_checkpoints.status`
    /// column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::RolledBack => "rolled_back",
        }
    }

    fn from_db_str(s: &str) -> VaultResult<Self> {
        match s {
            "active" => Ok(Self::Active),
            "rolled_back" => Ok(Self::RolledBack),
            other => Err(VaultError::Storage(format!(
                "unknown checkpoint status {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// CheckpointEntry — the input to create_checkpoint
// ---------------------------------------------------------------------------

/// One captured change, supplied by the consolidator to
/// [`StorageBackend::create_checkpoint`] right before it mutates a memory.
///
/// Modeled as an enum so illegal states are unrepresentable: a `Modified`
/// entry ALWAYS carries its pre-image (memory + embedding); a `Created` entry
/// NEVER does. This mirrors the `change_type` / `pre_image` nullability split
/// in the `checkpoint_entries` schema.
#[derive(Clone, Debug)]
pub enum CheckpointEntry {
    /// A memory that existed before the run and is about to be mutated
    /// (supersede / invalidate / decay / enrich). Carries the full
    /// pre-consolidation [`Memory`] and its embedding so rollback restores it
    /// EXACTLY via [`StorageBackend::update_memory`].
    Modified {
        /// The memory exactly as it was BEFORE the run touched it. Boxed so
        /// this variant is not far larger than `Created` (clippy
        /// `large_enum_variant`).
        memory: Box<Memory>,
        /// Its embedding exactly as it was before the run (LanceDB side).
        embedding: Vec<f32>,
    },
    /// A memory the run created (new merged / enriched row). Rollback deletes
    /// it. We only need its id + boundary — there is no prior state.
    Created {
        /// Id of the newly-created memory.
        memory_id: MemoryId,
        /// Boundary the new memory belongs to (recorded for audit symmetry).
        boundary: Boundary,
    },
}

impl CheckpointEntry {
    /// Id of the memory this entry concerns.
    pub fn memory_id(&self) -> MemoryId {
        match self {
            Self::Modified { memory, .. } => memory.id,
            Self::Created { memory_id, .. } => *memory_id,
        }
    }

    /// Boundary the memory belongs to.
    pub fn boundary(&self) -> &Boundary {
        match self {
            Self::Modified { memory, .. } => &memory.boundary,
            Self::Created { boundary, .. } => boundary,
        }
    }

    /// Whether this is a `Modified` or `Created` change.
    pub fn change_type(&self) -> ChangeType {
        match self {
            Self::Modified { .. } => ChangeType::Modified,
            Self::Created { .. } => ChangeType::Created,
        }
    }
}

/// Versioned on-disk shape of the `checkpoint_entries.pre_image` blob. Present
/// only for `Modified` entries; `Created` entries store NULL. Tagged by
/// `pre_image_version` ([`CHECKPOINT_PAYLOAD_FORMAT_VERSION`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CheckpointPreImageV1 {
    memory: Memory,
    embedding: Vec<f32>,
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// One checkpoint as surfaced by [`StorageBackend::list_checkpoints`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointSummary {
    /// The checkpoint id.
    pub id: CheckpointId,
    /// When the run that produced it completed (RFC3339 in storage).
    pub created_at: DateTime<Utc>,
    /// `Active` (rollback-able) or `RolledBack`.
    pub status: CheckpointStatus,
    /// Number of changed memories captured.
    pub entry_count: usize,
}

/// Outcome of a [`StorageBackend::rollback_checkpoint`] call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RollbackReport {
    /// The checkpoint that was rolled back.
    pub checkpoint_id: CheckpointId,
    /// Count of `Modified` entries restored to their pre-image.
    pub restored: usize,
    /// Count of `Created` entries deleted.
    pub deleted: usize,
}

// ---------------------------------------------------------------------------
// Internal row shapes
// ---------------------------------------------------------------------------

/// Owned, `Send`-able shape of a `checkpoint_entries` row, prepared on the
/// caller's task before crossing the `spawn_blocking` boundary in
/// [`StorageBackend::create_checkpoint`].
struct CheckpointEntryRow {
    memory_id: Vec<u8>,
    boundary: String,
    change_type: &'static str,
    pre_image_version: Option<i64>,
    pre_image: Option<Vec<u8>>,
}

/// A decoded entry loaded during rollback. The `Modified` pre-image is boxed
/// so the enum's two variants don't differ wildly in size (clippy
/// `large_enum_variant`).
enum LoadedEntry {
    Modified(Box<CheckpointPreImageV1>),
    Created(MemoryId),
}

// ---------------------------------------------------------------------------
// StorageBackend API
// ---------------------------------------------------------------------------

impl StorageBackend {
    /// Record a checkpoint for one consolidation run, then prune to
    /// [`CHECKPOINT_RETENTION`].
    ///
    /// Atomic on the SQLite side: the `consolidation_checkpoints` row, all
    /// `checkpoint_entries` rows, and the retention prune (delete all but the
    /// newest N — their entries cascade-delete) commit in ONE transaction.
    /// Metadata-only — no LanceDB / DuckDB cascade (the pre-images are stored
    /// in SQLite; nothing downstream changes at capture time).
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`] if `entries` is empty (a checkpoint
    ///   records a run that changed at least one memory — schema invariant).
    /// - [`VaultError::Storage`] on transaction-side failure.
    #[instrument(skip(self, entries), fields(entry_count = entries.len()))]
    pub async fn create_checkpoint(
        &self,
        entries: &[CheckpointEntry],
    ) -> VaultResult<CheckpointId> {
        if entries.is_empty() {
            return Err(VaultError::InvalidInput(
                "create_checkpoint requires at least one changed memory".into(),
            ));
        }

        let cp_id = CheckpointId::new();
        let created_at = Utc::now();

        // Serialize every entry into owned, Send-able rows BEFORE crossing the
        // spawn_blocking boundary (the closure must be `'static`).
        let mut rows: Vec<CheckpointEntryRow> = Vec::with_capacity(entries.len());
        for entry in entries {
            let pre_image = match entry {
                CheckpointEntry::Modified { memory, embedding } => {
                    let blob = serde_json::to_vec(&CheckpointPreImageV1 {
                        memory: (**memory).clone(),
                        embedding: embedding.clone(),
                    })
                    .map_err(|e| {
                        VaultError::Serde(format!("serialize checkpoint pre-image: {e}"))
                    })?;
                    Some(blob)
                }
                CheckpointEntry::Created { .. } => None,
            };
            rows.push(CheckpointEntryRow {
                memory_id: entry.memory_id().0.as_bytes().to_vec(),
                boundary: entry.boundary().as_str().to_string(),
                change_type: entry.change_type().as_str(),
                pre_image_version: pre_image
                    .as_ref()
                    .map(|_| CHECKPOINT_PAYLOAD_FORMAT_VERSION),
                pre_image,
            });
        }

        let entry_count = rows.len() as i64;
        let cp_id_bytes = cp_id.0.as_bytes().to_vec();
        let created_at_str = created_at.to_rfc3339();

        self.metadata()
            .with_transaction(move |tx| {
                tx.execute(
                    "INSERT INTO consolidation_checkpoints (id, created_at, status, entry_count)
                     VALUES (?1, ?2, 'active', ?3)",
                    params![cp_id_bytes, created_at_str, entry_count],
                )
                .map_err(|e| VaultError::Storage(format!("insert checkpoint: {e}")))?;

                for row in &rows {
                    tx.execute(
                        "INSERT INTO checkpoint_entries
                           (checkpoint_id, memory_id, boundary, change_type,
                            pre_image_version, pre_image)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            cp_id_bytes,
                            row.memory_id,
                            row.boundary,
                            row.change_type,
                            row.pre_image_version,
                            row.pre_image,
                        ],
                    )
                    .map_err(|e| VaultError::Storage(format!("insert checkpoint entry: {e}")))?;
                }

                // Retention prune: keep the newest N, drop the rest. Their
                // `checkpoint_entries` rows cascade-delete (FK ON DELETE
                // CASCADE + `foreign_keys = ON`). Tiebreak by id (UUID v7 =
                // time-ordered) so the cut is deterministic when two runs
                // share a created_at.
                tx.execute(
                    "DELETE FROM consolidation_checkpoints
                      WHERE id NOT IN (
                        SELECT id FROM consolidation_checkpoints
                        ORDER BY created_at DESC, id DESC
                        LIMIT ?1
                      )",
                    params![CHECKPOINT_RETENTION as i64],
                )
                .map_err(|e| VaultError::Storage(format!("prune checkpoints: {e}")))?;

                Ok(())
            })
            .await?;

        Ok(cp_id)
    }

    /// Restore the vault to the state captured by checkpoint `id`.
    ///
    /// For each entry, in storage order:
    /// - `Modified` → [`StorageBackend::update_memory`] with the pre-image
    ///   (restores content / confidence / `superseded_by` / `valid_until` +
    ///   embedding, cascading to LanceDB).
    /// - `Created` → [`StorageBackend::delete_memory`] (removes the row +
    ///   cascades the LanceDB vector delete).
    ///
    /// Then marks the checkpoint `status = 'rolled_back'`.
    ///
    /// # Errors
    ///
    /// - [`VaultError::NotFound`] if no checkpoint with that id exists (unknown
    ///   or already pruned).
    /// - [`VaultError::InvalidInput`] if the checkpoint is already rolled back
    ///   (double-rollback guard).
    /// - [`VaultError::Storage`] on transaction-side failure.
    #[instrument(skip(self), fields(checkpoint_id = %id))]
    pub async fn rollback_checkpoint(&self, id: CheckpointId) -> VaultResult<RollbackReport> {
        // Phase 1 — validate + load entries in ONE metadata transaction. We
        // must NOT hold the metadata lock across the update_memory /
        // delete_memory calls below: those re-lock the same connection, so
        // holding it here would deadlock. Hence load-then-apply-then-mark in
        // three separate transactions.
        let id_bytes = id.0.as_bytes().to_vec();
        let loaded: Vec<LoadedEntry> = self
            .metadata()
            .with_transaction(move |tx| {
                let status_str: Option<String> = tx
                    .query_row(
                        "SELECT status FROM consolidation_checkpoints WHERE id = ?1",
                        params![id_bytes],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|e| VaultError::Storage(format!("load checkpoint status: {e}")))?;
                let status_str = status_str.ok_or_else(|| {
                    VaultError::NotFound(format!("checkpoint {id} does not exist"))
                })?;
                if CheckpointStatus::from_db_str(&status_str)? == CheckpointStatus::RolledBack {
                    return Err(VaultError::InvalidInput(format!(
                        "checkpoint {id} has already been rolled back"
                    )));
                }

                let mut stmt = tx
                    .prepare(
                        "SELECT change_type, memory_id, pre_image
                           FROM checkpoint_entries WHERE checkpoint_id = ?1",
                    )
                    .map_err(|e| VaultError::Storage(format!("prepare entry load: {e}")))?;
                let raw: Vec<(String, Vec<u8>, Option<Vec<u8>>)> = stmt
                    .query_map(params![id_bytes], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })
                    .map_err(|e| VaultError::Storage(format!("query checkpoint entries: {e}")))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| VaultError::Storage(format!("read checkpoint entries: {e}")))?;

                let mut loaded = Vec::with_capacity(raw.len());
                for (change_type, mid_bytes, pre_image) in raw {
                    match ChangeType::from_db_str(&change_type)? {
                        ChangeType::Modified => {
                            let blob = pre_image.ok_or_else(|| {
                                VaultError::Storage(
                                    "modified checkpoint entry is missing its pre_image".into(),
                                )
                            })?;
                            let pre: CheckpointPreImageV1 =
                                serde_json::from_slice(&blob).map_err(|e| {
                                    VaultError::Serde(format!("decode checkpoint pre-image: {e}"))
                                })?;
                            loaded.push(LoadedEntry::Modified(Box::new(pre)));
                        }
                        ChangeType::Created => {
                            let uuid = Uuid::from_slice(&mid_bytes).map_err(|e| {
                                VaultError::Storage(format!("decode entry memory_id: {e}"))
                            })?;
                            loaded.push(LoadedEntry::Created(MemoryId(uuid)));
                        }
                    }
                }
                Ok(loaded)
            })
            .await?;

        // Phase 2 — apply the undo. 'modified' restores the pre-image exactly
        // (cascades content/confidence/superseded_by/valid_until + embedding);
        // 'created' deletes the row the run added.
        let mut restored = 0usize;
        let mut deleted = 0usize;
        for entry in loaded {
            match entry {
                LoadedEntry::Modified(pre) => {
                    self.update_memory(&pre.memory, &pre.embedding).await?;
                    restored += 1;
                }
                LoadedEntry::Created(memory_id) => {
                    self.delete_memory(&memory_id).await?;
                    deleted += 1;
                }
            }
        }

        // Phase 3 — mark the checkpoint spent (double-rollback guard).
        let id_bytes = id.0.as_bytes().to_vec();
        self.metadata()
            .with_transaction(move |tx| {
                tx.execute(
                    "UPDATE consolidation_checkpoints SET status = 'rolled_back' WHERE id = ?1",
                    params![id_bytes],
                )
                .map_err(|e| VaultError::Storage(format!("mark checkpoint rolled_back: {e}")))?;
                Ok(())
            })
            .await?;

        Ok(RollbackReport {
            checkpoint_id: id,
            restored,
            deleted,
        })
    }

    /// All checkpoints, newest first.
    ///
    /// # Errors
    ///
    /// - [`VaultError::Storage`] on transaction-side failure.
    #[instrument(skip(self))]
    pub async fn list_checkpoints(&self) -> VaultResult<Vec<CheckpointSummary>> {
        self.metadata()
            .with_transaction(|tx| {
                let mut stmt = tx
                    .prepare(
                        "SELECT id, created_at, status, entry_count
                           FROM consolidation_checkpoints
                          ORDER BY created_at DESC, id DESC",
                    )
                    .map_err(|e| VaultError::Storage(format!("prepare list checkpoints: {e}")))?;
                let raw: Vec<(Vec<u8>, String, String, i64)> = stmt
                    .query_map([], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                    })
                    .map_err(|e| VaultError::Storage(format!("query checkpoints: {e}")))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|e| VaultError::Storage(format!("read checkpoints: {e}")))?;

                let mut out = Vec::with_capacity(raw.len());
                for (id_bytes, created_at_s, status_s, entry_count) in raw {
                    let uuid = Uuid::from_slice(&id_bytes)
                        .map_err(|e| VaultError::Storage(format!("decode checkpoint id: {e}")))?;
                    let created_at = DateTime::parse_from_rfc3339(&created_at_s)
                        .map_err(|e| {
                            VaultError::Storage(format!("decode checkpoint created_at: {e}"))
                        })?
                        .with_timezone(&Utc);
                    out.push(CheckpointSummary {
                        id: CheckpointId(uuid),
                        created_at,
                        status: CheckpointStatus::from_db_str(&status_s)?,
                        entry_count: entry_count as usize,
                    });
                }
                Ok(out)
            })
            .await
    }
}

// ===========================================================================
// Tests — written FIRST (held for review before the bodies above are filled in)
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    use vault_core::{MemoryType, NewMemory};

    use crate::key::SqlCipherKey;
    use crate::metadata_store::tx_get_memory;

    const DIM: usize = 4;
    const TEST_AT_REST_KEY: [u8; 32] = [0xcd; 32];

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

    async fn make_backend() -> (TempDir, StorageBackend) {
        let tmp = TempDir::new().unwrap();
        let backend = StorageBackend::open_with_at_rest_key(
            &tmp.path().join("vault.db"),
            &tmp.path().join("lance"),
            &tmp.path().join("graph.duckdb"),
            SqlCipherKey::new("checkpoint-test-key"),
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap();
        (tmp, backend)
    }

    /// Read a memory's current SQLite-side state back, or `None` if absent.
    async fn get_memory(backend: &StorageBackend, id: MemoryId) -> Option<Memory> {
        backend
            .metadata()
            .with_transaction(move |tx| tx_get_memory(tx, &id))
            .await
            .unwrap()
    }

    // ------------------------------------------------------------------
    // create_checkpoint + list_checkpoints
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn create_checkpoint_rejects_empty_entries() {
        // A checkpoint records a run that changed >= 1 memory (schema
        // invariant). An empty list is caller misuse, not a no-op success.
        let (_tmp, backend) = make_backend().await;
        let err = backend.create_checkpoint(&[]).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn create_then_list_returns_active_summary() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "fact A");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let cp = backend
            .create_checkpoint(&[CheckpointEntry::Modified {
                memory: Box::new(m.clone()),
                embedding: embedding(0.1),
            }])
            .await
            .unwrap();

        let list = backend.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, cp);
        assert_eq!(list[0].status, CheckpointStatus::Active);
        assert_eq!(list[0].entry_count, 1);
    }

    // ------------------------------------------------------------------
    // rollback — the core round-trip
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn rollback_modified_restores_memory_exactly() {
        let (_tmp, backend) = make_backend().await;

        let m = sample_memory("work", "the original fact");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();
        // Capture the persisted pre-image exactly as the consolidator will
        // (step 2 reads the memory from storage before mutating it).
        let before = get_memory(&backend, m.id).await.unwrap();

        let cp = backend
            .create_checkpoint(&[CheckpointEntry::Modified {
                memory: Box::new(before.clone()),
                embedding: embedding(0.2),
            }])
            .await
            .unwrap();

        // Simulate the consolidator changing content + confidence.
        let mut mutated = before.clone();
        mutated.content = "the consolidator changed this".into();
        mutated.confidence = 0.1;
        backend
            .update_memory(&mutated, &embedding(0.9))
            .await
            .unwrap();
        assert_ne!(get_memory(&backend, m.id).await.unwrap(), before);

        // Roll back — the row must match the pre-image EXACTLY.
        let report = backend.rollback_checkpoint(cp).await.unwrap();
        assert_eq!(report.restored, 1);
        assert_eq!(report.deleted, 0);
        assert_eq!(
            get_memory(&backend, m.id).await.unwrap(),
            before,
            "rollback must restore the exact pre-image"
        );

        // The checkpoint is now spent.
        let list = backend.list_checkpoints().await.unwrap();
        assert_eq!(list[0].status, CheckpointStatus::RolledBack);
    }

    #[tokio::test]
    async fn rollback_created_deletes_memory() {
        let (_tmp, backend) = make_backend().await;

        // Simulate a run that CREATED a new merged row.
        let created = sample_memory("work", "a newly merged fact");
        backend
            .write_memory(&created, &embedding(0.3))
            .await
            .unwrap();

        let cp = backend
            .create_checkpoint(&[CheckpointEntry::Created {
                memory_id: created.id,
                boundary: created.boundary.clone(),
            }])
            .await
            .unwrap();

        let report = backend.rollback_checkpoint(cp).await.unwrap();
        assert_eq!(report.restored, 0);
        assert_eq!(report.deleted, 1);
        assert!(get_memory(&backend, created.id).await.is_none());
    }

    #[tokio::test]
    async fn rollback_mixed_run_restores_and_deletes() {
        // One run that both mutated an existing memory AND created a new one.
        let (_tmp, backend) = make_backend().await;

        let existing = sample_memory("work", "existing fact");
        backend
            .write_memory(&existing, &embedding(0.4))
            .await
            .unwrap();
        let before = get_memory(&backend, existing.id).await.unwrap();
        let created = sample_memory("work", "merged fact");
        backend
            .write_memory(&created, &embedding(0.5))
            .await
            .unwrap();

        let cp = backend
            .create_checkpoint(&[
                CheckpointEntry::Modified {
                    memory: Box::new(before.clone()),
                    embedding: embedding(0.4),
                },
                CheckpointEntry::Created {
                    memory_id: created.id,
                    boundary: created.boundary.clone(),
                },
            ])
            .await
            .unwrap();

        // Mutate the existing one after capture.
        let mut mutated = before.clone();
        mutated.content = "changed".into();
        backend
            .update_memory(&mutated, &embedding(0.9))
            .await
            .unwrap();

        let report = backend.rollback_checkpoint(cp).await.unwrap();
        assert_eq!(report.restored, 1);
        assert_eq!(report.deleted, 1);
        assert_eq!(get_memory(&backend, existing.id).await.unwrap(), before);
        assert!(get_memory(&backend, created.id).await.is_none());
    }

    // ------------------------------------------------------------------
    // retention + error guards
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn create_prunes_to_retention_keeping_newest() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "anchor");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let mut ids = Vec::new();
        for _ in 0..(CHECKPOINT_RETENTION + 3) {
            let cp = backend
                .create_checkpoint(&[CheckpointEntry::Modified {
                    memory: Box::new(m.clone()),
                    embedding: embedding(0.1),
                }])
                .await
                .unwrap();
            ids.push(cp);
        }

        let list = backend.list_checkpoints().await.unwrap();
        assert_eq!(list.len(), CHECKPOINT_RETENTION);
        let present: Vec<CheckpointId> = list.iter().map(|s| s.id).collect();
        // Newest is kept; the very first-created (well past the boundary) is gone.
        assert!(present.contains(ids.last().unwrap()));
        assert!(!present.contains(ids.first().unwrap()));
    }

    #[tokio::test]
    async fn rollback_unknown_checkpoint_errors() {
        let (_tmp, backend) = make_backend().await;
        let err = backend
            .rollback_checkpoint(CheckpointId::new())
            .await
            .unwrap_err();
        assert!(matches!(err, VaultError::NotFound(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn double_rollback_is_rejected() {
        let (_tmp, backend) = make_backend().await;
        let m = sample_memory("work", "fact");
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();
        let cp = backend
            .create_checkpoint(&[CheckpointEntry::Modified {
                memory: Box::new(m.clone()),
                embedding: embedding(0.1),
            }])
            .await
            .unwrap();

        backend.rollback_checkpoint(cp).await.unwrap();
        let err = backend.rollback_checkpoint(cp).await.unwrap_err();
        assert!(matches!(err, VaultError::InvalidInput(_)), "got {err:?}");
    }
}
