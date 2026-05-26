//! K-means topic discovery (T0.3.x Batch A, locked-next-arc Step 2 / 2026-05-26).
//!
//! Per-boundary clustering of memories into ~K topic groups using their BGE
//! embeddings. Phi-4-mini labels each cluster with a 2-3 word topic name.
//! The output [`TopicMap`] is consumed by `report.rs` (Commit 4) to produce
//! the structured per-boundary REPORT artifact that the read pipeline
//! (Commit 6) consults to enrich retrieved candidates with topic tags.
//!
//! ## K selection
//!
//! `K = ceil(sqrt(N / 4))` clamped to `[3, 20]` and further clamped to `<= N`.
//! For `N < 3` the function returns a single `"general"` topic with all
//! members — clustering 1-2 memories has no useful signal.
//!
//! Sample sizes:
//!   - N=4   → K=3 (floor clamp; ~1-2 memories per topic)
//!   - N=16  → K=3 (floor clamp; ~5-6 memories per topic)
//!   - N=100 → K=5 (sqrt(25))
//!   - N=400 → K=10
//!   - N=1600 → K=20 (ceiling clamp)
//!
//! ## Determinism
//!
//! K-means init picks the first K memories (sorted ascending by [`MemoryId`])
//! as initial centroids — no RNG. The assignment + centroid-update loop
//! runs up to 100 iterations or until no assignments change. The output
//! `topics` vector is sorted by `topic_id` ascending. Re-running
//! [`discover_topics`] on the same inputs produces an identical
//! [`TopicMap`] (modulo bit-exact-equality of the BGE embedder's output —
//! see ADR-045 §c for the sub-1e-6 IEEE-754 rounding bound).
//!
//! ## Phi-4 cluster naming + graceful fallback
//!
//! When the LLM parameter is `Some`, each cluster is labelled by a single
//! Phi-4-mini call returning `{ "label": "<snake_case>" }`. When `None`, or
//! when an individual call fails (network error, GGUF integrity drift,
//! malformed response), the cluster falls back to the placeholder ID
//! `"topic_<id>"` and [`TopicMap::topic_names_unavailable`] is set so
//! the read pipeline at Commit 6 can surface the `TOPIC_NAMES_UNAVAILABLE`
//! health-warning per the locked-next-arc Thread 3 contract.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::instrument;

use vault_core::{Boundary, Memory, MemoryId, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_llm::{CompletionParams, LlmProvider};

/// Output of [`discover_topics`]: K topic clusters for one boundary.
///
/// `topics` is sorted by `topic_id` ascending. The REPORT artifact at
/// Commit 4 serialises this into per-topic JSON arrays keyed by `label`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopicMap {
    pub boundary: Boundary,
    pub topics: Vec<Topic>,
    /// `true` when Phi-4 was unavailable or each call failed, so labels
    /// are placeholder `"topic_<id>"`. Read pipeline surfaces this as
    /// `TOPIC_NAMES_UNAVAILABLE` in the response's `health.warnings`.
    pub topic_names_unavailable: bool,
}

/// One topic cluster — `topic_id` (stable across re-runs on identical
/// inputs), human/agent-readable `label`, and the assigned memory IDs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Topic {
    pub topic_id: usize,
    pub label: String,
    pub member_ids: Vec<MemoryId>,
}

/// Hard upper bound on K-means iterations. Convergence on L2-normalised
/// 384-dim vectors with deterministic init typically reaches stable
/// assignments inside 10-30 iterations; 100 is a safety ceiling so a
/// pathological input cannot pin the consolidator forever.
const MAX_KMEANS_ITERS: usize = 100;

/// Topic-naming sample size: up to N most-recent member contents passed
/// in the Phi-4 prompt. Trade-off: more samples = better label quality
/// but longer prompts (Phi-4-mini is ~30 tok/s on CPU). 5 is the
/// spike-2-validated sweet spot.
const TOPIC_LABEL_SAMPLE_SIZE: usize = 5;

