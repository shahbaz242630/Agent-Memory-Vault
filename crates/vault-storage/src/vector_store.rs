//! [`VectorStore`] trait + [`LanceVectorStore`] — LanceDB-backed embedding
//! store for the cascading backend (T0.1.4, BRD §5.2.2).
//!
//! ## V0.1 — vector data is stored unencrypted on disk (ADR-010)
//!
//! LanceDB 0.8 has no native at-rest encryption. ADR-010 documents the
//! V0.1-only deviation from BRD §11.5.1 ("All data on disk is encrypted.
//! No exceptions") — encryption-at-FS-layer ships at T0.2.0 as a HARD
//! GATE before T0.2.16 (Beta Onboarding). The deviation is bounded to
//! the founder-only internal alpha; no external user receives a build
//! containing the V0.1 plaintext code path.
//!
//! [`LanceVectorStore::open`] enforces the four ADR-010 compensating
//! controls that live in this crate:
//!
//! 1. `ALPHA_DO_NOT_STORE_REAL_DATA.txt` is auto-written into the data
//!    directory on every open and made read-only (cross-platform: Unix
//!    clears the write bits, Windows sets the read-only attribute via
//!    `std::fs::Permissions::set_readonly(true)`).
//! 2. A WARN-level `tracing` event fires on every open while the data
//!    dir is plaintext, naming ADR-010 and T0.2.0.
//!
//! The remaining two compensating controls (modal first-run banner and
//! persistent UI banner) live in `vault-tauri` (T0.1.11). All four are
//! removed by T0.2.0 when encryption ships.
//!
//! ## Boundary access control (BRD §11.4.3)
//!
//! [`VectorStore::search`] takes a non-`Optional` `&[Boundary]` slice as
//! its `authorized_boundaries` parameter. Callers cannot run a search
//! without explicitly authorising at least one boundary at the type
//! level — the trait makes "I forgot to filter" structurally
//! impossible. An empty slice is a valid input that returns an empty
//! result (access denied semantics, not error).
//!
//! `Boundary` itself enforces a tight ASCII identifier charset (see
//! `vault_core::boundary`), so the boundary value is safe to interpolate
//! into LanceDB's `only_if` SQL filter at the query layer — there is no
//! parameter-binding API for `only_if` in lancedb 0.8, so the type
//! system is the only line of defence against quote breakout.

use std::fs;
use std::path::Path;
use std::sync::Arc;

use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::Table;
use lancedb::DistanceType;
use tracing::{info, instrument, warn};

use vault_core::{Boundary, MemoryId, VaultError, VaultResult};

/// Filename of the V0.1 alpha-warning file written into the data directory
/// on every [`LanceVectorStore::open`]. Removed by T0.2.0 when encryption
/// ships.
pub const ALPHA_WARNING_FILENAME: &str = "ALPHA_DO_NOT_STORE_REAL_DATA.txt";

const TABLE_NAME: &str = "memories";

/// Async vector-store contract. Speaks the workspace's domain types
/// (`MemoryId`, `Boundary`, `&[f32]`) only — never LanceDB internals
/// (`RecordBatch`, Arrow types) per BRD §2.2.
///
/// Implementors are expected to be cheap to clone (typically wrapping a
/// reference-counted backend handle).
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Insert or update the embedding for `id`. The boundary is mandatory at
    /// write time (BRD §11.4.3 — every memory belongs to exactly one
    /// boundary).
    ///
    /// Implementations must reject embeddings whose length differs from
    /// [`Self::dimension`].
    async fn upsert(
        &self,
        id: &MemoryId,
        embedding: &[f32],
        boundary: &Boundary,
    ) -> VaultResult<()>;

    /// Delete the embedding for `id`. Idempotent: deleting an absent id is
    /// not an error.
    async fn delete(&self, id: &MemoryId) -> VaultResult<()>;

    /// k-NN search over embeddings, scoped to `authorized_boundaries`.
    ///
    /// **Mandatory access control (BRD §11.4.3).** `authorized_boundaries`
    /// is non-`Optional`. The trait makes it impossible to call retrieval
    /// without explicit boundary authorisation. An empty slice is a valid
    /// input that returns an empty result — *not* an error — so the caller
    /// receives "access denied" as "no matches" without information leakage.
    ///
    /// Returned scores are distances under the configured metric (cosine for
    /// V0.1; smaller = closer). Implementations apply the boundary filter
    /// at the query layer, never in application code post-fetch.
    async fn search(
        &self,
        query: &[f32],
        limit: usize,
        authorized_boundaries: &[Boundary],
    ) -> VaultResult<Vec<(MemoryId, f32)>>;

    /// Count embeddings, optionally scoped to a single boundary.
    ///
    /// Operational query — used by the Sync Health surface (ADR-009) and by
    /// the periodic divergence-verification job that compares SQLite memory
    /// IDs against the vector store. The boundary is `Optional` here
    /// because `count` does not return memory contents to the caller.
    async fn count(&self, boundary: Option<&Boundary>) -> VaultResult<usize>;

    /// Embedding dimension this store expects. Used by the cascading backend
    /// (T0.1.6) to validate compatibility before forwarding writes.
    fn dimension(&self) -> usize;
}

