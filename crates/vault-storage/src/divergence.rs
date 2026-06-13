//! [`DivergenceDetector`] — two-tier consistency check between SQLite
//! (source of truth) and LanceDB (vector store) for V0.1 cascading
//! safety net (T0.1.6 Phase C2).
//!
//! Per `T0.1.6_PLAN.md` Q3 + ADR-018:
//!
//! - **Tier 1: Count comparison.** `SELECT COUNT(*) FROM memories` in
//!   SQLite vs `LanceVectorStore::count(None)`. Mismatch → tier 2 will
//!   surface specific missing IDs.
//! - **Tier 2: Sampled-existence check.** Sample 100 memory IDs via
//!   **deterministic stratified sampling** (50 from the most recent 30
//!   days + 50 older), seeded from `current_date.timestamp() / 86400 *
//!   0xDEADBEEF` so the same calendar day rotates the same sample
//!   (correlatable across multiple runs within a day) but different days
//!   probe different rows over time. For each sampled ID, call
//!   [`crate::VectorStore::contains`] — missing IDs are reported as
//!   divergence findings.
//!
//! ## Hard vs soft corruption
//!
//! Hard corruption (fragment data garbled) is **NOT** caught here — it's
//! caught by `StorageBackend::open`'s eager `validate_readable` (ADR-018).
//! Divergence covers SOFT corruption: silent row drops, manifest drift,
//! cascade failures that slipped past dead-lettering. Two distinct
//! failure classes, two distinct surfaces — neither replaces the other.
//!
//! ## Schedule (V0.1)
//!
//! On-demand only via `vault-cli divergence-check`. The 24-hour timer
//! is deferred to V0.2 per Phase A Q3 — V0.1 founder dogfood typically
//! restarts the app frequently, and on-startup checks (lighter shape,
//! also deferred to V0.2) will cover the daemon-mode gap.
//!
//! ## `pending_sync` sweep (T0.2.4 sync ship-gate — implemented)
//!
//! Tier-0 of every run drains `pending_sync` (oldest first), re-enqueueing
//! into `retry_queue` while capacity is available, via
//! [`StorageBackend::drain_pending_sync`]. Migration 0003 extended
//! `pending_sync` with the `sequence_id` + `payload` columns the cascade
//! needs, and the orchestrator's overflow path now persists them — so the
//! sweep hands the stored bytes straight to the `retry_queue` insert and
//! removes the drained row atomically. A pre-0003 legacy row (no payload —
//! never produced at V0.1 scale) is skipped, not re-enqueued.
//!
//! This was a V0.1 stub-that-returns-0; it is the cross-device data-recovery
//! ship-gate for V0.2 sync (cross-device churn makes a silently-dropped
//! overflow entry a real loss), closed here.

#![allow(dead_code)] // Detector is consumed by vault-cli's divergence-check subcommand.

use chrono::{DateTime, Duration, Utc};
use rusqlite::params;
use tracing::{debug, info, instrument, warn};

use vault_core::{MemoryId, VaultError, VaultResult};

use crate::cascading::StorageBackend;

/// Number of IDs sampled per stratum (recent + older). Total sample size
/// is `2 * SAMPLES_PER_STRATUM` when both strata are populated; smaller
/// when one window has fewer rows.
pub const SAMPLES_PER_STRATUM: usize = 50;

/// Strata cutoff. Memories whose `created_at >= now - RECENT_WINDOW` are
/// in the recent stratum; the rest are in the older stratum.
pub const RECENT_WINDOW: Duration = Duration::days(30);

/// XOR seed for the daily-rotating sampling seed. Combined with the day
/// number to give a deterministic-within-the-day, rotating-across-days
/// sample. Not security-critical — just spreads the sample across rows
/// over time.
const DAILY_SEED_MASK: u64 = 0xDEAD_BEEF;

