//! Property-grade integration tests for `KeywordIndex` + `KeywordRetriever`
//! (BM25 lexical retrieval).
//!
//! Phase 1 of T0.2.7 (hybrid retrieval). The tests exercise the public
//! Phase-1 contract surface:
//!
//! - **Index lifecycle**: insert (idempotent), upsert (replaces), delete
//!   (idempotent absent), bulk_insert, search.
//! - **Memory-length coverage**: single-line, medium, long paragraphs —
//!   Shahbaz's explicit Phase-1 requirement.
//! - **Retriever boundary isolation**: authorized_boundaries non-empty
//!   semantics, no cross-boundary leakage.
//! - **Concurrency**: parallel reads against a shared index (Send + Sync
//!   correctness for `Arc<KeywordIndex>`).
//! - **Unicode + apostrophe**: Tantivy default tokenizer + Lucene-operator
//!   sanitization (per spike apostrophe failure root-cause).
//!
//! Tests live under `tests/` so they consume only the public crate API.

#![forbid(unsafe_code)]

mod common;

use std::sync::Arc;

use vault_core::{Boundary, MemoryId};
use vault_retrieval::{KeywordIndex, KeywordRetriever, Retriever};
use vault_storage::{MetadataStore, SqlCipherKey};

use common::{boundary, make_memory, query};

/// Bundle for keyword-only tests: index + retriever + metadata store +
/// temp dir keepalive. No vector store, no embedder — the keyword
/// channel is standalone at the Phase-1 contract surface.
struct TestKeywordSetup {
    index: Arc<KeywordIndex>,
    retriever: KeywordRetriever,
    metadata: Arc<MetadataStore>,
    _dir: tempfile::TempDir,
}

async fn setup_keyword() -> TestKeywordSetup {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = SqlCipherKey::new("test-only-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key)
        .await
        .expect("open metadata");
    let metadata = Arc::new(metadata);
    let index = Arc::new(KeywordIndex::new().expect("new keyword index"));
    let retriever = KeywordRetriever::new(index.clone(), metadata.clone());
    TestKeywordSetup {
        index,
        retriever,
        metadata,
        _dir: dir,
    }
}

/// Create a Memory via the common helper, persist to MetadataStore,
/// then index its content in KeywordIndex. Returns the assigned id.
async fn insert_memory(s: &TestKeywordSetup, content: &str, b: &Boundary) -> MemoryId {
    let m = make_memory(content, b);
    let id = m.id;
    s.metadata.create_memory(&m).await.expect("create_memory");
    s.index.insert(id, content).await.expect("index insert");
    id
}

// ── Memory-length coverage (Shahbaz Phase-1 requirement) ─────────────────

#[tokio::test]
async fn short_memory_searchable() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let id = insert_memory(&s, "Comcast bill is $89 per month", &b).await;

    let results = s
        .retriever
        .retrieve(query("Comcast", vec![b], 10))
        .await
        .expect("retrieve");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].memory.id, id);
    assert!(
        results[0].score > 0.0,
        "BM25 score on a real hit must be > 0"
    );
}

#[tokio::test]
async fn medium_memory_searchable() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let medium_content = "Captured a long debrief on the GA launch readiness review held \
        yesterday. We discussed beta-readiness assessment, the next milestone for the engineering \
        team, marketing site copy QA timing, pricing page sign-off, and the press push timing. \
        The GA launch is currently scheduled for Q2 2027, postponed from the original Q1 2027 \
        target. Sales enablement materials need updating to reflect this shift, and customer \
        communications will go out next week.";
    assert!(medium_content.len() > 200 && medium_content.len() < 700);
    let id = insert_memory(&s, medium_content, &b).await;

    let results = s
        .retriever
        .retrieve(query("Q1 2027 GA launch", vec![b], 10))
        .await
        .expect("retrieve");

    assert!(!results.is_empty());
    assert_eq!(results[0].memory.id, id);
}

#[tokio::test]
async fn long_memory_searchable() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let mut long_content = String::new();
    while long_content.len() < 1100 {
        long_content.push_str("This is filler content about general project context. ");
    }
    long_content.push_str("The rare anchor token DENTAL_POLICY_X7Q9 appears here. ");
    while long_content.len() < 2400 {
        long_content.push_str("More filler about engineering standup notes. ");
    }
    assert!(
        long_content.len() > 2000,
        "long memory should be > 2000 chars, got {}",
        long_content.len()
    );
    let id = insert_memory(&s, &long_content, &b).await;

    let results = s
        .retriever
        .retrieve(query("DENTAL_POLICY_X7Q9", vec![b], 10))
        .await
        .expect("retrieve");

    assert!(
        !results.is_empty(),
        "rare anchor in long doc must be findable"
    );
    assert_eq!(results[0].memory.id, id);
}

