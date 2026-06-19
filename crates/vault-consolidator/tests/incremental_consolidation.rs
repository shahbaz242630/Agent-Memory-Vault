//! Incremental consolidation — cross-corpus recall-safety tests (Pillar 2, ADR-082).
//!
//! These pin the invariant that makes incremental consolidation SAFE: when a
//! nightly run scopes its seeds to facts created since the last successful run
//! (`since = Some(watermark)`), a NEW seed must still cluster with an OLD
//! (pre-watermark) near-duplicate — otherwise we'd silently miss new-vs-old
//! merges, the exact recall loss the recall-sacrosanct lock forbids
//! ([[project_memory_read_primary_search_recall_safe]]). The naive
//! "drop edges to non-seed ids" version FAILS R1; the cross-corpus fix
//! (validate edges against the whole active set) PASSES it.
//!
//! A deterministic keyed embedder (basis-vector per `kN` tag) is used instead of
//! BGE so the tests are fast and need no ONNX model — clustering geometry is
//! exactly controllable: same tag → identical vector (cosine 1.0, clusters);
//! different tag → orthogonal (cosine 0, never clusters).

use async_trait::async_trait;
use chrono::{Duration, Utc};
use vault_consolidator::find_candidate_clusters;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory, VaultResult};
use vault_embedding::{EmbeddingProvider, EMBEDDING_DIM};
use vault_storage::{RetryWorker, SqlCipherKey, StepResult, StorageBackend};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];
const THRESHOLD: f32 = 0.92;

/// Deterministic embedder: the leading `kN` token of the content selects a unit
/// basis vector (a single 1.0 at index `N % EMBEDDING_DIM`). Same tag → identical
/// vector → cosine 1.0 (a near-duplicate that clusters); different tag →
/// orthogonal → cosine 0 (never an edge). Re-embedding the same content yields
/// the same vector, so the stored vector and the clustering query vector match.
struct KeyedEmbedder;

#[async_trait]
impl EmbeddingProvider for KeyedEmbedder {
    async fn embed(&self, text: &str) -> VaultResult<Vec<f32>> {
        let idx = text
            .split_whitespace()
            .next()
            .and_then(|t| t.strip_prefix('k'))
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0)
            % EMBEDDING_DIM;
        let mut v = vec![0.0_f32; EMBEDDING_DIM];
        v[idx] = 1.0;
        Ok(v)
    }
}

async fn open_storage() -> (StorageBackend, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let storage = StorageBackend::open_with_at_rest_key(
        &dir.path().join("metadata.db"),
        &dir.path().join("vectors"),
        &dir.path().join("graph.duckdb"),
        SqlCipherKey::new("incremental-test"),
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
    .await
    .expect("open StorageBackend");
    (storage, dir)
}

/// Write a fact (content tag drives its vector) and return its id.
async fn write_fact(storage: &StorageBackend, boundary: &Boundary, content: &str) -> MemoryId {
    let memory = Memory::try_new(NewMemory {
        content: content.into(),
        memory_type: MemoryType::Semantic,
        boundary: boundary.clone(),
        source_agent: None,
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    })
    .expect("valid memory");
    let embedding = KeyedEmbedder.embed(content).await.expect("embed");
    storage
        .write_memory(&memory, &embedding)
        .await
        .expect("write_memory");
    memory.id
}

/// Drain the cascade queue so just-written vectors land in LanceDB (the
/// clustering primitive reads from LanceDB). Mirrors the acceptance-test drain.
async fn drain(storage: &StorageBackend) {
    let mut worker = RetryWorker::new(storage.clone());
    let drain_at = Utc::now() + Duration::seconds(60);
    for _ in 0..200 {
        match worker.step_at(drain_at).await.expect("worker step_at") {
            StepResult::Idle => return,
            StepResult::SucceededEntry { .. } => {}
            other => panic!("unexpected worker outcome during drain: {other:?}"),
        }
    }
    panic!("cascade drain did not reach Idle within 200 steps");
}

/// R1 — THE keystone. A NEW seed must cluster with an OLD (pre-watermark)
/// near-duplicate. This is what the naive incremental version (drop edges to
/// non-seed ids) silently breaks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incremental_seed_clusters_with_old_neighbour() {
    let (storage, _dir) = open_storage().await;
    let boundary = Boundary::new("personal").expect("boundary");

    // Two OLD facts: `old_dup` (tag k0) and an unrelated `old_other` (tag k7).
    let old_dup = write_fact(
        &storage,
        &boundary,
        "k0 the user settled in Porto years ago",
    )
    .await;
    let old_other = write_fact(&storage, &boundary, "k7 the user enjoys hiking on weekends").await;
    drain(&storage).await;

    // Watermark AFTER the old facts: the incremental run will seed only on
    // facts created from here on.
    let watermark = Utc::now();

    // A NEW fact (tag k0) that duplicates `old_dup`, created after the watermark.
    let new_dup = write_fact(&storage, &boundary, "k0 the user calls Porto home").await;
    drain(&storage).await;

    // Incremental run: seed = {new_dup}; active = {old_dup, old_other, new_dup}.
    let clusters = find_candidate_clusters(
        &storage,
        &KeyedEmbedder,
        &boundary,
        THRESHOLD,
        Some(watermark),
    )
    .await
    .expect("find_candidate_clusters");

    assert_eq!(
        clusters.len(),
        1,
        "exactly one cluster expected (the new+old Porto pair); got {clusters:?}"
    );
    let mut members = clusters[0].member_row_ids.clone();
    members.sort();
    let mut expected = vec![old_dup, new_dup];
    expected.sort();
    assert_eq!(
        members, expected,
        "the NEW seed must cluster with the OLD duplicate (cross-corpus invariant); \
         the unrelated old fact {old_other} must NOT be pulled in"
    );
}