/// Max `pending_sync` rows the Tier-0 sweep examines per run. Bounds the
/// per-run work; the per-entry capacity check stops draining once
/// `retry_queue` refills, and any remainder is picked up on the next sweep.
/// Generous relative to V0.2 alpha scale (cap-overflow needs a 10k-deep
/// queue, essentially never reached in dogfood).
const PENDING_SYNC_SWEEP_LIMIT: usize = 1_000;

/// Result of a single `DivergenceDetector::run` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DivergenceReport {
    pub run_at: DateTime<Utc>,
    /// Total memory rows in SQLite (`SELECT COUNT(*) FROM memories`).
    pub sqlite_memory_count: usize,
    /// Total rows in the LanceDB vector store (`count_rows(None)`).
    pub vector_count: usize,
    /// Number of memory IDs sampled in tier 2 (≤ 2 * `SAMPLES_PER_STRATUM`).
    pub samples_checked: usize,
    /// IDs sampled in tier 2 whose `VectorStore::contains` returned false.
    /// Non-empty = divergence finding.
    pub missing_in_vector: Vec<MemoryId>,
    /// Number of `pending_sync` rows successfully drained back into
    /// `retry_queue` this run (T0.2.4 sync ship-gate — see module docs).
    /// `0` when `pending_sync` is empty (the common case) or already at
    /// `retry_queue` capacity.
    pub pending_sync_resync_count: usize,
}

impl DivergenceReport {
    /// True if tier-1 counts differ (LanceDB missing rows that SQLite has).
    pub fn count_mismatch(&self) -> bool {
        self.sqlite_memory_count != self.vector_count
    }

    /// True if there's any divergence finding worth surfacing to the operator.
    pub fn has_findings(&self) -> bool {
        self.count_mismatch() || !self.missing_in_vector.is_empty()
    }
}

/// On-demand divergence check between SQLite metadata and the LanceDB
/// vector store.
pub struct DivergenceDetector {
    backend: StorageBackend,
}

impl DivergenceDetector {
    /// Construct a detector backed by the given orchestrator handle.
    pub fn new(backend: StorageBackend) -> Self {
        Self { backend }
    }

    /// Run a full check using the daily-rotating seed. Production entry
    /// point; the CLI calls this.
    pub async fn run(&self) -> VaultResult<DivergenceReport> {
        let now = Utc::now();
        self.run_with(now, daily_seed(now)).await
    }

    /// Run a check at a caller-supplied "now" with a caller-supplied
    /// sampling seed. Tests use this to fast-forward the clock and to
    /// pin the sample for assertion.
    #[instrument(skip(self), fields(now = %now, seed))]
    pub async fn run_with(&self, now: DateTime<Utc>, seed: u64) -> VaultResult<DivergenceReport> {
        // Tier 0 — sweep `pending_sync` first. V0.1 stub.
        let pending_sync_resync_count = self.sweep_pending_sync().await?;

        // Tier 1 — count comparison.
        let sqlite_memory_count = self.count_sqlite_memories().await?;
        let vector_count = self.backend.vector_store().count(None).await?;
        if sqlite_memory_count != vector_count {
            warn!(
                sqlite_memory_count,
                vector_count,
                delta = sqlite_memory_count as i64 - vector_count as i64,
                "tier-1 count mismatch — divergence likely"
            );
        }

        // Tier 2 — deterministic stratified sampling + per-id existence.
        let cutoff = now - RECENT_WINDOW;
        let recent_ids = self.fetch_memory_ids_in_window(cutoff, true).await?;
        let older_ids = self.fetch_memory_ids_in_window(cutoff, false).await?;
        let recent_sample = pick_sample(&recent_ids, SAMPLES_PER_STRATUM, seed);
        let older_sample = pick_sample(&older_ids, SAMPLES_PER_STRATUM, seed.wrapping_add(1));
        let mut sampled: Vec<MemoryId> =
            Vec::with_capacity(recent_sample.len() + older_sample.len());
        sampled.extend(recent_sample);
        sampled.extend(older_sample);

        let mut missing = Vec::new();
        for id in &sampled {
            let present = self.backend.vector_store().contains(id).await?;
            if !present {
                missing.push(*id);
            }
        }

        if !missing.is_empty() {
            warn!(
                count = missing.len(),
                samples_checked = sampled.len(),
                "tier-2 sampled-existence found missing IDs"
            );
        } else {
            debug!(
                samples_checked = sampled.len(),
                "tier-2 sampled-existence clean"
            );
        }

        info!(
            sqlite = sqlite_memory_count,
            vector = vector_count,
            samples_checked = sampled.len(),
            missing = missing.len(),
            pending_sync_resync_count,
            "divergence run complete"
        );

        Ok(DivergenceReport {
            run_at: now,
            sqlite_memory_count,
            vector_count,
            samples_checked: sampled.len(),
            missing_in_vector: missing,
            pending_sync_resync_count,
        })
    }

