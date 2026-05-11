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

use uuid::Uuid;

use arrow_array::{
    Array, FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use chrono::TimeDelta;
// `Utc` is only used by the gated `write_alpha_warning` (timestamp in the
// ALPHA marker body) per sub-task (b)+(c) P4 bundle (2026-05-11). Without
// the gate, non-feature-enabled builds trip unused-import under -D warnings.
#[cfg(any(test, feature = "v0_1_migration"))]
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::connection::Connection;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::{CompactionOptions, OptimizeAction, Table};
use lancedb::{DistanceType, ObjectStoreRegistry, Session};
use tokio::sync::Mutex;
use tracing::{info, instrument};
// `warn` is only used by the gated plaintext `LanceVectorStore::open`
// (ADR-010 compensating control #3 WARN log) per sub-task (b)+(c) P4
// bundle (2026-05-11). Without this gate, non-feature-enabled builds
// trip the unused-import lint under -D warnings.
#[cfg(any(test, feature = "v0_1_migration"))]
use tracing::warn;
use zeroize::Zeroizing;

use vault_core::{Boundary, MemoryId, VaultError, VaultResult};

use crate::sealed_object_store::{
    make_vault_sealed_uri, SealedFileStoreProvider, VAULT_SEALED_SCHEME,
};

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

    /// Returns `true` if the vector store has a row for `id`.
    ///
    /// Used by the divergence detector (T0.1.6 Phase C2) to verify that
    /// every SQLite memory has a matching LanceDB row. Implementations
    /// should make this O(1) — LanceDB's `count_rows` with an `id =
    /// '<uuid>'` filter is the canonical shape (the UUID charset cannot
    /// contain a quote, but [`crate::vector_store::quote_sql_string`]
    /// is the defense-in-depth construction site).
    ///
    /// Existence check, not content hash — content lives in SQLite,
    /// embeddings in LanceDB; cross-store content comparison would
    /// require redundant storage. Existence is the right invariant.
    async fn contains(&self, id: &MemoryId) -> VaultResult<bool>;

    /// Embedding dimension this store expects. Used by the cascading backend
    /// (T0.1.6) to validate compatibility before forwarding writes.
    fn dimension(&self) -> usize;

    /// Eager validation that the store is readable end-to-end (not merely
    /// open-able). Used by `StorageBackend::open` (T0.1.6 Phase C, ADR-018)
    /// to surface hard fragment corruption immediately, not the next time
    /// the user searches.
    ///
    /// **Contract — load-bearing, do not relax in implementations:**
    /// - The check MUST be a *minimum-cost end-to-end read that exercises
    ///   data-decode*. **Not** metadata-only.
    /// - Counts, manifests, version files, and other metadata-only paths
    ///   are NOT sufficient — the LanceDB corruption spike (2026-04-30,
    ///   `crates/vault-storage/examples/lance_corruption_spike.rs`) showed
    ///   that LanceDB's row count and metadata both succeed on a store
    ///   whose fragment data is corrupted to unreadability. A
    ///   metadata-only impl recreates that blind spot.
    /// - The recommended shape is "scan the row with smallest UUID, ORDER
    ///   BY id ASC LIMIT 1" — deterministic, cheap, exercises the full
    ///   decode path. Empty store validates vacuously (no decode happens
    ///   because there are no rows; this is correct — there's nothing to
    ///   corrupt).
    ///
    /// Returns `Ok(())` on a clean / empty store. Returns `Err` carrying
    /// the underlying decode error on fragment corruption — the caller
    /// (`StorageBackend::open`) translates this into a CRITICAL audit
    /// event and degraded-mode flag rather than failing `open()`
    /// outright (per ADR-010 / Phase A Change 1: founders must retain
    /// vault-cli access for triage even when the vector store is unreadable).
    async fn validate_readable(&self) -> VaultResult<()>;
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
    /// Serialises concurrent [`VectorStore::upsert`] calls to bound the
    /// peak memory of lancedb 0.27's `merge_insert` path. lance 4.0 routes
    /// `merge_insert` through datafusion's full JOIN planner
    /// (`HashJoinExec` + `ExternalSorter` + `RepartitionExec`); each
    /// concurrent call independently spawns its own physical plan with
    /// `target_partitions = get_num_compute_intensive_cpus().min(8)` and
    /// no RAM ceiling — 20 concurrent calls allocated 8 GB and aborted
    /// the process on Windows in the V0.1→V0.2 lancedb 0.8→0.27.2
    /// dep-bump (Phase 0a). See ADR-038, plus upstream:
    /// lance-format/lance#1983 (configurable RAM limit) and
    /// lance-format/lance#3601 (spill config too small with many cores).
    /// Defense-in-depth pairs with the `LANCE_MEM_POOL_SIZE=268435456`
    /// shell-level ceiling enforced at every process-launch site
    /// (`.cargo/config.toml`, CI workflow, V0.2 alpha-distribution
    /// launchers).
    upsert_lock: Arc<Mutex<()>>,
    /// Owned [`Session`] for the at-rest sealed-storage path. `None`
    /// when [`Self::open`] (the V0.1 plaintext path) constructs the
    /// store; `Some(_)` when [`Self::open_with_at_rest_key`] (the
    /// T0.2.0 sealed path) constructs it. The [`SealedFileStoreProvider`]
    /// registered in the session's [`ObjectStoreRegistry`] holds the
    /// at-rest key and is invoked lazily by Lance on first I/O. Holding
    /// this `Arc` here keeps the provider alive for the lifetime of the
    /// store — without it, Lance might reach a registered provider that
    /// has been dropped.
    ///
    /// The leading underscore is intentional: the field is never read
    /// after construction. Its sole purpose is the lifetime guarantee.
    _session: Option<Arc<Session>>,
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
    ///
    /// **Visibility (T0.2.0 Phase 3 sub-task (b)+(c), 2026-05-11):** gated to
    /// `#[cfg(any(test, feature = "v0_1_migration"))]` per HANDOFF.md iteration 4
    /// §4 amendment. The `any(test, feature)` form keeps vault-storage's own unit
    /// tests compiling without needing the feature flag (cfg(test) applies inside
    /// the crate's own test build); the `feature` half exposes the plaintext path
    /// to downstream crates (vault-tauri, vault-retrieval's tests) that opt in by
    /// enabling `v0_1_migration` on their vault-storage dep. vault-cli does NOT
    /// enable the feature; its per-package build excludes plaintext open from the
    /// binary.
    ///
    /// **`pub` (not `pub(crate)`) — iteration 4 §4 amendment retraction (2026-05-11
    /// sub-task (b)+(c) recon iteration 4):** the original §4 wording locked
    /// `pub(crate)`, but the local DoD-gate run surfaced that vault-retrieval's
    /// tests at `crates/vault-retrieval/tests/common/mod.rs:103` +
    /// `crates/vault-retrieval/src/strategies/semantic.rs:354` call this function
    /// from a separate crate's test code. `pub(crate)` blocks cross-crate access
    /// even with the feature flag enabled. Iteration 4 §1's caller enumeration
    /// missed these. The feature flag (cfg-gate) remains the architectural gate
    /// controlling EXISTENCE — `pub` vs `pub(crate)` only controls VISIBILITY when
    /// the function exists. The "callable from migration.rs only" intent is
    /// preserved at the user-distribution surface (vault-cli's binary excludes
    /// plaintext open via its per-package build without the feature); test
    /// consumers (vault-retrieval) opt in via dev-deps. Sub-task (d) later
    /// migrates vault-retrieval's tests to sealed open_with_at_rest_key, at
    /// which point the test-side feature dep can be removed.
    #[cfg(any(test, feature = "v0_1_migration"))]
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

        Ok(Self {
            table,
            dimension,
            upsert_lock: Arc::new(Mutex::new(())),
            _session: None,
        })
    }

    /// Open or create a LanceDB store at `data_dir` with at-rest AEAD
    /// sealing keyed off the already-derived 32-byte `at_rest_key`.
    ///
    /// **Caller MUST pass the already-derived at-rest key**
    /// (`K3(master_key)` per the ADR-008 amendment K3 KDF —
    /// `blake3::derive_key("vault memory at-rest sealing v1", &master_key)`).
    /// The canonical production derivation site is
    /// [`vault_app::keychain::derive_at_rest_key`] per ADR-040 amendment
    /// ("at_rest_key flows from keychain through AppConfig to migration
    /// consumer"). Re-passing `master_key` here would silently produce
    /// `K3(K3(master_key))` and break seal compatibility with anything
    /// else in the system. The K3-once invariant is locked.
    ///
    /// Routes ALL of Lance's I/O through [`SealedFileStoreProvider`] via
    /// the custom `vault-sealed://` URI scheme registered with a
    /// [`Session`]'s [`ObjectStoreRegistry`]. Per ADR-008 amendment
    /// (Phase 0e) and the Phase 0c spike v2 runtime confirmation, this
    /// is the production at-rest-encryption path; the existing
    /// [`Self::open`] (plaintext) constructor remains for V0.1 alpha
    /// backwards compatibility and is removed at the formal at-rest
    /// gate close.
    ///
    /// Differences from [`Self::open`]:
    /// - Does NOT write the V0.1 ALPHA warning file (ADR-010 banners
    ///   are removed at the formal at-rest gate close).
    /// - Does NOT emit the V0.1 plaintext-on-disk WARN log.
    /// - URI scheme is `vault-sealed://` not the bare absolute path.
    /// - Connection is opened with a session whose registry routes
    ///   `vault-sealed://` to a [`SealedFileStoreProvider`].
    ///
    /// Key handling:
    /// - The caller's `at_rest_key` is consumed by reference; this
    ///   function copies the 32 bytes into a [`Zeroizing`] wrapper held
    ///   inside the [`SealedFileStoreProvider`] for the store's
    ///   lifetime. The copy zeros on drop alongside the provider.
    /// - The caller is responsible for zeroizing the original buffer.
    #[instrument(
        skip(data_dir, at_rest_key),
        fields(data_dir = %data_dir.display(), dimension)
    )]
    pub async fn open_with_at_rest_key(
        data_dir: &Path,
        dimension: usize,
        at_rest_key: &[u8; 32],
    ) -> VaultResult<Self> {
        if dimension == 0 {
            return Err(VaultError::InvalidInput(
                "vector dimension must be greater than zero".into(),
            ));
        }

        fs::create_dir_all(data_dir)?;

        // Wrap the caller's already-derived at-rest key in Zeroizing so
        // the provider-held copy zeros on drop. K3 derivation happens
        // OUTSIDE this function (canonical site: vault-app::keychain).
        let provider_key: Zeroizing<[u8; 32]> = Zeroizing::new(*at_rest_key);

        // Build a registry with our SealedFileStoreProvider registered
        // for the vault-sealed:// scheme. `default()` registers the
        // built-in providers (file://, s3://, az://, gs://, memory://);
        // we add ours alongside.
        let registry = ObjectStoreRegistry::default();
        registry.insert(
            VAULT_SEALED_SCHEME,
            Arc::new(SealedFileStoreProvider::new(provider_key)),
        );
        let session = Arc::new(Session::new(0, 0, Arc::new(registry)));

        // Canonicalise the path before URL-encoding. `Url::from_directory_path`
        // (called inside make_vault_sealed_uri) requires absolute. Most
        // callers pass absolute already, but `data_dir.canonicalize()`
        // handles the edge case where a relative path slipped through.
        let abs = if data_dir.is_absolute() {
            data_dir.to_path_buf()
        } else {
            std::fs::canonicalize(data_dir)?
        };
        let uri = make_vault_sealed_uri(&abs);

        let connection = lancedb::connect(&uri)
            .session(session.clone())
            .execute()
            .await
            .map_err(|e| VaultError::Storage(format!("lancedb sealed connect: {e}")))?;

        let table = open_or_create_table(&connection, dimension).await?;

        info!(
            table = TABLE_NAME,
            dimension, "LanceVectorStore opened (at-rest sealed path)"
        );

        // The `Connection` is dropped here; `Table` holds its own internal
        // reference to keep it alive. We retain the `Session` so the
        // registered provider stays alive for the store's lifetime.
        let _ = connection;

        Ok(Self {
            table,
            dimension,
            upsert_lock: Arc::new(Mutex::new(())),
            _session: Some(session),
        })
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

        // ADR-038: bound peak memory by serialising concurrent merge_insert
        // calls. Held across the merge_insert await so only one lance
        // datafusion plan runs at a time per store. See struct-field doc
        // for the full rationale + upstream cross-links.
        let _guard = self.upsert_lock.lock().await;

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

        // ADR-039 (Phase 0b finding, 2026-05-07): hold the ADR-038 upsert
        // mutex across delete + Prune so the prune cannot race a concurrent
        // upsert (the prune removes old version files; an in-flight upsert
        // creating a new version is left untouched, but holding the mutex
        // is defense-in-depth for the privacy contract).
        let _guard = self.upsert_lock.lock().await;

        self.table
            .delete(&predicate)
            .await
            .map_err(|e| VaultError::Storage(format!("delete: {e}")))?;

        // ADR-039 hard-delete privacy property — Memory Vault sells "user
        // owns and controls their memories, including deletion" as
        // differentiator vs cloud-AI-memory products. lance 4.0 tombstones
        // on `delete()` by default; the row's bytes remain in old version
        // files AND in the original data file fragment until both compaction
        // (rewrites partial fragments dropping tombstoned rows) and prune
        // (removes orphaned old files) run. Without the explicit
        // Compact-then-Prune sequence below, an authorised reader (future
        // agent with token, current agent overstepping scope) could recover
        // content the user thought was deleted via low-level fragment
        // inspection. Encryption at rest (T0.2.0) helps against stolen-disk
        // attackers; does nothing against authorised key holders.
        //
        // ADR-039 amendment (Phase 0c spike Stage E, 2026-05-08): the
        // earlier Phase 0b verification (Prune-alone) covered only the
        // FULL-fragment-delete case (delete every row in a fragment → Lance
        // can drop the empty fragment via Prune alone). Memory Vault's
        // actual delete API is single-memory-id, which is a PARTIAL-fragment
        // delete; under that pattern Lance's `OptimizeAction::Prune` with
        // zero retention reports `data_files_removed: 0` (verified by spike
        // Stage E's 2×2 matrix on both plain file:// and vault-sealed:// —
        // confirmed identical behaviour, ruling out sealing-wrapper
        // interference). The fragment-rewrite work is in `OptimizeAction::
        // Compact`, which drops tombstoned rows from partial fragments;
        // Prune-after-Compact then removes the now-orphaned old data file.
        // Spike Stage E reports `compaction.fragments_removed: 1,
        // fragments_added: 1, files_removed: 2, files_added: 1` followed by
        // `prune.bytes_removed: 159379, data_files_removed: 1` for a 50/100
        // partial-delete on 384-dim embeddings.
        //
        // Trade-off: lose lance time-travel undo capability — accepted as
        // correct for a privacy-property memory vault. Latency cost: each
        // delete pays Compact + Prune (~0.5-2s on test fixture; scales with
        // fragment size, not old-version count). Acceptable for the
        // privacy property.
        //
        // Regression pin: `delete_physically_removes_content_per_adr_039`
        // covers full-fragment delete (boundary-wide). Companion test
        // `delete_partial_fragment_physically_removes_content_per_adr_039`
        // covers single-id partial-fragment delete (the actual API pattern)
        // — fails CI loudly if Compact or Prune is ever removed, or if
        // lance changes its compaction/retention semantics.
        self.table
            .optimize(OptimizeAction::Compact {
                options: CompactionOptions::default(),
                remap_options: None,
            })
            .await
            .map_err(|e| VaultError::Storage(format!("delete: hard-delete compact: {e}")))?;

        self.table
            .optimize(OptimizeAction::Prune {
                older_than: Some(TimeDelta::zero()),
                delete_unverified: Some(true),
                error_if_tagged_old_versions: Some(false),
            })
            .await
            .map_err(|e| VaultError::Storage(format!("delete: hard-delete prune: {e}")))?;

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

    #[instrument(skip(self), fields(id = %id.0))]
    async fn contains(&self, id: &MemoryId) -> VaultResult<bool> {
        // UUID hyphenated form is hex + dashes only — cannot contain a
        // SQL quote — but quote_sql_string is the defense-in-depth
        // construction site, used identically to upsert / search /
        // count. A future refactor that changes MemoryId's inner type
        // cannot accidentally introduce SQL injection here.
        let filter = format!("id = {}", quote_sql_string(&id.0.to_string()));
        let n = self
            .table
            .count_rows(Some(filter))
            .await
            .map_err(|e| VaultError::Storage(format!("count_rows for contains: {e}")))?;
        Ok(n > 0)
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    /// Per the trait contract: minimum-cost end-to-end read that exercises
    /// the data-decode path (NOT metadata-only).
    ///
    /// **Implementation shape (load-bearing).** Scans the `id` column
    /// across all rows AND parses each value as a UUID. The C1 spike
    /// (and the corresponding test) revealed that `try_collect()` alone
    /// returns RecordBatches with garbage bytes on a corrupted store
    /// without erroring — Arrow IPC framing on a corrupted Lance fragment
    /// often round-trips as "valid Arrow batch with nonsense data."
    /// **Decode and parse, not just decode.**
    ///
    /// The full shape: `query().limit(count).execute().try_collect()`,
    /// then walk every batch, downcast the `id` column to `StringArray`,
    /// and `Uuid::parse_str` each value. The UUID parse is what the
    /// spike's `search()` does internally — that's what surfaces "invalid
    /// uuid in row N: ..." on corrupted bytes. Without the parse, the
    /// validation passes vacuously even on a fully-corrupted store.
    ///
    /// Empty stores: `count_rows` returns 0; we short-circuit with
    /// `Ok(())` — vacuous pass.
    ///
    /// Cost on a populated table: ~O(N) — read every id, parse each.
    /// Fine for V0.1's expected store size (≤ tens of thousands of
    /// memories per founder dogfood). If V0.2 grows the store
    /// dramatically, revisit (sample-and-decode probe).
    ///
    /// Spike: `crates/vault-storage/examples/lance_corruption_spike.rs`.
    /// The journey through five alternative shapes that DIDN'T surface
    /// corruption is documented in the commit history for posterity.
    #[instrument(skip(self))]
    async fn validate_readable(&self) -> VaultResult<()> {
        // count_rows is metadata-only (cheap) — verified by spike. Empty
        // store → no rows to decode → vacuous pass.
        let row_count = self
            .table
            .count_rows(None)
            .await
            .map_err(|e| VaultError::Storage(format!("validate_readable count: {e}")))?;
        if row_count == 0 {
            return Ok(());
        }
        let stream = self
            .table
            .query()
            .limit(row_count)
            .execute()
            .await
            .map_err(|e| VaultError::Storage(format!("validate_readable execute: {e}")))?;
        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| VaultError::Storage(format!("validate_readable collect: {e}")))?;
        // Parse every id value — this is what surfaces UUID-decode errors
        // on corrupted-but-Arrow-framing-valid fragments.
        for batch in &batches {
            let ids = batch
                .column_by_name("id")
                .ok_or_else(|| VaultError::Storage("validate_readable: missing id column".into()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    VaultError::Storage("validate_readable: id column not Utf8".into())
                })?;
            for i in 0..ids.len() {
                let id_str = ids.value(i);
                Uuid::parse_str(id_str).map_err(|e| {
                    VaultError::Storage(format!("validate_readable: invalid uuid at row {i}: {e}"))
                })?;
            }
        }
        Ok(())
    }
}