// ── Index lifecycle ──────────────────────────────────────────────────────

#[tokio::test]
async fn delete_invariant() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let id = insert_memory(&s, "Memo about the quarterly review meeting", &b).await;

    let before = s
        .retriever
        .retrieve(query("quarterly review", vec![b.clone()], 10))
        .await
        .expect("retrieve before");
    assert_eq!(before.len(), 1, "baseline: memory should be searchable");

    s.index.delete(id).await.expect("delete");

    let after = s
        .retriever
        .retrieve(query("quarterly review", vec![b], 10))
        .await
        .expect("retrieve after");
    assert_eq!(after.len(), 0, "deleted memory must not be returned");
}

#[tokio::test]
async fn upsert_replaces_content() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let m = make_memory("Original token APPLE_ANCHOR_77", &b);
    let id = m.id;
    s.metadata.create_memory(&m).await.expect("create_memory");
    s.index
        .insert(id, "Original token APPLE_ANCHOR_77")
        .await
        .expect("insert");

    s.index
        .upsert(id, "Replaced token BANANA_ANCHOR_88")
        .await
        .expect("upsert");

    let old_hit = s
        .retriever
        .retrieve(query("APPLE_ANCHOR_77", vec![b.clone()], 10))
        .await
        .expect("retrieve old");
    assert!(
        old_hit.is_empty(),
        "old content must not match after upsert (Tantivy index level)"
    );

    let new_hit = s
        .retriever
        .retrieve(query("BANANA_ANCHOR_88", vec![b], 10))
        .await
        .expect("retrieve new");
    assert_eq!(new_hit.len(), 1, "new content must match after upsert");
    assert_eq!(new_hit[0].memory.id, id);
}

#[tokio::test]
async fn idempotent_insert() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let m = make_memory("Project status update CAT_TOKEN_42", &b);
    let id = m.id;
    s.metadata.create_memory(&m).await.expect("create_memory");

    s.index.insert(id, &m.content).await.expect("insert 1");
    s.index.insert(id, &m.content).await.expect("insert 2");

    let results = s
        .retriever
        .retrieve(query("CAT_TOKEN_42", vec![b], 10))
        .await
        .expect("retrieve");

    assert_eq!(
        results.len(),
        1,
        "duplicate insert must yield exactly one hit (idempotent upsert semantics)"
    );
}

#[tokio::test]
async fn delete_absent_is_idempotent() {
    let s = setup_keyword().await;
    // Generate a fresh MemoryId without inserting anything.
    let m = make_memory("never indexed", &boundary("work"));
    let r = s.index.delete(m.id).await;
    assert!(
        r.is_ok(),
        "deleting an absent id must be a no-op, not an error"
    );
}

// ── Retriever-level: boundary isolation ──────────────────────────────────

#[tokio::test]
async fn boundary_isolation() {
    let s = setup_keyword().await;
    let work_b = boundary("work");
    let personal_b = boundary("personal");

    let work_id = insert_memory(&s, "Shared anchor token UNICORN_999", &work_b).await;
    let _personal_id = insert_memory(&s, "Shared anchor token UNICORN_999", &personal_b).await;

    // Query with ONLY work boundary authorized.
    let results = s
        .retriever
        .retrieve(query("UNICORN_999", vec![work_b], 10))
        .await
        .expect("retrieve");

    assert_eq!(
        results.len(),
        1,
        "boundary filter must exclude personal memory"
    );
    assert_eq!(results[0].memory.id, work_id);
}

#[tokio::test]
async fn empty_authorized_boundaries_returns_empty() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let _ = insert_memory(&s, "Shared anchor token TIGER_5", &b).await;

    // Q1 contract: empty authorized_boundaries → empty result, no error.
    let results = s
        .retriever
        .retrieve(query("TIGER_5", vec![], 10))
        .await
        .expect("retrieve");
    assert!(results.is_empty());
}

#[tokio::test]
async fn empty_query_returns_error() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let _ = insert_memory(&s, "Some content", &b).await;

    // Q2 contract: empty/whitespace query → InvalidInput error, matching
    // SemanticRetriever surface for parity.
    for q in ["", "   ", "\n\t"] {
        let r = s.retriever.retrieve(query(q, vec![b.clone()], 10)).await;
        assert!(
            r.is_err(),
            "empty/whitespace query {q:?} must error per Q2 invariant"
        );
    }
}

// ── Query sanitization (Lucene operators) ────────────────────────────────

