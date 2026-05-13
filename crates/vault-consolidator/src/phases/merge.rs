//! Phase 2 (LLM merge decisions) + Phase 3 (Apply merges) — BRD §5.6 lines 940-950.
//!
//! T0.2.3 commit 1 ships **Phase 2 only** — the [`decide_merge`] primitive
//! that takes a [`Cluster`] (from Phase 1) and asks the configured
//! [`LlmProvider`] to decide whether the cluster merges, stays separate,
//! or contains a contradiction. The output is a [`MergeOutcome`] enum
//! value; the orchestrator consumes it at commit 2 via the `apply_merge`
//! primitive (also landing in this module).
//!
//! Cluster member content is hydrated from `MetadataStore` at decision time
//! — BRD §5.6 line 941 verbatim "send the cluster contents to Phi-4-mini" —
//! same re-read pattern as T0.2.2 Phase 1's re-embed-at-consolidation
//! (`Memory.embedding = None` after metadata-side read; embeddings live in
//! LanceDB). For Phase 2 the cluster's `member_row_ids` are looked up by ID
//! and their `content` strings collected into the LLM prompt.
//!
//! See ADR-044 Amendment 1 (rides with T0.2.3 commit 1) for the
//! [`CompletionParams::system_prompt`] override that Phase 2 uses to swap
//! the default merge-classifier system message for the N-ary cluster
//! merge-decision system message.
//!
//! [`Cluster`]: crate::phases::cluster::Cluster
//! [`LlmProvider`]: vault_llm::LlmProvider
//! [`CompletionParams::system_prompt`]: vault_llm::CompletionParams

use serde::{Deserialize, Serialize};
use tracing::{instrument, warn};
use vault_core::{MemoryId, VaultError, VaultResult};
use vault_llm::{CompletionParams, LlmProvider};
use vault_storage::{MemoryFilter, StorageBackend};

use crate::phases::cluster::Cluster;

/// Outcome of one Phase 2 merge decision per BRD §5.6 line 942.
///
/// Per ADR-045 §e + T0.2.3 iteration 2 Q2 (α single-decision-per-cluster),
/// **one decision per cluster** — the LLM does not split a cluster into
/// partial merges at T0.2.3. If V0.2 dogfood shows the LLM systematically
/// wants partial splits, amend to β multi-decision shape at T0.2.3 close
/// or T0.2.7.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum MergeOutcome {
    /// LLM decided the cluster's memories should be consolidated into one.
    /// `merged_text` is the LLM-produced consolidated content; the orchestrator
    /// (Phase 3 `apply_merge`, T0.2.3 commit 2) writes a new memory with this
    /// content and marks the cluster members as `superseded_by` the new row.
    Merge {
        merged_text: String,
        reasoning: String,
    },
    /// LLM decided the cluster's memories are NOT duplicates despite the
    /// vector-similarity edge from Phase 1. No action — originals stay.
    KeepSeparate { reasoning: String },
    /// LLM decided the cluster's memories conflict (e.g., "Mom's birthday
    /// is June 15" vs "Mom's birthday is July 15"). Per BRD §5.6 line 944,
    /// **do not auto-resolve** — the orchestrator emits a `ConflictReview`
    /// row in `ConsolidationReport.conflicts_for_user_review` instead.
    Contradiction { reasoning: String },
}

/// JSON schema (string form) the LLM is constrained to emit. GBNF-compiled
/// in `Phi4MiniProvider`; ignored by `MockLlmProvider`. Locked at T0.2.3
/// iteration 2 §Q2 (α single-decision-per-cluster).
const MERGE_DECISION_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "decision": {
            "type": "string",
            "enum": ["merge", "keep_separate", "contradiction"]
        },
        "merged_text": { "type": "string" },
        "reasoning": { "type": "string" }
    },
    "required": ["decision", "reasoning"],
    "additionalProperties": false
}"#;

