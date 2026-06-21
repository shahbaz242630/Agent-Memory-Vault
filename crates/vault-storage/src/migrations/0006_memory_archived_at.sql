-- 0006_memory_archived_at.sql
-- A1 Cold Archive (ADR-084) — the demote-not-delete tool the "keep when unsure"
-- posture leans on (BRD §5.6 lines 995-996, the other half of Phase 4).
--
-- Adds a nullable `archived_at` marker to `memories`. A fact untouched past
-- `archive_after_days` (default 365) is moved to cold archive by the nightly
-- consolidator's Phase 4: `archived_at` is set to the run time, and the fact
-- drops OUT of default retrieval (filtered exactly like superseded/expired
-- facts). It surfaces only via an explicit "search archive" call
-- (`RetrievalOptions::include_archived = true`).
--
-- Soft state, not a separate store (ADR-084): the fact stays in the already
-- SQLCipher-encrypted `vault.db`, so the zero-knowledge guarantee is unchanged
-- and no new crypto path is opened. Archive is reversible — clearing
-- `archived_at` un-archives the fact — and never deletes, so the "no memory
-- ever lost" property (BRD §5.6 line 1023: active | superseded | archived)
-- holds with archived as a first-class third end-state.
--
-- Mirrors the existing nullable-marker columns (`valid_until`, `superseded_by`):
-- RFC3339 (UTC) text, NULL = active. The partial index keeps the
-- "active-only" default-retrieval scan (archived_at IS NULL) cheap as the
-- archive grows.

ALTER TABLE memories ADD COLUMN archived_at TEXT;  -- RFC3339 (UTC); NULL = active

CREATE INDEX IF NOT EXISTS idx_memories_archived_at
    ON memories(archived_at) WHERE archived_at IS NOT NULL;
