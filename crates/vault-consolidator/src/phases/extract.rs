//! Graph-filling: entity + relationship extraction (T0.2.x — closes the
//! tech-debt #2 "entity-extraction-at-consolidation" gap).
//!
//! ## What this does
//!
//! The combined enrichment LLM call (see [`super::enrich`]) returns, alongside
//! the search-alias line, two extra arrays describing the knowledge graph the
//! memory implies:
//!
//! ```json
//! { "entities":      [{"name": "Acme Corp", "type": "organization"}, ...],
//!   "relationships": [{"from": "the user", "relation": "works_at", "to": "Acme Corp"}, ...] }
//! ```
//!
//! This module turns that raw, model-generated JSON into clean graph writes:
//!
//! - [`parse_extracted`] — best-effort parse + cleanup. Never errors (a bad
//!   extraction must not fail the recall-critical alias enrichment it rides
//!   with). Drops empty / over-long names, maps the type label to
//!   [`EntityType`], dedups entities, normalises relation labels, and discards
//!   any relationship whose endpoints are not in the entity list (the model
//!   occasionally references a name it did not list — those links are dropped
//!   rather than written dangling).
//! - [`write_extracted_to_graph`] — **get-or-create** each entity (so nightly
//!   re-runs reuse ids instead of hitting the `(name, type, boundary)` UNIQUE
//!   constraint), then create the relationships. Every write is scoped to the
//!   memory's own [`Boundary`] so the privacy boundary holds.
//!
//! ## Idempotency
//!
//! Extraction rides inside [`super::enrich::enrich_one`], which is skipped for
//! a fact already enriched for its current content (the `content_fp`
//! fingerprint). So a steady-state nightly run never re-extracts an unchanged
//! fact → no duplicate entities or relationships.
//!
//! ## Stale-link retirement (ADR-SEC-002 Part 2)
//!
//! A fact whose *content* changed re-extracts. Its prior content's edges are
//! retired by [`super::enrich`]'s orchestrator
//! ([`crate::Consolidator::enrich_facts`]), which calls
//! [`vault_storage::GraphStore::retire_relationships_for_memory`] for the fact
//! BEFORE writing the fresh extraction — and reconciles merged-away /
//! contradiction-retired facts (no longer active) the same way. Every edge this
//! module writes is therefore tagged with its `source_memory_id` (the
//! `source_memory_id` argument to [`write_extracted_to_graph`]) so that
//! retirement can find it. Without provenance, an obsolete fact's edges (e.g.
//! `user —works_at→ Acme` after the fact became Globex) would live forever.

use std::collections::HashMap;

use serde_json::Value;
use tracing::warn;
use vault_core::{
    Boundary, Entity, EntityType, MemoryId, NewEntity, Relationship, VaultResult,
    MAX_ENTITY_NAME_BYTES, MAX_RELATION_TYPE_BYTES,
};
use vault_storage::GraphStore;

/// One entity the model pulled out of a memory, after cleanup. `name` is the
/// first-seen spelling (used verbatim as the graph node name); `entity_type`
/// is the mapped [`EntityType`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedEntity {
    pub name: String,
    pub entity_type: EntityType,
}

/// One relationship after cleanup. `from` and `to` are guaranteed to match an
/// [`ExtractedEntity::name`] in the same [`ExtractedGraph`] (case-insensitively
/// resolved at parse time, then canonicalised to the listed spelling).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedRelationship {
    pub from: String,
    pub relation: String,
    pub to: String,
}

/// The cleaned graph a single memory implies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractedGraph {
    pub entities: Vec<ExtractedEntity>,
    pub relationships: Vec<ExtractedRelationship>,
}

impl ExtractedGraph {
    /// `true` when there is nothing to write (no entities). A graph with
    /// entities but no relationships is still worth writing — the nodes are
    /// useful on their own.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }
}

/// Counts from one memory's graph write, for the run summary + logs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtractionWriteStats {
    pub entities_created: usize,
    pub entities_reused: usize,
    pub relationships_created: usize,
    pub relationships_failed: usize,
}

