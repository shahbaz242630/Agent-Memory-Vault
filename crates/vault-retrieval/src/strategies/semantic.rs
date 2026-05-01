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
//!     append_audit(query, result, latency_ms)              # Q-3.5 v1.2 — every call
//!     result                                               # propagate inner Ok or Err
//! ```
//!
//! Three load-bearing watch-points (per Shahbaz, lock at implementation):
//!
//! 1. **Empty boundaries (Q1)** — short-circuit returns `Ok(vec![])` BEFORE any
//!    `embedder` or `vector_store` round-trip; the audit append still runs
//!    with `boundary_count = 0` and `result_count = 0`. Empty-auth must not
//!    accidentally bypass the audit chain.
//! 2. **Score-transform site (Q7)** — `score = 1.0 - distance` happens
//!    *exactly once*, at the boundary right after `vector_store.search`,
//!    before any sort / threshold / take. Tests 5 (cosine sanity) and
//!    8 (score range) catch transform-direction errors loudly.
//! 3. **Audit shape v1.2 (Q-3.5)** — `details_json` keys are
//!    `boundary_count, error?, include_archived, latency_ms, max_results,
//!    query_length, result_count, score_threshold` (alphabetic = canonical
//!    sorted order via `BTreeMap`). **No `query_hash`** — the salt scheme
//!    was reversed in v1.2 because audit logs are local-only.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::instrument;
use vault_core::{Memory, MemoryId, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_storage::{
    ActorKind, AuditEventType, AuditResult, MetadataStore, PendingAuditEvent, VectorStore,
};

use crate::retriever::{
    RetrievalQuery, RetrievedMemory, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP,
};

/// V0.1 single-strategy semantic retriever.
///
/// Holds three reference-counted handles:
///
/// - `metadata_store` — batched memory hydration via `get_memories_batch`
///   (Q10) AND audit-event append at retrieval-pipeline level (Q-3.5).
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

    /// Build the v1.2-shape audit event for a completed retrieval and
    /// append it to the local audit chain.
    ///
    /// Watch-point #3: `details_json` keys are exactly
    /// `boundary_count, error?, include_archived, latency_ms, max_results,
    /// query_length, result_count, score_threshold` — alphabetical order
    /// via `BTreeMap` gives the canonical sorted-key form per BRD §11.9.2.
    /// **No `query_hash`** (the salt scheme was reversed in v1.2 because
    /// audit logs are local-only in V0.1 / V0.2).
    async fn append_retrieval_audit(
        &self,
        query: &RetrievalQuery,
        result: &VaultResult<Vec<RetrievedMemory>>,
        latency_ms: u64,
    ) -> VaultResult<()> {
        let trimmed_len = query.query_text.trim().len() as u32;
        let boundary_count = query.authorized_boundaries.len() as u32;
        let max_results = query.max_results as u32;
        let include_archived = query.options.include_archived;
        let score_threshold_value = match query.options.score_threshold {
            Some(t) => json!(t),
            None => Value::Null,
        };

        let (audit_result, result_count, error_field) = match result {
            Ok(v) => (AuditResult::Success, v.len() as u32, None),
            Err(e) => (AuditResult::Error, 0_u32, Some(e.to_string())),
        };

        // BTreeMap iterates by sorted key — that's the canonical form
        // serde_json serialises (we don't enable serde_json's
        // `preserve_order` feature, so the canonical-sorted invariant
        // holds without extra ceremony).
        let mut details: BTreeMap<&'static str, Value> = BTreeMap::new();
        details.insert("boundary_count", json!(boundary_count));
        if let Some(err) = error_field {
            details.insert("error", json!(err));
        }
        details.insert("include_archived", json!(include_archived));
        details.insert("latency_ms", json!(latency_ms));
        details.insert("max_results", json!(max_results));
        details.insert("query_length", json!(trimmed_len));
        details.insert("result_count", json!(result_count));
        details.insert("score_threshold", score_threshold_value);

        let details_json = serde_json::to_string(&details).map_err(|e| {
            VaultError::Serde(format!("retrieval audit details_json serialise: {e}"))
        })?;

        let pending = PendingAuditEvent {
            event_type: AuditEventType::RetrievalQuery,
            resource_type: None,
            resource_id: None,
            // No single boundary — retrieval spans `boundary_count`
            // boundaries, recorded inside `details_json`. The audit
            // table's nullable `boundary` column matches this.
            boundary: None,
            actor_kind: ActorKind::System,
            actor_name: None,
            user_id: None,
            device_id: None,
            result: audit_result,
            details_json,
        };
        self.metadata_store.append_audit_event(pending).await?;
        Ok(())
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
        self.append_retrieval_audit(&query, &result, latency_ms)
            .await?;
        result
    }
}

