//! Retrieval strategy implementations.
//!
//! V0.1 ships [`SemanticRetriever`] only. T0.2.7 adds `GraphRetriever`,
//! `TemporalRetriever`, `KeywordRetriever`, `FrequencyRetriever`, plus
//! the orchestrating `MultiStrategyRetriever`. All implement the same
//! [`crate::Retriever`] trait — multi-strategy is a new struct, not a
//! breaking trait change.

pub mod semantic;

pub use semantic::SemanticRetriever;
