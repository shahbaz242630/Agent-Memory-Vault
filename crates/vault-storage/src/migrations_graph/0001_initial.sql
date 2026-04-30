-- T0.1.5 — initial DuckDB graph-store schema (ADR-015).
--
-- Boundary scoping is enforced at the schema layer:
--   * entities.boundary is NOT NULL
--   * (name, entity_type, boundary) is UNIQUE — same name in two different
--     boundaries → two distinct entities (ADR-015 watch-item #3).
--
-- Relationships carry a denormalised `boundary` column so traversal queries
-- can filter by boundary at SQL level without joining back to entities on
-- every hop. The within-boundary invariant ("from + to in same boundary
-- unless relation_type IN ('same_as', 'alias_for')") is APP-LAYER ENFORCED
-- inside DuckDbGraphStore::create_relationship — DuckDB 1.x supports
-- neither subquery-CHECK nor triggers, and CHECK constraints in DuckDB are
-- per-row only. The property test in `mod tests` is the substitute for
-- the SQL-layer backstop.
--
-- Bi-temporal columns live on relationships from day one (ADR-015 watch-
-- item #2): `valid_until = NULL` means "still valid"; the consolidator
-- (T0.2.x) will set `valid_until = now` and insert a successor edge to
-- supersede a relationship without losing history.

-- entity_type is JSON-serialised (e.g. '"Person"' or '{"custom":"team"}') to
-- handle the EntityType::Custom(String) variant cleanly. SQL queries for
-- fixed types must use quoted JSON strings:
--     WHERE entity_type = '"Person"'   -- correct
--     WHERE entity_type = 'Person'     -- wrong, no match
-- Future: if SQL ergonomics matter (analytics, ad-hoc queries), split into
-- (entity_type_kind TEXT NOT NULL, entity_type_custom TEXT NULL).
CREATE TABLE IF NOT EXISTS entities (
    id           BLOB    PRIMARY KEY,        -- UUID v7, 16 bytes
    name         TEXT    NOT NULL,
    entity_type  TEXT    NOT NULL,           -- JSON-serialised EntityType (see note above)
    boundary     TEXT    NOT NULL,           -- validated Boundary newtype value
    created_at   TEXT    NOT NULL,           -- RFC3339 UTC
    UNIQUE (name, entity_type, boundary)
);

CREATE INDEX IF NOT EXISTS idx_entities_boundary ON entities(boundary);

-- INVARIANT (ADR-015):
--     relationships.boundary == from_entity.boundary == to_entity.boundary
-- with one exception: when relation_type IN ('same_as', 'alias_for'), the
-- two endpoint boundaries may differ; in that case relationships.boundary
-- is set to the from-side endpoint's boundary (asymmetric but consistent —
-- enables traversal-time filtering even on alias rows).
--
-- The invariant is enforced at the application layer inside
-- DuckDbGraphStore::create_relationship (DuckDB 1.x supports neither
-- subquery-CHECK nor triggers; the property test in `mod tests` is the
-- substitute SQL-layer backstop).
--
-- Any future code path that mutates entity.boundary (none in V0.1; possibly
-- in V1.0 for user-driven boundary moves) MUST propagate the change to
-- every relationship row whose from_entity_id or to_entity_id references
-- the entity. Failing to do so silently breaks traversal-time boundary
-- filtering. There is no SQL-layer guard against this — the comment is
-- the breadcrumb.
CREATE TABLE IF NOT EXISTS relationships (
    id              BLOB     PRIMARY KEY,
    from_entity_id  BLOB     NOT NULL REFERENCES entities(id),
    to_entity_id    BLOB     NOT NULL REFERENCES entities(id),
    relation_type   TEXT     NOT NULL,
    boundary        TEXT     NOT NULL,         -- denormalised; see invariant comment above
    valid_from      TEXT     NOT NULL,         -- RFC3339 UTC
    valid_until     TEXT,                      -- NULL = still valid
    confidence      DOUBLE   NOT NULL,
    CHECK (confidence >= 0.0 AND confidence <= 1.0),
    CHECK (valid_until IS NULL OR valid_until >= valid_from)
);

CREATE INDEX IF NOT EXISTS idx_rel_from_entity ON relationships(from_entity_id);
CREATE INDEX IF NOT EXISTS idx_rel_to_entity   ON relationships(to_entity_id);
CREATE INDEX IF NOT EXISTS idx_rel_boundary    ON relationships(boundary);