// =============================================================================
// Unit tests — Phase 2 (real bodies; should_panic markers removed)
// =============================================================================
//
// Coverage matches T0.1.8_PLAN.md §5 v1.2 (13 unit tests). The 10
// formerly-`should_panic` tests now run real assertions; the 3
// audit-event tests un-ignore now that `AuditEventType::RetrievalQuery`
// exists.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::retriever::{RetrievalOptions, RetrievalQuery};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use tempfile::tempdir;
    use vault_core::{Boundary, Memory, MemoryType, NewMemory, VaultError};
    use vault_embedding::{EmbeddingProvider, EMBEDDING_DIM};
    use vault_storage::{
        AuditEventType, AuditResult, LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore,
    };

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
    async fn embed_query_path_propagates_embedder_error() {
        let b = make_bundle().await;
        let res = b
            .retriever
            .retrieve(query("FAIL me please", vec![boundary("work")], 10))
            .await;
        assert!(matches!(res, Err(VaultError::Embedding(_))));
        // The audit event still appended (error path).
        let events = b.metadata.list_audit_events(100).await.expect("audit list");
        let last = events.last().expect("at least one event");
        assert_eq!(last.event_type, AuditEventType::RetrievalQuery);
        assert_eq!(last.result, AuditResult::Error);
    }

    // --- 2. empty-boundaries short circuit ------------------------------

    #[tokio::test]
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
        // Audit event still appended with boundary_count = 0, result_count = 0.
        let events = b.metadata.list_audit_events(100).await.expect("audit");
        let last = events.last().expect("audit event");
        assert_eq!(last.event_type, AuditEventType::RetrievalQuery);
        assert!(last.details_json.contains(r#""boundary_count":0"#));
        assert!(last.details_json.contains(r#""result_count":0"#));
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

    // --- 11. audit event on success path --------------------------------

    #[tokio::test]
    async fn audit_event_appended_on_success() {
        let bundle = make_bundle().await;
        let work = boundary("work");
        let m = make_memory("hello", &work);
        insert(&bundle, &m, 1).await;
        let pre = bundle
            .metadata
            .list_audit_events(1000)
            .await
            .expect("audit pre")
            .len();
        let _ = bundle
            .retriever
            .retrieve(query("hello", vec![work], 5))
            .await
            .expect("retrieve");
        let events = bundle
            .metadata
            .list_audit_events(1000)
            .await
            .expect("audit post");
        assert_eq!(events.len(), pre + 1, "exactly one new audit event");
        let last = events.last().unwrap();
        assert_eq!(last.event_type, AuditEventType::RetrievalQuery);
        assert_eq!(last.result, AuditResult::Success);
        // Watch-point #3: v1.2 shape — fields present, no query_hash.
        let d = &last.details_json;
        for key in [
            "boundary_count",
            "include_archived",
            "latency_ms",
            "max_results",
            "query_length",
            "result_count",
            "score_threshold",
        ] {
            assert!(d.contains(&format!("\"{key}\"")), "missing {key} in {d}");
        }
        assert!(
            !d.contains("query_hash"),
            "watch-point #3: v1.2 must NOT include query_hash; got {d}"
        );
    }

    // --- 12. audit event on failure path --------------------------------

    #[tokio::test]
    async fn audit_event_appended_on_failure_with_error_field() {
        let bundle = make_bundle().await;
        let res = bundle
            .retriever
            .retrieve(query("FAIL stub", vec![boundary("work")], 5))
            .await;
        assert!(matches!(res, Err(VaultError::Embedding(_))));
        let events = bundle
            .metadata
            .list_audit_events(1000)
            .await
            .expect("audit");
        let last = events.last().unwrap();
        assert_eq!(last.event_type, AuditEventType::RetrievalQuery);
        assert_eq!(last.result, AuditResult::Error);
        assert!(
            last.details_json.contains(r#""error":"#),
            "error field must be populated on failure path: {}",
            last.details_json
        );
    }

    // --- 13. audit event records latency_ms -----------------------------

    #[tokio::test]
    async fn audit_event_latency_ms_is_recorded() {
        let bundle = make_bundle().await;
        let _ = bundle
            .retriever
            .retrieve(query("anything", vec![boundary("work")], 5))
            .await
            .expect("retrieve");
        let events = bundle
            .metadata
            .list_audit_events(1000)
            .await
            .expect("audit");
        let last = events.last().unwrap();
        // Field-existence check — we don't bound the value (CI flakiness).
        assert!(
            last.details_json.contains(r#""latency_ms":"#),
            "latency_ms field must be present in details_json: {}",
            last.details_json
        );
    }
}
