//! [`SemanticRetriever`] — V0.1's single retrieval strategy.
//!
//! The pipeline:
//!
//! ```text
//! retrieve(query):
//!     start = Instant::now()
//!     result = retrieve_inner(query):
//!         validate(query)                                  # Q2 / Q3 checks
//!         if query.authorized_boundaries.is_empty():
//!             return Ok(vec![])                            # Q1 short-circuit
//!         embedding = embedding_provider.embed(query.query_text)
//!         hits = vector_store.search(
//!             embedding,
//!             query.max_results,
//!             &query.authorized_boundaries,
//!         )
//!         memories = metadata_store.get_memories_batch(&ids_in_hit_order)
//!         scored = zip(memories, distances).map(|(m, dist)| (m, 1.0 - dist))   # Q7
//!         scored.retain(|(m,_)| include_archived || !m.is_superseded())        # Q5b
//!         scored.retain(|(_,s)| s >= threshold)                                # Q4
//!         scored.sort_by(score DESC, then memory.created_at DESC)              # Q9
//!         scored.truncate(max_results)
//!         result = scored.enumerate().map(|(rank, (m, score))| RetrievedMemory)
//!         Ok(result)
//!     latency_ms = start.elapsed()
//!     tracing::info!(target: "vault_retrieval::query", ...)  # operational only
//!     result                                                 # propagate inner Ok or Err
//! ```
//!
//! ## T0.1.9 audit-removal sub-phase (v1.3 plan §6)
//!
//! T0.1.8 v1.2 emitted an `AuditEventType::RetrievalQuery` audit event from
//! this pipeline. T0.1.9 §6 moves audit-event accounting up to the MCP
//! layer — `vault_mcp` is the single audit boundary; this pipeline emits
//! operational `tracing::info!` only. The contract:
//!
//! - **No audit append.** `append_retrieval_audit` was removed; the chain
//!   gets its `mcp.tool_invoke` entry from the caller instead.
//! - **Operational `tracing::info!` retained.** Same diagnostic fields as
//!   the old audit shape (query_length, boundary_count, result_count,
//!   max_results, score_threshold, include_archived, latency_ms, error?)
//!   emitted as structured `tracing` fields at `target: "vault_retrieval::query"`.
//!
//! Two load-bearing watch-points (Phase 2 watch-point #3 — audit shape —
//! moved to vault-mcp; the two below stay local to this pipeline):
//!
//! 1. **Empty boundaries (Q1)** — short-circuit returns `Ok(vec![])` BEFORE any
//!    `embedder` or `vector_store` round-trip; the operational `tracing::info!`
//!    still emits with `boundary_count = 0` and `result_count = 0`.
//! 2. **Score-transform site (Q7)** — `score = 1.0 - distance` happens
//!    *exactly once*, at the boundary right after `vector_store.search`,
//!    before any sort / threshold / take. Tests 5 (cosine sanity) and
//!    8 (score range) catch transform-direction errors loudly.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use tracing::instrument;
use vault_core::{Memory, MemoryId, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_storage::{MetadataStore, VectorStore};

use crate::retriever::{
    RetrievalQuery, RetrievedMemory, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP,
};

/// V0.1 single-strategy semantic retriever.
///
/// Holds three reference-counted handles:
///
/// - `metadata_store` — batched memory hydration via `get_memories_batch`
///   (Q10). T0.1.9 §6 moved audit-event accounting up to vault-mcp;
///   `MetadataStore` is no longer used for audit append from this layer.
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
    metadata_store: Arc<MetadataStore>,
    embedding_provider: Arc<dyn EmbeddingProvider>,
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

    /// Inner pipeline. Returns the retrieval result; the outer
    /// [`Self::retrieve`] wraps this with timing + audit append so the
    /// audit event captures both success and error paths uniformly.
    async fn retrieve_inner(&self, query: &RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // -- Q2: query text validation -------------------------------------
        let trimmed = query.query_text.trim();
        if trimmed.is_empty() {
            return Err(VaultError::InvalidInput(
                "query text empty after trim".into(),
            ));
        }
        if trimmed.bytes().any(|b| b.is_ascii_control()) {
            return Err(VaultError::InvalidInput(
                "query text contains ASCII control characters".into(),
            ));
        }
        if trimmed.len() > MAX_QUERY_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "query length {} > MAX_QUERY_BYTES ({MAX_QUERY_BYTES})",
                trimmed.len()
            )));
        }

        // -- Q3: max_results validation ------------------------------------
        if query.max_results == 0 || query.max_results > MAX_RESULTS_CAP {
            return Err(VaultError::InvalidInput(format!(
                "max_results {} not in 1..={MAX_RESULTS_CAP}",
                query.max_results
            )));
        }

        // -- Q1: empty-boundary short-circuit ------------------------------
        // Watch-point #1: returns BEFORE any embedder / vector_store
        // round-trip. The outer `retrieve()` still appends an audit event
        // with `boundary_count = 0`, `result_count = 0` — empty-auth is a
        // legitimate audit data point, not a bypass.
        if query.authorized_boundaries.is_empty() {
            return Ok(Vec::new());
        }

        // -- Embed -----------------------------------------------------------
        let embedding = self.embedding_provider.embed(trimmed).await?;

        // -- k-NN search with mandatory boundary filter ---------------------
        let hits = self
            .vector_store
            .search(&embedding, query.max_results, &query.authorized_boundaries)
            .await?;
        if hits.is_empty() {
            return Ok(Vec::new());
        }

        // -- Hydrate memories via batched fetch (Q10) -----------------------
        let ids: Vec<MemoryId> = hits.iter().map(|(id, _)| *id).collect();
        let memories = self.metadata_store.get_memories_batch(&ids).await?;

        // -- Watch-point #2: score transform at THIS boundary ---------------
        // `score = 1.0 - distance` (cosine similarity in [-1, 1], higher
        // = better). LanceDB's `DistanceType::Cosine` returns cosine
        // *distance* (smaller = closer). The transform happens exactly
        // once, here, before any sort / threshold / take. See
        // `vault_storage::vector_store::VectorStore::search` doc-comment
        // for the source-of-truth distance contract.
        //
        // The HashMap re-alignment makes the pipeline orphan-safe: if
        // `get_memories_batch` dropped any IDs (LanceDB row exists but
        // SQLite row doesn't — the "deleted but not purged" partial
        // state from T0.1.6's cascade tests), we filter those out here
        // rather than zip-mis-aligning.
        let distances: HashMap<MemoryId, f32> = hits.into_iter().collect();
        let mut scored: Vec<(Memory, f32)> = memories
            .into_iter()
            .filter_map(|m| distances.get(&m.id).map(|&d| (m, 1.0_f32 - d)))
            .collect();

        // -- Q5b: include_archived filter ----------------------------------
        // V0.1 has no superseded memories in production yet, but the
        // filter is wired from day one so the V0.2 contract holds.
        if !query.options.include_archived {
            scored.retain(|(m, _)| !m.is_superseded());
        }

        // -- Q4: score_threshold filter ------------------------------------
        if let Some(threshold) = query.options.score_threshold {
            scored.retain(|(_, s)| *s >= threshold);
        }

        // -- Q9: sort score-DESC, then created_at-DESC for ties ------------
        scored.sort_by(|(am, asc), (bm, bsc)| {
            // `partial_cmp` returns `None` for NaN; defensive
            // `unwrap_or(Equal)` keeps sort stable even though
            // `EmbeddingProvider` already validates finite outputs.
            bsc.partial_cmp(asc)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| bm.created_at.cmp(&am.created_at))
        });

        // -- Q3 / max_results -----------------------------------------------
        scored.truncate(query.max_results);

        // -- Q6: build RetrievedMemory list with rank-aware explanation ----
        let total = scored.len();
        let result: Vec<RetrievedMemory> = scored
            .into_iter()
            .enumerate()
            .map(|(idx, (memory, score))| RetrievedMemory {
                memory,
                score,
                explanation: format!("semantic: cosine={score:.4} (rank {}/{})", idx + 1, total),
            })
            .collect();
        Ok(result)
    }
}

