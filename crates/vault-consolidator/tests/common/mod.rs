//! Shared helpers for vault-consolidator integration + property tests.
//!
//! Lives at `tests/common/mod.rs` (not `tests/common.rs`) so Cargo does NOT
//! treat it as a separate test binary — `tests/<name>.rs` becomes a test
//! crate, `tests/<dir>/mod.rs` is an internal module each test crate can
//! `mod common;` into.

#![allow(dead_code)] // Each test file uses a subset.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use serde::Deserialize;
use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_llm::{CompletionParams, LlmProvider, VaultLlmError, VaultLlmResult};
use vault_storage::{RetryWorker, SqlCipherKey, StepResult, StorageBackend};

/// Test-only at-rest key. Matches the cross-crate convention from
/// `vault-storage/tests/migration_v0_1_to_sealed.rs:96` +
/// `vault-consolidator/tests/acceptance.rs:60` +
/// `vault-consolidator/src/phases/merge.rs:384`.
pub const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

// ── Fixture types ────────────────────────────────────────────────────────

/// One entry from `merge_acceptance_100.json`. Schema locked at T0.2.3
/// commit 3 plan iteration 1 (2026-05-14) per the ground-truth design.
#[derive(Debug, Clone, Deserialize)]
pub struct MergeAcceptanceFixtureEntry {
    pub id: String,
    pub boundary: String,
    pub topic_label: String,
    pub content: String,
    pub ground_truth: GroundTruth,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroundTruth {
    /// One of `"merge"` / `"keep_separate"` / `"contradiction"`.
    pub outcome: String,
    /// Same value across memories that should end up in the same
    /// consolidated record. `None` for `keep_separate` singletons.
    pub cluster: Option<String>,
}

// ── Path helpers ─────────────────────────────────────────────────────────

pub fn vault_consolidator_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

// ── Fixture loaders ──────────────────────────────────────────────────────

pub fn load_merge_acceptance_fixture() -> Vec<MergeAcceptanceFixtureEntry> {
    let path = vault_consolidator_root().join("tests/fixtures/merge_acceptance_100.json");
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse fixture JSON at {path:?}: {e}"))
}

/// Load one named canned response from `canned_merge_decisions_nary.json`
/// and serialize it to a JSON string ready for `MockLlmProvider::new` or
/// `ScriptedLlmProvider::new`.
pub fn load_canned_response_as_string(name: &str) -> String {
    let path = vault_consolidator_root().join("tests/fixtures/canned_merge_decisions_nary.json");
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("read canned fixture {path:?}: {e}"));
    let parsed: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("parse canned fixture: {e}"));
    let response = parsed
        .get("responses")
        .and_then(|m| m.get(name))
        .unwrap_or_else(|| panic!("canned response '{name}' not found in fixture"));
    serde_json::to_string(response).expect("serialize canned response to string")
}

// ── Storage setup ────────────────────────────────────────────────────────

/// Open a fresh sealed `StorageBackend` against a tempdir. Returns the
/// backend (wrap in `Arc` at the call site if needed for `Consolidator::new`)
/// alongside the `TempDir` guard which the caller MUST keep alive until the
/// test finishes (otherwise files get cleaned up mid-run).
pub async fn open_sealed_storage_for_test(passphrase: &str) -> (StorageBackend, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = SqlCipherKey::new(passphrase);
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
    (storage, dir)
}

// ── Memory construction ──────────────────────────────────────────────────

pub fn make_memory_from_fixture(entry: &MergeAcceptanceFixtureEntry) -> Memory {
    let boundary = Boundary::new(&entry.boundary).expect("valid boundary in fixture");
    Memory::try_new(NewMemory {
        content: entry.content.clone(),
        memory_type: MemoryType::Semantic,
        boundary,
        source_agent: None,
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    })
    .expect("valid memory")
}

pub fn make_memory_with_content(content: &str, boundary: &Boundary) -> Memory {
    Memory::try_new(NewMemory {
        content: content.into(),
        memory_type: MemoryType::Semantic,
        boundary: boundary.clone(),
        source_agent: None,
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    })
    .expect("valid memory")
}

