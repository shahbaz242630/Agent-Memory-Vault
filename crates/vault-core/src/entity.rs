//! [`Entity`] and [`Relationship`] — the knowledge-graph types.
//!
//! Entities represent the people, projects, organisations, and concepts that
//! memories reference. Relationships are bi-temporal edges between entities
//! (BRD §1.3 confidence-decay knowledge graph: every fact has `valid_from` /
//! `valid_until`, allowing the consolidator to retire superseded edges
//! without losing history).
//!
//! See `Agent Build Specification.txt` §5.1.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::boundary::Boundary;
use crate::error::{VaultError, VaultResult};
use crate::memory::MemoryId;

/// Maximum length of an entity name in bytes (BRD §11.7.1).
pub const MAX_ENTITY_NAME_BYTES: usize = 256;

/// Strongly-typed identifier for an [`Entity`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityId(pub Uuid);

impl EntityId {
    /// Create a new, unique, time-ordered ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for EntityId {
    type Err = VaultError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|e| VaultError::InvalidInput(format!("invalid entity id: {e}")))
    }
}

/// Strongly-typed identifier for a [`Relationship`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RelationshipId(pub Uuid);

impl RelationshipId {
    /// Create a new, unique, time-ordered ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for RelationshipId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RelationshipId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for RelationshipId {
    type Err = VaultError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|e| VaultError::InvalidInput(format!("invalid relationship id: {e}")))
    }
}

/// The kind of an [`Entity`]. The `Custom` variant lets callers introduce
/// user-defined types (e.g., `"product"`, `"team"`) without modifying this
/// enum — the consolidator and retrieval layers treat unknown types as
/// opaque labels.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Person,
    Organization,
    Project,
    Location,
    Concept,
    Custom(String),
}

/// A graph node. Entities deduplicate references across memories — multiple
/// memories about "Sara" share one [`EntityId`] so graph traversal can find
/// related context.
///
/// Each entity is **boundary-scoped** at the schema layer (HANDOFF.md
/// ADR-015). The same name in two different boundaries is two distinct
/// entities — cross-boundary fusion is a privacy decision the user opts into
/// via an explicit `same_as` / `alias_for` relationship, never a default.
///
/// **Invariants** (enforced by [`Entity::try_new`] / [`Entity::validate`]):
/// - `name` is non-empty and ≤ [`MAX_ENTITY_NAME_BYTES`] bytes
/// - `name` contains no control characters
/// - `boundary` is a validated [`Boundary`] (charset enforced by the newtype)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entity {
    pub id: EntityId,
    pub name: String,
    pub entity_type: EntityType,
    pub boundary: Boundary,
    pub created_at: DateTime<Utc>,
}

/// Builder-style arguments for [`Entity::try_new`].
#[derive(Clone, Debug)]
pub struct NewEntity {
    pub name: String,
    pub entity_type: EntityType,
    pub boundary: Boundary,
}

impl Entity {
    /// Create a new validated entity with a fresh ID and current timestamp.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::InvalidInput`] if any of the invariants
    /// listed on [`Entity`] are violated by the provided arguments.
    pub fn try_new(args: NewEntity) -> VaultResult<Self> {
        let entity = Self {
            id: EntityId::new(),
            name: args.name,
            entity_type: args.entity_type,
            boundary: args.boundary,
            created_at: Utc::now(),
        };
        entity.validate()?;
        Ok(entity)
    }

    /// Re-check all invariants. Storage layers must call this before write
    /// (BRD §11.7.1: validate at every public API boundary).
    pub fn validate(&self) -> VaultResult<()> {
        if self.name.is_empty() {
            return Err(VaultError::InvalidInput(
                "entity name must not be empty".into(),
            ));
        }
        if self.name.len() > MAX_ENTITY_NAME_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "entity name exceeds {MAX_ENTITY_NAME_BYTES} bytes",
            )));
        }
        if self.name.chars().any(|c| c.is_control()) {
            return Err(VaultError::InvalidInput(
                "entity name contains control characters".into(),
            ));
        }
        if let EntityType::Custom(label) = &self.entity_type {
            if label.is_empty() {
                return Err(VaultError::InvalidInput(
                    "custom entity type label must not be empty".into(),
                ));
            }
        }
        Ok(())
    }
}