/// System prompt for the N-ary cluster merge-decision call. Sent via
/// [`CompletionParams::system_prompt`] per ADR-044 Amendment 1 (rides with
/// T0.2.3 commit 1) — overrides `Phi4MiniProvider`'s default merge-classifier
/// system message which is pairwise-shaped and unfit for N-ary input.
const MERGE_DECISION_SYSTEM_PROMPT: &str =
    "You are a JSON-only memory consolidator. You receive a cluster of N memories \
     that a vector-similarity pass flagged as potential duplicates. Decide whether \
     to (a) merge them into one consolidated memory, (b) keep them separate \
     (vector similarity was a false positive), or (c) flag a contradiction (the \
     memories describe the same entity with conflicting facts — do NOT \
     auto-resolve, the user reviews). Respond with strict JSON matching the \
     schema. For merge decisions, set merged_text to the consolidated content; \
     for keep_separate and contradiction, omit merged_text.";

/// Phase 2 primitive — ask the LLM to classify a cluster.
///
/// **Inputs:**
/// - `cluster`: the cluster from Phase 1 ([`find_candidate_clusters`] output).
///   Members are read by [`MemoryId`]; content is hydrated from storage.
/// - `llm`: any [`LlmProvider`]. Production uses `Phi4MiniProvider`; tests
///   use `MockLlmProvider`.
/// - `storage`: source of memory-row content. The cluster carries only
///   `MemoryId`s per ADR-045 §a; this fn calls
///   [`StorageBackend::list_memories`] with a default filter and matches
///   by `id` membership.
///
/// **Output:** [`MergeOutcome`] — one of `Merge`/`KeepSeparate`/`Contradiction`.
///
/// **Errors:**
/// - [`VaultError::Storage`] propagated from `storage.list_memories`.
/// - [`VaultError::Llm`] propagated from the LLM call.
/// - [`VaultError::Llm`] wrapping a parse error if the LLM returns JSON the
///   schema accepts but our enum can't deserialize (shouldn't happen under
///   GBNF, but defense in depth per the [`LlmProvider`] trait docs).
/// - [`VaultError::Storage`] if a cluster member ID can't be found in storage
///   (would indicate a TOCTOU race between Phase 1 and Phase 2 — concurrent
///   delete; surfaced rather than silently dropped).
///
/// [`find_candidate_clusters`]: crate::find_candidate_clusters
#[instrument(skip(cluster, llm, storage), fields(cluster_id = cluster.id, cluster_size = cluster.size()))]
pub async fn decide_merge(
    cluster: &Cluster,
    llm: &dyn LlmProvider,
    storage: &StorageBackend,
) -> VaultResult<MergeOutcome> {
    // Hydrate cluster member content from storage. Cluster carries IDs only;
    // we need the .content strings to build the LLM prompt. Read all memories
    // in the workspace (filter=default) then filter by membership — at V0.2
    // scale (BRD §6.1 100-1000 memories) this is sub-millisecond; for V0.3+
    // a `MetadataStore::get_memories_by_ids` batch read would be cleaner.
    let all_memories = storage.list_memories(MemoryFilter::default(), None).await?;
    let member_set: std::collections::HashSet<MemoryId> =
        cluster.member_row_ids.iter().copied().collect();
    let mut hydrated: Vec<&vault_core::Memory> = all_memories
        .iter()
        .filter(|m| member_set.contains(&m.id))
        .collect();
    // Preserve cluster's locked-ascending ordering for deterministic prompt
    // construction (cluster.member_row_ids is sorted ascending per ADR-045 §a).
    hydrated.sort_by_key(|m| m.id);

    if hydrated.len() != cluster.size() {
        return Err(VaultError::Storage(format!(
            "Phase 2 cluster hydration mismatch: cluster has {} members, storage returned {} \
             (concurrent delete or replication lag suspected)",
            cluster.size(),
            hydrated.len()
        )));
    }

    // Build the JSON prompt body. Schema locked iteration 2 §Q1 (α structured).
    #[derive(Serialize)]
    struct PromptMemory<'a> {
        id: String,
        content: &'a str,
    }
    #[derive(Serialize)]
    struct PromptBody<'a> {
        memories: Vec<PromptMemory<'a>>,
    }
    let body = PromptBody {
        memories: hydrated
            .iter()
            .map(|m| PromptMemory {
                id: m.id.to_string(),
                content: &m.content,
            })
            .collect(),
    };
    let user_prompt = serde_json::to_string(&body)
        .map_err(|e| VaultError::Llm(format!("Phase 2 prompt JSON serialisation failed: {e}")))?;

    // Call the LLM with the N-ary merge-decision system prompt override.
    // ADR-044 Amendment 1: `system_prompt: Some(...)` swaps the provider's
    // default merge-classifier text for the N-ary cluster shape.
    let params = CompletionParams {
        system_prompt: Some(MERGE_DECISION_SYSTEM_PROMPT.to_string()),
        ..CompletionParams::default()
    };
    let raw = llm
        .complete_json(&user_prompt, MERGE_DECISION_SCHEMA, &params)
        .await
        .map_err(|e| VaultError::Llm(format!("Phase 2 LLM call failed: {e}")))?;

    // Parse the response. GBNF guarantees schema validity but enum-variant
    // mismatch is a real risk — defense in depth per LlmProvider trait
    // docs ("treats parse-failure as a hard error, NOT a retry case").
    let outcome: MergeOutcome = serde_json::from_str(&raw).map_err(|e| {
        warn!(raw_response = %raw, "Phase 2 LLM returned malformed JSON");
        VaultError::Llm(format!(
            "Phase 2 LLM response failed to parse as MergeOutcome: {e} (raw: {raw})"
        ))
    })?;

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use vault_core::{Boundary, Memory, MemoryType, NewMemory};
    use vault_embedding::EMBEDDING_DIM;
    use vault_llm::MockLlmProvider;
    use vault_storage::{RetryWorker, SqlCipherKey, StepResult};

    /// Test-only at-rest key. Matches the cross-crate convention from
    /// `vault-storage/tests/migration_v0_1_to_sealed.rs:96`.
    const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];

    async fn open_test_storage() -> (StorageBackend, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = SqlCipherKey::new("phase2-decide-merge-test");
        let storage = StorageBackend::open_with_at_rest_key(
            &dir.path().join("metadata.db"),
            &dir.path().join("vectors"),
            &dir.path().join("graph.duckdb"),
            key,
            EMBEDDING_DIM,
            &TEST_AT_REST_KEY,
        )
        .await
        .expect("open StorageBackend");
        (storage, dir)
    }

    fn make_memory(content: &str, boundary: &Boundary) -> Memory {
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

    /// Insert memories via cascading write then drain so list_memories sees
    /// the committed SQLite-side rows. Phase 2 doesn't need vector-side
    /// readability — only metadata.content — but the cascade path is the
    /// realistic insertion shape.
    async fn insert_and_drain(storage: &StorageBackend, memories: &[Memory]) -> Vec<MemoryId> {
        let mut ids = Vec::new();
        let embedding = vec![1.0_f32 / (EMBEDDING_DIM as f32).sqrt(); EMBEDDING_DIM];
        for memory in memories {
            ids.push(memory.id);
            storage
                .write_memory(memory, &embedding)
                .await
                .expect("write_memory");
        }
        let mut worker = RetryWorker::new(storage.clone());
        let drain_at = Utc::now() + chrono::Duration::seconds(60);
        for _ in 0..(memories.len() * 2 + 10) {
            match worker.step_at(drain_at).await.expect("worker step") {
                StepResult::Idle => break,
                StepResult::SucceededEntry { .. } => continue,
                other => panic!("unexpected worker outcome: {other:?}"),
            }
        }
        ids
    }

    fn cluster_of(id: u32, member_ids: Vec<MemoryId>) -> Cluster {
        let mut sorted = member_ids;
        sorted.sort();
        Cluster {
            id,
            member_row_ids: sorted,
        }
    }

    // ─── floor 1: merge outcome round-trip ────────────────────────────────

    #[tokio::test]
    async fn decide_merge_returns_merge_outcome_on_merge_response() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Buy milk today", &boundary),
            make_memory("Buy milk later", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new(
            "test-mock",
            r#"{"decision":"merge","merged_text":"Buy milk","reasoning":"identical intent"}"#,
        );
        let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
        match outcome {
            MergeOutcome::Merge {
                merged_text,
                reasoning,
            } => {
                assert_eq!(merged_text, "Buy milk");
                assert_eq!(reasoning, "identical intent");
            }
            other => panic!("expected Merge, got {other:?}"),
        }
        assert_eq!(mock.call_count(), 1, "exactly one LLM call per cluster");
    }

    // ─── floor 2: keep_separate outcome round-trip ────────────────────────

    #[tokio::test]
    async fn decide_merge_returns_keep_separate_on_keep_response() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Buy milk today", &boundary),
            make_memory("Dentist appointment Tuesday", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new(
            "test-mock",
            r#"{"decision":"keep_separate","reasoning":"unrelated topics"}"#,
        );
        let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
        assert!(matches!(outcome, MergeOutcome::KeepSeparate { .. }));
    }

    // ─── floor 3: contradiction outcome round-trip ────────────────────────

    #[tokio::test]
    async fn decide_merge_returns_contradiction_on_contradiction_response() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Mom's birthday is June 15", &boundary),
            make_memory("Mom's birthday is July 15", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new(
            "test-mock",
            r#"{"decision":"contradiction","reasoning":"same person, conflicting dates"}"#,
        );
        let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
        assert!(matches!(outcome, MergeOutcome::Contradiction { .. }));
    }

    // ─── floor 4: malformed JSON surfaces as VaultError::Llm ──────────────

    #[tokio::test]
    async fn decide_merge_surfaces_malformed_json_as_llm_error() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        let memories = vec![
            make_memory("Buy milk today", &boundary),
            make_memory("Buy milk later", &boundary),
        ];
        let ids = insert_and_drain(&storage, &memories).await;
        let cluster = cluster_of(0, ids);

        let mock = MockLlmProvider::new("test-mock", "not even close to JSON");
        let err = decide_merge(&cluster, &mock, &storage).await.unwrap_err();
        assert!(
            matches!(err, VaultError::Llm(_)),
            "expected VaultError::Llm, got {err:?}"
        );
    }

    // ─── floor 5: N-boundary cases (N=2 / N=5 / N=10) dispatch one call ──

    #[tokio::test]
    async fn decide_merge_handles_cluster_size_n2_n5_n10_with_one_llm_call() {
        let (storage, _dir) = open_test_storage().await;
        let boundary = Boundary::new("test").unwrap();
        // Build 10 memories so we can slice into N=2, N=5, N=10 clusters.
        let memories: Vec<Memory> = (0..10)
            .map(|i| make_memory(&format!("Buy milk variant {i}"), &boundary))
            .collect();
        let ids = insert_and_drain(&storage, &memories).await;

        for n in [2usize, 5, 10] {
            let cluster = cluster_of(0, ids[..n].to_vec());
            let mock = MockLlmProvider::new(
                "test-mock",
                r#"{"decision":"merge","merged_text":"Buy milk","reasoning":"N-ary cluster"}"#,
            );
            let outcome = decide_merge(&cluster, &mock, &storage).await.unwrap();
            assert!(
                matches!(outcome, MergeOutcome::Merge { .. }),
                "N={n} cluster should merge"
            );
            assert_eq!(
                mock.call_count(),
                1,
                "N={n} cluster must produce exactly one LLM call (single-decision shape per Q2)"
            );
        }
    }
}
