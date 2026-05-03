//! [`Memory`] — the central domain entity for the vault.
//!
//! A `Memory` represents one piece of remembered information: a fact, an
//! event, a preference, or a procedure. Memories are time-aware
//! (`valid_from` / `valid_until`), confidence-scored, and supersession-linked
//! so the consolidator can merge duplicates without losing provenance.
//!
//! See `Agent Build Specification.txt` §5.1 (data model) and §1.3 (the
//! confidence-decay knowledge graph bet).

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::boundary::Boundary;
use crate::error::{VaultError, VaultResult};

/// Maximum size of a single memory's content in bytes (BRD §11.7.1).
pub const MAX_MEMORY_CONTENT_BYTES: usize = 100 * 1024;

/// The kind of memory, used for query routing in the retrieval layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryType {
    /// Specific events with time and place ("Met Sara on Tuesday in Berlin").
    Episodic,
    /// Facts, preferences, knowledge ("I prefer dark mode").
    Semantic,
    /// How-to, workflows, patterns ("My usual approach to PR reviews").
    Procedural,
}

/// Strongly-typed identifier for a [`Memory`]. Wraps a UUID v7 (time-ordered)
/// so on-disk indexes get good locality without sacrificing uniqueness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemoryId(pub Uuid);

impl MemoryId {
    /// Create a new, unique, time-ordered ID.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }
}

impl Default for MemoryId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for MemoryId {
    type Err = VaultError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s)
            .map(Self)
            .map_err(|e| VaultError::InvalidInput(format!("invalid memory id: {e}")))
    }
}

/// The central domain entity. See module docs for design notes.
///
/// Fields are public per BRD §5.1 so storage and retrieval can read them
/// directly. Construction goes through [`Memory::try_new`] (or the
/// [`Memory::validate`] check) to enforce invariants. Direct field
/// mutation is allowed but downstream callers are expected to call
/// `validate()` again before persisting.
///
/// **Invariants**:
/// - `content` is non-empty and ≤ [`MAX_MEMORY_CONTENT_BYTES`] bytes
/// - `confidence` is finite and in `[0.0, 1.0]`
/// - `valid_until`, if present, is ≥ `valid_from`
/// - `embedding`, if present, is non-empty and contains finite values
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: MemoryId,
    pub content: String,
    pub memory_type: MemoryType,
    pub source_agent: Option<String>,
    pub boundary: Boundary,
    pub created_at: DateTime<Utc>,
    pub valid_from: DateTime<Utc>,
    pub valid_until: Option<DateTime<Utc>>,
    pub confidence: f32,
    pub access_count: u32,
    pub last_accessed: DateTime<Utc>,
    pub superseded_by: Option<MemoryId>,
    pub embedding: Option<Vec<f32>>,
    pub metadata: serde_json::Value,
}

/// Builder-style arguments for [`Memory::try_new`]. Defaults below are
/// applied when fields are omitted by the caller.
#[derive(Clone, Debug)]
pub struct NewMemory {
    pub content: String,
    pub memory_type: MemoryType,
    pub boundary: Boundary,
    pub source_agent: Option<String>,
    pub confidence: f32,
    pub valid_from: Option<DateTime<Utc>>,
    pub valid_until: Option<DateTime<Utc>>,
    pub metadata: serde_json::Value,
}

impl Memory {
    /// Create a new validated memory with sensible defaults for the
    /// system-managed fields (`id`, `created_at`, `last_accessed`,
    /// `access_count`, `superseded_by`, `embedding`).
    ///
    /// # Errors
    ///
    /// Returns [`VaultError::InvalidInput`] if any of the invariants
    /// listed on [`Memory`] are violated by the provided arguments.
    pub fn try_new(args: NewMemory) -> VaultResult<Self> {
        let now = Utc::now();
        let memory = Self {
            id: MemoryId::new(),
            content: args.content,
            memory_type: args.memory_type,
            source_agent: args.source_agent,
            boundary: args.boundary,
            created_at: now,
            valid_from: args.valid_from.unwrap_or(now),
            valid_until: args.valid_until,
            confidence: args.confidence,
            access_count: 0,
            last_accessed: now,
            superseded_by: None,
            embedding: None,
            metadata: args.metadata,
        };
        memory.validate()?;
        Ok(memory)
    }

