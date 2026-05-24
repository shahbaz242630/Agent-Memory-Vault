//! Smoke test for the **full Phase 4 production retriever stack** wired
//! through `ReadPipeline` with a `MockLlmProvider`. Proves end-to-end
//! that the stack compiles, instantiates, and executes — no Qwen GGUF
//! required.
//!
//! Stack composition (matches `vault_app::Application::new` wiring
//! verbatim):
//!
//! ```text
//! AbstainingRetriever            ← top-1 BM25 < threshold → empty result
//!     └── HybridRetriever        ← Reciprocal Rank Fusion of:
//!            ├── SemanticRetriever   ← stub embedder + LanceDB
//!            └── KeywordRetriever    ← Tantivy BM25 in-RAM
//!                     └── KeywordIndex (bulk-loaded from MetadataStore)
//! ReadPipeline(stack, MockLlmProvider with canned JSON)
//! ```
//!
//! Two scenarios:
//!
//! 1. **Strong-anchor query** — BM25 finds a real hit above threshold,
//!    abstain does NOT fire, ReadPipeline reaches the LLM, the mock's
//!    canned JSON parses into a `ReadResponse`. Asserts the mock was
//!    called exactly once.
//! 2. **Hard-negative query** — BM25 finds no hits, abstain fires,
//!    AbstainingRetriever returns empty, `ReadPipeline` short-circuits
//!    to `vault_has_no_relevant_content=true` WITHOUT calling the LLM.
//!    Asserts the mock was called zero times.

#![forbid(unsafe_code)]

mod common;

use std::sync::Arc;

use vault_core::{Boundary, MemoryId};
use vault_embedding::EMBEDDING_DIM;
use vault_llm::{LlmProvider, MockLlmProvider};
use vault_retrieval::{
    AbstainConfig, AbstainingRetriever, HybridRetriever, KeywordIndex, KeywordRetriever,
    ReadPipeline, ReadQuery, Retriever,
};
use vault_storage::{MetadataStore, VectorStore};

use common::{boundary, make_memory, make_test_retriever};

/// Bundle for the full-stack smoke test. Holds:
/// - `pipeline`: the production ReadPipeline wrapping the full retriever
///   stack and a `MockLlmProvider` that returns a canned JSON response.
/// - `mock_llm`: the same mock as inside the pipeline (kept as `Arc` for
///   direct `.call_count()` inspection after the test runs).
/// - underlying stores so tests can insert memories.
struct StackSetup {
    pipeline: ReadPipeline,
    mock_llm: Arc<MockLlmProvider>,
    metadata: Arc<MetadataStore>,
    vectors: Arc<dyn VectorStore>,
    keyword_index: Arc<KeywordIndex>,
    _dir: tempfile::TempDir,
}

/// Canonical canned response — valid JSON matching `READ_TIME_JSON_SCHEMA`.
const MOCK_CANNED_JSON: &str = r#"{
    "synthesis_markdown": "MOCK_SYNTHESIS",
    "contradictions_flagged": [],
    "vault_has_no_relevant_content": false
}"#;

/// Use a calibrated low abstain threshold so a tiny-corpus BM25 hit
/// can clear it. V0.2 production threshold is 1.0 (see
/// `src/strategies/abstain.rs`); tests with ≤5 docs may not even
/// reach 1.0 on a perfect match because IDF is tiny at micro-corpus
/// scale. See `abstain_tests.rs` module docs for the same reasoning.
const TEST_ABSTAIN_THRESHOLD: f32 = 0.05;

async fn setup_stack() -> StackSetup {
    let tr = make_test_retriever().await;
    let keyword_index = Arc::new(KeywordIndex::new().expect("kw index"));

    let semantic: Arc<dyn Retriever> = Arc::new(tr.retriever);
    let keyword: Arc<dyn Retriever> = Arc::new(KeywordRetriever::new(
        keyword_index.clone(),
        tr.metadata.clone(),
    ));
    let hybrid: Arc<dyn Retriever> = Arc::new(HybridRetriever::new(semantic, keyword.clone()));
    let abstain: Arc<dyn Retriever> = Arc::new(AbstainingRetriever::with_config(
        hybrid,
        keyword,
        AbstainConfig {
            bm25_top_score_threshold: TEST_ABSTAIN_THRESHOLD,
        },
    ));

    let mock = Arc::new(MockLlmProvider::new("mock-llm", MOCK_CANNED_JSON));
    let llm: Arc<dyn LlmProvider> = mock.clone();
    let pipeline = ReadPipeline::new(abstain, llm);

    StackSetup {
        pipeline,
        mock_llm: mock,
        metadata: tr.metadata,
        vectors: tr.vectors,
        keyword_index,
        _dir: tr._dir,
    }
}