#[tokio::test]
async fn apostrophe_query_sanitized() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let id = insert_memory(&s, "Comcast bill is overdue this month", &b).await;

    // Lucene treats `'` as an operator without a binding operand →
    // parser error unless sanitized. Spike confirmed at SCALE=100 smoke
    // (4/9 queries failed before sanitization landed).
    let results = s
        .retriever
        .retrieve(query("What's the Comcast bill?", vec![b], 10))
        .await
        .expect("apostrophe + ? query must not error");

    assert!(!results.is_empty());
    assert_eq!(results[0].memory.id, id);
}

#[tokio::test]
async fn lucene_operator_chars_in_query_are_safe() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let id = insert_memory(&s, "Discussion about Project Alpha milestones", &b).await;

    // All of these would crash a naive QueryParser. Each MUST succeed
    // (return Ok, possibly empty) after sanitization.
    for q in [
        "Alpha+", "Alpha-", "Alpha:", "Alpha(", "Alpha)", "Alpha*", "Alpha?", "\"Alpha", "Alpha~",
        "Alpha^", "Alpha|", "Alpha&",
    ] {
        let r = s.retriever.retrieve(query(q, vec![b.clone()], 10)).await;
        assert!(r.is_ok(), "query {q:?} must not error after sanitization");
        let hits = r.expect("ok");
        // At least Q11 "Alpha+", "Alpha-", "Alpha:" should still match
        // "Alpha" as the surviving token. Not all queries are guaranteed
        // to hit (e.g. "Alpha~" sanitizes to "Alpha " which matches; OK).
        if q == "Alpha+" || q == "Alpha-" || q == "Alpha~" {
            assert!(
                !hits.is_empty(),
                "query {q:?} should still find the memory via the surviving 'Alpha' token"
            );
            assert_eq!(hits[0].memory.id, id);
        }
    }
}

// ── Bulk + concurrency ───────────────────────────────────────────────────

#[tokio::test]
async fn bulk_insert_smoke() {
    let s = setup_keyword().await;
    let b = boundary("work");

    let mut memories = Vec::with_capacity(100);
    for i in 0..100 {
        let content = format!("Memo number {i} about topic ALPHA_TOKEN_{i:03}");
        let m = make_memory(&content, &b);
        s.metadata.create_memory(&m).await.expect("create_memory");
        memories.push(m);
    }
    s.index.bulk_insert(&memories).await.expect("bulk insert");

    let n = s.index.len().await.expect("len");
    assert_eq!(n, 100, "bulk_insert must populate exactly 100 docs");

    // Sample 5 distinct tokens; each must be findable + uniquely so.
    for i in [3, 27, 50, 73, 99] {
        let token = format!("ALPHA_TOKEN_{i:03}");
        let results = s
            .retriever
            .retrieve(query(&token, vec![b.clone()], 10))
            .await
            .expect("retrieve");
        assert!(
            !results.is_empty(),
            "bulk-inserted token {token} not findable"
        );
        assert!(
            results[0].memory.content.contains(&token),
            "top hit content must contain the query token"
        );
    }
}

#[tokio::test]
async fn concurrent_search_safe() {
    let s = setup_keyword().await;
    let b = boundary("work");

    // Seed 20 memories with distinct tokens.
    for i in 0..20 {
        let content = format!("Memo {i} containing rare token CONCURRENT_{i:02}");
        let m = make_memory(&content, &b);
        s.metadata.create_memory(&m).await.expect("create_memory");
        s.index.insert(m.id, &m.content).await.expect("insert");
    }

    let retriever = Arc::new(s.retriever);
    let mut handles = Vec::with_capacity(8);
    for i in 0..8 {
        let r = retriever.clone();
        let b = b.clone();
        handles.push(tokio::spawn(async move {
            let token = format!("CONCURRENT_{i:02}");
            r.retrieve(query(&token, vec![b], 10)).await
        }));
    }

    for h in handles {
        let result = h.await.expect("join").expect("retrieve");
        assert!(
            !result.is_empty(),
            "parallel search must find its target token"
        );
    }
}

// ── Misc ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn new_index_is_empty() {
    let index = KeywordIndex::new().expect("new");
    assert_eq!(index.len().await.expect("len"), 0);

    let results = index.search("anything", 10).await.expect("search");
    assert!(results.is_empty());
}