    /// Construct a memory with a caller-supplied id, sharing the
    /// validation and default behaviour of [`Self::try_new`]. Used
    /// by the MCP `memory.update` path in vault-app: the existing
    /// memory's id must be preserved across the update per ADR-028,
    /// but `try_new` always generates a fresh id. This is the
    /// canonical entry point — vault-app does NOT manually
    /// construct `Memory` from struct literals (which would bypass
    /// validation).
    ///
    /// `valid_from` / `valid_until` / `confidence` / etc. follow
    /// `try_new`'s defaults: `valid_from` defaults to `now` if
    /// omitted, system-managed fields (`created_at`, `last_accessed`,
    /// `access_count`, `superseded_by`, `embedding`) get the same
    /// defaults as `try_new`. Callers that need to preserve those
    /// (per ADR-028) read the existing memory first and then mutate
    /// the relevant fields on the returned struct.
    ///
    /// # Errors
    ///
    /// Same as [`Self::try_new`] — returns [`VaultError::InvalidInput`]
    /// if any of the invariants listed on [`Memory`] are violated.
    pub fn try_new_with_id(id: MemoryId, args: NewMemory) -> VaultResult<Self> {
        let mut memory = Self::try_new(args)?;
        memory.id = id;
        Ok(memory)
    }

    /// Re-check all invariants. Storage layers must call this before write
    /// (BRD §11.7.1: validate at every public API boundary).
    pub fn validate(&self) -> VaultResult<()> {
        if self.content.is_empty() {
            return Err(VaultError::InvalidInput(
                "memory content must not be empty".into(),
            ));
        }
        if self.content.len() > MAX_MEMORY_CONTENT_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "memory content exceeds {MAX_MEMORY_CONTENT_BYTES} bytes",
            )));
        }
        if self.content.contains('\0') {
            return Err(VaultError::InvalidInput(
                "memory content contains null bytes".into(),
            ));
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
        if let Some(emb) = self.embedding.as_ref() {
            if emb.is_empty() {
                return Err(VaultError::InvalidInput("embedding is empty".into()));
            }
            if emb.iter().any(|x| !x.is_finite()) {
                return Err(VaultError::InvalidInput(
                    "embedding contains non-finite values".into(),
                ));
            }
        }
        Ok(())
    }

    /// True if this memory has been superseded by a consolidator merge.
    /// Retrieval should skip these by default (BRD §5.6 phase 3).
    #[must_use]
    pub fn is_superseded(&self) -> bool {
        self.superseded_by.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample_args() -> NewMemory {
        NewMemory {
            content: "Shahbaz prefers terse PR descriptions".into(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new("work").unwrap(),
            source_agent: Some("claude".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        }
    }

    #[test]
    fn try_new_produces_valid_memory() {
        let memory = Memory::try_new(sample_args()).unwrap();
        memory.validate().unwrap();
        assert_eq!(memory.access_count, 0);
        assert!(!memory.is_superseded());
        assert_eq!(memory.created_at, memory.valid_from);
        assert_eq!(memory.created_at, memory.last_accessed);
    }

    #[test]
    fn try_new_with_id_uses_supplied_id_not_generated() {
        // Buggy impl that ignores the supplied id and calls
        // MemoryId::new() must fail this — pin the contract.
        let supplied = MemoryId::new();
        let memory = Memory::try_new_with_id(supplied, sample_args()).unwrap();
        assert_eq!(
            memory.id, supplied,
            "try_new_with_id MUST use the caller-supplied id, not generate fresh"
        );
    }

    #[test]
    fn try_new_with_id_applies_same_validation_as_try_new() {
        // Pick a representative validation case (empty content) and
        // assert try_new_with_id rejects identically to try_new.
        // Pinning the validation parity matters because vault-app's
        // update path is the only consumer; if validation diverges
        // here, MCP-driven updates could write content shapes that
        // direct write paths reject.
        let mut args = sample_args();
        args.content = String::new();
        let id = MemoryId::new();
        assert!(
            matches!(
                Memory::try_new_with_id(id, args.clone()),
                Err(VaultError::InvalidInput(_))
            ),
            "try_new_with_id must reject empty content like try_new does"
        );
        // Same shape: oversized content rejected.
        args.content = "x".repeat(MAX_MEMORY_CONTENT_BYTES + 1);
        assert!(matches!(
            Memory::try_new_with_id(id, args),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn empty_content_rejected() {
        let mut args = sample_args();
        args.content = String::new();
        assert!(matches!(
            Memory::try_new(args),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn oversized_content_rejected() {
        let mut args = sample_args();
        args.content = "x".repeat(MAX_MEMORY_CONTENT_BYTES + 1);
        assert!(matches!(
            Memory::try_new(args),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn null_byte_in_content_rejected() {
        let mut args = sample_args();
        args.content = "hello\0world".into();
        assert!(matches!(
            Memory::try_new(args),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn out_of_range_confidence_rejected() {
        let mut args = sample_args();
        args.confidence = 1.5;
        assert!(matches!(
            Memory::try_new(args.clone()),
            Err(VaultError::InvalidInput(_))
        ));
        args.confidence = -0.1;
        assert!(matches!(
            Memory::try_new(args.clone()),
            Err(VaultError::InvalidInput(_))
        ));
        args.confidence = f32::NAN;
        assert!(matches!(
            Memory::try_new(args),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn valid_until_before_valid_from_rejected() {
        let mut args = sample_args();
        let now = Utc::now();
        args.valid_from = Some(now);
        args.valid_until = Some(now - chrono::Duration::days(1));
        assert!(matches!(
            Memory::try_new(args),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn empty_embedding_rejected_by_validate() {
        let mut memory = Memory::try_new(sample_args()).unwrap();
        memory.embedding = Some(vec![]);
        assert!(matches!(
            memory.validate(),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn nan_in_embedding_rejected_by_validate() {
        let mut memory = Memory::try_new(sample_args()).unwrap();
        memory.embedding = Some(vec![0.1, f32::NAN, 0.3]);
        assert!(matches!(
            memory.validate(),
            Err(VaultError::InvalidInput(_))
        ));
    }

    #[test]
    fn memory_id_round_trips_via_display_and_fromstr() {
        let id = MemoryId::new();
        let s = id.to_string();
        let parsed: MemoryId = s.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn memory_id_uniqueness_across_many_constructions() {
        // UUID v7 includes a 48-bit timestamp + ~74 bits of randomness; collisions
        // within a single process should be effectively zero.
        let n = 10_000;
        let ids: std::collections::HashSet<_> = (0..n).map(|_| MemoryId::new()).collect();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn memory_serde_round_trip() {
        let memory = Memory::try_new(sample_args()).unwrap();
        let json = serde_json::to_string(&memory).unwrap();
        let back: Memory = serde_json::from_str(&json).unwrap();
        assert_eq!(memory, back);
        back.validate().unwrap();
    }

    #[test]
    fn memory_type_serializes_in_snake_case() {
        let json = serde_json::to_string(&MemoryType::Procedural).unwrap();
        assert_eq!(json, "\"procedural\"");
        let back: MemoryType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, MemoryType::Procedural);
    }

    proptest! {
        #[test]
        fn memory_id_serde_roundtrip(raw in any::<u128>()) {
            let id = MemoryId(Uuid::from_u128(raw));
            let json = serde_json::to_string(&id).unwrap();
            let back: MemoryId = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(id, back);
        }

        #[test]
        fn validate_accepts_in_range_confidence(c in 0.0f32..=1.0) {
            let mut args = sample_args();
            args.confidence = c;
            let memory = Memory::try_new(args).unwrap();
            prop_assert!(memory.validate().is_ok());
        }
    }
}