impl LanceVectorStore {
    /// Read every (id, embedding, boundary) tuple from the table for the
    /// V0.1 → V0.2 migration loop. Crate-private — sole consumer is
    /// [`crate::migration::migrate_v0_1_to_sealed_if_needed`]. Speaks
    /// domain types only (per BRD §2.2 — the trait surface never exposes
    /// arrow_array internals); the migration consumer treats the result
    /// as a re-insertable iterator.
    ///
    /// **Iteration shape (locked by Phase 2 plan iteration 2 §1):** uses
    /// the same `query().limit(count).execute().try_collect()` pattern as
    /// `validate_readable()` (line 645 doc comment) — in-tree runtime
    /// evidence that the shape works against lancedb 0.27.2 supersedes
    /// the open question raised in iteration 1 §7.
    ///
    /// **Bulk-collect over streaming.** V0.1 vault scale is bounded by
    /// founder-dogfood reality (ADR-029: 11 memories at V0.1 SHIPPED).
    /// Even at 10K rows × ~1.6 KB/row ≈ 16 MB peak — trivial. Streaming
    /// would be premature optimisation; bulk-collect matches the existing
    /// `validate_readable()` shape exactly.
    ///
    /// Gated to `#[cfg(any(test, feature = "v0_1_migration"))]` per sub-task
    /// (b)+(c) P4 bundle (2026-05-11) — sole caller is migration.rs which
    /// is itself gated on the same feature; without the gate this function
    /// trips dead-code under -D warnings on per-package builds (vault-cli)
    /// that don't enable the feature.
    #[cfg(any(test, feature = "v0_1_migration"))]
    pub(crate) async fn scan_all_rows_for_migration(
        &self,
    ) -> VaultResult<Vec<(MemoryId, Vec<f32>, Boundary)>> {
        let row_count = self
            .table
            .count_rows(None)
            .await
            .map_err(|e| VaultError::Storage(format!("scan_all_rows count: {e}")))?;
        if row_count == 0 {
            return Ok(Vec::new());
        }

        let stream = self
            .table
            .query()
            .limit(row_count)
            .execute()
            .await
            .map_err(|e| VaultError::Storage(format!("scan_all_rows execute: {e}")))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| VaultError::Storage(format!("scan_all_rows collect: {e}")))?;