/// Map a model-emitted type label to an [`EntityType`]. The combined schema
/// constrains the label to the five canonical names, but the model can still
/// drift (e.g. a British spelling, or a label not in the enum); anything
/// unrecognised becomes [`EntityType::Concept`] — the safe catch-all — rather
/// than polluting the graph with a `Custom` junk type.
fn entity_type_from_label(label: &str) -> EntityType {
    match label.trim().to_lowercase().as_str() {
        "person" => EntityType::Person,
        "organization" | "organisation" => EntityType::Organization,
        "project" => EntityType::Project,
        "location" => EntityType::Location,
        _ => EntityType::Concept,
    }
}

/// Normalise a relation label to a short snake_case token: trim, lowercase,
/// collapse internal whitespace to `_`, drop characters that are not
/// alphanumeric or `_`. Returns an empty string when nothing usable remains
/// (the caller drops empty-relation edges).
fn normalize_relation(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = false;
    for ch in raw.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            prev_underscore = false;
        } else if (ch == '_' || ch.is_whitespace() || ch == '-')
            && !out.is_empty()
            && !prev_underscore
        {
            out.push('_');
            prev_underscore = true;
        }
        // any other char is dropped
    }
    while out.ends_with('_') {
        out.pop();
    }
    // Bound to the domain limit; a sane relation is far shorter.
    if out.len() > MAX_RELATION_TYPE_BYTES {
        out.truncate(MAX_RELATION_TYPE_BYTES);
        while out.ends_with('_') {
            out.pop();
        }
    }
    out
}

/// A normalisation key for matching a relationship endpoint to a listed
/// entity: trimmed + lowercased. Handles the model writing "The user" in a
/// relationship but "the user" in the entity list.
fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Best-effort parse + cleanup of the model's `entities` / `relationships`
/// arrays. NEVER errors: malformed or unusable entries are dropped, and an
/// all-junk response yields an empty [`ExtractedGraph`].
#[must_use]
pub fn parse_extracted(value: &Value) -> ExtractedGraph {
    let entities = parse_entities(value);
    let relationships = parse_relationships(value, &entities);
    ExtractedGraph {
        entities,
        relationships,
    }
}

fn parse_entities(value: &Value) -> Vec<ExtractedEntity> {
    let mut seen: HashMap<(String, EntityType), ()> = HashMap::new();
    let mut out = Vec::new();
    let Some(arr) = value.get("entities").and_then(Value::as_array) else {
        return out;
    };
    for item in arr {
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || name.len() > MAX_ENTITY_NAME_BYTES
            || name.chars().any(char::is_control)
        {
            continue;
        }
        let entity_type = item
            .get("type")
            .and_then(Value::as_str)
            .map_or(EntityType::Concept, entity_type_from_label);
        let dedup_key = (name_key(name), entity_type.clone());
        if seen.insert(dedup_key, ()).is_some() {
            continue; // duplicate (name, type) within one extraction
        }
        out.push(ExtractedEntity {
            name: name.to_string(),
            entity_type,
        });
    }
    out
}

fn parse_relationships(value: &Value, entities: &[ExtractedEntity]) -> Vec<ExtractedRelationship> {
    // normalised endpoint name -> canonical listed spelling
    let canonical: HashMap<String, String> = entities
        .iter()
        .map(|e| (name_key(&e.name), e.name.clone()))
        .collect();
    let mut seen: HashMap<(String, String, String), ()> = HashMap::new();
    let mut out = Vec::new();
    let Some(arr) = value.get("relationships").and_then(Value::as_array) else {
        return out;
    };
    for item in arr {
        let (Some(from), Some(relation), Some(to)) = (
            item.get("from").and_then(Value::as_str),
            item.get("relation").and_then(Value::as_str),
            item.get("to").and_then(Value::as_str),
        ) else {
            continue;
        };
        let relation = normalize_relation(relation);
        if relation.is_empty() {
            continue;
        }
        // Both endpoints must be listed entities (drop dangling links).
        let (Some(from_canon), Some(to_canon)) =
            (canonical.get(&name_key(from)), canonical.get(&name_key(to)))
        else {
            continue;
        };
        if from_canon == to_canon {
            continue; // drop self-loops
        }
        let dedup_key = (from_canon.clone(), relation.clone(), to_canon.clone());
        if seen.insert(dedup_key, ()).is_some() {
            continue;
        }
        out.push(ExtractedRelationship {
            from: from_canon.clone(),
            relation,
            to: to_canon.clone(),
        });
    }
    out
}

