-- 0005_consolidation_watermark.sql
-- Pillar 2 — Incremental Consolidation (ADR-082).
--
-- Records the START time of the last consolidation run that completed its FULL
-- pipeline (run_consolidation -> enrich_facts -> generate_reports -> REPORT
-- persist). The nightly / catch-up incremental run reads this as the `since`
-- watermark so it only re-examines facts created at or after it, turning a
-- run's cost from O(whole vault) into O(facts changed since last night).
--
-- Two invariants make this safe (BRD §5.6 line 936 "memory added since last
-- consolidation"; ADR-082):
--   1. Persisted ONLY on full success. A timed-out / crashed / errored run does
--      NOT advance it, so the next run retries the same backlog — no lost work.
--   2. Stores the run's START time (not its end). A fact created mid-run has a
--      newer created_at than the watermark, so it is picked up next run, never
--      skipped.
--
-- Single-row table: `id` is pinned to 1. `last_run_started_at` is NULL until the
-- first fully-successful run; NULL means "no watermark yet -> full scan" (which
-- is exactly the cold-start / first-run behaviour).

CREATE TABLE IF NOT EXISTS consolidation_state (
    id                   INTEGER PRIMARY KEY CHECK (id = 1),
    last_run_started_at  TEXT  -- RFC3339 (UTC); NULL until first full success
);

-- Seed the singleton row so the getter can always UPDATE/SELECT id = 1.
INSERT OR IGNORE INTO consolidation_state (id, last_run_started_at) VALUES (1, NULL);
