//! Connected-components topic discovery (T0.3.x Batch A, locked-next-arc
//! Step 2 / 2026-05-26; reworked from K-means to connected-components at
//! ADR-068, 2026-06-04).
//!
//! Per-boundary grouping of memories into topics using their BGE embeddings.
//! Phi-4-mini labels each group with a 2-3 word topic name. The output
//! [`TopicMap`] is consumed by `report.rs` (Commit 4) to produce the
//! structured per-boundary REPORT artifact that the read pipeline (Commit 6)
//! consults to enrich retrieved candidates with topic tags.
//!
//! ## Why connected-components, not K-means (ADR-068)
//!
//! The original K-means forced every boundary into a *fixed* number of
//! buckets (`K = ceil(sqrt(N/4))` clamped to `[3, 20]`). On a small, diverse
//! vault that jams unrelated facts together and mislabels them — the §7 live
//! dogfood (2026-06-02) showed a job fact and a dog fact both bucketed into
//! `vehicle_transitions`. K-means *cannot* leave a fact ungrouped; it fills K
//! clusters whether or not the facts belong together. (The same false-premise
//! that retired K-means from contradiction detection at ADR-065 — see
//! [`crate::phases::candidates`].)
//!
//! Topic membership is a *similarity* question, not a partitioning one: two
//! facts share a topic only when they are actually close in embedding space.
//! So we group by **connected components** over the cosine-similarity graph:
//! for each fact take its top-[`TOPIC_NN_TOP_K`] neighbors at or above
//! [`TOPIC_NN_SIMILARITY_FLOOR`], keep an edge only where the two facts are
//! **mutual** near-neighbours (each in the other's top-K — ADR-085), and run
//! the union-find transitive closure
//! ([`crate::phases::cluster::union_find_components`]). Genuinely-related facts
//! cluster; unrelated facts stay as their own singleton topic with an honest
//! per-fact label. A singleton with a correct label beats a forced group with a
//! wrong one ([[correctness-is-the-product]]).
//!
//! ## Why edges must be *mutual* (ADR-085 — Finding F, 2026-06-24)
//!
//! Connected components take the *transitive closure* of the edge set, so a
//! single one-way link (`j` was `i`'s 5th-nearest just above the floor, but `i`
//! is nowhere near `j`'s top-K) silently welds two unrelated topics together.
//! At 1k facts those weak bridge edges are everywhere: the 2026-06-21 scale run
//! collapsed **629/651 facts (~97%) into one catch-all topic**. Requiring
//! reciprocity (mutual k-NN) drops the bridges — a fact only joins a topic when
//! its neighbours agree — fixing the giant-component collapse. Display-only:
//! never affects which facts surface at read time
//! ([[project_report_topics_shown_to_agent_but_dont_gate_recall]]).
//!
//! ## Threshold provenance (provisional — calibrate as the vault fills)
//!
//! [`TOPIC_NN_SIMILARITY_FLOOR`] = **0.70**, reusing the measured basis of the
//! contradiction floor ([`crate::phases::candidates::CONTRADICTION_NN_SIMILARITY_FLOOR`]):
//! on the real bge-small dogfood embeddings the unrelated-fact noise band tops
//! out at **0.634** and genuine same-subject pairs sit at **0.823+**, so 0.70
//! sits in the clean gap — above noise, below real relationships. This is
//! deliberately **conservative**: on today's small vaults it yields mostly
//! singletons (honest), and as the vault fills with multiple facts per subject
//! they will cluster. Topic breadth is fuzzier than contradiction detection,
//! so this floor is **provisional** and should be re-measured on real fill
//! data (the read_quality / calibration harness is the tool). It is
//! display-only — it never affects whether a fact surfaces at read time.
//!
//! ## Determinism
//!
//! Memories are sorted ascending by [`MemoryId`] before embedding; edges are
//! built over that fixed order with deterministic top-K tie-breaking (ascending
//! index). `union_find_components` is deterministic (BTreeMap/BTreeSet backed),
//! and topic IDs are assigned by ascending smallest-member order. Re-running
//! [`discover_topics`] on the same inputs produces an identical [`TopicMap`]
//! (modulo bit-exact-equality of the BGE embedder's output — see ADR-045 §c
//! for the sub-1e-6 IEEE-754 rounding bound).
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

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use tracing::instrument;