        let mut out = Vec::with_capacity(row_count);
        for batch in &batches {
            let ids = batch
                .column_by_name("id")
                .ok_or_else(|| VaultError::Storage("scan_all_rows: missing id column".into()))?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| VaultError::Storage("scan_all_rows: id column not Utf8".into()))?;
            let boundaries = batch
                .column_by_name("boundary")
                .ok_or_else(|| {
                    VaultError::Storage("scan_all_rows: missing boundary column".into())
                })?
                .as_any()
                .downcast_ref::<StringArray>()
                .ok_or_else(|| {
                    VaultError::Storage("scan_all_rows: boundary column not Utf8".into())
                })?;
            let embeddings = batch
                .column_by_name("embedding")
                .ok_or_else(|| {
                    VaultError::Storage("scan_all_rows: missing embedding column".into())
                })?
                .as_any()
                .downcast_ref::<FixedSizeListArray>()
                .ok_or_else(|| {
                    VaultError::Storage("scan_all_rows: embedding column not FixedSizeList".into())
                })?;

            for i in 0..batch.num_rows() {
                let id_str = ids.value(i);
                let uuid = Uuid::parse_str(id_str).map_err(|e| {
                    VaultError::Storage(format!("scan_all_rows: invalid uuid at row {i}: {e}"))
                })?;
                let boundary_str = boundaries.value(i);
                let boundary = Boundary::new(boundary_str).map_err(|e| {
                    VaultError::Storage(format!("scan_all_rows: invalid boundary at row {i}: {e}"))
                })?;
                let inner = embeddings.value(i);
                let inner_f32 = inner
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| {
                        VaultError::Storage(
                            "scan_all_rows: embedding inner array not Float32".into(),
                        )
                    })?;
                let embedding: Vec<f32> = inner_f32.values().to_vec();
                out.push((MemoryId(uuid), embedding, boundary));
            }
        }

        Ok(out)
    }
}

