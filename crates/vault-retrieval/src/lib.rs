//! `vault-retrieval` — multi-strategy parallel retrieval (semantic + graph + temporal
//! + keyword + frequency) with intent classification and reranking.
//!
//! See `Agent Build Specification.txt` §5.5 for the public API specification.
//! V0.1 (T0.1.8) ships **semantic-only**; full multi-strategy lands in T0.2.7
//! purely additively (same trait, new implementer).
//!
//! ## Public surface (V0.1)
//!
//! - [`Retriever`] — the abstract retrieval trait. Single implementer in
//!   V0.1: [`SemanticRetriever`].
//! - [`RetrievalQuery`] / [`RetrievalOptions`] / [`RetrievedMemory`] — the
//!   query and result types.
//! - [`SemanticRetriever`] — V0.1 implementer. Embeds the query, runs k-NN
//!   over the LanceDB vector store filtered by `authorized_boundaries`,
//!   hydrates memories from the SQLite metadata store, and appends a
//!   `RetrievalQuery` audit event (Phase 2).
//!
//! See `T0.1.8_PLAN.md` for the full design rationale, the v1.0 → v1.2
//! plan history, and the Q1–Q10 + Q-3.5 design-question resolutions
//! that lock the contracts surfaced here.

#![forbid(unsafe_code)]

pub mod retriever;
pub mod strategies;

pub use retriever::{
    RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP,
};
pub use strategies::SemanticRetriever;