#[async_trait]
impl Retriever for SemanticRetriever {
    #[instrument(
        level = "debug",
        skip_all,
        fields(
            query_len = query.query_text.len(),
            boundary_count = query.authorized_boundaries.len(),
            max_results = query.max_results,
        )
    )]
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        let start = Instant::now();
        let result = self.retrieve_inner(&query).await;
        let latency_ms = start.elapsed().as_millis() as u64;

        // T0.1.9 §6: operational logging only — audit accounting moved
        // to the MCP layer (`mcp.tool_invoke` event). Same diagnostic
        // shape as the old v1.2 audit `details_json` so the operator
        // log retains its forensic value, just at info-log level not
        // audit chain level.
        let trimmed_len = query.query_text.trim().len();
        let (result_count, error_str) = match &result {
            Ok(v) => (v.len(), None::<String>),
            Err(e) => (0_usize, Some(e.to_string())),
        };
        tracing::info!(
            target: "vault_retrieval::query",
            query_length = trimmed_len,
            boundary_count = query.authorized_boundaries.len(),
            result_count = result_count,
            max_results = query.max_results,
            include_archived = query.options.include_archived,
            score_threshold = ?query.options.score_threshold,
            latency_ms = latency_ms,
            error = ?error_str,
            "retrieval pipeline completed"
        );

        result
    }
}

