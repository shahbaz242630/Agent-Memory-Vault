//! Incremental-consolidation watermark accessors (Pillar 2, ADR-082).
//!
//! The consolidator reads [`StorageBackend::get_consolidation_watermark`] at the
//! start of a run to scope Phase 1 / Phase 2b to facts created since the last
//! fully-successful run (the `since` cutoff per BRD §5.6 line 936, "memory added
//! since last consolidation"). The application-layer safety wrapper calls
//! [`StorageBackend::set_consolidation_watermark`] with the run's START time
//! ONLY after the full pipeline (`run_consolidation` → `enrich_facts` →
//! `generate_reports` → REPORT persist) succeeds. A timed-out / crashed /
//! errored run never advances the watermark, so the next run retries the same
//! backlog — no lost work.
//!
//! Storage: the single-row `consolidation_state` table (migration 0005). A NULL
//! `last_run_started_at` means "no successful run yet" → full scan (the
//! cold-start / first-run behaviour).

use chrono::{DateTime, Utc};
use rusqlite::OptionalExtension;
use tracing::instrument;

use vault_core::{VaultError, VaultResult};

use crate::StorageBackend;

impl StorageBackend {
    /// Read the incremental-consolidation watermark: the START time of the last
    /// run that completed its full pipeline, or `None` when no run has ever
    /// fully succeeded (cold start → the caller should full-scan).
    ///
    /// # Errors
    ///
    /// [`VaultError::Storage`] on a SQLite read failure or an unparseable
    /// stored timestamp.
    #[instrument(skip(self))]
    pub async fn get_consolidation_watermark(&self) -> VaultResult<Option<DateTime<Utc>>> {
        let raw: Option<String> = self
            .metadata()
            .with_conn_blocking(|conn| {
                conn.query_row(
                    "SELECT last_run_started_at FROM consolidation_state WHERE id = 1",
                    [],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                // `optional()` maps a missing row → None; flatten folds a NULL
                // column value (also None) into the same "no watermark" case.
                .map(|opt| opt.flatten())
                .map_err(|e| VaultError::Storage(format!("read consolidation watermark: {e}")))
            })
            .await?;

        match raw {
            None => Ok(None),
            Some(s) => {
                let ts = DateTime::parse_from_rfc3339(&s)
                    .map_err(|e| {
                        VaultError::Storage(format!("decode consolidation watermark {s:?}: {e}"))
                    })?
                    .with_timezone(&Utc);
                Ok(Some(ts))
            }
        }
    }

    /// Advance the watermark to `run_started_at`. Called by the app-layer safety
    /// wrapper ONLY after the full pipeline succeeds; persisting the run's START
    /// time (not its end) guarantees a fact created mid-run is picked up next
    /// run rather than skipped. Idempotent UPDATE of the singleton row.
    ///
    /// # Errors
    ///
    /// [`VaultError::Storage`] on a SQLite write failure, or if the singleton
    /// row is missing (migration 0005 not applied) — surfaced rather than
    /// silently no-op'd, since a lost watermark advance would make the next run
    /// re-process the whole backlog.
    #[instrument(skip(self))]
    pub async fn set_consolidation_watermark(
        &self,
        run_started_at: DateTime<Utc>,
    ) -> VaultResult<()> {
        let ts = run_started_at.to_rfc3339();
        let rows = self
            .metadata()
            .with_conn_blocking(move |conn| {
                conn.execute(
                    "UPDATE consolidation_state SET last_run_started_at = ?1 WHERE id = 1",
                    rusqlite::params![ts],
                )
                .map_err(|e| VaultError::Storage(format!("write consolidation watermark: {e}")))
            })
            .await?;
        if rows != 1 {
            return Err(VaultError::Storage(format!(
                "consolidation watermark UPDATE affected {rows} rows (expected 1; \
                 consolidation_state singleton missing — migration 0005 not applied?)"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use crate::key::SqlCipherKey;
    use crate::StorageBackend;

    // Tiny test dimension — the vector store accepts any positive dim; these
    // tests never embed, so the real 384 is unnecessary (mirrors checkpoint.rs).
    const DIM: usize = 4;
    const TEST_AT_REST_KEY: [u8; 32] = [0x5c; 32];

    async fn open_test_storage() -> (StorageBackend, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = SqlCipherKey::new("consolidation-state-test");
        let storage = StorageBackend::open_with_at_rest_key(
            &dir.path().join("metadata.db"),
            &dir.path().join("vectors"),
            &dir.path().join("graph.duckdb"),
            key,
            DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .expect("open StorageBackend");
        (storage, dir)
    }

    #[tokio::test]
    async fn watermark_starts_none_then_round_trips() {
        let (storage, _dir) = open_test_storage().await;

        // Cold start: no run has succeeded → None (full-scan signal).
        assert_eq!(
            storage.get_consolidation_watermark().await.unwrap(),
            None,
            "a fresh vault must report no watermark (cold-start full scan)"
        );

        // Advance it, then read it back exactly.
        let ts = Utc.with_ymd_and_hms(2026, 6, 17, 3, 0, 0).single().unwrap();
        storage.set_consolidation_watermark(ts).await.unwrap();
        assert_eq!(
            storage.get_consolidation_watermark().await.unwrap(),
            Some(ts),
            "the watermark must round-trip exactly"
        );
    }

    #[tokio::test]
    async fn set_watermark_overwrites_previous() {
        let (storage, _dir) = open_test_storage().await;

        let older = Utc.with_ymd_and_hms(2026, 6, 16, 3, 0, 0).single().unwrap();
        let newer = Utc.with_ymd_and_hms(2026, 6, 17, 3, 0, 0).single().unwrap();
        storage.set_consolidation_watermark(older).await.unwrap();
        storage.set_consolidation_watermark(newer).await.unwrap();

        assert_eq!(
            storage.get_consolidation_watermark().await.unwrap(),
            Some(newer),
            "the latest successful run's start time must win"
        );
    }
}
