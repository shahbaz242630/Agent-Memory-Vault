//! [`SemanticRetriever`] — V0.1's single retrieval strategy.
//!
//! The pipeline (Phase 2 fills the body):
//!
//! ```text
//! retrieve(query):
//!     validate(query)                                  # Q2 / Q3 / Q5b checks
//!     if query.authorized_boundaries.is_empty():
//!         append_audit(boundary_count=0, result_count=0)
//!         return Ok(vec![])
//!     embedding = embedding_provider.embed(query.query_text)
//!     hits = vector_store.search(
//!         embedding,
//!         limit_with_overhead,                         # ≥ max_results to allow filtering
//!         &query.authorized_boundaries,
//!     )
//!     ids_in_hit_order = hits.map(|(id, _dist)| id)
//!     memories = metadata_store.get_memories_batch(&ids_in_hit_order)  # Q10
//!     scored = zip(memories, hits).map(|(m, dist)| (m, 1.0 - dist))     # Q7
//!     scored.sort_by(score DESC, then memory.created_at DESC)           # Q9
//!     scored.retain(|s| s.score >= threshold && include_archived_filter)
//!     scored.truncate(max_results)
//!     result = scored.map(|(m, score, rank, total)| RetrievedMemory { ... })
//!     append_audit(boundary_count, result_count, latency_ms, ...)        # Q-3.5 v1.2
//!     Ok(result)
//! ```
//!
//! All five steps land in Phase 2. Phase 1 just defines the struct,
//! the constructor, and the trait-impl skeleton with `unimplemented!()`.

use std::sync::Arc;

use async_trait::async_trait;
use vault_core::VaultResult;
use vault_embedding::EmbeddingProvider;
use vault_storage::{MetadataStore, VectorStore};

use crate::retriever::{RetrievalQuery, RetrievedMemory, Retriever};

/// V0.1 single-strategy semantic retriever.
///
/// Holds three reference-counted handles:
///
/// - `metadata_store` — used for batched memory hydration (Q10's
///   `get_memories_batch`, lands Phase 2) AND for appending the
///   `AuditEventType::RetrievalQuery` event on every retrieval.
/// - `embedding_provider` — embeds the user's query text to a 384-dim
///   L2-normalised vector.
/// - `vector_store` — runs cosine k-NN with the boundary filter applied
///   at the SQL layer (LanceDB `only_if`).
///
/// The struct is intentionally `Clone`-via-`Arc`-fields-only — copying
/// it is cheap, and downstream wiring at vault-app (T0.1.10) can hand
/// the same retriever to multiple async tasks without locks.
///
/// Per ADR-007 this type does **not** implement `Debug`: it holds live
/// storage handles that we don't want logged through accidental
/// `{:?}` interpolation.
#[derive(Clone)]
pub struct SemanticRetriever {
    // Phase 1 stores these but does not read them (`retrieve()` body is
    // `unimplemented!()`). Phase 2's pipeline reads all three; the
    // `dead_code` allow lifts at Phase 2 commit when the body lands.
    #[allow(dead_code)]
    metadata_store: Arc<MetadataStore>,
    #[allow(dead_code)]
    embedding_provider: Arc<dyn EmbeddingProvider>,
    #[allow(dead_code)]
    vector_store: Arc<dyn VectorStore>,
}

impl SemanticRetriever {
    /// Construct a new retriever from the three downstream handles.
    ///
    /// All three are `Arc`-shared by convention — the retriever does not
    /// own them exclusively, and vault-app holds the canonical handles.
    pub fn new(
        metadata_store: Arc<MetadataStore>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
        vector_store: Arc<dyn VectorStore>,
    ) -> Self {
        Self {
            metadata_store,
            embedding_provider,
            vector_store,
        }
    }
}

#[async_trait]
impl Retriever for SemanticRetriever {
    async fn retrieve(&self, _query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // Phase 2 fills the body. The skeleton lands now so:
        //   - the trait `impl` compiles,
        //   - all Phase 1 scaffolded tests reach `retrieve()` and panic
        //     loudly at this `unimplemented!()` instead of returning
        //     `Ok(vec![])` (which would silently green a broken Phase 2).
        // Phase 2 turns each panicking test green by implementing the
        // pipeline described in the module docs.
        unimplemented!("T0.1.8 Phase 2: SemanticRetriever::retrieve pipeline")
    }
}