/// LanceDB-backed implementation of [`VectorStore`].
///
/// Cheap to clone — `lancedb::Table` is `Clone + Send + Sync` by design and
/// holds its own reference to the underlying connection. Share freely
/// across tasks.
///
/// Intentionally does **not** implement `Debug` (ADR-007 — types holding
/// live storage handles do not get manual `Debug` impls).
///
/// ## Distance metric
///
/// Search uses [`DistanceType::Cosine`]. Our embedding model
/// (bge-small-en-v1.5, T0.1.7) outputs L2-normalised 384-dim vectors, so
/// cosine distance and Euclidean distance differ only by a monotonic
/// transform — both rank the same way. Cosine is the conventional choice
/// for sentence embeddings and is what the downstream reranker (T0.2.7)
/// will be calibrated to. Returned scores are *distances* (smaller =
/// closer; identical vectors → 0). Changing this metric here changes the
/// score semantics for every consumer of [`VectorStore::search`] —
/// don't change it lightly.
#[derive(Clone)]
pub struct LanceVectorStore {
    table: Table,
    dimension: usize,
}

impl LanceVectorStore {
    /// Open or create a LanceDB store at `data_dir` with embedding dimension
    /// `dimension`. On every call:
    ///
    /// 1. Creates `data_dir` if missing.
    /// 2. Writes/refreshes [`ALPHA_WARNING_FILENAME`] in the data dir and
    ///    sets it read-only (cross-platform).
    /// 3. Emits a WARN-level `tracing` event naming ADR-010 + T0.2.0.
    /// 4. Connects to LanceDB and opens (or creates) the `memories` table
    ///    with the schema `(id: Utf8, embedding: FixedSizeList<Float32,
    ///    dimension>, boundary: Utf8)`.
    ///
    /// Steps 2 and 3 are non-negotiable V0.1 compensating controls for the
    /// plaintext-on-disk deviation; both are removed by T0.2.0.
    #[instrument(
        skip(data_dir),
        fields(data_dir = %data_dir.display(), dimension)
    )]
    pub async fn open(data_dir: &Path, dimension: usize) -> VaultResult<Self> {
        if dimension == 0 {
            return Err(VaultError::InvalidInput(
                "vector dimension must be greater than zero".into(),
            ));
        }

        fs::create_dir_all(data_dir)?;

        // ADR-010 compensating control #4: write the loud warning file.
        // Per ADR-014: the file is a SECONDARY safety control. If the data
        // dir is read-only / quota-exceeded / otherwise un-writable, log a
        // WARN with the underlying error and proceed — failing `open()`
        // here would be a denial-of-service against legitimate use, and
        // the primary safety control (the WARN log below) still fires.
        if let Err(e) = write_alpha_warning(data_dir) {
            warn!(
                error = %e,
                data_dir = %data_dir.display(),
                "ALPHA warning file write failed (data dir may be read-only \
                 or out of space). Continuing because the startup WARN log is \
                 the primary safety control — see ADR-014."
            );
        }

        // ADR-010 compensating control #3 (PRIMARY): WARN every open while
        // plaintext. This fires regardless of whether the secondary ALPHA
        // file write succeeded.
        warn!(
            data_dir = %data_dir.display(),
            "LanceDB data dir is plaintext (V0.1 alpha — see ADR-010). \
             Encryption layer ships in T0.2.0."
        );

        let uri = data_dir.to_string_lossy().to_string();
        let connection = lancedb::connect(&uri)
            .execute()
            .await
            .map_err(|e| VaultError::Storage(format!("lancedb connect: {e}")))?;

        let table = open_or_create_table(&connection, dimension).await?;

        info!(table = TABLE_NAME, dimension, "LanceVectorStore opened");

        // The `Connection` is dropped here; `Table` holds its own internal
        // reference to keep the connection alive for the store's lifetime.
        let _ = connection;

        Ok(Self { table, dimension })
    }
}