/// A graph edge with bi-temporal validity.
///
/// `valid_from` and `valid_until` together enable contradiction-aware
/// retrieval: when two relationships disagree, the consolidator marks the
/// older one's `valid_until` and creates a new edge — the timeline is
/// preserved (BRD §1.3, §5.6).
///
/// **Invariants**:
/// - `relation_type` is non-empty and ≤ 64 bytes
/// - `confidence` is finite and in `[0.0, 1.0]`
/// - `valid_until`, if present, is ≥ `valid_from`
///
/// ## Provenance (`source_memory_id`, ADR-SEC-002 Part 2)
///
/// The memory this edge was extracted from. `Some` for every edge the
/// consolidator's enrichment pass writes; `None` only for edges that predate
/// provenance (legacy rows migrated from the pre-provenance schema). Provenance
/// is what lets the consolidator **retire an edge when its source fact stops
/// being the current truth** — a fact whose content changed (re-extracted), was
/// merged away, or was retired by a contradiction. Without it, an edge from an
/// obsolete fact (e.g. `user —works_at→ Acme` after the fact changed to Globex)
/// would live forever and pollute the graph. See
/// `vault_consolidator::phases::extract` and `Consolidator::enrich_facts`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relationship {
    pub id: RelationshipId,
    pub from_entity: EntityId,
    pub to_entity: EntityId,
    pub relation_type: String,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: f32,
    /// The memory this edge was extracted from (ADR-SEC-002 Part 2). `None`
    /// only for legacy rows that predate provenance.
    pub source_memory_id: Option<MemoryId>,
}

/// Maximum length of a relation-type label in bytes.
pub const MAX_RELATION_TYPE_BYTES: usize = 64;