/// Write a cleaned [`ExtractedGraph`] into the graph store, scoped to
/// `boundary`. Entities are **get-or-created** (reused by id when already
/// present); relationships are then created between the resolved ids.
///
/// `confidence` is the source memory's confidence — carried onto each edge so
/// retrieval can weight graph facts the same way it weights memories.
///
/// `source_memory_id` is the memory these entities/relationships were extracted
/// from; it is tagged onto every edge as provenance so the consolidator can
/// retire the edge when the fact stops being current (ADR-SEC-002 Part 2). The
/// caller ([`crate::Consolidator::enrich_facts`]) retires this memory's prior
/// edges BEFORE calling this, so the write replaces rather than accumulates.
///
/// # Errors
///
/// Returns the underlying [`VaultError`] only on a genuine storage failure
/// during entity get-or-create (the orchestrator logs-and-counts it per fact
/// and continues). A *relationship* that fails to write is counted in
/// [`ExtractionWriteStats::relationships_failed`] and skipped — one bad edge
/// never aborts the rest of a memory's graph.
pub async fn write_extracted_to_graph(
    graph: &dyn GraphStore,
    boundary: &Boundary,
    extracted: &ExtractedGraph,
    confidence: f32,
    source_memory_id: MemoryId,
) -> VaultResult<ExtractionWriteStats> {
    let mut stats = ExtractionWriteStats::default();
    let mut name_to_id = HashMap::new();

    for ent in &extracted.entities {
        let id = match graph
            .get_entity(&ent.name, &ent.entity_type, boundary)
            .await?
        {
            Some(existing) => {
                stats.entities_reused += 1;
                existing.id
            }
            None => {
                let new_entity = Entity::try_new(NewEntity {
                    name: ent.name.clone(),
                    entity_type: ent.entity_type.clone(),
                    boundary: boundary.clone(),
                })?;
                let id = new_entity.id;
                graph.create_entity(&new_entity).await?;
                stats.entities_created += 1;
                id
            }
        };
        name_to_id.insert(name_key(&ent.name), id);
    }

    for r in &extracted.relationships {
        let (Some(&from_id), Some(&to_id)) = (
            name_to_id.get(&name_key(&r.from)),
            name_to_id.get(&name_key(&r.to)),
        ) else {
            // parse_extracted guarantees endpoints are listed entities, so this
            // is defensive only.
            stats.relationships_failed += 1;
            continue;
        };
        match Relationship::try_new(
            from_id,
            to_id,
            r.relation.clone(),
            confidence,
            Some(source_memory_id),
        ) {
            Ok(rel) => match graph.create_relationship(&rel).await {
                Ok(()) => stats.relationships_created += 1,
                Err(e) => {
                    warn!(
                        from = %r.from, relation = %r.relation, to = %r.to,
                        error = %e, "extracted relationship failed to write; skipping"
                    );
                    stats.relationships_failed += 1;
                }
            },
            Err(e) => {
                warn!(
                    relation = %r.relation, error = %e,
                    "extracted relationship failed validation; skipping"
                );
                stats.relationships_failed += 1;
            }
        }
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ─── entity_type_from_label ───────────────────────────────────────────

    #[test]
    fn entity_type_label_mapping_incl_unknown_to_concept() {
        assert_eq!(entity_type_from_label("person"), EntityType::Person);
        assert_eq!(
            entity_type_from_label("Organization"),
            EntityType::Organization
        );
        assert_eq!(
            entity_type_from_label("organisation"),
            EntityType::Organization
        );
        assert_eq!(entity_type_from_label("LOCATION"), EntityType::Location);
        assert_eq!(entity_type_from_label("project"), EntityType::Project);
        assert_eq!(entity_type_from_label("concept"), EntityType::Concept);
        // Unknown / drifted label → Concept (safe catch-all), never Custom junk.
        assert_eq!(entity_type_from_label("vehicle"), EntityType::Concept);
        assert_eq!(entity_type_from_label(""), EntityType::Concept);
    }

    // ─── normalize_relation ───────────────────────────────────────────────

    #[test]
    fn normalize_relation_makes_snake_case_tokens() {
        assert_eq!(normalize_relation("works_at"), "works_at");
        assert_eq!(normalize_relation("  Works At "), "works_at");
        assert_eq!(normalize_relation("is allergic to"), "is_allergic_to");
        assert_eq!(normalize_relation("sibling-of"), "sibling_of");
        assert_eq!(normalize_relation("__weird__"), "weird");
        assert_eq!(normalize_relation("   "), "");
        assert_eq!(normalize_relation("!!!"), "");
    }

    // ─── parse_extracted ──────────────────────────────────────────────────

    #[test]
    fn parse_extracts_clean_entities_and_relationships() {
        let v = json!({
            "aliases": ["family"],
            "entities": [
                {"name": "the user", "type": "person"},
                {"name": "Maria", "type": "person"},
                {"name": "Lisbon", "type": "location"},
                {"name": "hospital", "type": "organization"}
            ],
            "relationships": [
                {"from": "the user", "relation": "sibling_of", "to": "Maria"},
                {"from": "Maria", "relation": "lives_in", "to": "Lisbon"},
                {"from": "Maria", "relation": "works_at", "to": "hospital"}
            ]
        });
        let g = parse_extracted(&v);
        assert_eq!(g.entities.len(), 4);
        assert_eq!(g.relationships.len(), 3);
        assert!(g
            .entities
            .iter()
            .any(|e| e.name == "Lisbon" && e.entity_type == EntityType::Location));
    }

    #[test]
    fn parse_drops_relationship_with_unlisted_endpoint() {
        // "shellfish allergy" is referenced but never listed as an entity →
        // the link is dropped rather than written dangling.
        let v = json!({
            "entities": [{"name": "the user", "type": "person"}, {"name": "shellfish", "type": "concept"}],
            "relationships": [
                {"from": "the user", "relation": "has", "to": "shellfish allergy"},
                {"from": "the user", "relation": "is_allergic_to", "to": "shellfish"}
            ]
        });
        let g = parse_extracted(&v);
        assert_eq!(g.relationships.len(), 1, "dangling endpoint link dropped");
        assert_eq!(g.relationships[0].to, "shellfish");
    }

    #[test]
    fn parse_resolves_endpoints_case_insensitively_to_listed_spelling() {
        // model lists "the user" but writes "The user" in the relationship.
        let v = json!({
            "entities": [{"name": "the user", "type": "person"}, {"name": "Porto", "type": "location"}],
            "relationships": [{"from": "The user", "relation": "settled_in", "to": "porto"}]
        });
        let g = parse_extracted(&v);
        assert_eq!(g.relationships.len(), 1);
        assert_eq!(
            g.relationships[0].from, "the user",
            "canonicalised to listed spelling"
        );
        assert_eq!(g.relationships[0].to, "Porto");
    }

    #[test]
    fn parse_dedups_entities_and_relationships_and_drops_self_loops() {
        let v = json!({
            "entities": [
                {"name": "the user", "type": "person"},
                {"name": "the user", "type": "person"},
                {"name": "cello", "type": "concept"}
            ],
            "relationships": [
                {"from": "the user", "relation": "learning", "to": "cello"},
                {"from": "the user", "relation": "learning", "to": "cello"},
                {"from": "the user", "relation": "is", "to": "the user"}
            ]
        });
        let g = parse_extracted(&v);
        assert_eq!(g.entities.len(), 2, "duplicate (name,type) deduped");
        assert_eq!(
            g.relationships.len(),
            1,
            "duplicate edge deduped, self-loop dropped"
        );
    }

    #[test]
    fn parse_drops_empty_overlong_and_malformed_names() {
        let huge = "x".repeat(MAX_ENTITY_NAME_BYTES + 1);
        let v = json!({
            "entities": [
                {"name": "", "type": "person"},
                {"name": huge, "type": "concept"},
                {"name": 42, "type": "person"},
                {"name": "valid", "type": "concept"}
            ],
            "relationships": []
        });
        let g = parse_extracted(&v);
        assert_eq!(g.entities.len(), 1);
        assert_eq!(g.entities[0].name, "valid");
    }

    #[test]
    fn parse_missing_or_wrong_typed_fields_yield_empty_never_panics() {
        assert!(parse_extracted(&json!({})).is_empty());
        assert!(parse_extracted(&json!({"entities": "not an array"})).is_empty());
        assert!(parse_extracted(&json!({"entities": [], "relationships": "nope"})).is_empty());
        assert!(parse_extracted(&json!(null)).is_empty());
    }
}
