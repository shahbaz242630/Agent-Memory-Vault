-- T0.1.6 — Cascading orchestrator infrastructure.
--
-- Three tables added to the SQLite/SQLCipher metadata DB:
--   * retry_queue   — strict FIFO per memory_id; cascade-ordering invariant
--                      anchored to the audit-chain sequence_id (Q1 in plan).
--   * dead_letter   — terminal state for entries that exhausted retries OR
--                      classified as permanent on attempt 1 (is_permanent).
--                      Resolution enum tracks pending / retried / acknowledged.
--   * pending_sync  — cap-overflow catch-up table. When retry_queue is at
--                      cap (10k), new SQLite-acked writes register here;
--                      divergence detector sweeps and re-enqueues when
--                      capacity restored. Latest operation supersedes (PK
--                      on memory_id, UPSERT semantics).
--
-- See T0.1.6_PLAN.md for the full design.

CREATE TABLE IF NOT EXISTS retry_queue (
    id                       BLOB    PRIMARY KEY,        -- UUID v7
    memory_id                BLOB    NOT NULL,           -- references memories(id) (no FK to allow
                                                          -- best-effort retries even if memories row
                                                          -- was deleted; orphan retries are dropped
                                                          -- by the worker)
    operation                TEXT    NOT NULL,           -- 'write' | 'update' | 'delete'
                                                          -- per-cascade (covers BOTH LanceDB + DuckDB
                                                          -- sub-ops); see ADR-016 / ADR-017 (T0.1.6 Phase C)
    payload_format_version   INTEGER NOT NULL,           -- so retry payloads can evolve
    payload                  BLOB    NOT NULL,           -- JSON-serialised retry context
    sequence_id              INTEGER NOT NULL,           -- audit-chain index for FIFO ordering
    attempts_made            INTEGER NOT NULL DEFAULT 0,
    next_attempt_at          TEXT    NOT NULL,           -- RFC3339 — worker polls where this <= now
    created_at               TEXT    NOT NULL,           -- RFC3339 — when first enqueued
    last_error               TEXT                         -- most recent failure message (truncated)
);

-- Strict FIFO per memory: the (memory_id, sequence_id) tuple is unique,
-- so concurrent updates to the same memory get distinct sequence_ids
-- (sourced from the audit chain) and the worker processes them in order.
CREATE UNIQUE INDEX IF NOT EXISTS idx_retry_queue_mem_seq
    ON retry_queue(memory_id, sequence_id);

-- Worker polls "due" entries via this index (next_attempt_at <= now).
CREATE INDEX IF NOT EXISTS idx_retry_queue_next_attempt
    ON retry_queue(next_attempt_at);

CREATE TABLE IF NOT EXISTS dead_letter (
    id                       BLOB    PRIMARY KEY,
    memory_id                BLOB    NOT NULL,
    failed_operation         TEXT    NOT NULL,
    failure_reason           TEXT    NOT NULL,           -- last error message (caller truncates to 4KB)
    attempts_made            INTEGER NOT NULL,
    first_failed_at          TEXT    NOT NULL,
    last_attempted_at        TEXT    NOT NULL,
    payload_format_version   INTEGER NOT NULL,
    payload                  BLOB    NOT NULL,
    resolution               TEXT,                        -- NULL = pending
                                                          -- 'retried_succeeded' | 'retried_failed'
                                                          -- | 'acknowledged' | 'auto_recovered'
    resolved_at              TEXT                         -- RFC3339 when resolution set
);

-- Partial index on unresolved entries — the common query path
-- (vault-cli `dead-letter list`, divergence detector cross-reference).
CREATE INDEX IF NOT EXISTS idx_dead_letter_unresolved
    ON dead_letter(memory_id) WHERE resolution IS NULL;

CREATE TABLE IF NOT EXISTS pending_sync (
    memory_id                BLOB    PRIMARY KEY,        -- one pending op per memory (UPSERT semantics:
                                                          -- a later operation supersedes an earlier one)
    operation                TEXT    NOT NULL,
    queued_at                TEXT    NOT NULL            -- RFC3339 — divergence detector sweeps oldest first
);