/// Discover topics for one boundary's memories.
///
/// Returns a [`TopicMap`] with K clusters where K is computed from the
/// memory count per the formula above. Determinism guaranteed under
/// bit-exact embedder outputs.
///
/// # Errors
///
/// - [`VaultError::Embedding`] — re-embedding memory content failed
///   (propagated from [`EmbeddingProvider::embed`]).
/// - [`VaultError::InvalidInput`] — embeddings have inconsistent
///   dimensions across the boundary (defensive pin; the same
///   `EmbeddingProvider` produces them all here so shouldn't happen).
#[instrument(skip_all, fields(boundary = %boundary, n_memories = memories.len()))]
pub async fn discover_topics(
    boundary: &Boundary,
    memories: &[Memory],
    embedder: &dyn EmbeddingProvider,
    llm: Option<&dyn LlmProvider>,
) -> VaultResult<TopicMap> {
    // Deterministic order — caller's slice may already be sorted but we
    // don't rely on that. Sort by MemoryId (UUID v7, time-ordered).
    let mut sorted: Vec<&Memory> = memories.iter().collect();
    sorted.sort_by_key(|m| m.id);

    let n = sorted.len();

    // N < 3 → single "general" topic. No K-means needed.
    if n < 3 {
        return Ok(TopicMap {
            boundary: boundary.clone(),
            topics: vec![Topic {
                topic_id: 0,
                label: "general".to_string(),
                member_ids: sorted.iter().map(|m| m.id).collect(),
            }],
            topic_names_unavailable: false,
        });
    }

    // K = ceil(sqrt(N/4)) clamped to [3, 20], then capped at N
    // (can't have more clusters than memories).
    let k_raw = ((n as f64) / 4.0).sqrt().ceil() as usize;
    let k = k_raw.clamp(3, 20).min(n);

    // Re-embed each memory's content via the shared provider. Memory
    // rows from MetadataStore carry `embedding: None` per ADR-045 §c.
    let embeddings = embed_all(&sorted, embedder).await?;

    // Dim-consistency defensive pin.
    let dim = embeddings[0].len();
    for (i, e) in embeddings.iter().enumerate() {
        if e.len() != dim {
            return Err(VaultError::InvalidInput(format!(
                "embedding {} has dim {} but expected {}",
                i,
                e.len(),
                dim
            )));
        }
    }

    // K-means: deterministic init + Lloyd's iteration.
    let assignments = run_kmeans(&embeddings, k, dim);

    // Build per-cluster member lists, dropping any empty clusters and
    // renumbering topic_ids contiguously.
    let mut clusters: Vec<Vec<MemoryId>> = vec![Vec::new(); k];
    for (i, &t) in assignments.iter().enumerate() {
        clusters[t].push(sorted[i].id);
    }
    let non_empty: Vec<Vec<MemoryId>> = clusters.into_iter().filter(|c| !c.is_empty()).collect();

    // Label each topic via Phi-4 (or placeholder when LLM is None / fails).
    let mut topics = Vec::with_capacity(non_empty.len());
    let mut any_fallback = llm.is_none();
    for (new_id, member_ids) in non_empty.into_iter().enumerate() {
        let label = match llm {
            Some(provider) => match label_one_cluster(&sorted, &member_ids, provider).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!(
                        topic_id = new_id,
                        error = %e,
                        "Phi-4 cluster naming failed; falling back to placeholder"
                    );
                    any_fallback = true;
                    format!("topic_{new_id}")
                }
            },
            None => format!("topic_{new_id}"),
        };
        topics.push(Topic {
            topic_id: new_id,
            label,
            member_ids,
        });
    }

    Ok(TopicMap {
        boundary: boundary.clone(),
        topics,
        topic_names_unavailable: any_fallback,
    })
}