// =============================================================================
// Unit tests (Phase 1 scaffold — Phase 2 turns them green)
// =============================================================================
//
// Coverage matches T0.1.8_PLAN.md §5 v1.2 (13 unit tests). Tests that
// depend on Phase 2 API (`AuditEventType::RetrievalQuery`,
// `MetadataStore::get_memories_batch`, full `retrieve()` body) call
// `retrieve()` and panic at `unimplemented!()` in Phase 1. Tests whose
// Phase 2 dependency is *only* a new audit-enum variant or a new
// MetadataStore method are `#[ignore]`-d with a clear reason — the
// alternative would be to write `todo!()` placeholders that don't
// compile-test the contract surface, which defeats the scaffold's
// purpose.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::retriever::{RetrievalOptions, RetrievalQuery};
    use std::sync::Arc;
    use tempfile::tempdir;
    use vault_core::{Boundary, MemoryType, NewMemory, VaultError};
    use vault_embedding::{EmbeddingProvider, EMBEDDING_DIM};
    use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey};

    // --- test infrastructure --------------------------------------------

    /// A simple deterministic stub embedder for tests. Returns a fixed
    /// L2-normalised vector — `[1, 0, 0, ..., 0]` — regardless of
    /// input, except when the input contains the marker substring
    /// `"FAIL"`, in which case it returns `VaultError::Embedding`.
    /// Phase 1 needs only the type to exist; Phase 2 leans on the
    /// fail-marker to drive `embed_query_path_propagates_embedder_error`.
    struct StubEmbedder;

    #[async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, text: &str) -> VaultResult<Vec<f32>> {
            if text.contains("FAIL") {
                return Err(VaultError::Embedding("stub: induced failure".into()));
            }
            let mut v = vec![0.0_f32; EMBEDDING_DIM];
            v[0] = 1.0;
            Ok(v)
        }
    }

    /// Build a trio of (metadata, embedder, vector) all wired into a
    /// shared tempdir. SQLCipher key is a fixed test-only value.
    /// Returns the trio plus the tempdir handle so callers can pin its
    /// lifetime to the test scope.
    async fn make_retriever() -> (SemanticRetriever, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let key = SqlCipherKey::new("test-only-passphrase");
        let metadata = MetadataStore::open(dir.path().join("metadata.db"), key)
            .await
            .expect("open metadata");
        let vectors = LanceVectorStore::open(&dir.path().join("vectors"), EMBEDDING_DIM)
            .await
            .expect("open vectors");
        let retriever = SemanticRetriever::new(
            Arc::new(metadata),
            Arc::new(StubEmbedder),
            Arc::new(vectors),
        );
        (retriever, dir)
    }

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("valid boundary in test")
    }

    fn query(text: &str, boundaries: Vec<Boundary>, max_results: usize) -> RetrievalQuery {
        RetrievalQuery {
            query_text: text.into(),
            authorized_boundaries: boundaries,
            max_results,
            options: RetrievalOptions::default(),
        }
    }

    // Suppress dead-code lints for the helpers Phase 2 will start
    // exercising — the helpers are defined now so Phase 2 deltas stay
    // small and reviewable.
    #[allow(dead_code)]
    fn new_memory(text: &str, b: &Boundary) -> NewMemory {
        NewMemory {
            content: text.into(),
            memory_type: MemoryType::Semantic,
            boundary: b.clone(),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        }
    }

    // --- 1. embedder error propagation ----------------------------------

    /// Q2 / contract: when the embedder fails, `retrieve()` must surface
    /// the `VaultError::Embedding` rather than swallow it. Phase 2's
    /// pipeline routes embedder errors directly out and (per Q-3.5)
    /// also writes an `error`-tagged audit event before returning.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn embed_query_path_propagates_embedder_error() {
        let (retriever, _dir) = make_retriever().await;
        let q = query("FAIL me please", vec![boundary("work")], 10);
        let _ = retriever.retrieve(q).await;
    }

    // --- 2. empty-boundaries short circuit ------------------------------

    /// Q1: empty `authorized_boundaries` returns an empty result without
    /// round-tripping to the embedder or vector store. Phase 2 wires
    /// the short-circuit + audit event with `boundary_count = 0`.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn empty_authorized_boundaries_returns_empty_result_no_round_trip() {
        let (retriever, _dir) = make_retriever().await;
        let q = query("anything", vec![], 10);
        let _ = retriever.retrieve(q).await;
    }

    // --- 3. query text validation ---------------------------------------

    /// Q2: empty / whitespace-only / control-char / oversized queries
    /// are rejected with `VaultError::InvalidInput`. Phase 2 implements
    /// the validation; Phase 1 panics at `unimplemented!()`.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn query_text_validation_rejects_invalid_inputs() {
        let (retriever, _dir) = make_retriever().await;
        // Empty.
        let _ = retriever
            .retrieve(query("", vec![boundary("work")], 10))
            .await;
        // Whitespace only.
        let _ = retriever
            .retrieve(query("   \t\n   ", vec![boundary("work")], 10))
            .await;
        // Control chars.
        let _ = retriever
            .retrieve(query("hello\x07world", vec![boundary("work")], 10))
            .await;
        // Oversized (> MAX_QUERY_BYTES after trim).
        let big = "x".repeat(crate::retriever::MAX_QUERY_BYTES + 1);
        let _ = retriever
            .retrieve(query(&big, vec![boundary("work")], 10))
            .await;
    }

    // --- 4. result-limit validation -------------------------------------

    /// Q3: `max_results == 0` and `max_results > MAX_RESULTS_CAP` are
    /// rejected. Phase 2 wires the check.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn result_limit_validation_rejects_out_of_range() {
        let (retriever, _dir) = make_retriever().await;
        let _ = retriever
            .retrieve(query("hello", vec![boundary("work")], 0))
            .await;
        let _ = retriever
            .retrieve(query(
                "hello",
                vec![boundary("work")],
                crate::retriever::MAX_RESULTS_CAP + 1,
            ))
            .await;
    }

    // --- 5. score is cosine similarity, not distance --------------------

    /// Q7: `RetrievedMemory.score = 1.0 - lance_distance`. Phase 2 wires
    /// the transform; Phase 1 panics. Test 8 (range proof, in
    /// integration tests) is the perimeter; this test asserts the
    /// transform direction with a known-near-duplicate fixture.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn score_is_cosine_similarity_not_distance() {
        let (retriever, _dir) = make_retriever().await;
        let _ = retriever
            .retrieve(query("hello", vec![boundary("work")], 10))
            .await;
    }

    // --- 6. result ordering ----------------------------------------------

    /// Q9: results sort by `score DESC` then `created_at DESC` for
    /// equal scores. Phase 2 wires the sort.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn result_order_is_score_descending_then_created_at_descending() {
        let (retriever, _dir) = make_retriever().await;
        let _ = retriever
            .retrieve(query("hello", vec![boundary("work")], 10))
            .await;
    }

    // --- 7. score threshold ----------------------------------------------

    /// Q4: when `options.score_threshold = Some(t)`, results with
    /// `score < t` are dropped. Phase 2 wires the filter.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn score_threshold_drops_below_threshold_results() {
        let (retriever, _dir) = make_retriever().await;
        let mut q = query("hello", vec![boundary("work")], 10);
        q.options.score_threshold = Some(0.5);
        let _ = retriever.retrieve(q).await;
    }

    // --- 8. include_archived filters superseded -------------------------

    /// Q5b: `include_archived = false` (default) filters memories whose
    /// `superseded_by` is set. V0.2 contract pinned in V0.1 via a
    /// test-only `INSERT INTO memories ... superseded_by = '<uuid>'`
    /// fixture. Phase 2 wires the SQL filter into `get_memories_batch`.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn include_archived_false_filters_superseded_when_present() {
        let (retriever, _dir) = make_retriever().await;
        let _ = retriever
            .retrieve(query("hello", vec![boundary("work")], 10))
            .await;
    }

    // --- 9. include_archived no-op when nothing is superseded -----------

    /// Q5b: with no superseded memories in the vault, `include_archived`
    /// is a no-op. Phase 2 wires the filter; this test pins the
    /// no-op semantic.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn include_archived_default_is_no_op_when_no_superseded_memories_exist() {
        let (retriever, _dir) = make_retriever().await;
        let _ = retriever
            .retrieve(query("hello", vec![boundary("work")], 10))
            .await;
    }

    // --- 10. explanation string format ----------------------------------

    /// Q6: `RetrievedMemory.explanation` format is locked to
    /// `"semantic: cosine={score:.4} (rank {rank}/{total})"`. Phase 2
    /// wires the format string.
    #[tokio::test]
    #[should_panic(expected = "T0.1.8 Phase 2")]
    async fn explanation_string_format_is_stable() {
        let (retriever, _dir) = make_retriever().await;
        let _ = retriever
            .retrieve(query("hello", vec![boundary("work")], 10))
            .await;
    }

    // --- 11. audit event on success path --------------------------------

    /// Q-3.5 v1.2: every successful `retrieve()` appends one
    /// `AuditEventType::RetrievalQuery` event to the local audit chain
    /// with the v1.2 details_json shape (no query_hash).
    ///
    /// Phase 2 dependency: the new audit-enum variant. Phase 1 cannot
    /// even *write* this test body without that variant existing —
    /// using `#[ignore]` here keeps the test list visible without
    /// blocking compile.
    #[tokio::test]
    #[ignore = "T0.1.8 Phase 2: depends on AuditEventType::RetrievalQuery variant"]
    async fn audit_event_appended_on_success() {
        // Phase 2: assert append + parse details_json + check fields.
        unimplemented!("T0.1.8 Phase 2");
    }

    // --- 12. audit event on failure path --------------------------------

    /// Q-3.5 v1.2: failure paths still append an audit event, with
    /// `result = error` and the `details_json.error` field populated.
    #[tokio::test]
    #[ignore = "T0.1.8 Phase 2: depends on AuditEventType::RetrievalQuery variant"]
    async fn audit_event_appended_on_failure_with_error_field() {
        unimplemented!("T0.1.8 Phase 2");
    }

    // --- 13. audit event records latency_ms -----------------------------

    /// Q-3.5 v1.2: `latency_ms` is part of the v1.2 details_json shape.
    /// Field-existence check (we don't bound the value — too flaky on
    /// CI). Replaces the salt-related tests that were dropped in v1.2.
    #[tokio::test]
    #[ignore = "T0.1.8 Phase 2: depends on AuditEventType::RetrievalQuery variant"]
    async fn audit_event_latency_ms_is_recorded() {
        unimplemented!("T0.1.8 Phase 2");
    }
}
