//! Shared test fixtures + a deterministic stub embedder for integration
//! tests. Used by `retrieval_tests.rs` and `trait_invariants.rs`.
//!
//! `tests/common/mod.rs` is a Rust integration-test convention: each
//! test file declares `mod common;` and gets its own copy. The source
//! is shared, the compiled artefact is not (each integration test
//! file is its own crate).

#![allow(dead_code)] // Phase 1: many helpers are referenced only by
                     // ignored / panic-asserting tests; live by Phase 2/3.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use async_trait::async_trait;
use vault_core::{Boundary, Memory, MemoryType, NewMemory, VaultError, VaultResult};
use vault_embedding::{EmbeddingProvider, EMBEDDING_DIM};
use vault_retrieval::{RetrievalOptions, RetrievalQuery, SemanticRetriever};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

/// Test-only at-rest key (32 bytes, fixed pattern). Per-mod local
/// const per HANDOFF sub-task (d) §"Const placement" decision lock;
/// matches the convention in `vault-storage/tests/migration_v0_1_to_sealed.rs:96`.
pub const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

/// A deterministic stub embedder. Returns a fixed unit vector
/// `[1, 0, 0, ..., 0]` for every input, except inputs containing the
/// marker `"FAIL"` (returns `VaultError::Embedding`) or `"DRIFT_<n>"`
/// (returns a slightly perturbed unit vector — `[1, 0, ..., 0, n*1e-3]`
/// L2-renormalised — used by score-distinguishing tests).
///
/// The stub also tracks call count so tests can prove the empty-
/// boundaries short-circuit didn't reach the embedder.
pub struct StubEmbedder {
    pub calls: AtomicU64,
}

impl StubEmbedder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicU64::new(0),
        })
    }

    pub fn call_count(&self) -> u64 {
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
        // Small per-input drift so different texts get distinguishable
        // (but still tightly clustered) embeddings — Phase 2 / 3 use
        // this for ordering and score-range tests.
        if let Some(rest) = text.strip_prefix("DRIFT_") {
            if let Ok(n) = rest.parse::<usize>() {
                v[0] = 1.0;
                if n < EMBEDDING_DIM - 1 {
                    v[n + 1] = (n as f32) * 1e-3;
                }
            } else {
                v[0] = 1.0;
            }
        } else {
            v[0] = 1.0;
        }
        // L2-renormalise so the contract `EmbeddingProvider` documents
        // (unit norm within 1e-6) holds even after the drift offset.
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        Ok(v)
    }
}

/// Bundle of components a typical integration test needs.
pub struct TestRetriever {
    pub retriever: SemanticRetriever,
    pub metadata: Arc<MetadataStore>,
    pub vectors: Arc<dyn VectorStore>,
    pub embedder: Arc<StubEmbedder>,
    /// Held to keep the temp dir alive for the lifetime of the bundle.
    pub _dir: tempfile::TempDir,
}

/// Build a fresh `SemanticRetriever` over a tempdir-backed
/// `MetadataStore` + `LanceVectorStore` + `StubEmbedder`. Returns the
/// bundle so callers can keep handles to each component (e.g., to
/// directly insert memories without going through the cascading
/// orchestrator).
pub async fn make_test_retriever() -> TestRetriever {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = SqlCipherKey::new("test-only-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key)
        .await
        .expect("open metadata");
    let vectors = LanceVectorStore::open_with_at_rest_key(
        &dir.path().join("vectors"),
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
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
    TestRetriever {
        retriever,
        metadata,
        vectors,
        embedder,
        _dir: dir,
    }
}

/// Convenience constructor for a `Boundary` in tests.
pub fn boundary(name: &str) -> Boundary {
    Boundary::new(name).expect("valid boundary in test")
}

/// Convenience constructor for a `RetrievalQuery` with default
/// `RetrievalOptions`.
pub fn query(text: &str, boundaries: Vec<Boundary>, max_results: usize) -> RetrievalQuery {
    RetrievalQuery {
        query_text: text.into(),
        authorized_boundaries: boundaries,
        max_results,
        options: RetrievalOptions::default(),
    }
}

/// Build a validated [`Memory`] from a content string + boundary, with
/// sensible test defaults (Semantic type, confidence 0.9, no
/// `valid_until`).
pub fn make_memory(content: &str, b: &Boundary) -> Memory {
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
    .expect("valid memory in test")
}

/// Insert `memory` into the metadata store and a stub embedding into
/// the vector store. The embedding is a unit vector with a single
/// "drift index" `drift` so that different memories get distinguishable
/// distances under cosine. Used by Phase 2/3 ordering tests.
pub async fn insert_memory_with_drift(t: &TestRetriever, memory: &Memory, drift: usize) {
    t.metadata
        .create_memory(memory)
        .await
        .expect("create_memory");
    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    emb[0] = 1.0;
    if drift > 0 && drift + 1 < EMBEDDING_DIM {
        emb[drift + 1] = (drift as f32) * 1e-3;
    }
    let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
    for x in &mut emb {
        *x /= norm;
    }
    t.vectors
        .upsert(&memory.id, &emb, &memory.boundary)
        .await
        .expect("vector upsert");
}