#[async_trait]
impl VectorStore for LanceVectorStore {
    #[instrument(skip(self, embedding), fields(id = %id.0, boundary = %boundary, dim = embedding.len()))]
    async fn upsert(
        &self,
        id: &MemoryId,
        embedding: &[f32],
        boundary: &Boundary,
    ) -> VaultResult<()> {
        if embedding.len() != self.dimension {
            return Err(VaultError::DimensionMismatch {
                expected: self.dimension,
                actual: embedding.len(),
            });
        }

        let schema = Arc::new(make_schema(self.dimension));
        let batch = build_record_batch(schema.clone(), id, embedding, boundary)?;
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema);

        // SECURITY-CRITICAL: matching column for merge_insert is `id` ONLY.
        // If we accidentally matched on (id, boundary) we would create a
        // duplicate row when a memory moves boundaries — Phase 3 review
        // (Shahbaz, 2026-04-29) called this out explicitly. The
        // `upsert_with_same_id_different_boundary_updates_existing_no_duplicate`
        // test below pins this invariant in code.
        //
        // Builder ergonomics: `when_*` methods take `&mut self` and return
        // `&mut Self`, but `execute` consumes by value. We can't chain
        // through, so configure via mutable bindings then move into execute.
        let mut builder = self.table.merge_insert(&["id"]);
        builder.when_matched_update_all(None);
        builder.when_not_matched_insert_all();
        builder
            .execute(Box::new(reader))
            .await
            .map_err(|e| VaultError::Storage(format!("merge_insert: {e}")))?;

        Ok(())
    }

    #[instrument(skip(self), fields(id = %id.0))]
    async fn delete(&self, id: &MemoryId) -> VaultResult<()> {
        // `id` is a UUID — `MemoryId.0.to_string()` is hex + dashes only,
        // can never contain a quote. Wrap in `quote_sql_string` anyway as
        // defense-in-depth: a future refactor that changes MemoryId's inner
        // type cannot accidentally introduce SQL injection here.
        let predicate = format!("id = {}", quote_sql_string(&id.0.to_string()));
        self.table
            .delete(&predicate)
            .await
            .map_err(|e| VaultError::Storage(format!("delete: {e}")))?;
        Ok(())
    }

    #[instrument(
        skip(self, query, authorized_boundaries),
        fields(
            dim = query.len(),
            limit,
            n_boundaries = authorized_boundaries.len(),
        )
    )]
    async fn search(
        &self,
        query: &[f32],
        limit: usize,
        authorized_boundaries: &[Boundary],
    ) -> VaultResult<Vec<(MemoryId, f32)>> {
        // Mandatory access control: empty authorisation → empty result, no
        // round-trip to LanceDB. This is the runtime expression of the
        // type-level invariant in the trait signature; the trait already
        // forces the caller to pass *some* slice.
        if authorized_boundaries.is_empty() {
            return Ok(Vec::new());
        }

        if query.len() != self.dimension {
            return Err(VaultError::DimensionMismatch {
                expected: self.dimension,
                actual: query.len(),
            });
        }

        let filter = build_boundary_filter(authorized_boundaries);

        let stream = self
            .table
            .query()
            .nearest_to(query)
            .map_err(|e| VaultError::Storage(format!("nearest_to: {e}")))?
            .only_if(&filter)
            .limit(limit)
            .distance_type(DistanceType::Cosine)
            .execute()
            .await
            .map_err(|e| VaultError::Storage(format!("query execute: {e}")))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| VaultError::Storage(format!("collect batches: {e}")))?;

        let mut out = Vec::with_capacity(limit);
        for batch in &batches {
            let ids = batch
                .column_by_name("id")
                .ok_or_else(|| VaultError::Storage("missing `id` column".into()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| VaultError::Storage("`id` column not Utf8".into()))?;
            let distances = batch
                .column_by_name("_distance")
                .ok_or_else(|| VaultError::Storage("missing `_distance` column".into()))?
                .as_any()
                .downcast_ref::<Float32Array>()
                .ok_or_else(|| VaultError::Storage("`_distance` column not Float32".into()))?;

            for i in 0..batch.num_rows() {
                let id_str = ids.value(i);
                let uuid = uuid::Uuid::parse_str(id_str)
                    .map_err(|e| VaultError::Storage(format!("invalid uuid in row {i}: {e}")))?;
                out.push((MemoryId(uuid), distances.value(i)));
            }
        }

        Ok(out)
    }

    #[instrument(skip(self), fields(boundary = boundary.map(Boundary::as_str)))]
    async fn count(&self, boundary: Option<&Boundary>) -> VaultResult<usize> {
        let filter = boundary.map(|b| format!("boundary = {}", quote_sql_string(b.as_str())));
        self.table
            .count_rows(filter)
            .await
            .map_err(|e| VaultError::Storage(format!("count_rows: {e}")))
    }

    fn dimension(&self) -> usize {
        self.dimension
    }
}