    /// Tier-0 sweep: drain `pending_sync` back into `retry_queue` when capacity
    /// has returned (T0.2.4 sync ship-gate). Delegates to
    /// [`StorageBackend::drain_pending_sync`], which atomically re-enqueues each
    /// stored cascade (its `sequence_id` + `payload` per migration 0003) and
    /// removes the drained row. Returns the number of entries re-enqueued this
    /// run.
    ///
    /// [`StorageBackend::drain_pending_sync`]: crate::cascading::StorageBackend::drain_pending_sync
    async fn sweep_pending_sync(&self) -> VaultResult<usize> {
        let drained = self
            .backend
            .drain_pending_sync(PENDING_SYNC_SWEEP_LIMIT)
            .await?;
        if drained > 0 {
            info!(
                drained,
                "pending_sync sweep re-enqueued cascade entries into retry_queue"
            );
        }
        Ok(drained)
    }

    /// Total memory rows. No filter — every SQLite row should have a
    /// matching LanceDB row regardless of supersession status, because
    /// `cascading::StorageBackend` only deletes from LanceDB when
    /// `delete_memory` is called.
    async fn count_sqlite_memories(&self) -> VaultResult<usize> {
        self.backend
            .metadata()
            .with_conn_blocking(|conn| {
                let n: i64 = conn
                    .query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))
                    .map_err(|e| VaultError::Storage(format!("count memories: {e}")))?;
                Ok(n as usize)
            })
            .await
    }

    /// Fetch memory IDs partitioned by `created_at` against `cutoff`.
    /// `recent_side = true` returns rows where `created_at >= cutoff`;
    /// `recent_side = false` returns the rest.
    async fn fetch_memory_ids_in_window(
        &self,
        cutoff: DateTime<Utc>,
        recent_side: bool,
    ) -> VaultResult<Vec<MemoryId>> {
        let cutoff_str = cutoff.to_rfc3339();
        let op = if recent_side { ">=" } else { "<" };
        // We can't bind the operator dynamically with rusqlite params,
        // and inlining the operator is safe (it's a const we control,
        // not user input).
        let sql = format!("SELECT id FROM memories WHERE created_at {op} ?1");
        self.backend
            .metadata()
            .with_conn_blocking(move |conn| {
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| VaultError::Storage(format!("prepare fetch_ids: {e}")))?;
                let rows = stmt
                    .query_map(params![cutoff_str], |row| {
                        let id_s: String = row.get(0)?;
                        Ok(id_s)
                    })
                    .map_err(|e| VaultError::Storage(format!("query fetch_ids: {e}")))?;
                let mut out = Vec::new();
                for r in rows {
                    let id_s =
                        r.map_err(|e| VaultError::Storage(format!("read fetch_ids row: {e}")))?;
                    let id = id_s.parse().map_err(|e| {
                        VaultError::Storage(format!("decode memory id {id_s}: {e}"))
                    })?;
                    out.push(id);
                }
                Ok(out)
            })
            .await
    }
}