use vault_core::{Boundary, Memory, MemoryId, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_llm::{CompletionParams, LlmProvider};

use crate::phases::cluster::union_find_components;

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

/// Per-fact neighbor count for topic edge-building. Each fact contributes at
/// most its top-K most-similar other facts as topic edges; the union-find
/// transitive closure then merges chains, so a broad topic forms even though
/// each fact only edges to a few neighbors. 5 mirrors the BRD §5.6 clustering
/// `TOP_K_NEIGHBORS` and gives a large topic enough internal connectivity.
const TOPIC_NN_TOP_K: usize = 5;

/// Minimum cosine similarity for two facts to share a topic edge.
///
/// **0.70 is provisional, on the measured basis of the contradiction floor**
/// ([`crate::phases::candidates::CONTRADICTION_NN_SIMILARITY_FLOOR`]): the
/// bge-small dogfood noise band tops out at 0.634 and genuine same-subject
/// pairs sit at 0.823+, so 0.70 sits in the clean gap (above noise, below real
/// relationships). Conservative by design — see the module "Threshold
/// provenance" note. Display-only; never gates read-time recall. Re-measure on
/// real fill data as the vault grows.
const TOPIC_NN_SIMILARITY_FLOOR: f32 = 0.70;

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

    // N < 3 → single "general" topic. No clustering needed (0-2 memories
    // carry no useful grouping signal); preserves the historical small-vault
    // shape the REPORT consumer expects.
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

    // Connected-components over the cosine-similarity graph (ADR-068). Each
    // fact edges to its top-K neighbors at/above the floor; the union-find
    // transitive closure groups them. `union_find_components` returns a group
    // for EVERY node, so singletons (a fact with no qualifying neighbor) are
    // kept as their own topic — honest per-fact labelling beats forcing
    // unrelated facts together (the K-means failure ADR-068 fixes). The
    // BTreeMap is keyed by component root, which (union-by-smaller-id) is the
    // smallest member id — so iterating yields components ordered by smallest
    // member ascending, deterministically.
    let node_ids: Vec<MemoryId> = sorted.iter().map(|m| m.id).collect();
    let edges = topic_edges(&embeddings, &node_ids);
    let non_empty: Vec<Vec<MemoryId>> = union_find_components(&node_ids, &edges)
        .into_values()
        .collect();

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

/// Build the undirected topic-edge list as a **reciprocal (mutual) k-NN
/// graph** (ADR-068, mutualised at ADR-085). An edge `(i, j)` exists only when
/// `j` is among `i`'s [`TOPIC_NN_TOP_K`] most-similar facts at/above
/// [`TOPIC_NN_SIMILARITY_FLOOR`] **AND** `i` is among `j`'s — i.e. the two facts
/// each consider the other a near neighbour. `ids[i]` is the [`MemoryId`] for
/// `embeddings[i]` (both in the same sorted order). Edges are canonicalised
/// `(min_id, max_id)` in a [`BTreeSet`] so the output is deterministic.
///
/// ## Why mutual, not one-way (ADR-085 — Finding F fix, 2026-06-24)
///
/// The original ADR-068 graph added an edge whenever `j` was in `i`'s top-K
/// (a *directed* top-K, symmetrised). At small N that is harmless, but the
/// edges feed [`union_find_components`]'s **transitive closure**: one stray
/// one-way link `i → j` (j was i's 5th-nearest at 0.71, but i is nowhere near
/// j's top-K) welds two otherwise-separate topics together, and at 1k facts
/// those weak bridge edges are everywhere. The 2026-06-21 scale run measured
/// the result: **629 of 651 facts (~97%) collapsed into ONE catch-all topic**.
/// Requiring the relationship to be *reciprocal* drops the bridge edges — a
/// fact only joins a topic when its neighbours agree it belongs — so genuine
/// clusters stay intact while unrelated facts stop chaining. This is the
/// standard mutual-kNN fix for giant-component percolation in NN graphs. It is
/// **display-only**: topic membership never gates read-time recall or rank
/// ([[project_report_topics_shown_to_agent_but_dont_gate_recall]]).
///
/// Cost: O(N²) cosine evaluations + O(N·K) reciprocity checks (cheap,
/// in-memory, offline at nightly consolidation). Mirrors
/// [`crate::phases::candidates::nearest_neighbor_candidate_pairs`]; kept
/// separate so topic grouping can carry its own (provisional) floor + K without
/// coupling to the contradiction ship-gate path.
fn topic_edges(embeddings: &[Vec<f32>], ids: &[MemoryId]) -> Vec<(MemoryId, MemoryId)> {
    let n = embeddings.len();

    // Pass 1: each fact's directed top-K neighbour set (indices) at/above the
    // floor. Descending similarity; ascending index breaks ties so the top-K
    // selection is deterministic. `total_cmp` is NaN-safe (no `.unwrap()` on
    // `partial_cmp` — hard rule).
    let top_k: Vec<BTreeSet<usize>> = (0..n)
        .map(|i| {
            let mut sims: Vec<(usize, f32)> = (0..n)
                .filter(|&j| j != i)
                .map(|j| (j, cosine_similarity(&embeddings[i], &embeddings[j])))
                .collect();
            sims.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            sims.into_iter()
                .take(TOPIC_NN_TOP_K)
                .filter(|(_, sim)| *sim >= TOPIC_NN_SIMILARITY_FLOOR)
                .map(|(j, _)| j)
                .collect()
        })
        .collect();

    // Pass 2: keep an edge only when membership is MUTUAL — `j ∈ top_k[i]`
    // AND `i ∈ top_k[j]`. The reciprocity check is what removes the one-way
    // bridge edges (ADR-085). Canonicalised + deduped via the BTreeSet.
    let mut edges: BTreeSet<(MemoryId, MemoryId)> = BTreeSet::new();
    for i in 0..n {
        for &j in &top_k[i] {
            if top_k[j].contains(&i) {
                let (a, b) = (ids[i].min(ids[j]), ids[i].max(ids[j]));
                edges.insert((a, b));
            }
        }
    }
    edges.into_iter().collect()
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

/// Cosine similarity: `a·b / (|a|·|b|)`. For the L2-normalised vectors the
/// [`EmbeddingProvider`] contract guarantees this reduces to a dot product; the
/// explicit norm denominator (clamped away from zero) is belt-and-braces
/// against a degenerate input and matches
/// [`crate::phases::candidates`]'s helper.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = (na.sqrt() * nb.sqrt()).max(1e-12);
    dot / denom
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
    async fn discover_topics_keeps_orthogonal_facts_as_separate_singleton_topics() {
        // ADR-068: N=4 mutually-orthogonal facts (cosine 0 between every pair,
        // all below the 0.70 floor) form NO edges → four singleton components →
        // four topics. The retired K-means forced these into exactly 3 buckets
        // (one cluster absorbed the 4th axis), inventing a false grouping; the
        // connected-components rework leaves unrelated facts apart.
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
        assert_eq!(
            map.topics.len(),
            4,
            "4 orthogonal facts (all cosines below the floor) MUST each be their \
             own singleton topic; got {}",
            map.topics.len()
        );
        for t in &map.topics {
            assert_eq!(
                t.member_ids.len(),
                1,
                "each orthogonal fact is its own topic; got {:?}",
                t.member_ids
            );
        }
        // Each member ID appears in exactly one topic.
        let total_members: usize = map.topics.iter().map(|t| t.member_ids.len()).sum();
        assert_eq!(total_members, 4, "every input memory must be assigned");
    }

    #[tokio::test]
    async fn discover_topics_groups_related_pair_and_isolates_unrelated_facts() {
        // ADR-068 core fix, mirroring the §7 dogfood failure: two facts about
        // the same subject (near-identical embeddings) MUST share one topic,
        // while unrelated facts (orthogonal) MUST NOT be dragged in. K-means
        // jammed a job fact + a dog fact into a "vehicle_transitions" bucket;
        // connected-components keeps them apart.
        let mems = vec![
            memory_with_id("car-a", 1),
            memory_with_id("car-b", 2),
            memory_with_id("job", 3),
            memory_with_id("dog", 4),
        ];
        // car-a / car-b identical (both on the X axis, cosine 1.0) → one topic.
        // job (Y) + dog (Z) orthogonal to everything → two singletons.
        let embedder = MockEmbedder::new(vec![
            ("car-a", ex()),
            ("car-b", ex()),
            ("job", ey()),
            ("dog", ez()),
        ]);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();
        assert_eq!(
            map.topics.len(),
            3,
            "two car facts share ONE topic; job + dog each their own → 3 topics; got {}",
            map.topics.len()
        );
        let mut sizes: Vec<usize> = map.topics.iter().map(|t| t.member_ids.len()).collect();
        sizes.sort_unstable();
        assert_eq!(
            sizes,
            vec![1, 1, 2],
            "expected one 2-member topic (cars) + two singletons (job, dog); got {sizes:?}"
        );
        // The car pair must be co-located in the size-2 topic.
        let car_a = MemoryId(Uuid::from_u128(1));
        let car_b = MemoryId(Uuid::from_u128(2));
        assert!(
            map.topics
                .iter()
                .any(|t| t.member_ids.contains(&car_a) && t.member_ids.contains(&car_b)),
            "the two car facts MUST share a topic; got {:?}",
            map.topics
        );
    }

    #[tokio::test]
    async fn discover_topics_mutual_knn_drops_one_way_bridge_edges() {
        // ADR-085 (Finding F fix): the reciprocal-kNN graph must NOT absorb a
        // fact into a cluster via a one-way edge. Construction: 6 identical
        // "hub" facts (all on the X axis, cosine 1.0 to each other) + one
        // outsider C at cosine 0.80 to every hub fact (above the 0.70 floor).
        //
        // With TOP_K = 5 each hub fact's five nearest are the five OTHER hub
        // facts (cosine 1.0), so C — at 0.80 — never makes any hub fact's
        // top-K. C's own top-K, however, IS full of hub facts. Under the old
        // one-way rule those C→hub edges would drag C into the hub blob (one
        // topic of 7). Under mutual-kNN the edge is dropped (no hub fact
        // reciprocates), so C stays its own topic.
        //
        // This is the exact mechanism behind the 1k collapse, in miniature:
        // weak one-way links chaining unrelated facts into one giant topic.
        let c_vec = vec![0.8f32, 0.6, 0.0, 0.0]; // unit; cosine to ex() == 0.8
        let mems: Vec<Memory> = (1..=7)
            .map(|n| memory_with_id(&format!("m{n}"), n as u128))
            .collect();
        let embedder = MockEmbedder::new(vec![
            ("m1", ex()),
            ("m2", ex()),
            ("m3", ex()),
            ("m4", ex()),
            ("m5", ex()),
            ("m6", ex()),
            ("m7", c_vec),
        ]);
        let map = discover_topics(&boundary("personal"), &mems, &embedder, None)
            .await
            .unwrap();

        let mut sizes: Vec<usize> = map.topics.iter().map(|t| t.member_ids.len()).collect();
        sizes.sort_unstable();
        assert_eq!(
            sizes,
            vec![1, 6],
            "mutual-kNN MUST keep the 6 hub facts as one topic and leave the \
             one-way-linked outsider C as its own singleton; got {sizes:?} \
             (the old one-way rule would have produced a single topic of 7)"
        );
        // C (id 7) must be alone — proving the one-way bridge was dropped.
        let c_id = MemoryId(Uuid::from_u128(7));
        assert!(
            map.topics.iter().any(|t| t.member_ids == vec![c_id]),
            "outsider C MUST be its own singleton topic; got {:?}",
            map.topics
        );
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