/// Write (or refresh) the V0.1 alpha-warning file in `data_dir` and set it
/// read-only.
///
/// `Permissions::set_readonly(true)` is the cross-platform primitive — on
/// Unix it clears write bits, on Windows it sets `FILE_ATTRIBUTE_READONLY`.
/// We deliberately re-write the body on every open so the timestamp is
/// fresh and the message can't be overwritten by accident.
fn write_alpha_warning(data_dir: &Path) -> VaultResult<()> {
    let path = data_dir.join(ALPHA_WARNING_FILENAME);
    let now = Utc::now().to_rfc3339();
    let body = format!(
        "ALPHA BUILD — vector data is stored UNENCRYPTED on disk.\n\
         \n\
         Do NOT put real personal data, credentials, or sensitive\n\
         information into this vault. Encryption ships in V0.2 (task\n\
         T0.2.0) before any beta user receives the product.\n\
         \n\
         See ADR-010 in HANDOFF.md for full context.\n\
         \n\
         File last refreshed (UTC): {now}\n",
    );

    // If the file already exists and is read-only from a previous open,
    // make it writable so we can refresh the body. Otherwise fs::write
    // fails with "permission denied" on Windows.
    if path.exists() {
        let mut perms = fs::metadata(&path)?.permissions();
        if perms.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            fs::set_permissions(&path, perms)?;
        }
    }

    fs::write(&path, body)?;

    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms)?;

    Ok(())
}

async fn open_or_create_table(connection: &Connection, dimension: usize) -> VaultResult<Table> {
    let table_names = connection
        .table_names()
        .execute()
        .await
        .map_err(|e| VaultError::Storage(format!("list tables: {e}")))?;

    if table_names.iter().any(|n| n == TABLE_NAME) {
        connection
            .open_table(TABLE_NAME)
            .execute()
            .await
            .map_err(|e| VaultError::Storage(format!("open_table {TABLE_NAME}: {e}")))
    } else {
        let schema = Arc::new(make_schema(dimension));
        connection
            .create_empty_table(TABLE_NAME, schema)
            .execute()
            .await
            .map_err(|e| VaultError::Storage(format!("create_empty_table {TABLE_NAME}: {e}")))
    }
}

fn make_schema(dimension: usize) -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension as i32,
            ),
            false,
        ),
        Field::new("boundary", DataType::Utf8, false),
    ])
}

/// Build a single-row Arrow `RecordBatch` for one (id, embedding, boundary)
/// triple. Used by [`LanceVectorStore::upsert`].
///
/// Caller is responsible for embedding-length validation (the trait method
/// checks against `self.dimension` before calling).
fn build_record_batch(
    schema: Arc<Schema>,
    id: &MemoryId,
    embedding: &[f32],
    boundary: &Boundary,
) -> VaultResult<RecordBatch> {
    let id_array = Arc::new(StringArray::from(vec![id.0.to_string()]));
    let boundary_array = Arc::new(StringArray::from(vec![boundary.as_str().to_string()]));

    let values = Float32Array::from(embedding.to_vec());
    let item_field = Arc::new(Field::new("item", DataType::Float32, true));
    let embedding_array = Arc::new(FixedSizeListArray::new(
        item_field,
        embedding.len() as i32,
        Arc::new(values),
        None,
    ));

    RecordBatch::try_new(schema, vec![id_array, embedding_array, boundary_array])
        .map_err(|e| VaultError::Storage(format!("record batch: {e}")))
}

