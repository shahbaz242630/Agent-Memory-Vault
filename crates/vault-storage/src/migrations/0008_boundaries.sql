-- 0008_boundaries.sql
-- UI slice 2 — boundaries become a first-class row, not an implied column value.
--
-- Before this migration a boundary existed only as the TEXT value of
-- `memories.boundary`: a boundary with no memories in it had nowhere to live, so
-- the desktop UI could list boundaries (SELECT DISTINCT) but could not CREATE
-- one. This table lets a user name a boundary up front ("work", "personal") and
-- put memories in it afterwards.
--
-- The `memories.boundary` column stays authoritative for ACCESS CONTROL. This
-- table is a registry of names + metadata, NOT a new enforcement point: boundary
-- filtering remains in the WHERE clause at the storage layer per BRD §11.4.3
-- rule 3 ("boundary filtering happens BEFORE any retrieval logic — at the
-- storage layer"). Nothing here is consulted on a read path. Deliberately NO
-- foreign key from memories.boundary → boundaries.name: a FK would let a failed
-- registry write block a memory write, and losing a memory is worse than an
-- unregistered boundary name (recall is sacrosanct).
--
-- Lives in the already-SQLCipher-encrypted vault.db — same posture as every
-- other table, no new crypto path.

CREATE TABLE IF NOT EXISTS boundaries (
    name        TEXT PRIMARY KEY,   -- validated by vault_core::Boundary before it reaches here
    description TEXT,               -- optional user-supplied label; NULL = none
    created_at  TEXT NOT NULL       -- RFC3339 (UTC)
);

-- Backfill every boundary already implied by existing memories, so an upgrading
-- vault shows its real boundaries immediately rather than an empty tab.
-- created_at = the earliest memory in that boundary, which IS when the boundary
-- came into existence. INSERT OR IGNORE keeps this safe to re-run.
INSERT OR IGNORE INTO boundaries (name, description, created_at)
SELECT boundary, NULL, MIN(created_at)
FROM memories
GROUP BY boundary;

-- The conventional default boundary (vault_core::Boundary::default_name) always
-- exists, even in a vault with zero memories — the UI must never show an empty
-- boundary list. Placed after the backfill so a vault that already has `default`
-- memories keeps their honest earliest-memory timestamp instead of "now".
INSERT OR IGNORE INTO boundaries (name, description, created_at)
VALUES ('default', NULL, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