// ── Insert + drain retry worker ──────────────────────────────────────────

/// Write memories through the cascading path, then drain the retry worker
/// so LanceDB upserts complete before downstream tests query vectors.
/// Mirror of T0.2.2 acceptance.rs:331-344 + merge.rs:420-440.
pub async fn insert_and_drain(storage: &StorageBackend, pairs: Vec<(Memory, Vec<f32>)>) {
    let count = pairs.len();
    for (memory, embedding) in &pairs {
        storage
            .write_memory(memory, embedding)
            .await
            .expect("write_memory");
    }
    let mut worker = RetryWorker::new(storage.clone());
    let drain_at = chrono::Utc::now() + ChronoDuration::seconds(60);
    let mut succeeded = 0usize;
    for _ in 0..(count * 2 + 10) {
        match worker.step_at(drain_at).await.expect("worker step_at") {
            StepResult::Idle => break,
            StepResult::SucceededEntry { .. } => {
                succeeded += 1;
            }
            other => panic!("unexpected worker outcome during drain: {other:?}"),
        }
    }
    assert_eq!(
        succeeded, count,
        "expected all {count} cascade entries to drain successfully; got {succeeded}"
    );
}

// ── BGE embedding provider ───────────────────────────────────────────────

fn bge_fixture_root() -> PathBuf {
    vault_consolidator_root()
        .parent()
        .expect("vault-consolidator dir has parent")
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

#[cfg(target_os = "macos")]
fn ort_lib_name() -> &'static str {
    "libonnxruntime.dylib"
}

pub fn open_bge_provider() -> Arc<dyn EmbeddingProvider> {
    let model = require_bge_fixture("model.onnx");
    let tokenizer = require_bge_fixture("tokenizer.json");
    let ort_lib = require_bge_fixture(ort_lib_name());
    let provider = BgeSmallProvider::open(&model, &tokenizer, &ort_lib)
        .expect("BgeSmallProvider must open against the bundled fixtures");
    Arc::new(provider)
}

// ── ScriptedLlmProvider ──────────────────────────────────────────────────

/// Test-only `LlmProvider` that returns a pre-scripted sequence of canned
/// responses. First `complete_json` call returns the first response, second
/// call returns the second, etc. Returns `VaultLlmError::InferenceFailed`
/// when the queue is exhausted so test failures are loud + diagnosable.
///
/// Companion to `vault_llm::MockLlmProvider` (which always returns the
/// SAME canned response). Use ScriptedLlmProvider when a test wants the
/// orchestrator to dispatch different outcomes across clusters in one run.
/// Test-only — lives in tests/common/, never compiled into production code.
pub struct ScriptedLlmProvider {
    responses: Mutex<VecDeque<String>>,
    model_id: String,
}

impl ScriptedLlmProvider {
    pub fn new(model_id: impl Into<String>, responses: Vec<String>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            model_id: model_id.into(),
        }
    }

    /// Remaining responses in the queue. Useful for end-of-test assertions
    /// ("did the orchestrator consume all the responses I scripted?").
    pub fn remaining(&self) -> usize {
        self.responses
            .lock()
            .expect("ScriptedLlmProvider mutex poisoned")
            .len()
    }
}

impl std::fmt::Debug for ScriptedLlmProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedLlmProvider")
            .field("model_id", &self.model_id)
            .field("remaining", &self.remaining())
            .finish()
    }
}

#[async_trait]
impl LlmProvider for ScriptedLlmProvider {
    async fn complete_json(
        &self,
        _prompt: &str,
        _schema: &str,
        _params: &CompletionParams,
    ) -> VaultLlmResult<String> {
        let mut q = self
            .responses
            .lock()
            .expect("ScriptedLlmProvider mutex poisoned");
        q.pop_front().ok_or_else(|| {
            VaultLlmError::InferenceFailed(
                "ScriptedLlmProvider exhausted: test set up fewer canned responses than the \
                 orchestrator issued LLM calls"
                    .to_string(),
            )
        })
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}