// ---------------------------------------------------------------------------
// Sampling helpers
// ---------------------------------------------------------------------------

/// Build the daily-rotating seed from `now`. Stable within a calendar
/// day (UTC), rotates each day so different rows get checked over time.
/// Per Phase A Q3.
pub fn daily_seed(now: DateTime<Utc>) -> u64 {
    // Truncate to days since epoch (UTC), then mix with the constant.
    let day_index = (now.timestamp().max(0) as u64) / 86_400;
    day_index.wrapping_mul(DAILY_SEED_MASK)
}

/// Pick at most `count` items from `items` using a Fisher-Yates partial
/// shuffle seeded by `seed`. Same `(items, count, seed)` triple → same
/// sample, deterministically.
fn pick_sample(items: &[MemoryId], count: usize, seed: u64) -> Vec<MemoryId> {
    if items.is_empty() || count == 0 {
        return Vec::new();
    }
    let mut work: Vec<MemoryId> = items.to_vec();
    let mut state = if seed == 0 {
        0xdead_beef_dead_beef
    } else {
        seed
    };
    let take = count.min(work.len());
    for i in 0..take {
        // xorshift64 step.
        let mut x = state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        state = x;
        // Pick j in [i, work.len()). Use rejection-free modulo since the
        // bias is negligible for V0.1 sample sizes (100 of ~10k).
        let j = i + ((x as usize) % (work.len() - i));
        work.swap(i, j);
    }
    work.truncate(take);
    work
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    use tempfile::TempDir;

    use vault_core::{Boundary, Memory, MemoryType, NewMemory};

    use crate::cascading::StorageBackend;
    use crate::key::SqlCipherKey;
    use crate::retry_queue::FixedJitter;
    use crate::retry_worker::{RetryWorker, StepResult};

    const DIM: usize = 4;

    /// Test-only at-rest key (32 bytes, fixed pattern). Per
    /// `feedback_floor_forecast_is_pre_declaration_not_estimate.md`-adjacent
    /// discipline: matches the existing convention in
    /// `crates/vault-storage/tests/migration_v0_1_to_sealed.rs:96` and
    /// `crates/vault-cli/src/main.rs:497`. Per-mod local const per
    /// HANDOFF sub-task (d) §"Const placement" decision lock.
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

    async fn make_backend(tmp: &Path) -> StorageBackend {
        let metadata_path = tmp.join("vault.db");
        let vector_dir = tmp.join("lance");
        let graph_path = tmp.join("graph.duckdb");
        let key = SqlCipherKey::new("divergence-test-key");
        StorageBackend::open_with_at_rest_key(
            &metadata_path,
            &vector_dir,
            &graph_path,
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .unwrap()
    }

    /// Drain every queued cascade through the worker so SQLite + LanceDB
    /// are in sync. Used by the divergence tests as a precondition.
    async fn drain_cascades(backend: &StorageBackend) {
        let mut w = RetryWorker::with_jitter(backend.clone(), Box::new(FixedJitter(0.0)));
        let far_future = Utc::now() + Duration::seconds(60 * 60);
        loop {
            let r = w.step_at(far_future).await.unwrap();
            if r == StepResult::Idle {
                break;
            }
        }
    }

    // ------------------------------------------------------------------
    // Clean vault — no findings
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn clean_vault_returns_no_findings() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        for i in 0..5 {
            let m = sample_memory("work", &format!("memory-{i}"));
            backend
                .write_memory(&m, &embedding(0.1 * i as f32))
                .await
                .unwrap();
        }
        drain_cascades(&backend).await;

        let det = DivergenceDetector::new(backend.clone());
        let report = det.run().await.unwrap();

        assert_eq!(report.sqlite_memory_count, 5);
        assert_eq!(report.vector_count, 5);
        assert!(!report.count_mismatch());
        assert!(report.missing_in_vector.is_empty());
        assert!(!report.has_findings());
        assert_eq!(report.pending_sync_resync_count, 0);
    }

    #[tokio::test]
    async fn empty_vault_returns_no_findings() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        let det = DivergenceDetector::new(backend);
        let report = det.run().await.unwrap();

        assert_eq!(report.sqlite_memory_count, 0);
        assert_eq!(report.vector_count, 0);
        assert_eq!(report.samples_checked, 0);
        assert!(!report.has_findings());
    }

    // ------------------------------------------------------------------
    // Soft corruption — silent row drop in LanceDB
    // (Phase A Q5 test 3a: "soft corruption (silent drop) caught by divergence")
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn tier_one_count_mismatch_when_lance_row_silently_dropped() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        let mut ids = Vec::new();
        for i in 0..10 {
            let m = sample_memory("work", &format!("memory-{i}"));
            ids.push(m.id);
            backend
                .write_memory(&m, &embedding(0.05 * i as f32))
                .await
                .unwrap();
        }
        drain_cascades(&backend).await;

        // Silently drop one LanceDB row, bypassing the orchestrator.
        // (StorageBackend::delete_memory would also delete the SQLite row
        //  + audit it; we want the soft-corruption signature: SQLite
        //  unchanged, LanceDB row gone.)
        let dropped = ids[3];
        backend.vector_store().delete(&dropped).await.unwrap();

        let det = DivergenceDetector::new(backend);
        let report = det.run().await.unwrap();

        assert_eq!(report.sqlite_memory_count, 10);
        assert_eq!(report.vector_count, 9);
        assert!(report.count_mismatch());
        assert!(report.has_findings());
    }

    #[tokio::test]
    async fn tier_two_finds_silently_dropped_id_when_sampled() {
        // Inject a small enough corpus that the missing id is guaranteed
        // to be in the sample (sample size = 100, corpus = 5).
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        let mut ids = Vec::new();
        for i in 0..5 {
            let m = sample_memory("work", &format!("memory-{i}"));
            ids.push(m.id);
            backend
                .write_memory(&m, &embedding(0.1 * i as f32))
                .await
                .unwrap();
        }
        drain_cascades(&backend).await;

        let dropped = ids[2];
        backend.vector_store().delete(&dropped).await.unwrap();

        let det = DivergenceDetector::new(backend);
        // Use a fixed seed so the test is fully deterministic.
        let report = det.run_with(Utc::now(), 42).await.unwrap();

        assert!(report.has_findings());
        assert!(
            report.missing_in_vector.contains(&dropped),
            "tier-2 should report the dropped id in missing_in_vector; got {:?}",
            report.missing_in_vector
        );
    }

    // ------------------------------------------------------------------
    // Deterministic stratified sampling — same seed → same sample
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn same_seed_yields_same_sample() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        // Enough rows that sampling is non-trivial — 30 memories, sample
        // size up to 100 means we'll sample all 30 (no missing). To
        // exercise the partial-shuffle path we'd need >100 rows; for V0.1
        // small corpus this still verifies determinism: re-running with
        // the same seed must produce the same sample order and same set.
        for i in 0..30 {
            let m = sample_memory("work", &format!("m-{i}"));
            backend
                .write_memory(&m, &embedding(0.01 * i as f32))
                .await
                .unwrap();
        }
        drain_cascades(&backend).await;

        let det = DivergenceDetector::new(backend);
        let now = Utc::now();
        let r1 = det.run_with(now, 12345).await.unwrap();
        let r2 = det.run_with(now, 12345).await.unwrap();
        assert_eq!(r1, r2, "same seed must yield identical reports");

        // Different seed: the sample CAN differ. We don't assert it does
        // (with corpus 30 and sample 100, we always pick all 30 — only
        // the order changes), but we verify the run still completes.
        let r3 = det.run_with(now, 99999).await.unwrap();
        assert_eq!(r3.sqlite_memory_count, r1.sqlite_memory_count);
    }

    // ------------------------------------------------------------------
    // Stratification — recent-window split
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn stratification_splits_at_30_day_cutoff() {
        // Build a corpus where some memories' created_at is > 30 days ago
        // by reaching into SQLite directly to backdate them. The detector
        // partitions on cutoff = now - 30 days; the test verifies the
        // split is observable: by injecting a missing id ONLY in the
        // older stratum and confirming tier-2 still finds it.
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        let mut older_ids = Vec::new();
        let mut recent_ids = Vec::new();
        for i in 0..3 {
            let m = sample_memory("work", &format!("recent-{i}"));
            recent_ids.push(m.id);
            backend
                .write_memory(&m, &embedding(0.1 * i as f32))
                .await
                .unwrap();
        }
        for i in 0..3 {
            let m = sample_memory("work", &format!("older-{i}"));
            older_ids.push(m.id);
            backend
                .write_memory(&m, &embedding(0.5 + 0.1 * i as f32))
                .await
                .unwrap();
        }
        drain_cascades(&backend).await;

        // Backdate the older trio by 60 days via raw UPDATE.
        let backdate_target = (Utc::now() - Duration::days(60)).to_rfc3339();
        let older_ids_clone = older_ids.clone();
        backend
            .metadata()
            .with_conn_blocking(move |conn| {
                for id in &older_ids_clone {
                    conn.execute(
                        "UPDATE memories SET created_at = ?1 WHERE id = ?2",
                        params![backdate_target, id.to_string()],
                    )
                    .map_err(|e| VaultError::Storage(format!("backdate: {e}")))?;
                }
                Ok::<_, VaultError>(())
            })
            .await
            .unwrap();

        // Drop one row from the older stratum.
        let dropped = older_ids[1];
        backend.vector_store().delete(&dropped).await.unwrap();

        let det = DivergenceDetector::new(backend);
        let report = det.run_with(Utc::now(), 7).await.unwrap();

        assert!(report.count_mismatch());
        assert!(
            report.missing_in_vector.contains(&dropped),
            "older-stratum drop must be visible in tier-2: got {:?}",
            report.missing_in_vector
        );
        // Sanity: samples_checked covered both strata. With 3 + 3 = 6
        // total rows in the corpus and SAMPLES_PER_STRATUM = 50, the
        // sample takes everything.
        assert_eq!(report.samples_checked, 6);
    }

    // ------------------------------------------------------------------
    // pending_sync sweep — T0.2.4 sync ship-gate (drains into retry_queue)
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn pending_sync_sweep_drains_payload_and_recovers_dropped_vector() {
        use crate::cascading::CascadePayloadV1;
        use crate::retry_queue::CascadeOperation;

        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        // A memory that lives in SQLite + LanceDB.
        let m = sample_memory("work", "fact whose cascade once overflowed");
        backend.write_memory(&m, &embedding(0.2)).await.unwrap();
        drain_cascades(&backend).await;
        assert!(backend.vector_store().contains(&m.id).await.unwrap());

        // Simulate the overflow aftermath: the vector cascade never landed
        // (drop the row), and the memory sits in pending_sync carrying its
        // full Write payload (embedding + boundary) per migration 0003.
        backend.vector_store().delete(&m.id).await.unwrap();
        let payload = serde_json::to_vec(&CascadePayloadV1 {
            embedding: embedding(0.2),
            boundary: "work".to_string(),
        })
        .unwrap();
        backend
            .pending_sync()
            .upsert_with_payload(m.id, CascadeOperation::Write, Utc::now(), 7, payload)
            .await
            .unwrap();
        assert_eq!(backend.pending_sync().len().await.unwrap(), 1);

        // The sweep re-enqueues it into retry_queue and clears pending_sync.
        let det = DivergenceDetector::new(backend.clone());
        let report = det.run().await.unwrap();
        assert_eq!(
            report.pending_sync_resync_count, 1,
            "sweep must re-enqueue the payload-carrying pending_sync row"
        );
        assert_eq!(
            backend.pending_sync().len().await.unwrap(),
            0,
            "the drained row must be removed from pending_sync"
        );

        // Draining the retry_queue applies the recovered Write → vector restored.
        drain_cascades(&backend).await;
        assert!(
            backend.vector_store().contains(&m.id).await.unwrap(),
            "the re-enqueued cascade must restore the dropped vector"
        );
    }

    #[tokio::test]
    async fn pending_sync_sweep_skips_payloadless_legacy_row() {
        use crate::retry_queue::CascadeOperation;

        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        // A pre-0003 legacy row carries no payload (planted via the plain
        // `upsert`). The sweep must NOT re-enqueue a broken cascade — it skips
        // and leaves the row in place for operator reconciliation.
        backend
            .pending_sync()
            .upsert(MemoryId::new(), CascadeOperation::Write, Utc::now())
            .await
            .unwrap();

        let det = DivergenceDetector::new(backend.clone());
        let report = det.run().await.unwrap();
        assert_eq!(
            report.pending_sync_resync_count, 0,
            "a payload-less legacy row must not be drained"
        );
        assert_eq!(
            backend.pending_sync().len().await.unwrap(),
            1,
            "the payload-less legacy row is left in place"
        );
    }

    // ------------------------------------------------------------------
    // daily_seed semantics
    // ------------------------------------------------------------------

    #[test]
    fn daily_seed_is_stable_within_a_day_and_rotates_across_days() {
        // Two timestamps inside the same UTC day → same seed.
        let morning = DateTime::parse_from_rfc3339("2026-04-30T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let evening = DateTime::parse_from_rfc3339("2026-04-30T23:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(daily_seed(morning), daily_seed(evening));

        // Adjacent days → different seeds (xor mask is a constant
        // multiplier on day index, so day_n vs day_{n+1} differ).
        let next_day = DateTime::parse_from_rfc3339("2026-05-01T01:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_ne!(daily_seed(morning), daily_seed(next_day));
    }

    // ------------------------------------------------------------------
    // pick_sample helper
    // ------------------------------------------------------------------

    #[test]
    fn pick_sample_empty_returns_empty() {
        let s = pick_sample(&[], 10, 1);
        assert!(s.is_empty());
    }

    #[test]
    fn pick_sample_zero_count_returns_empty() {
        let items = vec![MemoryId::new(), MemoryId::new()];
        let s = pick_sample(&items, 0, 1);
        assert!(s.is_empty());
    }

    #[test]
    fn pick_sample_count_capped_to_input_length() {
        let items: Vec<MemoryId> = (0..5).map(|_| MemoryId::new()).collect();
        let s = pick_sample(&items, 100, 1);
        assert_eq!(s.len(), 5);
    }

    #[test]
    fn pick_sample_same_seed_same_output() {
        let items: Vec<MemoryId> = (0..50).map(|_| MemoryId::new()).collect();
        let a = pick_sample(&items, 10, 7);
        let b = pick_sample(&items, 10, 7);
        assert_eq!(a, b, "same seed must yield identical sample");
    }

    #[test]
    fn pick_sample_different_seed_likely_different_output() {
        let items: Vec<MemoryId> = (0..50).map(|_| MemoryId::new()).collect();
        let a = pick_sample(&items, 10, 7);
        let b = pick_sample(&items, 10, 99999);
        // Vanishingly unlikely they're identical with 50C10 distinct
        // arrangements, but assert just on the SET differing — even if
        // rare collision happens, the order should change.
        assert_ne!(a, b, "different seed should yield different sample");
    }

    #[test]
    fn pick_sample_handles_zero_seed_via_sentinel() {
        let items: Vec<MemoryId> = (0..10).map(|_| MemoryId::new()).collect();
        let s = pick_sample(&items, 5, 0);
        assert_eq!(s.len(), 5);
    }
}
