-- Memory Vault — initial metadata schema (T0.1.3, BRD §5.2).
--
-- Vector embeddings live in LanceDB (T0.1.4), not here. Graph entities and
-- relationships live in DuckDB (T0.1.5), not here. SQLite is the durable
-- record-of-truth for memory metadata, audit events, and migrations.

CREATE TABLE IF NOT EXISTS memories (
    id              TEXT    PRIMARY KEY,                 -- UUID v7
    content         TEXT    NOT NULL,
    memory_type     TEXT    NOT NULL,                    -- 'episodic' | 'semantic' | 'procedural'
    source_agent    TEXT,                                 -- nullable
    boundary        TEXT    NOT NULL,
    created_at      TEXT    NOT NULL,                     -- ISO 8601 UTC
    valid_from      TEXT    NOT NULL,
    valid_until     TEXT,                                 -- nullable
    confidence      REAL    NOT NULL,                     -- [0.0, 1.0]
    access_count    INTEGER NOT NULL DEFAULT 0,
    last_accessed   TEXT    NOT NULL,
    superseded_by   TEXT,                                 -- nullable, refers to memories.id
    metadata_json   TEXT    NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_memories_boundary       ON memories(boundary);
CREATE INDEX IF NOT EXISTS idx_memories_memory_type    ON memories(memory_type);
CREATE INDEX IF NOT EXISTS idx_memories_created_at     ON memories(created_at);
CREATE INDEX IF NOT EXISTS idx_memories_superseded_by  ON memories(superseded_by);

-- Tamper-evident audit log per BRD §11.9.2.
-- Each event hash-chains to the previous via prev_event_hash; event_hash =
-- BLAKE3(prev_event_hash || canonical_event_bytes). Tampering with any row
-- breaks the chain at validation time.
CREATE TABLE IF NOT EXISTS audit_log (
    seq             INTEGER PRIMARY KEY AUTOINCREMENT,    -- monotonic insertion order
    event_id        TEXT    NOT NULL UNIQUE,              -- UUID v7
    timestamp       TEXT    NOT NULL,                     -- ISO 8601 UTC
    user_id         TEXT,                                 -- nullable in V0.1
    device_id       TEXT,                                 -- nullable in V0.1
    event_type      TEXT    NOT NULL,                     -- 'memory.create' | 'memory.read' | etc.
    resource_type   TEXT,
    resource_id     TEXT,
    boundary        TEXT,
    actor_kind      TEXT    NOT NULL,                     -- 'user' | 'agent' | 'system'
    actor_name      TEXT,
    result          TEXT    NOT NULL,                     -- 'success' | 'denied' | 'error'
    details_json    TEXT    NOT NULL DEFAULT '{}',
    prev_event_hash TEXT    NOT NULL,                     -- hex-encoded BLAKE3 (64 chars)
    event_hash      TEXT    NOT NULL                      -- hex-encoded BLAKE3 (64 chars)
);

CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp  ON audit_log(timestamp);
CREATE INDEX IF NOT EXISTS idx_audit_log_event_type ON audit_log(event_type);
