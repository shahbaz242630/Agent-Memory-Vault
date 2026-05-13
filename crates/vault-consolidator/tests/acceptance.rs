//! T0.2.2 Phase 1 — BRD §6 acceptance integration test.
//!
//! BRD §6.2 T0.2.2 acceptance criterion (lines 1432-1434, verbatim):
//!
//! > **T0.2.2 — vault-consolidator: Phase 1 (Cluster).**
//! > Acceptance: 100 memories with known duplicates produces correct clusters.
//!
//! ## Implementation per ADR-045 §c + §d
//!
//! Fixture: `tests/fixtures/clustering_acceptance_100.json` — 20 topics × 5
//! paraphrastic variants each. **Hand-curated** (not Phi-4-driven) per the
//! plan amendment locked at T0.2.2 commit 2 (2026-05-13) — Phi4MiniProvider
//! ships with a hardcoded merge-classifier system prompt, so it cannot be
//! used as-is to generate paraphrastic variants. ADR-045 §c explicitly
//! allows hand-curated as the fallback path; commit 2 picks it for the
//! lower-risk, fully-reproducible route.
//!
//! Pipeline:
//! 1. Parse fixture; shape-assert exactly 100 entries / 20 topics / 5
//!    variants per topic.
//! 2. Open `BgeSmallProvider` against the workspace test-fixtures; embed
//!    every entry.
//! 3. **Gate A** (NN-shared-topic baseline ≥ 90/100 per ADR-045 §c) — for
//!    each variant's embedding, its nearest neighbour (excluding self) must
//!    share its `topic_id` in ≥ 90/100 cases. Catches the case where a
//!    hand-curated variant set looks semantically equivalent to a human but
//!    drifts apart in embedding space.
//! 4. Build a sealed `StorageBackend` over a tempdir; write all 100 memories
//!    and embeddings through the cascading write path; drain the retry queue
//!    so LanceDB upserts complete before clustering.
//! 5. Call `find_candidate_clusters` at the default 0.92 threshold,
//!    `since = None` (full-scan).
//! 6. Compute precision + recall against `topic_id` ground truth.
//! 7. **Gate B** (BRD §6.2 / ADR-045 §d) — assert `precision ≥ 0.95` AND
//!    `recall ≥ 0.90`.
//!
//! ## macOS deferral
//!
//! Gated `#![cfg(not(target_os = "macos"))]` per ADR-033 — the embedding
//! crate's same-OS deferral applies transitively. ONNX Runtime 1.21+ has a
//! known process-exit SIGABRT on macOS that we can't currently work
//! around; Linux + Windows CI matrix covers the embedding logic.

#![cfg(not(target_os = "macos"))]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use vault_consolidator::find_candidate_clusters;
use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_storage::{RetryWorker, SqlCipherKey, StepResult, StorageBackend};

/// Test-only at-rest key. Matches the cross-crate convention from
/// `vault-storage/tests/migration_v0_1_to_sealed.rs:96` +
/// `vault-retrieval/tests/common/mod.rs:26`.
const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

/// Default `merge_similarity_threshold` from `ConsolidatorConfig`
/// (BRD §5.6 line 904). Locked here so the test traces back to spec.
const ACCEPTANCE_THRESHOLD: f32 = 0.92;

/// Precision floor per ADR-045 §d.
const PRECISION_FLOOR: f64 = 0.95;

/// Recall floor per ADR-045 §d.
const RECALL_FLOOR: f64 = 0.90;

/// NN-shared-topic baseline floor per ADR-045 §c.
const NN_SHARED_TOPIC_FLOOR: usize = 90;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FixtureEntry {
    topic_id: u32,
    memory_text: String,
}

fn vault_consolidator_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bge_fixture_root() -> PathBuf {
    vault_consolidator_root()
        .parent()
        .expect("vault-consolidator crate dir has a parent (crates/)")
        .join("vault-embedding")
        .join("test-fixtures")
        .join("bge-small-en-v1.5")
}

fn require_bge_fixture(name: &str) -> PathBuf {
    let p = bge_fixture_root().join(name);
    assert!(
        p.exists(),
        "missing bge-small-en-v1.5 fixture {p:?} — run scripts/setup-dev-env.sh \
         (or .ps1 on Windows) from the repo root to provision it"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib_name() -> &'static str {
    "onnxruntime.dll"
}

#[cfg(target_os = "linux")]
fn ort_lib_name() -> &'static str {
    "libonnxruntime.so"
}

