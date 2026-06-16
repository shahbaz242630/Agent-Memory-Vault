-- 0004_consolidation_checkpoints.sql
-- T0.2.5 — Checkpoint & Rollback (BRD §5.6 line 998 + §6.2).
--
-- "Every consolidation run creates a checkpoint. Users can roll back to a
--  previous checkpoint via UI. Checkpoint is a snapshot of changed memory IDs
--  and their pre-consolidation state." — BRD §5.6.
--
-- A checkpoint is NOT a full vault copy — it is an undo-log of only what a run
-- touched. It lives in the SQLCipher metadata DB, so it inherits the vault's
-- zero-knowledge encryption at rest (the pre-image blobs hold memory content).
--
-- V1 scope (2026-06-15, founder-locked): captures the AUTHORITATIVE stores that
-- drive answers — the memory rows + their embeddings. Graph (DuckDB) rollback is
-- deferred until the graph enters the read path (HANDOFF tech-debt #2 tripwire);
-- the graph is write-only / not consumed at read in V0.2.

-- One row per consolidation run that changed at least one memory.
CREATE TABLE IF NOT EXISTS consolidation_checkpoints (
    id          BLOB    PRIMARY KEY,                 -- checkpoint UUID (16 bytes)
    created_at  TEXT    NOT NULL,                    -- RFC3339 run timestamp
    status      TEXT    NOT NULL DEFAULT 'active',   -- 'active' | 'rolled_back'
    entry_count INTEGER NOT NULL DEFAULT 0           -- # changed memories captured
);

-- One row per memory a run changed.
--   change_type = 'modified': the memory existed before the run and was mutated
--     (superseded / invalidated / decayed / enriched). `pre_image` holds the
--     full pre-consolidation Memory + its embedding (versioned blob) so restore
--     is EXACT (re-applied via StorageBackend::update_memory on rollback).
--   change_type = 'created': the memory was created by the run (new merged /
--     enriched row). `pre_image` is NULL; rollback deletes the memory + vector.
CREATE TABLE IF NOT EXISTS checkpoint_entries (
    checkpoint_id     BLOB    NOT NULL
        REFERENCES consolidation_checkpoints(id) ON DELETE CASCADE,
    memory_id         BLOB    NOT NULL,
    boundary          TEXT    NOT NULL,
    change_type       TEXT    NOT NULL,              -- 'modified' | 'created'
    pre_image_version INTEGER,                       -- payload format version (NULL for 'created')
    pre_image         BLOB,                          -- serialized {Memory, embedding} (NULL for 'created')
    PRIMARY KEY (checkpoint_id, memory_id)
);

-- Hot path: load all entries for one checkpoint (rollback) + prune-by-age
-- retention sweep (keep last N=7 checkpoints).
CREATE INDEX IF NOT EXISTS idx_checkpoint_entries_cp
    ON checkpoint_entries(checkpoint_id);
CREATE INDEX IF NOT EXISTS idx_consolidation_checkpoints_created
    ON consolidation_checkpoints(created_at);