/// Build the LanceDB `only_if` boundary filter for a search.
///
/// SECURITY-CRITICAL CONSTRUCTION SITE (BRD §11.4.3, defense-in-depth per
/// BRD §11.7.1).
///
/// LanceDB 0.8's `VectorQuery::only_if` is a string-only SQL filter — there
/// is no parameter-binding API. Two layers of protection sit between user
/// input and this site:
///
/// 1. **Type-level (vault-core/`Boundary`):** the `Boundary` newtype
///    validates input on construction to `[a-zA-Z0-9_-]{1,64}`. By the
///    time a `Boundary` reaches this function, it cannot contain a quote,
///    semicolon, space, or any other SQL metacharacter — `Boundary::new`
///    rejected it.
/// 2. **Construction-site (this function):** every value is passed through
///    [`quote_sql_string`], which doubles any embedded single quotes per
///    standard SQL string-literal escaping. Even if a future refactor
///    weakens `Boundary` validation, this site cannot be the entry point
///    for SQL injection on its own.
///
/// Both layers must hold for the security argument to hold. Do not
/// concatenate boundary strings directly into the filter without going
/// through `quote_sql_string`. Do not relax `Boundary` validation without
/// reviewing this function and the matching test in vault-core.
///
/// Caller guarantees `boundaries` is non-empty (the trait method
/// returns early on an empty slice).
pub(crate) fn build_boundary_filter(boundaries: &[Boundary]) -> String {
    let quoted: Vec<String> = boundaries
        .iter()
        .map(|b| quote_sql_string(b.as_str()))
        .collect();
    format!("boundary IN ({})", quoted.join(", "))
}