/// Write (or refresh) the V0.1 alpha-warning file in `data_dir` and set it
/// read-only.
///
/// `Permissions::set_readonly(true)` is the cross-platform primitive — on
/// Unix it clears write bits, on Windows it sets `FILE_ATTRIBUTE_READONLY`.
/// We deliberately re-write the body on every open so the timestamp is
/// fresh and the message can't be overwritten by accident.
///
/// Gated to `#[cfg(any(test, feature = "v0_1_migration"))]` per sub-task
/// (b)+(c) P4 bundle (2026-05-11) — sole caller is plaintext
/// `LanceVectorStore::open` which is itself gated on the same feature.
#[cfg(any(test, feature = "v0_1_migration"))]
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
    //   ADR-038 Layer 2 — env-var ceiling reaches the test runner
    // ============================================================

    /// Pins the `.cargo/config.toml` `[env]` block (per ADR-038 layer 2):
    /// `LANCE_MEM_POOL_SIZE` MUST be set to `268435456` (256 MiB) in every
    /// `cargo test` process. If the `[env]` block is ever accidentally
    /// removed, modified, or the `force = true` clause is dropped (so a
    /// shell override silently wins), this test fails loudly.
    ///
    /// Companion enforcement: CI's `.github/workflows/ci.yml` `env:` block
    /// sets the same value at the workflow level — this test asserts it
    /// reaches Rust on both dev (via cargo config) and CI (via workflow
    /// env). Failure mode caught: refactor that drops the cargo config
    /// `[env]` entry would let dev runs hit the lance#1983 unbounded-RAM
    /// path while CI passes (CI has its own env: block) — masking the
    /// regression locally.
    #[test]
    fn lance_mem_pool_size_env_var_ceiling_reaches_test_process() {
        let actual = std::env::var("LANCE_MEM_POOL_SIZE").expect(
            "LANCE_MEM_POOL_SIZE missing from test process env — \
             check `.cargo/config.toml` `[env]` block (ADR-038 layer 2)",
        );
        assert_eq!(
            actual, "268435456",
            "LANCE_MEM_POOL_SIZE expected 268435456 (256 MiB) per ADR-038 \
             but found {actual:?} — check `.cargo/config.toml` `[env]` \
             block; `force = true` should be set so config wins over shell"
        );
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

    /// **T0.1.10 Phase 3b — ADR-010 compensating-control #3 PRIMARY pin.**
    ///
    /// Closes the T0.1.4 test gap that ADR-010 line 539 specified
    /// (*"assert the WARN log fires on every open"*) but wasn't
    /// implemented at T0.1.4. The WARN itself has been live in
    /// `LanceVectorStore::open` since T0.1.4 (`vector_store.rs:240-244`);
    /// this test pins it against regression at CI level so a future
    /// "helpful" change that drops the WARN, demotes its level, or
    /// removes the ADR-010 / T0.2.0 references trips CI immediately.
    ///
    /// Asserts three properties:
    /// 1. A `tracing` event is emitted at any level under the
    ///    `vault_storage` scope when `LanceVectorStore::open` runs.
    /// 2. The event message contains the canonical "ADR-010" reference
    ///    (regression check that the ADR citation isn't silently
    ///    dropped from the message).
    /// 3. The event message contains "T0.2.0" (regression check that
    ///    the encryption-deferral milestone reference stays — that
    ///    cross-link is what makes the WARN actionable for an alpha
    ///    user reading their dev console).
    ///
    /// `tracing-test` captures events into a thread-local subscriber per
    /// `#[traced_test]`; the `no-env-filter` workspace feature ensures
    /// WARN events are captured regardless of `RUST_LOG`. Pattern matches
    /// the existing usage at `vault-retrieval/src/strategies/semantic.rs:423-444`.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn open_emits_adr_010_plaintext_warn_log() {
        let tmp = TempDir::new().unwrap();
        let _store = LanceVectorStore::open(tmp.path(), 384).await.unwrap();

        assert!(
            tracing_test::internal::logs_with_scope_contain(
                "vault_storage",
                "LanceDB data dir is plaintext",
            ),
            "ADR-010 compensating-control #3 (PRIMARY) WARN log MUST fire on every \
             LanceVectorStore::open. If this fails, the WARN at vector_store.rs:240-244 \
             has been removed, demoted, or its scope altered. Per ADR-010 line 525, \
             this WARN is the primary safety control while V0.1 ships plaintext-on-disk."
        );
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_storage", "ADR-010",),
            "ADR-010 reference MUST appear in the WARN message — alpha users reading \
             their dev console need the cross-link to the ADR's full context"
        );
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_storage", "T0.2.0",),
            "T0.2.0 reference MUST appear in the WARN message — alpha users need to \
             know which release closes the deviation (encryption layer)"
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

    /// 20 concurrent upserts must all be visible to a subsequent search.
    /// Pins the ADR-038 Layer 1 mutex contract — without serialisation,
    /// this test would either OOM (pre-fix lancedb 0.27 with no RAM
    /// ceiling) or surface fragment-flush races.
    ///
    /// **Why we use embeddings starting from 1.0, not 0.0** (Phase 0a-fix
    /// finding, 2026-05-07): lance 4.0 introduced a Cosine-search NaN
    /// regression where zero-magnitude vectors are excluded from results
    /// (cosine of `[0,0,0,0]` against any other vector is `0/(0*||v||)`
    /// = NaN, and lance 4.0's plan filters NaN rows out — lancedb 0.8 did
    /// not). Three sibling diagnostic tests during Phase 0a-fix confirmed
    /// the bug is metric-specific: same data + zero query + L2 distance
    /// passes; same data + zero query + Cosine fails. Production is
    /// unaffected (BGE-small-en-v1.5 produces L2-normalised non-zero
    /// embeddings); this is a test-only adjustment. Filed as upstream
    /// tech-debt — see HANDOFF.md "Phase 0a-fix Cosine NaN-vector
    /// upstream issue" entry. See ADR-038 Layer 4 sub-section for full
    /// finding narrative.
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
                // Non-zero embeddings (1.0..=20.0) avoid the lance 4.0
                // Cosine-NaN regression on zero-magnitude vectors.
                store
                    .upsert(&id, &embedding(4, (i + 1) as f32), &work)
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
        // Every id must be searchable under "work". Non-zero query for
        // the same lance 4.0 Cosine-NaN reason as the inserts above.
        let hits = store
            .search(&embedding(4, 1.0), 100, std::slice::from_ref(&work))
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
    //   Phase 0b memory-system invariant verifications
    //   (lance 0.8 → 0.27.2 dep upgrade, 2026-05-07)
    // ============================================================

    /// Phase 0b verification 1 — `merge_insert` last-write-wins for the
    /// embedding column, not just for the boundary column.
    ///
    /// The pre-existing test
    /// `upsert_with_same_id_different_boundary_updates_existing_no_duplicate`
    /// verifies the COUNT and BOUNDARY-membership semantics of repeat
    /// upsert on the same id, but its search query
    /// (`embedding(4, 1.0) = [1,1,1,1]`) gives the SAME cosine distance
    /// to both `[1,1,1,1]` and `[0.5,0.5,0.5,0.5]` (both are scalar
    /// multiples of the same direction), so it cannot distinguish
    /// whether the embedding column was actually overwritten. This
    /// gap was found during Phase 0b audit. Without this regression
    /// pin, a lance 4.0 change to "first wins" instead of "last wins"
    /// in `when_matched_update_all(None)` would silently corrupt
    /// memory-vault data on duplicate-id upserts.
    ///
    /// Probe: insert id=X with embedding `[1, 0, 0, 0]`; re-upsert with
    /// `[0, 1, 0, 0]`; query with `[1, 0, 0, 0]`. Cosine distance is 0
    /// to the FIRST embedding (overwritten state) and 1 to the SECOND
    /// (post-update state) — distinguishable. After re-upsert the row's
    /// distance to `[1,0,0,0]` MUST be ≥ 0.5 (i.e. closer to the
    /// post-update embedding than the original).
    #[tokio::test]
    async fn merge_insert_last_write_wins_for_embedding_column() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        let id = new_id();

        let original_emb = vec![1.0_f32, 0.0, 0.0, 0.0];
        let updated_emb = vec![0.0_f32, 1.0, 0.0, 0.0];

        store.upsert(&id, &original_emb, &work).await.unwrap();
        store.upsert(&id, &updated_emb, &work).await.unwrap();

        // Query with the ORIGINAL embedding direction. If
        // `when_matched_update_all(None)` is "last wins" (correct
        // semantics), the row's stored embedding is now `updated_emb`,
        // and cosine distance to `original_emb` should be ~1.0
        // (orthogonal vectors). If "first wins" (regression), the row's
        // stored embedding is still `original_emb` and cosine distance
        // would be ~0.0.
        let hits = store
            .search(&original_emb, 5, std::slice::from_ref(&work))
            .await
            .unwrap();
        let (rid, distance) = hits
            .iter()
            .find(|(rid, _)| *rid == id)
            .copied()
            .expect("re-upserted id must be present in search results");
        assert_eq!(rid, id);
        assert!(
            distance > 0.5,
            "merge_insert MUST be last-write-wins for embedding column; \
             expected cosine distance to original_emb > 0.5 (vectors now \
             orthogonal), got {distance} — lance 4.0 may have regressed \
             when_matched_update_all semantics"
        );
    }

    /// ADR-039 regression pin: `delete()` MUST physically remove the
    /// row's content from disk, not just tombstone it.
    ///
    /// Memory Vault sells "user owns and controls their memories,
    /// including deletion" as the differentiator vs cloud-AI-memory
    /// products. Tombstoned-recoverable-with-key fails this contract
    /// for the realistic threat model — authorised readers (future
    /// agent with token, current agent overstepping scope) can recover
    /// content the user thought was deleted via low-level fragment
    /// inspection. Encryption at rest (T0.2.0) helps against
    /// stolen-disk attackers; does nothing against authorised key
    /// holders.
    ///
    /// Phase 0b verification (2026-05-07) discovered lance 4.0
    /// tombstones on delete by default and `OptimizeAction::All` is
    /// insufficient (default 7-day retention preserves old version
    /// files containing deleted data); only
    /// `OptimizeAction::Prune { older_than: TimeDelta::zero(), ... }`
    /// achieves physical removal. ADR-039 modifies
    /// `LanceVectorStore::delete()` to call zero-retention Prune
    /// immediately after the lance-side delete; this test pins the
    /// privacy-contract invariant in code so any future regression
    /// (lance API change, accidental Prune-call removal) fails CI
    /// loudly.
    ///
    /// Probe pattern: write rows with a distinctive boundary string,
    /// delete them, then scan every file under the data dir for the
    /// raw bytes of that string. Assertion: 0 files contain the
    /// string. If this test ever fails, the privacy contract is
    /// broken and a beta cohort cannot receive the build.
    #[tokio::test]
    async fn delete_physically_removes_content_per_adr_039() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        // Distinctive boundary string. Stays within Boundary's
        // [a-zA-Z0-9_-]{1,64} charset (ADR-005). Long + unique enough
        // that random byte collisions are negligible.
        let boundary = Boundary::new("ADR_039_HARD_DELETE_PRIVACY_PROBE").unwrap();
        let probe_str = "ADR_039_HARD_DELETE_PRIVACY_PROBE";

        let mut ids = Vec::new();
        for i in 0..5 {
            let id = new_id();
            store
                .upsert(&id, &embedding(4, (i + 1) as f32), &boundary)
                .await
                .unwrap();
            ids.push(id);
        }
        assert_eq!(store.count(None).await.unwrap(), 5);

        // Delete every row. ADR-039 requires this to physically remove
        // the content from disk via the prune call inside `delete()`.
        for id in &ids {
            store.delete(id).await.unwrap();
        }

        // User-facing semantic: count reflects the deletion.
        assert_eq!(
            store.count(None).await.unwrap(),
            0,
            "delete must remove rows from query-visible state"
        );

        // Privacy-contract assertion: walk every file under the data
        // dir; NONE may still contain the probe string.
        fn walk_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(read) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk_files(&path, out);
                } else if path.is_file() {
                    out.push(path);
                }
            }
        }
        let mut all_files = Vec::new();
        walk_files(tmp.path(), &mut all_files);
        let mut tombstoned_in: Vec<std::path::PathBuf> = Vec::new();
        for f in &all_files {
            if let Ok(bytes) = std::fs::read(f) {
                if bytes
                    .windows(probe_str.len())
                    .any(|w| w == probe_str.as_bytes())
                {
                    tombstoned_in.push(f.clone());
                }
            }
        }
        assert!(
            tombstoned_in.is_empty(),
            "ADR-039 privacy-contract VIOLATION: deleted boundary string \
             {probe_str:?} found in {n} file(s) post-delete: {paths:?}. \
             `LanceVectorStore::delete()` is supposed to call \
             `optimize(OptimizeAction::Compact {{ ... }})` followed by \
             `optimize(OptimizeAction::Prune {{ older_than: zero, ... }})` \
             to physically remove tombstoned bytes — has either call been \
             removed, or has lance changed its compaction/retention semantics?",
            n = tombstoned_in.len(),
            paths = tombstoned_in
                .iter()
                .map(|p| p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string())
                .collect::<Vec<_>>()
        );
    }

    /// ADR-039 partial-fragment regression pin (Phase 0c spike Stage E,
    /// 2026-05-08).
    ///
    /// The full-fragment pin above (`delete_physically_removes_content_per_adr_039`)
    /// passes even if `delete()` only calls Prune (without Compact) because
    /// deleting EVERY row in a boundary's fragment empties the fragment and
    /// Lance can drop it via Prune alone. Memory Vault's actual delete API is
    /// single-memory-id — each `delete()` call is a PARTIAL-fragment delete.
    /// Under that pattern Lance's Prune-alone leaves the original data file
    /// unchanged on disk (`OptimizeStats { compaction: None, prune:
    /// RemovalStats { data_files_removed: 0, ... } }` — empirically verified
    /// by spike Stage E's 2×2 matrix on both plain file:// and vault-sealed://).
    /// Compact-then-Prune is the correct sequence: Compact rewrites the
    /// fragment with surviving rows only, Prune then removes the orphaned
    /// original.
    ///
    /// This test pins that exact pattern. It writes 10 rows to one boundary,
    /// deletes 5 by id (leaving 5 surviving rows in the same fragment),
    /// snapshots pre-delete data file content-hashes, and asserts that
    /// every pre-delete data file's content-hash is gone post-delete. With
    /// Compact-then-Prune, the original 10-row data file is removed and a
    /// new 5-row data file is written → set-difference non-empty. With
    /// Prune-alone, the original 10-row data file persists bit-for-bit →
    /// set-difference empty → assertion fails loudly.
    ///
    /// Companion to the full-fragment pin; both must hold for the privacy
    /// contract to survive any future `delete()` refactor.
    #[tokio::test]
    async fn delete_partial_fragment_physically_removes_content_per_adr_039() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let boundary = Boundary::new("ADR_039_PARTIAL_FRAGMENT_PROBE").unwrap();

        // Write 10 rows in the same boundary — one fragment.
        let mut ids = Vec::new();
        for i in 0..10 {
            let id = new_id();
            store
                .upsert(&id, &embedding(4, (i + 1) as f32), &boundary)
                .await
                .unwrap();
            ids.push(id);
        }
        assert_eq!(store.count(None).await.unwrap(), 10);

        // Helper: walk every file in the data dir.
        fn walk_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(read) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk_files(&path, out);
                } else if path.is_file() {
                    out.push(path);
                }
            }
        }
        let is_data_file = |p: &std::path::Path| -> bool {
            p.components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case("data"))
        };
        let file_hash = |bytes: &[u8]| -> [u8; 32] { *blake3::hash(bytes).as_bytes() };

        // Snapshot pre-delete data files by content-hash.
        let mut pre_files = Vec::new();
        walk_files(tmp.path(), &mut pre_files);
        let pre_data: std::collections::HashMap<[u8; 32], std::path::PathBuf> = pre_files
            .iter()
            .filter(|p| is_data_file(p))
            .filter_map(|p| std::fs::read(p).ok().map(|b| (file_hash(&b), p.clone())))
            .collect();
        assert!(
            !pre_data.is_empty(),
            "spike-shape regression: no data files found pre-delete"
        );

        // Delete the FIRST 5 rows by single-id call — partial-fragment delete.
        // Leaves 5 surviving rows in the original fragment, exercising the
        // exact pattern Memory Vault's delete API produces.
        for id in &ids[..5] {
            store.delete(id).await.unwrap();
        }

        // User-facing semantic: count reflects partial deletion.
        assert_eq!(
            store.count(None).await.unwrap(),
            5,
            "partial delete must leave 5 surviving rows visible to query"
        );

        // Privacy-contract assertion: every pre-delete data file's
        // content-hash MUST be gone post-delete. Compact-then-Prune
        // ensures the original fragment file is rewritten; Prune-alone
        // would leave it bit-for-bit unchanged.
        let mut post_files = Vec::new();
        walk_files(tmp.path(), &mut post_files);
        let post_hashes: std::collections::HashSet<[u8; 32]> = post_files
            .iter()
            .filter_map(|p| std::fs::read(p).ok().map(|b| file_hash(&b)))
            .collect();

        let surviving: Vec<&std::path::PathBuf> = pre_data
            .iter()
            .filter(|(hash, _)| post_hashes.contains(*hash))
            .map(|(_, path)| path)
            .collect();

        assert!(
            surviving.is_empty(),
            "ADR-039 PARTIAL-FRAGMENT privacy-contract VIOLATION: \
             {n} pre-delete data file(s) still BIT-FOR-BIT identical \
             post-delete. The encrypted/plaintext bytes of deleted rows \
             remain on disk. `LanceVectorStore::delete()` MUST call \
             `optimize(OptimizeAction::Compact {{ ... }})` BEFORE \
             `optimize(OptimizeAction::Prune {{ older_than: zero, ... }})` \
             — Prune-alone is insufficient for partial-fragment deletes \
             (verified via OptimizeStats `data_files_removed: 0` in spike \
             Stage E 2×2 matrix). Has Compact been removed, or has lance \
             changed compaction semantics? Surviving files: {paths:?}",
            n = surviving.len(),
            paths = surviving
                .iter()
                .map(|p| p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string())
                .collect::<Vec<_>>()
        );
    }

    /// Phase 0b verification 3 — read-during-write isolation.
    ///
    /// ADR-038 Layer 1 mutex serialises WRITES; reads aren't serialised.
    /// Lance v2 (4.0) MVCC should give snapshot reads — each reader
    /// sees a consistent manifest version, never partial/torn state.
    /// This test verifies the invariant under concurrent load: while a
    /// writer is doing N upserts, multiple readers calling count() must
    /// see monotonically non-decreasing values within each reader's
    /// observation sequence (no "saw 50, then saw 30" inversions).
    ///
    /// If lance 4.0's V2 path regressed read isolation, this would
    /// surface as: a reader observes a higher count, then a lower count,
    /// then a higher count again — indicating non-snapshot reads.
    #[tokio::test]
    async fn read_during_write_returns_monotonic_consistent_snapshots() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();

        // Initial state: 0 rows in "work".
        assert_eq!(store.count(Some(&work)).await.unwrap(), 0);

        let writer_count: usize = 30;
        let reader_count: usize = 4;

        // Writer task: upserts `writer_count` rows sequentially. Each
        // upsert acquires the ADR-038 mutex, holds it across
        // merge_insert. Other writers (none here) would queue.
        let writer_store = store.clone();
        let writer_work = work.clone();
        let writer = tokio::spawn(async move {
            for i in 0..writer_count {
                let id = MemoryId(uuid::Uuid::now_v7());
                writer_store
                    .upsert(&id, &embedding(4, (i + 1) as f32), &writer_work)
                    .await
                    .unwrap();
            }
        });

        // Reader tasks: each calls count() repeatedly until the writer
        // finishes. Records the sequence of observed counts.
        let mut readers = Vec::new();
        for _ in 0..reader_count {
            let reader_store = store.clone();
            let reader_work = work.clone();
            readers.push(tokio::spawn(async move {
                let mut observations = Vec::new();
                // Sample for at least as long as the writer; ~50 reads.
                for _ in 0..50 {
                    let n = reader_store.count(Some(&reader_work)).await.unwrap();
                    observations.push(n);
                    tokio::task::yield_now().await;
                }
                observations
            }));
        }

        writer.await.unwrap();

        // Final count must equal writer_count.
        assert_eq!(
            store.count(Some(&work)).await.unwrap(),
            writer_count,
            "post-writer count must equal writer_count={writer_count}"
        );

        // Every reader's observations must be:
        //   (a) within [0, writer_count] — never out of bounds
        //   (b) monotonically non-decreasing — no torn reads
        for (idx, reader) in readers.into_iter().enumerate() {
            let observations = reader.await.unwrap();
            for (i, &n) in observations.iter().enumerate() {
                assert!(
                    n <= writer_count,
                    "reader {idx} observation {i}: count {n} exceeds \
                     writer_count {writer_count} — torn read / phantom row"
                );
                if i > 0 {
                    assert!(
                        n >= observations[i - 1],
                        "reader {idx} observed count {n} after seeing \
                         {} — count went BACKWARDS, indicates non-snapshot \
                         read isolation under concurrent write",
                        observations[i - 1]
                    );
                }
            }
        }
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

    // ---------- validate_readable (ADR-018) ----------

    #[tokio::test]
    async fn validate_readable_passes_on_empty_store() {
        // Vacuous pass: empty table → no rows → no decode → Ok.
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        store.validate_readable().await.unwrap();
    }

    #[tokio::test]
    async fn validate_readable_passes_on_clean_store_with_rows() {
        let tmp = TempDir::new().unwrap();
        let store = LanceVectorStore::open(tmp.path(), 4).await.unwrap();
        let work = Boundary::new("work").unwrap();
        store
            .upsert(&new_id(), &embedding(4, 0.7), &work)
            .await
            .unwrap();
        store
            .upsert(&new_id(), &embedding(4, 0.3), &work)
            .await
            .unwrap();
        store.validate_readable().await.unwrap();
    }

    /// The corruption-detection invariant the trait contract is about.
    /// Mirrors `crates/vault-storage/examples/lance_corruption_spike.rs`'s
    /// step 3 — the corrupted-fragment scenario must surface here, not via
    /// metadata-only paths like `count`.
    ///
    /// LanceDB writes one fragment file per `upsert` call, so we corrupt
    /// **every** `.lance` fragment file before re-opening. Corrupting just
    /// the first one would let `validate_readable`'s `nearest_to` scan
    /// satisfy itself from a clean fragment without hitting the corruption.
    #[tokio::test]
    async fn validate_readable_returns_err_on_corrupted_fragment() {
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().to_path_buf();

        // Set up: open, insert, drop the store so files flush to disk.
        {
            let store = LanceVectorStore::open(&data_dir, 4).await.unwrap();
            let work = Boundary::new("work").unwrap();
            for i in 0..3 {
                store
                    .upsert(&new_id(), &embedding(4, 0.1 * (i as f32 + 1.0)), &work)
                    .await
                    .unwrap();
            }
        }

        // Corrupt every fragment file's FOOTER (last 64 bytes) — see
        // matching corruption shape in
        // `cascading::tests::open_on_corrupted_lance_fragments_returns_lance_unreadable`
        // for the lance v1 (header) → v2 (footer) format change rationale.
        // Lance v2 (4.0) holds magic + manifest at the END of the file;
        // header-corruption no longer fail-fasts and triggers a 32 GB
        // OOM allocation downstream. Footer-corruption destroys both
        // manifest AND row-decode paths, so `validate_readable` MUST
        // surface an Err either way (the original test's intent was
        // "decode-path corruption surfaces"; lance v2's footer-based
        // layout means manifest-and-decode share the same magic check,
        // so corrupting the footer is the only reliable way to signal
        // an unreadable fragment without OOM-aborting first).
        let fragments = find_all_fragments(&data_dir);
        assert!(
            !fragments.is_empty(),
            "expected at least one fragment file in {data_dir:?}"
        );
        for fragment in &fragments {
            let mut bytes = std::fs::read(fragment).unwrap();
            let len = bytes.len();
            let n = 64.min(len);
            let start = len.saturating_sub(n);
            for byte in bytes.iter_mut().skip(start).take(n) {
                *byte = 0xAB;
            }
            std::fs::write(fragment, &bytes).unwrap();
        }

        // Re-open + validate. validate_readable MUST return Err. The
        // metadata-only `count` would still succeed — we don't test that
        // here (the spike covered it); we test that our validation does
        // what `count` cannot.
        let store = LanceVectorStore::open(&data_dir, 4).await.unwrap();
        let result = store.validate_readable().await;
        assert!(
            result.is_err(),
            "corrupted fragment must surface as Err from validate_readable; got {result:?}"
        );
    }

    /// Walk `data_dir/*.lance/data/` recursively and return every `.lance`
    /// fragment file found. The corruption test corrupts all of them so
    /// `nearest_to` can't dodge the corruption by satisfying from a clean
    /// fragment.
    fn find_all_fragments(data_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => return,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.extension().and_then(|s| s.to_str()) == Some("lance")
                    && path
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n == "data")
                        .unwrap_or(false)
                {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(data_dir, &mut found);
        found
    }

    // ============================================================
    //   T0.2.0 Phase 0d — production at-rest sealed-path tests
    // ============================================================

    /// Walk every regular file under `dir`, recursively. Used by the
    /// sealed-path tests below to inspect on-disk bytes after Lance
    /// has written through `SealedFileStoreProvider`.
    fn walk_every_file(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        fn walk(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            let Ok(read) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in read.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path.is_file() {
                    found.push(path);
                }
            }
        }
        let mut found = Vec::new();
        walk(dir, &mut found);
        found
    }

    /// Phase 0d test 1: round-trip through `open_with_at_rest_key`.
    /// Upsert 4 orthogonal unit vectors + search; assert count + that
    /// the exact-match search returns the matching id as top hit.
    /// Confirms the sealed path's write+read flows compose end-to-end
    /// with the rest of `LanceVectorStore`'s API surface unchanged.
    ///
    /// Uses orthogonal unit vectors (not the `embedding(dim, fill)`
    /// all-same-fill helper) because cosine distance between collinear
    /// vectors is 0 — `[1,1,1,1]` and `[2,2,2,2]` are identical under
    /// Cosine, so a top-hit assertion would be non-deterministic. Per
    /// ADR-038 Layer 4 Memory Vault uses Cosine in production and BGE
    /// vectors are L2-normalised non-collinear; orthogonal unit vectors
    /// here give the test the same uniqueness property.
    #[tokio::test]
    async fn sealed_open_round_trip_returns_inserted_rows() {
        let tmp = TempDir::new().unwrap();
        // Model production caller flow: pre-derive K3 at fixture setup
        // (canonical production site: vault-app::keychain::derive_at_rest_key).
        // Context-string pinning lives in vault-app/src/keychain.rs's
        // `derive_at_rest_key_is_deterministic_and_uses_k3_kdf_context` test.
        let master_key: [u8; 32] = *b"phase-0d-master-key-32-bytes-RT1";
        let at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key);
        let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest_key)
            .await
            .unwrap();

        let boundary = Boundary::new("sealed_round_trip").unwrap();
        let unit_vectors: [[f32; 4]; 4] = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let mut ids = Vec::new();
        for v in &unit_vectors {
            let id = new_id();
            store.upsert(&id, v, &boundary).await.unwrap();
            ids.push(id);
        }

        assert_eq!(store.count(None).await.unwrap(), 4);

        // Search for the third axis: only ids[2] is on that axis, all
        // other vectors are orthogonal (cosine distance = 1) — top hit
        // is unambiguous.
        let target_idx = 2;
        let hits = store
            .search(
                &unit_vectors[target_idx],
                4,
                std::slice::from_ref(&boundary),
            )
            .await
            .unwrap();
        assert!(!hits.is_empty(), "search MUST return at least one hit");
        assert_eq!(
            hits[0].0, ids[target_idx],
            "exact-match search MUST return the matching id as top hit through the sealed path"
        );
    }

    /// Phase 0d test 2: wrong-key reopen fails closed.
    /// Open with K1, write rows, drop the store. Re-open the SAME
    /// data_dir with K2 and confirm we get an error (not silent-empty,
    /// not silent-corrupted-data). The dryoc DryocStream pull will fail
    /// AEAD authentication when the K3-derived at-rest key doesn't
    /// match the one used to seal the manifest; lancedb surfaces this
    /// as a generic ObjectStore error wrapping our SealedObjectStore
    /// "Generic" variant.
    #[tokio::test]
    async fn sealed_open_with_wrong_key_fails_closed() {
        let tmp = TempDir::new().unwrap();
        // Pre-derive both at-rest keys at fixture setup — see test 1 for
        // the production-caller-flow rationale.
        let master_correct: [u8; 32] = *b"phase-0d-correct-master-32-bytes";
        let master_wrong: [u8; 32] = *b"phase-0d-WRONG-master-32-bytes-X";
        let key1 = blake3::derive_key("vault memory at-rest sealing v1", &master_correct);
        let key2 = blake3::derive_key("vault memory at-rest sealing v1", &master_wrong);

        // Write phase: open with K1 + upsert + drop.
        {
            let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &key1)
                .await
                .unwrap();
            let b = Boundary::new("wrong_key_test").unwrap();
            for i in 0..3 {
                store
                    .upsert(&new_id(), &embedding(4, (i + 1) as f32), &b)
                    .await
                    .unwrap();
            }
        } // store dropped

        // Re-open with K2: MUST fail somewhere along the open or first
        // read. If both succeed silently, AEAD authentication is broken
        // and the privacy contract is violated.
        let reopen = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &key2).await;
        let first_read_result = match reopen {
            Err(_) => Err::<(), ()>(()), // open failed — pass
            Ok(store) => match store.count(None).await {
                Err(_) => Err::<(), ()>(()),
                Ok(_) => Ok::<(), ()>(()),
            },
        };
        assert!(
            first_read_result.is_err(),
            "Wrong-key reopen MUST fail closed (AEAD authentication mismatch). \
             If this passes silently, the sealing wrapper is not actually \
             enforcing per-file AEAD."
        );
    }

    /// Phase 0d test 3: every file written through the sealed path has
    /// the locked sealing-shape framing bytes (`0x01 || 0x00`) AND
    /// contains no Parquet magic. This is the single strongest on-disk
    /// signal that bytes Lance wrote actually went through
    /// `SealedObjectStore::put_opts` — combined with test 2's wrong-key
    /// fail-closed, it rules out the v1 LocalObjectReader-bypass class
    /// of regression in production.
    #[tokio::test]
    async fn sealed_open_writes_framing_bytes_to_disk() {
        let tmp = TempDir::new().unwrap();
        let master_key: [u8; 32] = *b"phase-0d-framing-master-32-bytes";
        let at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key);
        let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest_key)
            .await
            .unwrap();

        let b = Boundary::new("framing_check").unwrap();
        for i in 0..5 {
            store
                .upsert(&new_id(), &embedding(4, (i + 1) as f32), &b)
                .await
                .unwrap();
        }
        // Drop is necessary to release any internal buffering before we
        // walk the disk — Lance flushes on table drop.
        drop(store);

        let files = walk_every_file(tmp.path());
        assert!(
            !files.is_empty(),
            "no files written under {} — Lance wrote nothing OR temp tree shape changed",
            tmp.path().display()
        );
        for path in &files {
            let bytes = std::fs::read(path).unwrap();
            // (a) framing bytes
            if bytes.len() >= 2 {
                assert_eq!(
                    bytes[0],
                    crate::sealed_object_store::VERSION_BYTE,
                    "sealed-path file {} first byte {:#x} != VERSION_BYTE — Lance \
                     bypassed SealedObjectStore::put_opts for this write",
                    path.display(),
                    bytes[0]
                );
                assert_eq!(
                    bytes[1],
                    crate::sealed_object_store::GRANULARITY_PER_FILE,
                    "sealed-path file {} second byte {:#x} != GRANULARITY_PER_FILE",
                    path.display(),
                    bytes[1]
                );
            }
            // (b) zero PAR1 anywhere — full inspection, not just first 4
            assert!(
                !bytes.windows(4).any(|w| w == b"PAR1"),
                "sealed-path file {} contains plaintext PAR1 magic — sealing was \
                 bypassed mid-write OR a code path is writing plaintext through \
                 the sealed connection",
                path.display()
            );
        }
    }

    /// Phase 0d test 4: ADR-039 partial-fragment physical removal works
    /// through the sealed wrapper. Sealed-path companion to
    /// [`delete_partial_fragment_physically_removes_content_per_adr_039`]
    /// (which exercises the same invariant on the plaintext path).
    /// Write 10 rows in one fragment, single-id-delete 5 of them, then
    /// confirm via BLAKE3 content-hash-set-difference that no pre-delete
    /// data file's exact bytes survive post-Compact+Prune. With sealing,
    /// every file is unique-by-construction (per-file random AEAD nonce)
    /// so the content-hash check is the cleanest signal available.
    #[tokio::test]
    async fn sealed_delete_partial_fragment_physically_removes_content() {
        let tmp = TempDir::new().unwrap();
        let master_key: [u8; 32] = *b"phase-0d-delete-master-32-bytes-";
        let at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key);
        let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest_key)
            .await
            .unwrap();
        let b = Boundary::new("sealed_partial_delete").unwrap();

        let mut ids = Vec::new();
        for i in 0..10 {
            let id = new_id();
            store
                .upsert(&id, &embedding(4, (i + 1) as f32), &b)
                .await
                .unwrap();
            ids.push(id);
        }
        assert_eq!(store.count(None).await.unwrap(), 10);

        let is_data_file = |p: &std::path::Path| -> bool {
            p.components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case("data"))
        };
        let file_hash = |bytes: &[u8]| -> [u8; 32] { *blake3::hash(bytes).as_bytes() };

        let pre_files = walk_every_file(tmp.path());
        let pre_data: std::collections::HashMap<[u8; 32], std::path::PathBuf> = pre_files
            .iter()
            .filter(|p| is_data_file(p))
            .filter_map(|p| std::fs::read(p).ok().map(|b| (file_hash(&b), p.clone())))
            .collect();
        assert!(
            !pre_data.is_empty(),
            "no sealed data files found pre-delete"
        );

        for id in &ids[..5] {
            store.delete(id).await.unwrap();
        }
        assert_eq!(store.count(None).await.unwrap(), 5);

        let post_files = walk_every_file(tmp.path());
        let post_hashes: std::collections::HashSet<[u8; 32]> = post_files
            .iter()
            .filter_map(|p| std::fs::read(p).ok().map(|b| file_hash(&b)))
            .collect();

        let surviving: Vec<&std::path::PathBuf> = pre_data
            .iter()
            .filter(|(hash, _)| post_hashes.contains(*hash))
            .map(|(_, path)| path)
            .collect();

        assert!(
            surviving.is_empty(),
            "ADR-039-through-sealing FAIL: {n} pre-delete sealed data file(s) \
             still BIT-FOR-BIT identical post-Compact+Prune. The encrypted bytes \
             of deleted rows survive on disk through the sealed wrapper, \
             violating the privacy contract. Surviving: {paths:?}",
            n = surviving.len(),
            paths = surviving
                .iter()
                .map(|p| p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_string())
                .collect::<Vec<_>>()
        );
    }

    /// Phase 0d test 5: the sealed path emits its OWN distinguishable
    /// INFO log (`"at-rest sealed path"` substring) — the positive
    /// dual to `open_emits_adr_010_plaintext_warn_log`. Pins that the
    /// sealed-path INFO is wired AND that the constructor that emits
    /// it is reached.
    ///
    /// This is a positive assertion (substring present) rather than a
    /// negative one (no plaintext WARN) because tracing-test's
    /// `#[traced_test]` shares a process-local subscriber across
    /// concurrent tests — a negative assertion would suffer from
    /// `open_emits_adr_010_plaintext_warn_log` running in parallel and
    /// emitting "plaintext"/"ADR-010" strings into the shared log
    /// buffer. The positive marker `"at-rest sealed path"` is unique
    /// to this code path and bleed-resistant.
    #[tokio::test]
    #[tracing_test::traced_test]
    async fn sealed_open_emits_distinguishing_info_log() {
        let tmp = TempDir::new().unwrap();
        let master_key: [u8; 32] = *b"phase-0d-no-warn-master-32-bytes";
        let at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key);
        let _store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 384, &at_rest_key)
            .await
            .unwrap();

        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_storage", "at-rest sealed path",),
            "Sealed-path open MUST emit its distinguishing INFO log \
             ('at-rest sealed path' substring at vector_store.rs's \
             open_with_at_rest_key info!(...) site). Failure here means \
             either (a) the INFO log was removed/refactored, or (b) the \
             sealed-path constructor was never reached (open_with_at_rest_key \
             returned Err before logging)."
        );
    }

    /// Phase 2 (T0.2.0) test: rename-invariance. Open a sealed store at
    /// `path_a`, write rows, drop, atomically rename `path_a → path_b`,
    /// reopen with the SAME at-rest key, read the rows back. Pins
    /// ADR-008 amendment v2 (T0.2.0 Phase 2) — AAD must be relative to
    /// the vault root so renaming the vault root doesn't break sealing.
    /// This is the property the migration loop's `temp_dir →
    /// vector_dir` atomic swap requires; without the amendment AAD
    /// computed pre-rename (over the temp_dir absolute path) wouldn't
    /// match AAD computed post-rename (over the vector_dir absolute
    /// path) and unsealing would fail closed with "Message
    /// authentication mismatch."
    #[tokio::test]
    async fn sealed_open_at_path_a_rename_to_b_open_at_b_succeeds() {
        let tmp = TempDir::new().unwrap();
        let path_a = tmp.path().join("dir_a");
        let path_b = tmp.path().join("dir_b");

        let master_key: [u8; 32] = *b"phase-2-rename-master-32-bytes--";
        let at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key);

        // Seal at path A.
        {
            let store = LanceVectorStore::open_with_at_rest_key(&path_a, 4, &at_rest_key)
                .await
                .unwrap();
            let b = Boundary::new("rename_invariance").unwrap();
            for i in 0..3 {
                store
                    .upsert(&new_id(), &embedding(4, (i + 1) as f32), &b)
                    .await
                    .unwrap();
            }
            assert_eq!(store.count(None).await.unwrap(), 3);
        } // store dropped, locks released

        // Atomic rename A → B (the canonical migration-loop step 8b shape).
        std::fs::rename(&path_a, &path_b).unwrap();

        // Reopen at B with the SAME key. Under absolute-path AAD this
        // would fail closed with "Message authentication mismatch";
        // under ADR-008 amendment v2 (relative-to-vault-root AAD) it
        // succeeds.
        let store = LanceVectorStore::open_with_at_rest_key(&path_b, 4, &at_rest_key)
            .await
            .unwrap();
        assert_eq!(
            store.count(None).await.unwrap(),
            3,
            "post-rename count must equal pre-rename count — AAD must be invariant to \
             renaming the vault root per ADR-008 amendment v2 (T0.2.0 Phase 2). If this \
             fails, AAD is still binding to the absolute path and the migration loop's \
             atomic-swap pattern would silently brick all sealed data."
        );
    }
}
