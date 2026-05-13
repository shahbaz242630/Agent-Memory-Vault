//! `vault-consolidator` — the sleep cycle. Nightly job that merges duplicates,
//! decays old memories, resolves contradictions, and produces summaries.
//! The product gets better with use.
//!
//! See `Agent Build Specification.txt` §5.6 for the public API specification.
//! Real implementation lands across T0.2.2 → T0.2.6 (V0.2).
//!
//! ## Public surface (T0.2.2 — Phase 1 Cluster)
//!
//! - [`Cluster`] — N-ary cluster of memory-row references. Output type of
//!   the clustering primitive; consumed by T0.2.3's merge phase.
//! - [`find_candidate_clusters`] — the clustering primitive. Implements BRD
//!   §5.6 Phase 1 (top-5 NN above `merge_similarity_threshold` + transitive
//!   closure + singleton filter).
//!
//! Errors flow through `vault_core::VaultError` directly at T0.2.2 — no
//! `VaultConsolidatorError` enum yet. Per the concrete-vs-hypothetical
//! discipline, a crate-local error type lands when concrete category
//! distinctions emerge (T0.2.3 merge failures vs T0.2.4 decay failures vs
//! T0.2.5 checkpoint failures). At T0.2.2's clustering-only scope, every
//! failure is either an invalid input (threshold range) or a propagated
//! [`vault_core::VaultError`] from storage / embedding — both fit the
//! workspace catalogue cleanly. See ADR-045 §f.
//!
//! `Consolidator` struct (BRD §5.6 lines 894-913) is intentionally NOT
//! constructed at T0.2.2 — its `llm` + `embeddings` fields become
//! load-bearing at T0.2.3 (Phase 2 merge decisions) and that's where the
//! struct materialises. T0.2.2 ships the clustering primitive only.
//! See ADR-045 §f deferral note for the spec-compliance contract.

#![forbid(unsafe_code)]

pub mod clustering;

pub use clustering::{find_candidate_clusters, Cluster};