fn open_bge_provider() -> Arc<dyn EmbeddingProvider> {
    let model = require_bge_fixture("model.onnx");
    let tokenizer = require_bge_fixture("tokenizer.json");
    let ort_lib = require_bge_fixture(ort_lib_name());
    let provider = BgeSmallProvider::open(&model, &tokenizer, &ort_lib)
        .expect("BgeSmallProvider must open against the bundled fixtures");
    Arc::new(provider)
}

fn load_fixture() -> Vec<FixtureEntry> {
    let path = vault_consolidator_root().join("tests/fixtures/clustering_acceptance_100.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse fixture JSON at {path:?}: {e}"))
}

/// Cosine similarity for L2-normalised vectors (which `EmbeddingProvider`
/// guarantees) reduces to a dot product.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Gate A — for each entry, count how many have a nearest-neighbour
/// (excluding self) that shares their topic_id. Returns the count out of
/// `entries.len()`.
fn nn_shared_topic_count(entries: &[FixtureEntry], embeddings: &[Vec<f32>]) -> usize {
    let mut shared = 0;
    for (i, e_i) in embeddings.iter().enumerate() {
        let mut best_j = None;
        let mut best_score = f32::NEG_INFINITY;
        for (j, e_j) in embeddings.iter().enumerate() {
            if i == j {
                continue;
            }
            let score = cosine(e_i, e_j);
            if score > best_score {
                best_score = score;
                best_j = Some(j);
            }
        }
        if let Some(j) = best_j {
            if entries[i].topic_id == entries[j].topic_id {
                shared += 1;
            }
        }
    }
    shared
}