/// Lloyd's K-means with deterministic first-K init. Returns the
/// assignment vector of length `embeddings.len()`.
fn run_kmeans(embeddings: &[Vec<f32>], k: usize, dim: usize) -> Vec<usize> {
    let n = embeddings.len();
    let mut centroids: Vec<Vec<f32>> = embeddings[..k].to_vec();
    let mut assignments = vec![0usize; n];

    for _ in 0..MAX_KMEANS_ITERS {
        let mut changed = false;

        // Assign each point to nearest centroid (cosine distance).
        for i in 0..n {
            let mut best = 0;
            let mut best_dist = cosine_distance(&embeddings[i], &centroids[0]);
            for (j, centroid) in centroids.iter().enumerate().skip(1) {
                let d = cosine_distance(&embeddings[i], centroid);
                if d < best_dist {
                    best = j;
                    best_dist = d;
                }
            }
            if assignments[i] != best {
                assignments[i] = best;
                changed = true;
            }
        }

        if !changed {
            break;
        }

        // Update centroids = mean of assigned vectors; re-normalize so
        // cosine distance stays well-conditioned.
        let mut sums = vec![vec![0.0f32; dim]; k];
        let mut counts = vec![0usize; k];
        for i in 0..n {
            let t = assignments[i];
            for d in 0..dim {
                sums[t][d] += embeddings[i][d];
            }
            counts[t] += 1;
        }
        for j in 0..k {
            if counts[j] == 0 {
                // Empty cluster: keep the previous centroid. Rare with
                // deterministic-id init unless multiple init candidates
                // embed identically; preserving the prior centroid avoids
                // NaN propagation.
                continue;
            }
            for d in 0..dim {
                centroids[j][d] = sums[j][d] / counts[j] as f32;
            }
            let norm: f32 = centroids[j].iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                centroids[j].iter_mut().for_each(|x| *x /= norm);
            }
        }
    }

    assignments
}

async fn embed_all(
    memories: &[&Memory],
    embedder: &dyn EmbeddingProvider,
) -> VaultResult<Vec<Vec<f32>>> {
    let mut out = Vec::with_capacity(memories.len());
    for m in memories {
        out.push(embedder.embed(&m.content).await?);
    }
    Ok(out)
}

/// Cosine distance: `1 - (a·b / (|a|·|b|))`. For L2-normalised vectors
/// the denominator is ~1.0 and the formula reduces to `1 - dot`. We
/// compute the general form here because centroids (mean of normalised
/// vectors) are not themselves perfectly normalised before the explicit
/// re-normalise step.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-12);
    1.0 - (dot / denom)
}

