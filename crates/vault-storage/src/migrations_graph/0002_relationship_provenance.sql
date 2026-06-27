-- T0.2.x / ADR-SEC-002 Part 2 — relationship provenance (the stale-links fix).
--
-- Adds `source_memory_id`: the memory a relationship was extracted from. This
-- is what lets the consolidator RETIRE an edge when its source fact stops being
-- the current truth (content changed → re-extracted, merged away, or retired by
-- a contradiction). Before this column there was no link from an edge back to
-- its originating memory, so an obsolete fact's edges (e.g. `user —works_at→
-- Acme` after the fact changed to Globex) lived in the graph forever.
--
-- NULLABLE on purpose: rows written before this migration (and any future
-- non-extraction edge) carry NULL. Retirement targets only edges with a
-- matching `source_memory_id`, so NULL/legacy edges are simply never
-- auto-retired — they predate provenance and there is no fact to tie them to.
--
-- Stored as a 16-byte UUID BLOB, matching `entities.id` / `relationships.id`.
-- An index supports the retirement UPDATE's `WHERE source_memory_id = ?` lookup.

ALTER TABLE relationships ADD COLUMN source_memory_id BLOB;

CREATE INDEX IF NOT EXISTS idx_rel_source_memory ON relationships(source_memory_id);