// =============================================================================
// Unit tests — T0.1.9 v1.3 (audit-removal sub-phase: 3 audit-shape tests
// rewritten to assert `tracing::info!` emission via `tracing-test`)
// =============================================================================
//
// Coverage matches T0.1.8_PLAN.md §5 v1.2 (13 unit tests), with
// T0.1.9 §6 removing the audit-append from `Retriever::retrieve`. The
// 3 audit-event tests (formerly `audit_event_appended_*` /
// `audit_event_latency_ms_is_recorded`) become tracing-event tests:
// `tracing_event_emitted_*` / `tracing_event_latency_ms_is_recorded`.
// Tests #1 and #2 (which previously asserted audit fallout) now assert
// the equivalent `tracing::info!` emission.
//
// `#[tracing_test::traced_test]` installs a thread-local subscriber
// that captures `tracing` events emitted during the test. The
// `no-env-filter` feature on the workspace `tracing-test` dep ensures
// `vault_retrieval` events reach the capture buffer (the default
// macro-installed env filter is `{calling_crate}=trace`, which is fine
// here since the events fire from this crate).
#[cfg(test)]
mod tests {
    use super::*;
    use crate::retriever::{RetrievalOptions, RetrievalQuery};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;
    use vault_core::{Boundary, Memory, MemoryType, NewMemory, VaultError};
    use vault_embedding::{EmbeddingProvider, EMBEDDING_DIM};
    use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

    // --- test infrastructure --------------------------------------------

    /// A simple deterministic stub embedder for tests. Returns a fixed
    /// L2-normalised vector — `[1, 0, 0, ..., 0]` — regardless of
    /// input, except when the input contains the marker substring
    /// `"FAIL"`, in which case it returns `VaultError::Embedding`.
    /// Tracks call count so empty-boundary tests can prove the
    /// short-circuit didn't reach the embedder.
    struct StubEmbedder {
        calls: AtomicU64,
    }