impl Relationship {
    /// Create a new validated relationship with a fresh ID and `valid_from = now`.
    ///
    /// `source_memory_id` is the memory this edge was extracted from (the
    /// consolidator always supplies it; only legacy/migrated edges carry
    /// `None`). It drives edge retirement when the source fact changes — see
    /// [`Relationship`] docs.
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::InvalidInput`] if any invariant is violated.
    pub fn try_new(
        from_entity: EntityId,
        to_entity: EntityId,
        relation_type: impl Into<String>,
        confidence: f32,
        source_memory_id: Option<MemoryId>,
    ) -> VaultResult<Self> {
        let rel = Self {
            id: RelationshipId::new(),
            from_entity,
            to_entity,
            relation_type: relation_type.into(),
            valid_from: Utc::now(),
            valid_until: None,
            confidence,
            source_memory_id,
        };
        rel.validate()?;
        Ok(rel)
    }

    /// Re-check all invariants. Storage layers must call this before write.
    pub fn validate(&self) -> VaultResult<()> {
        if self.relation_type.is_empty() {
            return Err(VaultError::InvalidInput(
                "relation_type must not be empty".into(),
            ));
        }
        if self.relation_type.len() > MAX_RELATION_TYPE_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "relation_type exceeds {MAX_RELATION_TYPE_BYTES} bytes",
            )));
        }
        if !self.confidence.is_finite() || !(0.0..=1.0).contains(&self.confidence) {
            return Err(VaultError::InvalidInput(format!(
                "confidence {} is not in [0.0, 1.0]",
                self.confidence
            )));
        }
        if let Some(until) = self.valid_until {
            if until < self.valid_from {
                return Err(VaultError::InvalidInput(
                    "valid_until precedes valid_from".into(),
                ));
            }
        }
        Ok(())
    }

    /// True if this edge has been retired (consolidator set `valid_until`).
    #[must_use]
    pub fn is_retired_at(&self, when: DateTime<Utc>) -> bool {
        self.valid_until.is_some_and(|u| when >= u)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn ne(name: &str, entity_type: EntityType) -> NewEntity {
        NewEntity {
            name: name.to_string(),
            entity_type,
            boundary: Boundary::new("work").unwrap(),
        }
    }

    #[test]
    fn entity_try_new_produces_valid_entity() {
        let e = Entity::try_new(ne("Sara", EntityType::Person)).unwrap();
        e.validate().unwrap();
        assert_eq!(e.entity_type, EntityType::Person);
        assert_eq!(e.name, "Sara");
        assert_eq!(e.boundary.as_str(), "work");
    }

    #[test]
    fn empty_entity_name_rejected() {
        assert!(matches!(
            Entity::try_new(ne("", EntityType::Person)),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn overlong_entity_name_rejected() {
        let too_long = "x".repeat(MAX_ENTITY_NAME_BYTES + 1);
        assert!(matches!(
            Entity::try_new(ne(&too_long, EntityType::Concept)),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn empty_custom_entity_label_rejected() {
        assert!(matches!(
            Entity::try_new(ne("X", EntityType::Custom(String::new()))),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn entity_in_two_boundaries_with_same_name_are_distinct() {
        // ADR-015: same (name, entity_type) in different boundaries → two
        // distinct entities. Cross-boundary fusion is opt-in via same_as.
        let work = NewEntity {
            name: "Sarah".into(),
            entity_type: EntityType::Person,
            boundary: Boundary::new("work").unwrap(),
        };
        let personal = NewEntity {
            name: "Sarah".into(),
            entity_type: EntityType::Person,
            boundary: Boundary::new("personal").unwrap(),
        };
        let a = Entity::try_new(work).unwrap();
        let b = Entity::try_new(personal).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.name, b.name);
        assert_ne!(a.boundary, b.boundary);
    }

    #[test]
    fn entity_id_round_trips_via_display_and_fromstr() {
        let id = EntityId::new();
        let parsed: EntityId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn entity_id_uniqueness_across_many_constructions() {
        let n = 10_000;
        let ids: std::collections::HashSet<_> = (0..n).map(|_| EntityId::new()).collect();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn relationship_id_round_trips_via_display_and_fromstr() {
        let id = RelationshipId::new();
        let parsed: RelationshipId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn relationship_try_new_produces_valid_edge() {
        let a = EntityId::new();
        let b = EntityId::new();
        let r = Relationship::try_new(a, b, "works_with", 0.8, None).unwrap();
        r.validate().unwrap();
        assert!(!r.is_retired_at(Utc::now()));
        assert!(r.source_memory_id.is_none());
    }

    #[test]
    fn relationship_rejects_out_of_range_confidence() {
        let a = EntityId::new();
        let b = EntityId::new();
        assert!(matches!(
            Relationship::try_new(a, b, "rel", 2.0, None),
            Err(VaultError::InvalidInput(_))
        ));
        assert!(matches!(
            Relationship::try_new(a, b, "rel", f32::NAN, None),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn relationship_rejects_overlong_relation_type() {
        let a = EntityId::new();
        let b = EntityId::new();
        let too_long = "x".repeat(MAX_RELATION_TYPE_BYTES + 1);
        assert!(matches!(
            Relationship::try_new(a, b, too_long, 0.5, None),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn relationship_retired_at_after_valid_until() {
        let a = EntityId::new();
        let b = EntityId::new();
        let mut r = Relationship::try_new(a, b, "rel", 0.5, None).unwrap();
        let cutoff = r.valid_from + chrono::Duration::seconds(10);
        r.valid_until = Some(cutoff);
        r.validate().unwrap();
        assert!(!r.is_retired_at(r.valid_from));
        assert!(r.is_retired_at(cutoff));
        assert!(r.is_retired_at(cutoff + chrono::Duration::seconds(1)));
    }

    #[test]
    fn entity_type_serde_roundtrip_for_all_variants() {
        let cases = [
            EntityType::Person,
            EntityType::Organization,
            EntityType::Project,
            EntityType::Location,
            EntityType::Concept,
            EntityType::Custom("widget".into()),
        ];
        for et in cases {
            let json = serde_json::to_string(&et).unwrap();
            let back: EntityType = serde_json::from_str(&json).unwrap();
            assert_eq!(et, back, "round trip failed via {json}");
        }
    }

    #[test]
    fn entity_serde_roundtrip() {
        let e = Entity::try_new(ne("Acme Corp", EntityType::Organization)).unwrap();
        let json = serde_json::to_string(&e).unwrap();
        let back: Entity = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
        back.validate().unwrap();
    }

    #[test]
    fn relationship_serde_roundtrip() {
        let a = EntityId::new();
        let b = EntityId::new();
        let r = Relationship::try_new(a, b, "owns", 0.95, None).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let back: Relationship = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
        back.validate().unwrap();
    }

    #[test]
    fn relationship_carries_source_memory_provenance_and_round_trips() {
        let a = EntityId::new();
        let b = EntityId::new();
        let src = MemoryId::new();
        let r = Relationship::try_new(a, b, "works_at", 0.9, Some(src)).unwrap();
        assert_eq!(
            r.source_memory_id,
            Some(src),
            "provenance is carried onto the edge"
        );
        // Survives a serde round-trip (snapshot/restore relies on this).
        let back: Relationship = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(r, back);
        assert_eq!(back.source_memory_id, Some(src));
    }

    proptest! {
        #[test]
        fn entity_id_serde_roundtrip(raw in any::<u128>()) {
            let id = EntityId(Uuid::from_u128(raw));
            let json = serde_json::to_string(&id).unwrap();
            let back: EntityId = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(id, back);
        }

        #[test]
        fn relationship_id_serde_roundtrip(raw in any::<u128>()) {
            let id = RelationshipId(Uuid::from_u128(raw));
            let json = serde_json::to_string(&id).unwrap();
            let back: RelationshipId = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(id, back);
        }
    }
}
