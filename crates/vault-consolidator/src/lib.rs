//! `vault-consolidator` — the sleep cycle. Nightly job that merges duplicates,
//! decays old memories, resolves contradictions, and produces summaries.
//! The product gets better with use.
//!
//! See `Agent Build Specification.txt` §5.6 for the public API specification.
//! Real implementation lands across T0.2.2 → T0.2.6 (V0.2).
//!
//! ## Public surface (current state — through T0.2.3 commit 1 file-refactor step)
//!
//! - [`Cluster`], [`find_candidate_clusters`] — Phase 1 (T0.2.2). N-ary cluster
//!   output from the geometry-only top-K NN search + transitive closure. See
//!   [`phases::cluster`].
//!
//! Consolidator orchestrator, ConflictReview, MergeOutcome, decide_merge land
//! later in T0.2.3 commit 1 after the file-layout refactor verifies clean
//! against the T0.2.2 acceptance test.
//!
//! Errors flow through `vault_core::VaultError` directly at T0.2.3 — no
//! `VaultConsolidatorError` enum yet. Per the concrete-vs-hypothetical
//! discipline, a crate-local error type lands when concrete category
//! distinctions emerge (T0.2.3 merge failures vs T0.2.4 decay failures vs
//! T0.2.5 checkpoint failures). See ADR-045 §f.
//!
//! ## File layout (per BRD §5.6 lines 984-993)
//!
//! T0.2.3 commit 1 corrected the T0.2.2-era flat layout (`src/clustering.rs`)
//! to the BRD-specified hierarchical layout:
//!
//! ```text
//! src/
//!   lib.rs                       — this file
//!   consolidator.rs              — main orchestrator (T0.2.3 +)
//!   phases/
//!     mod.rs                     — phase-module index
//!     cluster.rs                 — Phase 1 (T0.2.2; moved here at T0.2.3 commit 1)
//!     merge.rs                   — Phase 2 + 3 (T0.2.3)
//!     decay.rs                   — Phase 4 (T0.2.4; not yet created)
//!   checkpoint.rs                — Checkpoint capture: run-diff → CheckpointEntry (T0.2.5)
//!   scheduler.rs                 — Scheduling (T0.2.6; not yet created)
//! ```

#![forbid(unsafe_code)]

pub(crate) mod checkpoint;
pub mod consolidator;
pub mod phases;
pub mod report;
pub mod scheduler;
mod summary;
pub mod topics;

pub use consolidator::{
    ConflictReview, ConsolidationReport, Consolidator, ConsolidatorConfig, EnrichmentReport,
};
pub use phases::cluster::{find_candidate_clusters, Cluster};
pub use phases::merge::{apply_merge, decide_merge, AppliedMerge, MergeOutcome};
pub use report::{generate_report, write_report_atomic, Report, ReportFact};
pub use topics::{discover_topics, Topic, TopicMap};