/// Quote an arbitrary string as a SQL string literal: wrap in single
/// quotes, double any embedded single quote.
///
/// This is the defense-in-depth half of [`build_boundary_filter`]'s
/// security argument. `Boundary` already restricts inputs to a charset
/// that cannot contain `'`; this function makes the SQL construction site
/// safe even if that invariant were ever weakened.
pub(crate) fn quote_sql_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push('\'');
        }
        out.push(c);
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    //! Phase 2 tests cover `open()` semantics + the ADR-010 compensating
    //! controls. The Phase 3 stubs at the bottom exercise the trait methods
    //! and *will fail* until Phase 3 implements them — that's the TDD
    //! red-bar for the rest of the task. They live here now so the trait's
    //! intended behaviour is locked in code before the implementation
    //! lands.
    use super::*;
    use tempfile::TempDir;
    use vault_core::MemoryId;

    fn embedding(dim: usize, fill: f32) -> Vec<f32> {
        (0..dim).map(|_| fill).collect()
    }

    fn new_id() -> MemoryId {
        MemoryId(uuid::Uuid::now_v7())
    }

    // ============================================================
    //   Phase 2 — open() semantics + ADR-010 compensating controls
    // ============================================================

    #[tokio::test]
    async fn open_creates_data_dir() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().join("vault-lance");
        assert!(!data_dir.exists());
        let _store = LanceVectorStore::open(&data_dir, 384).await.unwrap();
        assert!(data_dir.exists());
        assert!(data_dir.is_dir());
    }

    #[tokio::test]
    async fn open_writes_alpha_warning_file() {
        let tmp = TempDir::new().unwrap();
        let _store = LanceVectorStore::open(tmp.path(), 384).await.unwrap();
        let alpha = tmp.path().join(ALPHA_WARNING_FILENAME);
        assert!(alpha.exists(), "ALPHA warning file must exist after open");
        let body = fs::read_to_string(&alpha).unwrap();
        assert!(body.contains("ALPHA BUILD"));
        assert!(body.contains("UNENCRYPTED"));
        assert!(body.contains("ADR-010"));
        assert!(body.contains("T0.2.0"));
    }

    #[tokio::test]
    async fn alpha_warning_file_is_read_only_cross_platform() {
        let tmp = TempDir::new().unwrap();
        let _store = LanceVectorStore::open(tmp.path(), 384).await.unwrap();
        let alpha = tmp.path().join(ALPHA_WARNING_FILENAME);
        let perms = fs::metadata(&alpha).unwrap().permissions();
        assert!(
            perms.readonly(),
            "ALPHA warning file must be read-only \
             (Unix: write bits cleared; Windows: FILE_ATTRIBUTE_READONLY set)"
        );
    }

    /// ADR-014: if the ALPHA file write fails (read-only data dir, quota,
    /// FS error), `open()` must STILL succeed. The startup WARN log is the
    /// primary safety control; the file is secondary. We force the failure
    /// by pre-creating the alpha *path* as a directory — `fs::write` then
    /// fails because the path is a directory, but the rest of `open()`
    /// proceeds.
    #[tokio::test]
    async fn open_succeeds_when_alpha_file_write_fails_per_adr_014() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path();

        // Pre-create the alpha *path* as a directory. Subsequent fs::write
        // calls to this path will fail on every platform we support.
        let alpha_path = data_dir.join(ALPHA_WARNING_FILENAME);
        fs::create_dir(&alpha_path).unwrap();
        assert!(alpha_path.is_dir());

        // open() must succeed despite the alpha-file write failure.
        let store = LanceVectorStore::open(data_dir, 4).await.unwrap();

        // The alpha path is still a directory (the failed fs::write didn't
        // overwrite it) and the LanceDB store is otherwise functional.
        assert!(
            alpha_path.is_dir(),
            "alpha path was clobbered by failed write"
        );
        assert_eq!(store.dimension(), 4);
        assert_eq!(store.count(None).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn open_refreshes_alpha_file_even_when_existing_is_read_only() {
        // Reproduces the realistic case where the alpha file is left from a
        // previous run. Without the explicit "make writable, rewrite,
        // make read-only" sequence, fs::write would fail with permission
        // denied on Windows.
        let tmp = TempDir::new().unwrap();
        let _s1 = LanceVectorStore::open(tmp.path(), 384).await.unwrap();
        drop(_s1);
        let _s2 = LanceVectorStore::open(tmp.path(), 384).await.unwrap();
        let alpha = tmp.path().join(ALPHA_WARNING_FILENAME);
        assert!(alpha.exists());
        assert!(fs::metadata(&alpha).unwrap().permissions().readonly());
    }

    #[tokio::test]
    async fn open_rejects_zero_dimension() {
        let tmp = TempDir::new().unwrap();
        let result = LanceVectorStore::open(tmp.path(), 0).await;
        match result {
            Err(VaultError::InvalidInput(_)) => {}
            _ => panic!("expected InvalidInput for dimension=0"),
        }
    }

    #[tokio::test]
    async fn dimension_returns_configured_value() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 384).await.unwrap();
        assert_eq!(store.dimension(), 384);
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        // Reopening the same data dir should pick up the existing table,
        // not error.
        let tmp = TempDir::new().unwrap();
        {
            let _s1 = LanceVectorStore::open(tmp.path(), 384).await.unwrap();
        }
        let _s2 = LanceVectorStore::open(tmp.path(), 384).await.unwrap();
    }

    // ============================================================
    //   Phase 3 — VectorStore method behaviour
    // ============================================================

    #[tokio::test]
    async fn upsert_then_search_returns_id() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let id = new_id();
        store.upsert(&id, &embedding(4, 1.0), &work).await.unwrap();
        let hits = store
            .search(&embedding(4, 1.0), 5, std::slice::from_ref(&work))
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, id);
    }

    #[tokio::test]
    async fn search_with_no_authorized_boundaries_returns_empty() {
        // Mandatory access control — empty authorisation = no results.
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        store
            .upsert(&new_id(), &embedding(4, 1.0), &work)
            .await
            .unwrap();
        let hits = store.search(&embedding(4, 1.0), 5, &[]).await.unwrap();
        assert!(
            hits.is_empty(),
            "no authorised boundaries → no results (BRD §11.4.3)"
        );
    }

    #[tokio::test]
    async fn search_does_not_leak_unauthorized_boundary() {
        // The boundary-leak invariant: a search authorised only for
        // "work" must never surface a memory written under "personal".
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let personal = Boundary::new("personal").unwrap();
        let work_id = new_id();
        let personal_id = new_id();
        store
            .upsert(&work_id, &embedding(4, 1.0), &work)
            .await
            .unwrap();
        store
            .upsert(&personal_id, &embedding(4, 1.0), &personal)
            .await
            .unwrap();

        let hits = store
            .search(&embedding(4, 1.0), 5, std::slice::from_ref(&work))
            .await
            .unwrap();
        let ids: Vec<&MemoryId> = hits.iter().map(|(id, _)| id).collect();
        assert!(ids.iter().any(|id| **id == work_id));
        assert!(
            ids.iter().all(|id| **id != personal_id),
            "personal-boundary memory leaked into a work-only search"
        );
    }

    #[tokio::test]
    async fn delete_removes_from_search() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let id = new_id();
        store.upsert(&id, &embedding(4, 1.0), &work).await.unwrap();
        store.delete(&id).await.unwrap();
        let hits = store
            .search(&embedding(4, 1.0), 5, std::slice::from_ref(&work))
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn delete_absent_id_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        // No upsert beforehand — deletion should still succeed.
        store.delete(&new_id()).await.unwrap();
    }

    #[tokio::test]
    async fn count_with_and_without_boundary_filter() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let personal = Boundary::new("personal").unwrap();
        store
            .upsert(&new_id(), &embedding(4, 1.0), &work)
            .await
            .unwrap();
        store
            .upsert(&new_id(), &embedding(4, 1.0), &work)
            .await
            .unwrap();
        store
            .upsert(&new_id(), &embedding(4, 1.0), &personal)
            .await
            .unwrap();
        assert_eq!(store.count(None).await.unwrap(), 3);
        assert_eq!(store.count(Some(&work)).await.unwrap(), 2);
        assert_eq!(store.count(Some(&personal)).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn upsert_rejects_dimension_mismatch() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let result = store.upsert(&new_id(), &embedding(8, 1.0), &work).await;
        match result {
            Err(VaultError::DimensionMismatch {
                expected: 4,
                actual: 8,
            }) => {}
            _ => panic!("expected DimensionMismatch{{expected:4, actual:8}} for upsert"),
        }
    }

    #[tokio::test]
    async fn search_rejects_dimension_mismatch() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let result = store
            .search(&embedding(8, 1.0), 5, std::slice::from_ref(&work))
            .await;
        match result {
            Err(VaultError::DimensionMismatch {
                expected: 4,
                actual: 8,
            }) => {}
            _ => panic!("expected DimensionMismatch{{expected:4, actual:8}} for search"),
        }
    }

    /// Phase 3 review (Shahbaz, 2026-04-29): the merge_insert primitive
    /// must match on `id` only. If it ever matches on (id, boundary) or
    /// any other column combination, a memory that moves boundaries
    /// produces a duplicate row instead of an update — silent data
    /// corruption. This test pins the invariant in code.
    #[tokio::test]
    async fn upsert_with_same_id_different_boundary_updates_existing_no_duplicate() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let personal = Boundary::new("personal").unwrap();
        let id = new_id();

        // First upsert under "work".
        store.upsert(&id, &embedding(4, 1.0), &work).await.unwrap();
        assert_eq!(store.count(None).await.unwrap(), 1);
        assert_eq!(store.count(Some(&work)).await.unwrap(), 1);

        // Re-upsert SAME id under "personal" — must update in place.
        store
            .upsert(&id, &embedding(4, 0.5), &personal)
            .await
            .unwrap();

        assert_eq!(
            store.count(None).await.unwrap(),
            1,
            "merge_insert must not duplicate when boundary changes"
        );
        assert_eq!(
            store.count(Some(&work)).await.unwrap(),
            0,
            "old boundary must no longer hold the row"
        );
        assert_eq!(
            store.count(Some(&personal)).await.unwrap(),
            1,
            "new boundary must own the row"
        );

        // Cross-check via search: work search no longer finds the id;
        // personal search does.
        let work_hits = store
            .search(&embedding(4, 1.0), 5, std::slice::from_ref(&work))
            .await
            .unwrap();
        assert!(
            work_hits.iter().all(|(rid, _)| *rid != id),
            "id leaked back to old boundary via search"
        );
        let personal_hits = store
            .search(&embedding(4, 1.0), 5, std::slice::from_ref(&personal))
            .await
            .unwrap();
        assert!(
            personal_hits.iter().any(|(rid, _)| *rid == id),
            "id missing from new boundary's search result"
        );
    }

    #[tokio::test]
    async fn concurrent_upserts_all_succeed() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();

        let mut handles = Vec::new();
        for i in 0..20 {
            let store = store.clone();
            let work = work.clone();
            handles.push(tokio::spawn(async move {
                let id = MemoryId(uuid::Uuid::now_v7());
                store
                    .upsert(&id, &embedding(4, i as f32), &work)
                    .await
                    .unwrap();
                id
            }));
        }
        let mut ids = Vec::with_capacity(20);
        for h in handles {
            ids.push(h.await.unwrap());
        }

        assert_eq!(ids.len(), 20);
        assert_eq!(store.count(None).await.unwrap(), 20);
        // Every id must be searchable under "work".
        let hits = store
            .search(&embedding(4, 0.0), 100, std::slice::from_ref(&work))
            .await
            .unwrap();
        let hit_ids: std::collections::HashSet<MemoryId> =
            hits.into_iter().map(|(id, _)| id).collect();
        for id in &ids {
            assert!(hit_ids.contains(id), "concurrent insert lost id {}", id.0);
        }
    }

    // ============================================================
    //   Construction-site safety: SQL string quoting + filter shape
    // ============================================================

    #[test]
    fn quote_sql_string_doubles_embedded_quotes() {
        // Boundary's charset rejects `'`, but defense-in-depth means this
        // function must still escape correctly if reached with one.
        assert_eq!(quote_sql_string("plain"), "'plain'");
        assert_eq!(quote_sql_string("o'clock"), "'o''clock'");
        assert_eq!(quote_sql_string("'"), "''''");
        assert_eq!(quote_sql_string(""), "''");
    }

    #[test]
    fn build_boundary_filter_uses_quoted_in_clause() {
        let work = Boundary::new("work").unwrap();
        let personal = Boundary::new("personal").unwrap();
        assert_eq!(
            build_boundary_filter(std::slice::from_ref(&work)),
            "boundary IN ('work')"
        );
        assert_eq!(
            build_boundary_filter(&[work.clone(), personal.clone()]),
            "boundary IN ('work', 'personal')"
        );
    }

    // ============================================================
    //   Boundary-leak property test (Heavy crate, BRD §7.1)
    // ============================================================

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(8))]

        /// Adversarial: for any partition of memories across a set of
        /// boundaries, a search authorising any subset of those
        /// boundaries must return only ids whose boundary is in the
        /// authorised subset. No boundary leakage under any pattern of
        /// writes + auth choice.
        #[test]
        fn search_never_returns_unauthorized_boundary(
            // 2..5 distinct boundary names from the safe charset
            boundary_names in proptest::collection::hash_set(
                "[a-z]{4,8}",
                2..5,
            ),
            // Per boundary, 1..6 memories
            per_boundary_count in proptest::collection::vec(1usize..6, 2..5),
            // Authorised-subset bitmask (size matches boundary count later)
            auth_mask in proptest::collection::vec(proptest::bool::weighted(0.6), 2..5),
        ) {
            tokio_test::block_on(async move {
                let tmp = TempDir::new().unwrap();
                let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();

                let names: Vec<String> = boundary_names.into_iter().collect();
                let n = names.len().min(per_boundary_count.len()).min(auth_mask.len());
                if n == 0 {
                    return Ok(());
                }
                let names = &names[..n];
                let counts = &per_boundary_count[..n];
                let mask = &auth_mask[..n];

                // Track every (id, boundary_index) we insert.
                let mut planted: Vec<(MemoryId, usize)> = Vec::new();
                for (bi, name) in names.iter().enumerate() {
                    let b = Boundary::new(name.as_str()).unwrap();
                    for _ in 0..counts[bi] {
                        let id = new_id();
                        store.upsert(&id, &embedding(4, 1.0), &b).await.unwrap();
                        planted.push((id, bi));
                    }
                }

                // Build the authorised set from the mask. Skip the case
                // where no boundary is authorised — that's covered by
                // `search_with_no_authorized_boundaries_returns_empty`.
                let authorised: Vec<Boundary> = names
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| mask[*i])
                    .map(|(_, n)| Boundary::new(n.as_str()).unwrap())
                    .collect();
                if authorised.is_empty() {
                    return Ok(());
                }
                let authorised_indices: std::collections::HashSet<usize> = mask
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| if *b { Some(i) } else { None })
                    .collect();

                let total = planted.len();
                let hits = store
                    .search(&embedding(4, 1.0), total, &authorised)
                    .await
                    .unwrap();

                for (rid, _) in &hits {
                    let (_, bi) = planted
                        .iter()
                        .find(|(pid, _)| pid == rid)
                        .expect("hit must correspond to a planted id");
                    proptest::prop_assert!(
                        authorised_indices.contains(bi),
                        "boundary leak: id {} from unauthorised boundary {} surfaced",
                        rid.0,
                        names[*bi],
                    );
                }
                Ok(())
            })?;
        }
    }
}