/// Degenerate-query robustness: a query consisting ENTIRELY of stopwords
/// reduces to zero searchable terms once the `vault_text` analyzer's
/// StopWordFilter runs. Such a query must return a graceful empty result —
/// NOT a `VaultError::Storage` (which maps to JSON-RPC `-32603 internal
/// error` at the MCP boundary). Surfaced in §7 live dogfood 2026-05-30:
/// `memory_search "the a is of and to"` returned `-32603`. The keyword
/// channel is the relevant path because `AbstainingRetriever` probes it
/// before any threshold check, so an error here propagates straight out
/// before the abstain gate can return empty.
#[tokio::test]
async fn all_stopword_query_returns_empty_not_error() {
    let s = setup_keyword().await;
    let b = boundary("work");
    // Populate the index so we exercise the real (non-empty-index) path.
    insert_memory(&s, "Comcast bill is $89 per month", &b).await;

    let result = s.index.search("the a is of and to", 200).await;
    assert!(
        result.is_ok(),
        "all-stopword query must not error (got {result:?})"
    );
    assert!(
        result.expect("ok").is_empty(),
        "all-stopword query has no searchable terms; result must be empty"
    );
}

/// No over-suppression: a query that mixes stopwords with a single real
/// content word still tokenizes to ≥1 term, so it must reach the parser
/// and find the matching memory. Guards the degenerate-query short-circuit
/// against accidentally swallowing legitimate queries.
#[tokio::test]
async fn mixed_stopword_and_content_query_still_matches() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let id = insert_memory(&s, "Comcast bill is $89 per month", &b).await;

    let hits = s
        .index
        .search("what is the Comcast", 200)
        .await
        .expect("search must not error");
    assert!(
        hits.iter().any(|(hit_id, _)| *hit_id == id),
        "query with one content word ('Comcast') must still match"
    );
}

#[tokio::test]
async fn unicode_content_searchable() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let id = insert_memory(&s, "Met Müller for résumé review at the Café", &b).await;

    let results = s
        .retriever
        .retrieve(query("Müller résumé", vec![b], 10))
        .await
        .expect("retrieve");

    assert!(!results.is_empty(), "unicode tokens must be searchable");
    assert_eq!(results[0].memory.id, id);
}

// ── ADR-051 (T0.2.7 Phase B): bi-temporal `valid_until` filter ──────────

#[tokio::test]
async fn expired_memory_filtered_by_default() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let live_id = insert_memory(&s, "still-true fact about Comcast", &b).await;
    let expired_id = insert_memory(&s, "old fact about Comcast", &b).await;

    // Mark the second memory as expired one hour ago. Pin valid_from to
    // 2 days ago so the Memory invariant (valid_until >= valid_from)
    // holds independent of microsecond-scale clock skew.
    let mut expired = s
        .metadata
        .get_memory(&expired_id)
        .await
        .expect("get_memory")
        .expect("memory exists");
    expired.valid_from = chrono::Utc::now() - chrono::Duration::days(2);
    expired.valid_until = Some(chrono::Utc::now() - chrono::Duration::hours(1));
    s.metadata.update_memory(&expired).await.expect("update");

    let results = s
        .retriever
        .retrieve(query("Comcast", vec![b], 10))
        .await
        .expect("retrieve");

    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(
        ids.contains(&live_id),
        "live memory must surface; got {ids:?}"
    );
    assert!(
        !ids.contains(&expired_id),
        "expired memory must be filtered by default per ADR-051; got {ids:?}"
    );
}

#[tokio::test]
async fn expired_memory_included_with_include_archived_true() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let expired_id = insert_memory(&s, "old fact about Comcast", &b).await;

    let mut expired = s
        .metadata
        .get_memory(&expired_id)
        .await
        .expect("get_memory")
        .expect("memory exists");
    expired.valid_from = chrono::Utc::now() - chrono::Duration::days(2);
    expired.valid_until = Some(chrono::Utc::now() - chrono::Duration::hours(1));
    s.metadata.update_memory(&expired).await.expect("update");

    let mut q = query("Comcast", vec![b], 10);
    q.options.include_archived = true;
    let results = s.retriever.retrieve(q).await.expect("retrieve");

    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(
        ids.contains(&expired_id),
        "expired memory must surface when include_archived=true per ADR-051; got {ids:?}"
    );
}

#[tokio::test]
async fn future_dated_valid_until_does_not_exclude() {
    let s = setup_keyword().await;
    let b = boundary("work");
    let id = insert_memory(&s, "fact that expires next year about Comcast", &b).await;

    let mut m = s
        .metadata
        .get_memory(&id)
        .await
        .expect("get_memory")
        .expect("memory exists");
    m.valid_until = Some(chrono::Utc::now() + chrono::Duration::days(365));
    s.metadata.update_memory(&m).await.expect("update");

    let results = s
        .retriever
        .retrieve(query("Comcast", vec![b], 10))
        .await
        .expect("retrieve");

    let ids: Vec<MemoryId> = results.iter().map(|r| r.memory.id).collect();
    assert!(
        ids.contains(&id),
        "future-dated valid_until must NOT exclude per ADR-051; got {ids:?}"
    );
}