/// Full-sweep regression: with `since = None` the seed set IS the active set, so
/// the classic full-scan behaviour is unchanged — the duplicate pair still
/// clusters and the unrelated fact stays a singleton (filtered out).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_sweep_still_clusters_duplicates() {
    let (storage, _dir) = open_storage().await;
    let boundary = Boundary::new("personal").expect("boundary");

    let dup_a = write_fact(&storage, &boundary, "k0 the user settled in Porto").await;
    let dup_b = write_fact(&storage, &boundary, "k0 Porto is the user's home city").await;
    let _solo = write_fact(&storage, &boundary, "k7 the user enjoys hiking").await;
    drain(&storage).await;

    let clusters = find_candidate_clusters(&storage, &KeyedEmbedder, &boundary, THRESHOLD, None)
        .await
        .expect("find_candidate_clusters");

    assert_eq!(
        clusters.len(),
        1,
        "one cluster (the dup pair); got {clusters:?}"
    );
    let mut members = clusters[0].member_row_ids.clone();
    members.sort();
    let mut expected = vec![dup_a, dup_b];
    expected.sort();
    assert_eq!(
        members, expected,
        "the two duplicates must cluster under full sweep"
    );
}

/// Finding C: a retired (invalidated) fact must NOT be eligible for clustering.
/// Two `k0` duplicates would normally cluster; after invalidating one, the
/// active set holds a single `k0` fact → no pair → no cluster. A retired fact is
/// out of the current truth and must never be re-merged against a live one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalidated_fact_is_excluded_from_clustering() {
    let (storage, _dir) = open_storage().await;
    let boundary = Boundary::new("personal").expect("boundary");

    let _keep = write_fact(&storage, &boundary, "k0 the user settled in Porto").await;
    let retire = write_fact(&storage, &boundary, "k0 Porto is the user's home city").await;
    drain(&storage).await;

    // Retire one duplicate via the bi-temporal invalidate API (ADR-051).
    storage
        .invalidate(retire, Utc::now(), "test: retired".to_string())
        .await
        .expect("invalidate");

    // Full sweep: only the live `k0` fact remains active → no cluster possible.
    let clusters = find_candidate_clusters(&storage, &KeyedEmbedder, &boundary, THRESHOLD, None)
        .await
        .expect("find_candidate_clusters");

    assert!(
        clusters.is_empty(),
        "a retired fact must be excluded from the clustering pool; the lone live \
         k0 fact cannot form a cluster, but got {clusters:?}"
    );
}

/// Idle vault: an incremental run whose watermark is after every fact has no
/// seeds → returns no clusters without doing any embedding work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incremental_with_no_new_seeds_returns_empty() {
    let (storage, _dir) = open_storage().await;
    let boundary = Boundary::new("personal").expect("boundary");

    write_fact(&storage, &boundary, "k0 the user settled in Porto").await;
    write_fact(&storage, &boundary, "k0 Porto is home").await;
    drain(&storage).await;

    // Watermark in the future → no fact qualifies as a seed.
    let future = Utc::now() + Duration::seconds(3600);
    let clusters =
        find_candidate_clusters(&storage, &KeyedEmbedder, &boundary, THRESHOLD, Some(future))
            .await
            .expect("find_candidate_clusters");

    assert!(
        clusters.is_empty(),
        "no new seeds since the watermark → no clusters; got {clusters:?}"
    );
}
