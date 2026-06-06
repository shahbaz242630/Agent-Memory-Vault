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
//!   hydrates memories from the SQLite metadata store, and emits a
//!   structured `tracing::info!` event at `target: "vault_retrieval::query"`.
//!   Per T0.1.9 §6, audit-event accounting lives at the MCP layer
//!   (`AuditEventType::McpToolInvoke`); this layer is audit-neutral.
//!
//! See `T0.1.8_PLAN.md` for the original design rationale (Q1–Q10 +
//! Q-3.5) and `T0.1.9_PLAN.md` §6 for the audit-removal sub-phase that
//! moved retrieval audit accounting up to vault-mcp.

#![forbid(unsafe_code)]

pub mod report_io;
pub mod reranked_retriever;
pub mod retriever;
pub mod search_hint;
pub mod strategies;
pub mod structured_read_pipeline;

// The V0.2-era `read_pipeline` module (Qwen-7B single-call synthesis,
// ADR-048 + ADR-049) was retired by ADR-052 at Commit 6 (locked-next-arc,
// 2026-05-26). The deterministic [`structured_read_pipeline`] module
// replaces it with structured `relevant_facts` + `abstain` +
// `health.warnings` per ADR-054 Contract 2. No LLM in the read path.
pub use report_io::{FilesystemReportLoader, LoadedReport, LoadedReportFact, ReportLoader};
pub use reranked_retriever::{RerankedRetriever, RERANK_CANDIDATE_CAP, SEARCH_CANDIDATE_FANOUT};
pub use retriever::{
    RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP,
};
pub use search_hint::{search_hint, SearchHint, SEPARATION_RATIO, STRONG_RELEVANCE};
pub use strategies::{
    AbstainConfig, AbstainingRetriever, HybridConfig, HybridRetriever, KeywordIndex,
    KeywordRetriever, SemanticRetriever,
};
pub use structured_read_pipeline::{
    HealthInfo, HealthStatus, HealthWarning, ReadQuery, RelevantFact, StructuredReadPipeline,
    StructuredReadResponse, WarningCode, WarningSeverity,
};