/// Compute precision + recall of predicted clusters against ground-truth
/// topic_ids, using the pair-counting definition from ADR-045 §d.
///
/// - TP: pairs in the same predicted cluster AND same topic_id.
/// - FP: pairs in the same predicted cluster AND different topic_id.
/// - FN: pairs in different predicted clusters AND same topic_id.
fn precision_recall(
    predicted: &[Vec<MemoryId>],
    memory_to_topic: &HashMap<MemoryId, u32>,
) -> (f64, f64) {
    // Build "predicted cluster id per memory" map. Memories not in any
    // predicted cluster (singletons filtered by `find_candidate_clusters`)
    // get sentinel `None` — they cannot pair-cluster with anything.
    let mut predicted_cluster: HashMap<MemoryId, usize> = HashMap::new();
    for (cluster_idx, members) in predicted.iter().enumerate() {
        for id in members {
            predicted_cluster.insert(*id, cluster_idx);
        }
    }

    let ids: Vec<MemoryId> = memory_to_topic.keys().copied().collect();
    let mut tp = 0u64;
    let mut fp = 0u64;
    let mut fn_ = 0u64;

    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            let a = ids[i];
            let b = ids[j];
            let same_topic = memory_to_topic[&a] == memory_to_topic[&b];
            let same_predicted_cluster =
                match (predicted_cluster.get(&a), predicted_cluster.get(&b)) {
                    (Some(ca), Some(cb)) => ca == cb,
                    _ => false,
                };

            match (same_predicted_cluster, same_topic) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => fn_ += 1,
                (false, false) => {} // TN, not used.
            }
        }
    }

    let precision = if tp + fp == 0 {
        // Algorithm produced no positive predictions. Conventional choice:
        // precision = 1.0 (no false positives). Acceptance still requires
        // recall ≥ 0.90, which fails loudly under this case.
        1.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let recall = if tp + fn_ == 0 {
        1.0
    } else {
        tp as f64 / (tp + fn_) as f64
    };
    (precision, recall)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clustering_meets_brd_acceptance_floor() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_test_writer()
        .try_init();

    // ── Step 1: load + shape-assert fixture ──────────────────────────────
    let fixture = load_fixture();
    assert_eq!(
        fixture.len(),
        100,
        "fixture must contain exactly 100 entries"
    );
    let mut topic_counts: BTreeMap<u32, u32> = BTreeMap::new();
    for entry in &fixture {
        *topic_counts.entry(entry.topic_id).or_insert(0) += 1;
    }
    assert_eq!(
        topic_counts.len(),
        20,
        "fixture must cover exactly 20 distinct topic_ids; got {topic_counts:?}"
    );
    for (topic, count) in &topic_counts {
        assert_eq!(
            *count, 5,
            "topic {topic} must have exactly 5 variants; got {count}"
        );
    }

    // ── Step 2: embed all 100 entries ────────────────────────────────────
    let embedder = open_bge_provider();
    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(fixture.len());
    for entry in &fixture {
        let v = embedder
            .embed(&entry.memory_text)
            .await
            .unwrap_or_else(|e| panic!("embed failed on {:?}: {e}", entry.memory_text));
        assert_eq!(
            v.len(),
            EMBEDDING_DIM,
            "embedding must be of length EMBEDDING_DIM"
        );
        embeddings.push(v);
    }

    // ── Step 3: Gate A — NN-shared-topic baseline ≥ 90/100 ──────────────
    let shared = nn_shared_topic_count(&fixture, &embeddings);
    tracing::info!(
        nn_shared_topic = shared,
        floor = NN_SHARED_TOPIC_FLOOR,
        "Gate A: NN-shared-topic baseline"
    );
    assert!(
        shared >= NN_SHARED_TOPIC_FLOOR,
        "Gate A FAILED: NN-shared-topic baseline {shared}/100 below ADR-045 §c floor of \
         {NN_SHARED_TOPIC_FLOOR}/100. Fixture variants are too loosely paraphrastic in \
         embedding space — tighten same-topic paraphrases or revise cross-topic distance."
    );

    // ── Step 4: open sealed StorageBackend + write all 100 memories ─────
    let dir = tempfile::tempdir().expect("tempdir");
    let key = SqlCipherKey::new("acceptance-test-passphrase");
    let storage = StorageBackend::open_with_at_rest_key(
        &dir.path().join("metadata.db"),
        &dir.path().join("vectors"),
        &dir.path().join("graph.duckdb"),
        key,
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
    .await
    .expect("open sealed StorageBackend");

    let boundary = Boundary::new("acceptance").expect("valid boundary");
    let mut memory_to_topic: HashMap<MemoryId, u32> = HashMap::new();
    for (entry, embedding) in fixture.iter().zip(embeddings.iter()) {
        let memory = Memory::try_new(NewMemory {
            content: entry.memory_text.clone(),
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("valid memory");
        memory_to_topic.insert(memory.id, entry.topic_id);
        storage
            .write_memory(&memory, embedding)
            .await
            .expect("write_memory");
    }

    // ── Step 5: drive the retry worker until the cascade queue is empty ─
    // `write_memory` commits the SQLite-side state synchronously but
    // queues the LanceDB upsert for the worker (cascading.rs:5-7). The
    // clustering primitive reads from LanceDB, so we must drain before
    // calling `find_candidate_clusters`.
    //
    // New retry-queue entries get `next_attempt_at ≈ created_at + 1s`
    // (per `retry_queue.rs:962` initial-delay pin). Test-time fast-forward
    // via `step_at(now + 60s)` so all 100 just-enqueued entries poll as
    // due immediately — same pattern as vault-storage's own retry tests.
    let mut worker = RetryWorker::new(storage.clone());
    let drain_at = Utc::now() + Duration::seconds(60);
    let mut succeeded = 0;
    for _ in 0..200 {
        match worker.step_at(drain_at).await.expect("worker step_at") {
            StepResult::Idle => break,
            StepResult::SucceededEntry { .. } => succeeded += 1,
            other => panic!("unexpected worker outcome during drain: {other:?}"),
        }
    }
    assert_eq!(
        succeeded, 100,
        "all 100 cascade entries must drain successfully; got {succeeded}"
    );

    // ── Step 6: run clustering ──────────────────────────────────────────
    let clusters = find_candidate_clusters(
        &storage,
        embedder.as_ref(),
        &boundary,
        ACCEPTANCE_THRESHOLD,
        None,
    )
    .await
    .expect("find_candidate_clusters");

    tracing::info!(
        cluster_count = clusters.len(),
        sizes = ?clusters.iter().map(|c| c.size()).collect::<Vec<_>>(),
        "Phase 1 clustering produced clusters"
    );

    // ── Step 7: Gate B — precision + recall against topic_id ground truth ─
    let predicted_members: Vec<Vec<MemoryId>> =
        clusters.iter().map(|c| c.member_row_ids.clone()).collect();
    let (precision, recall) = precision_recall(&predicted_members, &memory_to_topic);

    tracing::info!(
        precision = precision,
        recall = recall,
        precision_floor = PRECISION_FLOOR,
        recall_floor = RECALL_FLOOR,
        "Gate B: BRD §6 acceptance scoring"
    );

    assert!(
        precision >= PRECISION_FLOOR,
        "Gate B FAILED: precision {precision:.4} below ADR-045 §d floor of {PRECISION_FLOOR}. \
         Clustering algorithm is grouping cross-topic memories together — false-positive edges \
         crossed the {ACCEPTANCE_THRESHOLD} cosine-similarity threshold."
    );
    assert!(
        recall >= RECALL_FLOOR,
        "Gate B FAILED: recall {recall:.4} below ADR-045 §d floor of {RECALL_FLOOR}. \
         Clustering algorithm is splitting same-topic memories across clusters — true-positive \
         edges fell below the {ACCEPTANCE_THRESHOLD} cosine-similarity threshold."
    );
}
