//! Retrieval strategy implementations.
//!
//! V0.1 ships [`SemanticRetriever`] only. T0.2.7 Phase 1 (2026-05-20)
//! adds [`KeywordRetriever`] (BM25 lexical, backed by
//! [`KeywordIndex`]). ADR-SEC-002 Part 2 adds [`GraphRetriever`]
//! (knowledge-graph relational recall). Future phases add
//! `TemporalRetriever`, `FrequencyRetriever`, plus the orchestrating
//! `MultiStrategyRetriever`. All implement the same [`crate::Retriever`]
//! trait — additive composition, no breaking trait changes.

pub mod abstain;
pub mod graph;
pub mod hybrid;
pub mod keyword;
pub mod semantic;

pub use abstain::{AbstainConfig, AbstainingRetriever};
pub use graph::GraphRetriever;
pub use hybrid::{HybridConfig, HybridRetriever};
pub use keyword::{KeywordIndex, KeywordRetriever};
pub use semantic::SemanticRetriever;
