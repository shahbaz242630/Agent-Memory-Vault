//! [`Retriever`] trait and the associated query / result types.
//!
//! The trait is the V0.1 retrieval contract that vault-mcp (T0.1.9) and
//! vault-app (T0.1.10) consume. It is deliberately small: one method,
//! one input struct, one result type. The full multi-strategy world
//! (intent classification, parallel strategy execution, reranking)
//! lands at T0.2.7 *additively* — same trait, new implementer
//! (`MultiStrategyRetriever`).
//!
//! ## Locked contracts (do not silently change)
//!
//! - **Boundary contract (Q1):** `RetrievalQuery::authorized_boundaries`
//!   is mandatory and owned. Empty `Vec` → empty `Vec<RetrievedMemory>`,
//!   never an error. The retriever short-circuits before round-tripping
//!   to the embedder or vector store, but still appends a retrieval
//!   audit event with `boundary_count = 0`. Empty-auth is a legitimate
//!   audit data point per BRD §11.4.3.
//!
//! - **Score semantics (Q7):** [`RetrievedMemory::score`] is **cosine
//!   similarity** in `[-1, 1]`, higher = better. `LanceVectorStore::search`
//!   returns cosine *distance* in `[0, 2]` (smaller = closer). The
//!   semantic implementer applies `score = 1.0 - distance` once at the
//!   boundary; downstream consumers see the IR-conventional "higher is
//!   better" shape only.
//!
//! - **Result-order contract (Q9):** results are sorted by
//!   `score DESC, memory.created_at DESC`. The tiebreak is load-bearing
//!   and locked by `tests/retrieval_tests.rs::result_ordering_score_then_created_at_desc`.
//!
//! - **Result-limit contract (Q3):** `max_results == 0` and
//!   `max_results > MAX_RESULTS_CAP` (currently 200) are rejected as
//!   `VaultError::InvalidInput`. Caller-side defaults (MCP = 10, UI = 20)
//!   live in those crates, not here — keeping the retriever cap-only
//!   avoids hidden defaults.
//!
//! - **Query-text validation (Q2):** trim leading/trailing whitespace;
//!   reject empty / whitespace-only / control-char-bearing inputs;
//!   reject inputs > [`MAX_QUERY_BYTES`] bytes after trim.

use async_trait::async_trait;
use serde::Serialize;
use vault_core::{Boundary, Memory, VaultResult};

/// Hard upper bound on `max_results`. Rejecting values above this prevents
/// callers from issuing unbounded retrievals; MCP / UI default to 10 / 20
/// respectively at their own layer (BRD §5.5 implementation note).
///
/// Raised 100 → 200 at T0.2.7 v8 10K Q25 diagnostic close (2026-05-18).
/// The read-time pipeline's value-aware retrieval needs to widen well
/// beyond the LLM-facing top-K=20 to detect contradiction pairs that span
/// the cosine-rank boundary at scale. At SCALE=10K diverse corpus, the
/// Q25 GA-launch contradiction's Memory B sat at brute-force cosine rank
/// 172 — outside any top_n ≤ 200 widening would have missed it. 200 is
/// the minimum sufficient cap for the V0.2 6-query gauntlet at 10K.
pub const MAX_RESULTS_CAP: usize = 200;

/// Hard upper bound on the post-trim query length, in bytes. 2,048 is
/// generous for natural-language queries while preventing abuse of
/// `embed()` as a free-text pipe.
pub const MAX_QUERY_BYTES: usize = 2_048;

/// A user query against the retrieval pipeline.
///
/// **All fields mandatory.** This is by design: the boundary slice
/// MUST be supplied at every call site so "I forgot to filter" is a
/// compile-time error, not a runtime privilege escalation
/// (BRD §11.4.3 mandatory access control).
#[derive(Clone, Debug)]
pub struct RetrievalQuery {
    /// The raw user / agent query text. Validation rules apply at
    /// `Retriever::retrieve` entry — see module docs.
    pub query_text: String,

    /// The set of boundaries the caller is authorised to read from.
    /// Empty `Vec` is a valid input that returns zero results without
    /// round-tripping to the embedder or vector store. Never `Option`-al.
    pub authorized_boundaries: Vec<Boundary>,

    /// Maximum number of results to return. Mandatory. Range
    /// `1..=MAX_RESULTS_CAP`. Out-of-range values return
    /// `VaultError::InvalidInput`.
    pub max_results: usize,

    /// Tunable knobs that have a defensible default. New options land
    /// here additively without breaking call sites.
    pub options: RetrievalOptions,
}