async fn insert_full(s: &StackSetup, content: &str, b: &Boundary) -> MemoryId {
    let m = make_memory(content, b);
    let id = m.id;
    s.metadata.create_memory(&m).await.expect("create_memory");

    // Unit-vector embedding (StubEmbedder's default shape).
    let mut emb = vec![0.0_f32; EMBEDDING_DIM];
    emb[0] = 1.0;
    s.vectors.upsert(&id, &emb, b).await.expect("vec upsert");

    s.keyword_index
        .insert(id, content)
        .await
        .expect("kw insert");
    id
}

// ── Strong-anchor: abstain skips, LLM called, response parses ────────────

#[tokio::test]
async fn full_stack_strong_anchor_invokes_llm() {
    let s = setup_stack().await;
    let b = boundary("work");

    let _ = insert_full(&s, "PHASE4_SMOKE_ANCHOR mission notes", &b).await;

    let response = s
        .pipeline
        .read(ReadQuery {
            query_text: "PHASE4_SMOKE_ANCHOR".to_string(),
            authorized_boundaries: vec![b],
        })
        .await
        .expect("read");

    // Mock was called exactly once → abstain did NOT fire AND the LLM
    // path was reached.
    assert_eq!(
        s.mock_llm.call_count(),
        1,
        "mock LLM must be called exactly once on a strong-anchor query"
    );
    // Canned JSON parses into the expected ReadResponse shape.
    assert_eq!(response.synthesis_markdown, "MOCK_SYNTHESIS");
    assert!(response.contradictions_flagged.is_empty());
    assert!(!response.vault_has_no_relevant_content);
}

// ── Hard-negative: abstain fires, LLM NOT called, short-circuit response ─

#[tokio::test]
async fn full_stack_hard_negative_abstains_without_llm() {
    let s = setup_stack().await;
    let b = boundary("work");

    // Insert memories that do NOT contain the query token.
    insert_full(&s, "Cat photos from yesterday", &b).await;
    insert_full(&s, "Dog training notes from the weekend", &b).await;

    let response = s
        .pipeline
        .read(ReadQuery {
            query_text: "ABSOLUTELY_ABSENT_TOKEN_X9Z".to_string(),
            authorized_boundaries: vec![b],
        })
        .await
        .expect("read");

    // Mock was NEVER called → abstain fired → ReadPipeline short-circuited
    // on empty retrieval before reaching the LLM.
    assert_eq!(
        s.mock_llm.call_count(),
        0,
        "mock LLM must NOT be called when abstain fires upstream"
    );
    // ReadPipeline's empty-candidates short-circuit sets this:
    assert!(
        response.vault_has_no_relevant_content,
        "empty retrieval → vault_has_no_relevant_content=true"
    );
    assert!(response.contradictions_flagged.is_empty());
}

// ── Empty-boundary contract: Q1 short-circuits the whole stack ───────────

#[tokio::test]
async fn full_stack_empty_boundaries_short_circuit() {
    let s = setup_stack().await;
    let b = boundary("work");
    let _ = insert_full(&s, "Indexed memory content here", &b).await;

    let response = s
        .pipeline
        .read(ReadQuery {
            query_text: "Indexed".to_string(),
            authorized_boundaries: vec![], // empty
        })
        .await
        .expect("read");

    // Mock NEVER called — Q1 short-circuit at the abstain/hybrid layer
    // (and inside the AbstainingRetriever before the BM25 probe).
    assert_eq!(s.mock_llm.call_count(), 0);
    assert!(response.vault_has_no_relevant_content);
}