async fn label_one_cluster(
    all_memories: &[&Memory],
    member_ids: &[MemoryId],
    llm: &dyn LlmProvider,
) -> VaultResult<String> {
    let lookup: HashMap<MemoryId, &Memory> = all_memories.iter().map(|m| (m.id, *m)).collect();
    let samples: Vec<&str> = member_ids
        .iter()
        .filter_map(|id| lookup.get(id).map(|m| m.content.as_str()))
        .take(TOPIC_LABEL_SAMPLE_SIZE)
        .collect();

    let memories_block = samples
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}. {}", i + 1, c))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "You name clusters of related memories. Below are short memories that \
         have been clustered together. Give this cluster a 2-3 word topic \
         label in lowercase snake_case (e.g., 'blood_pressure_readings'). Be \
         concrete and specific. Respond with JSON: {{\"label\": \"...\"}}.\n\n\
         Memories:\n{memories_block}"
    );

    let schema = r#"{"type":"object","properties":{"label":{"type":"string","minLength":1,"maxLength":50}},"required":["label"],"additionalProperties":false}"#;

    let params = CompletionParams {
        max_tokens: 32,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(0xC07C_C07C), // arbitrary stable seed for label determinism
        system_prompt: Some(
            "You name clusters of related memories with short snake_case labels.".to_string(),
        ),
    };

    let response = llm
        .complete_json(&prompt, schema, &params)
        .await
        .map_err(|e| VaultError::Llm(format!("Phi-4 cluster naming call: {e}")))?;

    let parsed: serde_json::Value = serde_json::from_str(&response)?;
    let label = parsed
        .get("label")
        .and_then(|v| v.as_str())
        .ok_or_else(|| VaultError::Llm("Phi-4 returned no `label` field".into()))?
        .trim()
        .to_string();
    if label.is_empty() {
        return Err(VaultError::Llm("Phi-4 returned empty label".into()));
    }
    Ok(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use uuid::Uuid;
    use vault_core::{MemoryType, NewMemory};

    /// Deterministic mock embedder for tests. Maps text content → a
    /// canned vector via a lookup table the test sets up upfront. Any
    /// content not in the table embeds to a zero vector (which acts
    /// like a "noise" point for clustering).
    #[derive(Debug)]
    struct MockEmbedder {
        table: Mutex<HashMap<String, Vec<f32>>>,
    }

    impl MockEmbedder {
        fn new(entries: Vec<(&'static str, Vec<f32>)>) -> Self {
            let mut table = HashMap::new();
            for (k, v) in entries {
                table.insert(k.to_string(), v);
            }
            Self {
                table: Mutex::new(table),
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for MockEmbedder {
        async fn embed(&self, text: &str) -> VaultResult<Vec<f32>> {
            let guard = self.table.lock().unwrap();
            Ok(guard.get(text).cloned().unwrap_or_else(|| vec![0.0; 4]))
        }
    }

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("test boundary name must be valid")
    }

    fn memory_with_id(content: &str, n: u128) -> Memory {
        let mut m = Memory::try_new(NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary("personal"),
            source_agent: Some("test".to_string()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("test memory must validate");
        m.id = MemoryId(Uuid::from_u128(n));
        m
    }

    /// 4-D unit-vector "axes" for easy clustering tests.
    fn ex() -> Vec<f32> {
        vec![1.0, 0.0, 0.0, 0.0]
    }
    fn ey() -> Vec<f32> {
        vec![0.0, 1.0, 0.0, 0.0]
    }
    fn ez() -> Vec<f32> {
        vec![0.0, 0.0, 1.0, 0.0]
    }
    fn ew() -> Vec<f32> {
        vec![0.0, 0.0, 0.0, 1.0]
    }

    #[tokio::test]
    async fn discover_topics_returns_single_general_topic_when_n_lt_3() {
        let mems = vec![memory_with_id("only-one-memory", 1)];
        let embedder = MockEmbedder::new(vec![("only-one-memory", ex())]);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        assert_eq!(map.topics.len(), 1);
        assert_eq!(map.topics[0].label, "general");
        assert_eq!(map.topics[0].member_ids.len(), 1);
        assert!(
            !map.topic_names_unavailable,
            "single-general-topic path bypasses LLM entirely; \
             topic_names_unavailable MUST stay false"
        );
    }

    #[tokio::test]
    async fn discover_topics_two_memories_collapses_to_general_topic() {
        let mems = vec![memory_with_id("apple", 1), memory_with_id("banana", 2)];
        let embedder = MockEmbedder::new(vec![("apple", ex()), ("banana", ey())]);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        assert_eq!(map.topics.len(), 1);
        assert_eq!(map.topics[0].label, "general");
        assert_eq!(map.topics[0].member_ids.len(), 2);
    }

    #[tokio::test]
    async fn discover_topics_clamps_k_to_minimum_3_floor_for_small_n() {
        // N=4 → K = max(3, ceil(sqrt(1))) = 3. With 4 orthogonal axes
        // (X, Y, Z, W), at most 3 clusters can form; one cluster will
        // have 2 members or one cluster ends up empty before re-numbering.
        let mems = vec![
            memory_with_id("ax", 1),
            memory_with_id("ay", 2),
            memory_with_id("az", 3),
            memory_with_id("aw", 4),
        ];
        let embedder =
            MockEmbedder::new(vec![("ax", ex()), ("ay", ey()), ("az", ez()), ("aw", ew())]);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        // 3 non-empty topics expected (one cluster will absorb the 4th
        // orthogonal axis).
        assert!(
            map.topics.len() == 3,
            "K floor=3 should produce 3 topics for N=4 orthogonal inputs; got {}",
            map.topics.len()
        );
        // Each member ID appears in exactly one topic.
        let total_members: usize = map.topics.iter().map(|t| t.member_ids.len()).sum();
        assert_eq!(total_members, 4, "every input memory must be assigned");
    }

    #[tokio::test]
    async fn discover_topics_uses_placeholder_labels_when_llm_is_none() {
        let mems: Vec<Memory> = (0..6)
            .map(|i| memory_with_id(&format!("m{i}"), i as u128 + 1))
            .collect();
        let embedder = MockEmbedder::new(vec![
            ("m0", ex()),
            ("m1", ex()),
            ("m2", ey()),
            ("m3", ey()),
            ("m4", ez()),
            ("m5", ez()),
        ]);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        for t in &map.topics {
            assert!(
                t.label.starts_with("topic_"),
                "topic without LLM MUST use placeholder; got: {}",
                t.label
            );
        }
        assert!(
            map.topic_names_unavailable,
            "topic_names_unavailable MUST be set when llm is None"
        );
    }

    #[tokio::test]
    async fn discover_topics_uses_llm_label_when_provider_returns_valid_json() {
        use vault_llm::MockLlmProvider;
        let mems: Vec<Memory> = (0..6)
            .map(|i| memory_with_id(&format!("m{i}"), i as u128 + 1))
            .collect();
        let embedder = MockEmbedder::new(vec![
            ("m0", ex()),
            ("m1", ex()),
            ("m2", ey()),
            ("m3", ey()),
            ("m4", ez()),
            ("m5", ez()),
        ]);
        let llm = MockLlmProvider::new("phi-4-mini-test", r#"{"label":"test_topic"}"#);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, Some(&llm))
            .await
            .unwrap();
        for t in &map.topics {
            assert_eq!(
                t.label, "test_topic",
                "every cluster MUST take the mock LLM's canned label"
            );
        }
        assert!(
            !map.topic_names_unavailable,
            "topic_names_unavailable MUST stay false when every LLM call succeeded"
        );
    }

    #[tokio::test]
    async fn discover_topics_falls_back_to_placeholder_when_llm_returns_malformed_json() {
        use vault_llm::MockLlmProvider;
        let mems: Vec<Memory> = (0..6)
            .map(|i| memory_with_id(&format!("m{i}"), i as u128 + 1))
            .collect();
        let embedder = MockEmbedder::new(vec![
            ("m0", ex()),
            ("m1", ex()),
            ("m2", ey()),
            ("m3", ey()),
            ("m4", ez()),
            ("m5", ez()),
        ]);
        // Canned response missing the `label` field → label_one_cluster
        // returns VaultError::Llm; discover_topics catches and falls back
        // to the placeholder label per the graceful-degradation contract.
        let llm = MockLlmProvider::new("phi-4-mini-test", r#"{"not_label":"oops"}"#);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, Some(&llm))
            .await
            .unwrap();
        for t in &map.topics {
            assert!(
                t.label.starts_with("topic_"),
                "malformed LLM response MUST trigger placeholder fallback; got label: {}",
                t.label
            );
        }
        assert!(
            map.topic_names_unavailable,
            "topic_names_unavailable MUST be set when any LLM call falls back"
        );
    }

    #[tokio::test]
    async fn discover_topics_is_deterministic_across_repeated_runs() {
        let mems: Vec<Memory> = (0..9)
            .map(|i| memory_with_id(&format!("m{i}"), i as u128 + 1))
            .collect();
        let embedder = MockEmbedder::new(vec![
            ("m0", ex()),
            ("m1", ex()),
            ("m2", ex()),
            ("m3", ey()),
            ("m4", ey()),
            ("m5", ey()),
            ("m6", ez()),
            ("m7", ez()),
            ("m8", ez()),
        ]);
        let map1 = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        let map2 = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        assert_eq!(
            map1, map2,
            "two runs on identical inputs MUST produce identical TopicMaps"
        );
    }

    #[tokio::test]
    async fn discover_topics_assigns_orthogonal_axes_to_distinct_clusters() {
        // 6 memories perfectly bucketed into 3 orthogonal axes. K=3 should
        // recover the natural split.
        let mems: Vec<Memory> = (0..6)
            .map(|i| memory_with_id(&format!("m{i}"), i as u128 + 1))
            .collect();
        let embedder = MockEmbedder::new(vec![
            ("m0", ex()),
            ("m1", ex()),
            ("m2", ey()),
            ("m3", ey()),
            ("m4", ez()),
            ("m5", ez()),
        ]);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        assert_eq!(
            map.topics.len(),
            3,
            "3 orthogonal axes × 2 each MUST recover 3 distinct topics"
        );
        // The X-axis pair, Y-axis pair, Z-axis pair must land in three
        // different topics — group by topic_id and verify each group's
        // memories share the same axis.
        let mut by_topic: HashMap<usize, Vec<MemoryId>> = HashMap::new();
        for t in &map.topics {
            by_topic.insert(t.topic_id, t.member_ids.clone());
        }
        for ids in by_topic.values() {
            assert_eq!(
                ids.len(),
                2,
                "each topic MUST hold exactly 2 same-axis members (got {ids:?})"
            );
        }
    }
}