/// Tunable retrieval options with safe defaults.
///
/// Both fields are forward-compat-safe per the v1.2 "concrete vs
/// hypothetical future use" test:
///
/// - `score_threshold`: T0.2.7 reranking will tune defaults; V0.1 keeps
///   `None` so all returned candidates surface to the caller.
/// - `include_archived`: archived = superseded-by-consolidator
///   (T0.2.x) OR user-archived (V0.2). The SQL filter
///   `AND superseded_by IS NULL` is wired from day one so the V0.2
///   contract is pinned in V0.1; the V0.1-only no-op semantics are
///   tested with a `superseded_by = '<uuid>'` fixture.
#[derive(Clone, Debug, Default)]
pub struct RetrievalOptions {
    /// If `Some(t)`, drop results with `score < t` after sorting and
    /// before `take(max_results)`. V0.1 default is `None` (no
    /// thresholding). Must be in `[-1.0, 1.0]` to be meaningful given
    /// the cosine-similarity score domain — out-of-range values are
    /// not validated here (the threshold is just compared with `<`),
    /// callers who pass garbage get garbage.
    pub score_threshold: Option<f32>,

    /// If `false` (default), exclude memories whose `superseded_by`
    /// is set. V0.1 has no superseded memories in production, but the
    /// filter is wired from day one so the V0.2 contract holds without
    /// a future API change.
    pub include_archived: bool,
}

/// One retrieved memory, with its similarity score and a stable
/// human-readable explanation.
///
/// **Wire format — load-bearing:** this struct is serialised into MCP
/// `memory_search` tool responses (T0.1.9 Phase 2 — see
/// `crates/vault-mcp/src/server.rs::tool_search`). Field renames are
/// **breaking changes** to the MCP API contract; downstream agents
/// (Claude Desktop, ChatGPT, Cursor, custom MCP clients) parse JSON
/// keyed on these field names. See ADR-024 for the JSON-RPC response
/// shape and the alpha-breaking-change policy (V0.x is alpha; wire
/// changes allowed through V0.2, frozen at V1.0).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RetrievedMemory {
    /// The hydrated memory row. Always belongs to a boundary in
    /// `RetrievalQuery::authorized_boundaries` — boundary leakage is
    /// the load-bearing invariant (`tests/trait_invariants.rs`).
    pub memory: Memory,

    /// Cosine similarity in `[-1, 1]`. Higher = more relevant. Equal
    /// scores tiebreak on `memory.created_at DESC` per Q9.
    pub score: f32,

    /// Stable, human-readable explanation. V0.1 format is:
    /// `"semantic: cosine={score:.4} (rank {rank}/{total})"` (Q6).
    /// T0.2.7 reranking extends this with strategy-fusion details
    /// without breaking the prefix.
    pub explanation: String,
}

/// The retrieval contract.
///
/// V0.1 has one implementer (`SemanticRetriever`); T0.2.7 will add
/// `MultiStrategyRetriever`. The trait stays stable so the integration
/// site (vault-app) doesn't change when the multi-strategy world lands.
///
/// # Invariants every implementer must uphold
///
/// 1. **No boundary leakage:** every returned `RetrievedMemory.memory.boundary`
///    is in `query.authorized_boundaries`. Verified for every implementer
///    via `tests/trait_invariants.rs::assert_boundary_leakage_invariant`.
/// 2. **Result count ≤ `query.max_results`** — the retriever may return
///    fewer (vault smaller than the cap, threshold filtered them out)
///    but never more.
/// 3. **Sort order:** results are sorted `score DESC`, then
///    `memory.created_at DESC` for equal scores.
/// 4. **Score domain:** every `score` is finite and in `[-1, 1]`.
/// 5. **Operational logging:** every call emits exactly one
///    structured `tracing::info!(target: "vault_retrieval::query", ...)`
///    event with the diagnostic shape (query_length, boundary_count,
///    result_count, max_results, score_threshold, include_archived,
///    latency_ms, optional error). Failure paths still emit, with
///    `error = Some("...")`. T0.1.9 §6 moved audit-event accounting up
///    to the MCP layer (`mcp.tool_invoke`); `Retriever` itself is
///    audit-neutral, so the chain is unaffected by `retrieve()`.
#[async_trait]
pub trait Retriever: Send + Sync {
    /// Run the retrieval pipeline. See trait-level invariants above.
    ///
    /// # Errors
    ///
    /// - [`VaultError::InvalidInput`](vault_core::VaultError::InvalidInput) —
    ///   query text fails validation (empty / whitespace-only /
    ///   control chars / oversized) or `max_results` is out of
    ///   `1..=MAX_RESULTS_CAP`.
    /// - [`VaultError::Embedding`](vault_core::VaultError::Embedding) —
    ///   underlying embedding provider failed.
    /// - [`VaultError::Storage`](vault_core::VaultError::Storage) —
    ///   underlying vector or metadata store failed.
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>>;
}