    impl StubEmbedder {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicU64::new(0),
            })
        }

        fn call_count(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EmbeddingProvider for StubEmbedder {
        async fn embed(&self, text: &str) -> VaultResult<Vec<f32>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if text.contains("FAIL") {
                return Err(VaultError::Embedding("stub: induced failure".into()));
            }
            let mut v = vec![0.0_f32; EMBEDDING_DIM];
            v[0] = 1.0;
            Ok(v)
        }
    }

    /// Bundle exposing each component so individual tests can directly
    /// drive the metadata + vector stores when needed.
    struct Bundle {
        retriever: SemanticRetriever,
        metadata: Arc<MetadataStore>,
        vectors: Arc<dyn VectorStore>,
        embedder: Arc<StubEmbedder>,
        _dir: tempfile::TempDir,
    }

    async fn make_bundle() -> Bundle {
        let dir = tempdir().expect("tempdir");
        let key = SqlCipherKey::new("test-only-passphrase");
        let metadata = MetadataStore::open(dir.path().join("metadata.db"), key)
            .await
            .expect("open metadata");
        let vectors = LanceVectorStore::open(&dir.path().join("vectors"), EMBEDDING_DIM)
            .await
            .expect("open vectors");
        let metadata = Arc::new(metadata);
        let vectors: Arc<dyn VectorStore> = Arc::new(vectors);
        let embedder = StubEmbedder::new();
        let retriever = SemanticRetriever::new(
            metadata.clone(),
            embedder.clone() as Arc<dyn EmbeddingProvider>,
            vectors.clone(),
        );
        Bundle {
            retriever,
            metadata,
            vectors,
            embedder,
            _dir: dir,
        }
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

    fn make_memory(content: &str, b: &Boundary) -> Memory {
        Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: b.clone(),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory")
    }

    /// Insert a memory with a stub embedding offset by `drift` so different
    /// memories get distinguishable cosine distances.
    async fn insert(b: &Bundle, memory: &Memory, drift: usize) {
        b.metadata.create_memory(memory).await.expect("create");
        let mut emb = vec![0.0_f32; EMBEDDING_DIM];
        emb[0] = 1.0;
        if drift > 0 && drift + 1 < EMBEDDING_DIM {
            emb[drift + 1] = (drift as f32) * 1e-3;
        }
        let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut emb {
            *x /= norm;
        }
        b.vectors
            .upsert(&memory.id, &emb, &memory.boundary)
            .await
            .expect("vector upsert");
    }

    // --- 1. embedder error propagation ----------------------------------

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn embed_query_path_propagates_embedder_error() {
        let b = make_bundle().await;
        let res = b
            .retriever
            .retrieve(query("FAIL me please", vec![boundary("work")], 10))
            .await;
        assert!(matches!(res, Err(VaultError::Embedding(_))));
        // T0.1.9 §6: operational tracing::info! emission carries the
        // diagnostic shape the audit chain used to. Error path
        // populates the `error` field with the propagated VaultError.
        assert!(
            tracing_test::internal::logs_with_scope_contain(
                "vault_retrieval",
                "retrieval pipeline completed",
            ),
            "expected info-log emission from retrieve() error path"
        );
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_retrieval", "stub: induced"),
            "error field must contain the underlying embedder error message"
        );
    }

    // --- 2. empty-boundaries short circuit ------------------------------

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn empty_authorized_boundaries_returns_empty_result_no_round_trip() {
        let b = make_bundle().await;
        let pre_calls = b.embedder.call_count();
        let res = b
            .retriever
            .retrieve(query("anything", vec![], 10))
            .await
            .expect("retrieve");
        assert!(res.is_empty(), "empty boundaries → empty result");
        assert_eq!(
            b.embedder.call_count(),
            pre_calls,
            "watch-point #1: embedder must not be called"
        );
        // T0.1.9 §6: tracing::info! still emits with boundary_count=0
        // and result_count=0. Empty-auth is a legitimate observability
        // data point, not a bypass — same posture as the old audit
        // contract, just at info-log level.
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_retrieval", "boundary_count=0",),
            "watch-point #1: tracing emission must record boundary_count=0"
        );
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_retrieval", "result_count=0",),
            "watch-point #1: tracing emission must record result_count=0"
        );
    }

    // --- 3. query text validation ---------------------------------------

    #[tokio::test]
    async fn query_text_validation_rejects_invalid_inputs() {
        let b = make_bundle().await;
        let auth = vec![boundary("work")];
        // Empty.
        let r = b.retriever.retrieve(query("", auth.clone(), 10)).await;
        assert!(matches!(r, Err(VaultError::InvalidInput(_))));
        // Whitespace only.
        let r = b
            .retriever
            .retrieve(query("   \t\n   ", auth.clone(), 10))
            .await;
        assert!(matches!(r, Err(VaultError::InvalidInput(_))));
        // Control chars (U+0007 BEL).
        let r = b
            .retriever
            .retrieve(query("hello\x07world", auth.clone(), 10))
            .await;
        assert!(matches!(r, Err(VaultError::InvalidInput(_))));
        // Oversized post-trim.
        let big = "x".repeat(crate::retriever::MAX_QUERY_BYTES + 1);
        let r = b.retriever.retrieve(query(&big, auth, 10)).await;
        assert!(matches!(r, Err(VaultError::InvalidInput(_))));
    }

    // --- 4. result-limit validation -------------------------------------

    #[tokio::test]
    async fn result_limit_validation_rejects_out_of_range() {
        let b = make_bundle().await;
        let auth = vec![boundary("work")];
        let r = b.retriever.retrieve(query("hello", auth.clone(), 0)).await;
        assert!(matches!(r, Err(VaultError::InvalidInput(_))));
        let r = b
            .retriever
            .retrieve(query("hello", auth, crate::retriever::MAX_RESULTS_CAP + 1))
            .await;
        assert!(matches!(r, Err(VaultError::InvalidInput(_))));
    }

    // --- 5. score is cosine similarity, not distance --------------------

    #[tokio::test]
    async fn score_is_cosine_similarity_not_distance() {
        // Watch-point #2 verifier. An identical-vector hit yields
        // distance ≈ 0, so score = 1.0 - 0 ≈ 1.0. If the transform
        // were inverted (score = distance, no transform), we'd see
        // score ≈ 0 instead of ≈ 1.
        let bundle = make_bundle().await;
        let work = boundary("work");
        let m = make_memory("identical", &work);
        insert(&bundle, &m, 0).await;
        let res = bundle
            .retriever
            .retrieve(query("identical", vec![work], 1))
            .await
            .expect("retrieve");
        assert_eq!(res.len(), 1);
        // Identical unit vectors → cosine distance 0 → similarity 1.
        let s = res[0].score;
        assert!(
            s > 0.99,
            "watch-point #2: score for identical embedding must be ~1.0 (cosine similarity), got {s}"
        );
    }

    // --- 6. result ordering (Q9: score DESC then created_at DESC) -------

    #[tokio::test]
    async fn result_order_is_score_descending_then_created_at_descending() {
        let bundle = make_bundle().await;
        let work = boundary("work");
        // Insert with strictly distinct drifts → distinct distances → distinct scores.
        for i in 0..5 {
            let m = make_memory(&format!("memory {i}"), &work);
            insert(&bundle, &m, i + 1).await;
        }
        let res = bundle
            .retriever
            .retrieve(query("memory", vec![work], 5))
            .await
            .expect("retrieve");
        for w in res.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "score must be non-increasing, got {} then {}",
                w[0].score,
                w[1].score
            );
            if (w[0].score - w[1].score).abs() < f32::EPSILON {
                assert!(
                    w[0].memory.created_at >= w[1].memory.created_at,
                    "tied scores must tiebreak created_at DESC"
                );
            }
        }
    }

    // --- 7. score threshold ----------------------------------------------

    #[tokio::test]
    async fn score_threshold_drops_below_threshold_results() {
        let bundle = make_bundle().await;
        let work = boundary("work");
        // Insert with a wide drift range so some scores drop below 0.5.
        for i in 0..10 {
            let m = make_memory(&format!("memory {i}"), &work);
            insert(&bundle, &m, (i + 1) * 5).await;
        }
        let mut q = query("memory", vec![work], 10);
        q.options.score_threshold = Some(0.999);
        let res = bundle.retriever.retrieve(q).await.expect("retrieve");
        for r in &res {
            assert!(
                r.score >= 0.999,
                "threshold must drop scores below 0.999, got {}",
                r.score
            );
        }
    }

    // --- 8. include_archived filters superseded -------------------------

    #[tokio::test]
    async fn include_archived_false_filters_superseded_when_present() {
        let bundle = make_bundle().await;
        let work = boundary("work");
        // Two real memories, one of which we'll mark superseded after creation.
        let parent = make_memory("parent", &work);
        let child = make_memory("child to be superseded", &work);
        insert(&bundle, &parent, 1).await;
        insert(&bundle, &child, 2).await;
        // Mark child superseded by parent.
        let mut superseded_child = child.clone();
        superseded_child.superseded_by = Some(parent.id);
        bundle
            .metadata
            .update_memory(&superseded_child)
            .await
            .expect("update");
        // Default options: include_archived = false.
        let res = bundle
            .retriever
            .retrieve(query("memory", vec![work], 10))
            .await
            .expect("retrieve");
        assert!(
            !res.iter().any(|r| r.memory.id == child.id),
            "superseded child must be filtered out when include_archived = false"
        );
        assert!(
            res.iter().any(|r| r.memory.id == parent.id),
            "parent (not superseded) must remain"
        );
    }

    // --- 9. include_archived no-op when nothing is superseded -----------

    #[tokio::test]
    async fn include_archived_default_is_no_op_when_no_superseded_memories_exist() {
        let bundle = make_bundle().await;
        let work = boundary("work");
        for i in 0..3 {
            let m = make_memory(&format!("memory {i}"), &work);
            insert(&bundle, &m, i + 1).await;
        }
        // Two retrievals: default options vs explicit include_archived=true.
        let default_res = bundle
            .retriever
            .retrieve(query("memory", vec![work.clone()], 10))
            .await
            .expect("retrieve default");
        let mut q_archived = query("memory", vec![work], 10);
        q_archived.options.include_archived = true;
        let archived_res = bundle
            .retriever
            .retrieve(q_archived)
            .await
            .expect("retrieve include_archived=true");
        assert_eq!(
            default_res.len(),
            archived_res.len(),
            "with no superseded memories, include_archived is a no-op"
        );
    }

    // --- 10. explanation string format ----------------------------------

    #[tokio::test]
    async fn explanation_string_format_is_stable() {
        let bundle = make_bundle().await;
        let work = boundary("work");
        let m = make_memory("only one", &work);
        insert(&bundle, &m, 1).await;
        let res = bundle
            .retriever
            .retrieve(query("only one", vec![work], 5))
            .await
            .expect("retrieve");
        assert_eq!(res.len(), 1);
        // Q6 format: "semantic: cosine={score:.4} (rank {rank}/{total})"
        let exp = &res[0].explanation;
        assert!(exp.starts_with("semantic: cosine="), "got: {exp}");
        assert!(
            exp.ends_with(" (rank 1/1)"),
            "rank suffix must be 'rank 1/1' for the only result; got: {exp}"
        );
    }

    // --- 11. tracing event on success path ------------------------------

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn tracing_event_emitted_on_success() {
        let bundle = make_bundle().await;
        let work = boundary("work");
        let m = make_memory("hello", &work);
        insert(&bundle, &m, 1).await;
        let _ = bundle
            .retriever
            .retrieve(query("hello", vec![work], 5))
            .await
            .expect("retrieve");
        // T0.1.9 §6: every diagnostic field from the old audit
        // `details_json` shape carries forward as a structured tracing
        // field. The default formatter renders `field=value`, so we
        // assert each expected field name appears in the captured log.
        for field_eq in [
            "query_length=",
            "boundary_count=",
            "result_count=",
            "max_results=",
            "include_archived=",
            "score_threshold=",
            "latency_ms=",
        ] {
            assert!(
                tracing_test::internal::logs_with_scope_contain("vault_retrieval", field_eq),
                "expected tracing field '{field_eq}' in retrieval pipeline log"
            );
        }
        // The "no query_hash" invariant from T0.1.8 v1.2 watch-point #3
        // carries forward — the v1.2 ADR-021 salt scheme reversal stays
        // applied (audit logs are local-only; no need for query hashing).
        assert!(
            !tracing_test::internal::logs_with_scope_contain("vault_retrieval", "query_hash"),
            "T0.1.9 §6 must NOT introduce query_hash in tracing emission"
        );
    }

    // --- 12. tracing event on failure path ------------------------------

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn tracing_event_emitted_on_failure_with_error_field() {
        let bundle = make_bundle().await;
        let res = bundle
            .retriever
            .retrieve(query("FAIL stub", vec![boundary("work")], 5))
            .await;
        assert!(matches!(res, Err(VaultError::Embedding(_))));
        // The `error = ?error_str` field renders as `error=Some("...")`
        // in the formatted log line (Debug format for the Option).
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_retrieval", "error=Some("),
            "error field must be Some(...) on failure path"
        );
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_retrieval", "stub: induced"),
            "error field must carry the underlying VaultError message"
        );
    }

    // --- 13. tracing event records latency_ms ---------------------------

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn tracing_event_latency_ms_is_recorded() {
        let bundle = make_bundle().await;
        let _ = bundle
            .retriever
            .retrieve(query("anything", vec![boundary("work")], 5))
            .await
            .expect("retrieve");
        // Field-existence check — we don't bound the value (CI flakiness).
        assert!(
            tracing_test::internal::logs_with_scope_contain("vault_retrieval", "latency_ms="),
            "latency_ms field must appear in retrieval pipeline log"
        );
    }
}
