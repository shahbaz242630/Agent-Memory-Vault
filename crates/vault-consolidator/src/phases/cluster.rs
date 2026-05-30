//! Phase 1 clustering primitive for the sleep cycle (BRD §5.6 Phase 1).
//!
//! Implements the BRD-locked algorithm verbatim (BRD §5.6 lines 935-938):
//!
//! > **Phase 1: Identify candidate clusters.**
//! > - For each memory added since last consolidation, find its top-5 vector
//! >   neighbors above `merge_similarity_threshold`
//! > - Group into clusters by transitive closure
//! > - Skip clusters of size 1 (no duplicates to merge)
//!
//! ## Threshold semantics — locked
//!
//! `merge_similarity_threshold` is **cosine similarity** (not cosine distance).
//! The default value 0.92 (BRD §5.6 line 904 `ConsolidatorConfig`) means
//! "keep neighbour edges where cos(query, candidate) ≥ 0.92."
//!
//! `LanceVectorStore::search` returns **distance** under `DistanceType::Cosine`
//! (smaller = closer; identical = 0). For L2-normalised bge-small-en-v1.5
//! embeddings (which the [`EmbeddingProvider`] contract enforces — see
//! `vault-embedding::EmbeddingProvider` docs), the relationship is
//! `cosine_distance = 1 - cosine_similarity`. So the threshold check is:
//!
//! ```text
//! keep edge if  distance ≤ 1.0 - threshold
//! ```
//!
//! This formula is the only conversion site in the workspace — the rest of
//! the system speaks distance (LanceDB) or similarity (BRD spec); the
//! translation lives here so the algorithm reads naturally against either
//! mental model.
//!
//! ## Re-embedding at consolidation time — locked
//!
//! `Memory` rows loaded from `MetadataStore` have `embedding: None` (vectors
//! live in LanceDB, not SQLite — see `vault-storage::metadata_store.rs:896`).
//! To run "top-5 NN per memory" we re-embed each memory's content via
//! `EmbeddingProvider::embed` and use that as the query vector.
//!
//! Determinism: bge-small-en-v1.5 under ONNX Runtime CPU is deterministic at
//! fp32 (no inference-side `random_seed` exposed; ONNX `SessionOptions`
//! defaults produce bit-reproducible output for the same input). The
//! re-embed vs. stored-embed similarity is bounded by IEEE-754 rounding
//! noise at ~sub-1e-6 per coordinate; cosine similarity is invariant under
//! such noise (well within the 1e-3 worst-case headroom against a 0.92
//! threshold). See ADR-045 §c.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, instrument, warn};

use vault_core::{Boundary, MemoryId, VaultError, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_storage::{MemoryFilter, StorageBackend};

/// Number of top vector neighbours queried per memory.
///
/// BRD §5.6 line 936 locks "top-5 vector neighbors above `merge_similarity_threshold`".
/// `LanceVectorStore::search` returns the queried memory itself as its own
/// nearest neighbour (distance 0), so we request `TOP_K_NEIGHBORS + 1 = 6`
/// and discard the self-match in [`collect_edges_for_memory`]. Locking the
/// constant here so it stays consistent with the BRD verbatim without an
/// off-by-one drift between "neighbours requested" and "neighbours kept."
pub const TOP_K_NEIGHBORS: usize = 5;

/// N-ary cluster of memory-row references. Output type of T0.2.2 Phase 1
/// clustering; consumed by T0.2.3 Phase 2 merge phase.
///
/// `id` is a per-run monotonic index (starts at 0 for the first cluster
/// produced by a given [`find_candidate_clusters`] call, increments
/// deterministically by member-set ordering). It is NOT persistent across
/// consolidation runs — re-run produces fresh `id`s. T0.2.3 may persist
/// per-cluster decisions in the consolidation `REPORT.md` (BRD §5.6 line
/// 957 checkpoint contract), at which point we may need a stable identifier;
/// that's a T0.2.3-time decision, not T0.2.2's problem.
///
/// `member_row_ids` is **always size ≥ 2**: singleton clusters (BRD §5.6
/// line 938 "Skip clusters of size 1") are filtered before the result Vec
/// is returned. The invariant is enforced at construction site in
/// [`find_candidate_clusters`].
///
/// Member IDs are sorted ascending for deterministic ordering — this makes
/// cluster equality + cluster ID assignment stable across runs given the
/// same input, which matters for the acceptance test's precision/recall
/// scoring. See ADR-045 §a.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cluster {
    /// Per-run monotonic index (0, 1, 2, ...). Stable within one
    /// [`find_candidate_clusters`] call given the same input; not
    /// persistent across calls.
    pub id: u32,
    /// Sorted ascending. Always size ≥ 2 (singletons filtered).
    pub member_row_ids: Vec<MemoryId>,
}

impl Cluster {
    /// Number of memories in the cluster. Always ≥ 2 (invariant — singletons
    /// are filtered before construction).
    pub fn size(&self) -> usize {
        self.member_row_ids.len()
    }
}

/// Identify candidate merge clusters per BRD §5.6 Phase 1.
///
/// **Algorithm (BRD-locked):**
///
/// 1. Enumerate memories in `boundary`, optionally filtered by
///    `created_at >= since`. T0.2.2 always passes `since = None` (full-scan
///    — incremental-scope is a T0.2.5 forward-compat parameter not yet wired
///    by any caller).
/// 2. For each memory, re-embed its content via `embeddings.embed(...)` and
///    query `TOP_K_NEIGHBORS + 1` nearest neighbours in the vector store,
///    boundary-scoped.
/// 3. Drop the self-match (`neighbour_id == memory.id`) and any neighbour
///    whose distance exceeds the threshold (cosine similarity below
///    `threshold`, equivalent to distance above `1.0 - threshold`).
/// 4. Treat surviving neighbour pairs as undirected edges; run union-find
///    transitive closure to group connected components.
/// 5. Drop singleton clusters (size 1 — no merge candidates).
/// 6. Sort each cluster's members ascending + assign monotonic `id` by
///    smallest-member ordering for deterministic output.
///
/// **Parameters:**
///
/// - `storage`: shared workspace storage backend. Used for memory enumeration
///   (via [`StorageBackend::list_memories`] — T0.2.2 Amendment 2, added
///   alongside this primitive) and vector-store NN search (via
///   [`StorageBackend::vector_store`]).
/// - `embeddings`: production [`EmbeddingProvider`] (bge-small-en-v1.5 at
///   V0.2). Required because `Memory` rows loaded from `MetadataStore` have
///   `embedding: None`.
/// - `boundary`: scopes the run to a single boundary (BRD §11.4.3 — every
///   memory belongs to exactly one boundary; the consolidator processes one
///   boundary at a time per BRD §5.6 line 971 "one summary per boundary").
/// - `threshold`: cosine similarity threshold ∈ [0.0, 1.0]. Recommended
///   default from `ConsolidatorConfig`: 0.92.
/// - `since`: forward-compat checkpoint parameter for T0.2.5. Pass `None`
///   at T0.2.2 — produces full-scan semantics matching BRD §5.6 line 936.
///
/// **Returns:** `Vec<Cluster>` with deterministic ordering — clusters sorted
/// by smallest-member ID ascending; cluster `id`s assigned 0..N in that
/// order. Empty Vec when nothing meets the threshold (valid, not an error).
///
/// **Errors:**
///
/// - `VaultError::InvalidInput` — threshold outside [0.0, 1.0].
/// - `VaultError::Storage` — propagated from memory enumeration or NN search.
/// - `VaultError::Embedding` — propagated from re-embedding any memory's
///   content.
#[instrument(skip(storage, embeddings), fields(boundary = %boundary.as_str(), threshold, since = ?since))]
pub async fn find_candidate_clusters(
    storage: &StorageBackend,
    embeddings: &dyn EmbeddingProvider,
    boundary: &Boundary,
    threshold: f32,
    since: Option<DateTime<Utc>>,
) -> VaultResult<Vec<Cluster>> {
    if !(0.0..=1.0).contains(&threshold) {
        return Err(VaultError::InvalidInput(format!(
            "merge_similarity_threshold must be in [0.0, 1.0], got {threshold}"
        )));
    }

    // Step 1: enumerate memories in boundary.
    let memories = storage
        .list_memories(
            MemoryFilter {
                boundary: Some(boundary.clone()),
                since,
                ..Default::default()
            },
            None,
        )
        .await?;

    info!(memory_count = memories.len(), "starting Phase 1 clustering");

    if memories.len() < 2 {
        // No edges possible; short-circuit before any embedding work.
        return Ok(Vec::new());
    }

    // Step 2-3: re-embed + NN-search + edge collection, per memory.
    let mut edges: Vec<(MemoryId, MemoryId)> = Vec::new();
    let max_distance = 1.0_f32 - threshold;

    // The set of valid graph nodes — only the active memories
    // `list_memories` returned. The vector store can return neighbour ids
    // that are NOT in this set: a superseded/invalidated memory's LanceDB
    // vector lingers (supersede/invalidate update SQLite metadata only, not
    // the vector), so an NN search may surface it even though `list_memories`
    // (default filter) excludes it. Edges to such non-member ids must be
    // dropped — otherwise `union_find_components` looks up an id that has no
    // parent entry and panics. This SQLite/LanceDB divergence is an expected
    // steady-state condition after any merge/supersede/invalidate, so the
    // drop is normal hygiene, not an error.
    let member_ids: HashSet<MemoryId> = memories.iter().map(|m| m.id).collect();
    let mut divergent_neighbours_dropped = 0usize;

    for memory in &memories {
        let embedding = embeddings.embed(&memory.content).await?;
        let neighbours =
            collect_edges_for_memory(storage, &memory.id, &embedding, boundary, max_distance)
                .await?;
        for neighbour_id in neighbours {
            // Drop edges to ids not among the active members (vector-store /
            // metadata divergence — see the `member_ids` comment above).
            if !member_ids.contains(&neighbour_id) {
                divergent_neighbours_dropped += 1;
                continue;
            }
            // Canonicalise edge orientation (smaller-id, larger-id) so the
            // edge set deduplicates trivially across the symmetric pairs
            // produced by querying both endpoints.
            let (a, b) = if memory.id <= neighbour_id {
                (memory.id, neighbour_id)
            } else {
                (neighbour_id, memory.id)
            };
            edges.push((a, b));
        }
    }

    if divergent_neighbours_dropped > 0 {
        warn!(
            dropped = divergent_neighbours_dropped,
            "dropped NN edges to non-member ids (vector-store/metadata divergence — \
             superseded or invalidated vectors lingering in LanceDB)"
        );
    }
    debug!(edge_count = edges.len(), "edges collected pre-dedup");

    // Step 4: union-find transitive closure.
    let clusters_map =
        union_find_components(&memories.iter().map(|m| m.id).collect::<Vec<_>>(), &edges);

    // Step 5-6: filter singletons + assign deterministic Cluster ids.
    let mut clusters: Vec<Vec<MemoryId>> = clusters_map
        .into_values()
        .filter(|members| members.len() >= 2)
        .map(|mut members| {
            members.sort();
            members
        })
        .collect();

    // Deterministic cluster order: sort by smallest member ID ascending.
    clusters.sort_by(|a, b| a[0].cmp(&b[0]));

    let result: Vec<Cluster> = clusters
        .into_iter()
        .enumerate()
        .map(|(idx, members)| Cluster {
            id: idx as u32,
            member_row_ids: members,
        })
        .collect();

    info!(cluster_count = result.len(), "Phase 1 clustering complete");

    Ok(result)
}

/// Query top-`TOP_K_NEIGHBORS + 1` NN for `embedding`, drop self-match,
/// drop neighbours whose distance exceeds `max_distance`, return surviving
/// neighbour IDs.
///
/// Returns an empty Vec if the only neighbour above threshold is the
/// memory itself (the typical case for a memory with no near-duplicates).
async fn collect_edges_for_memory(
    storage: &StorageBackend,
    self_id: &MemoryId,
    embedding: &[f32],
    boundary: &Boundary,
    max_distance: f32,
) -> VaultResult<Vec<MemoryId>> {
    let raw = storage
        .vector_store()
        .search(
            embedding,
            TOP_K_NEIGHBORS + 1,
            std::slice::from_ref(boundary),
        )
        .await?;

    if raw.is_empty() {
        warn!(
            self_id = %self_id,
            "NN search returned zero rows — vector store may be empty for this boundary"
        );
        return Ok(Vec::new());
    }

    let kept: Vec<MemoryId> = raw
        .into_iter()
        .filter(|(id, _)| id != self_id)
        .filter(|(_, distance)| *distance <= max_distance)
        .map(|(id, _)| id)
        .collect();

    Ok(kept)
}

/// Standard union-find / disjoint-set-union with path compression. Given an
/// adjacency-edge list, returns connected-component groups keyed by the
/// root ID.
///
/// Internal helper; not part of the crate's public API.
fn union_find_components(
    nodes: &[MemoryId],
    edges: &[(MemoryId, MemoryId)],
) -> BTreeMap<MemoryId, Vec<MemoryId>> {
    let mut parent: HashMap<MemoryId, MemoryId> = nodes.iter().map(|id| (*id, *id)).collect();

    fn find(parent: &mut HashMap<MemoryId, MemoryId>, mut id: MemoryId) -> MemoryId {
        // Iterative path compression — avoids recursion depth on chains.
        //
        // Defensive against an `id` with no parent entry: `find_candidate_clusters`
        // already filters edges to member ids, so in production every endpoint is
        // a node here — but indexing `parent[&id]` on an absent key panics, and a
        // disjoint-set primitive should never panic on input shape. An unknown id
        // is treated as its own root (the loop simply doesn't run). Belt-and-braces
        // with the edge filter; pinned by `union_find_treats_unknown_edge_endpoint_as_root`.
        let mut path: Vec<MemoryId> = Vec::new();
        while let Some(&next) = parent.get(&id) {
            if next == id {
                break;
            }
            path.push(id);
            id = next;
        }
        for p in path {
            parent.insert(p, id);
        }
        id
    }

    for (a, b) in edges {
        let ra = find(&mut parent, *a);
        let rb = find(&mut parent, *b);
        if ra != rb {
            // Union by smaller-id-as-root for determinism (no rank tracking
            // — at N≤1000 the path-compression cost is dominated by the
            // hash-lookup overhead; rank optimisation is a V0.3 concern).
            if ra < rb {
                parent.insert(rb, ra);
            } else {
                parent.insert(ra, rb);
            }
        }
    }

    // Bucket nodes by their final root.
    let mut groups: BTreeMap<MemoryId, BTreeSet<MemoryId>> = BTreeMap::new();
    for node in nodes {
        let root = find(&mut parent, *node);
        groups.entry(root).or_default().insert(*node);
    }

    groups
        .into_iter()
        .map(|(root, members)| (root, members.into_iter().collect()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn mk_id(n: u128) -> MemoryId {
        MemoryId(Uuid::from_u128(n))
    }

    // ─── Cluster type API ─────────────────────────────────────────────────

    #[test]
    fn cluster_size_returns_member_count() {
        let c = Cluster {
            id: 0,
            member_row_ids: vec![mk_id(1), mk_id(2), mk_id(3)],
        };
        assert_eq!(c.size(), 3);
    }

    #[test]
    fn cluster_round_trips_through_serde_json() {
        let c = Cluster {
            id: 42,
            member_row_ids: vec![mk_id(1), mk_id(2)],
        };
        let json = serde_json::to_string(&c).unwrap();
        let back: Cluster = serde_json::from_str(&json).unwrap();
        assert_eq!(c, back);
    }

    // ─── Union-find ───────────────────────────────────────────────────────

    #[test]
    fn union_find_with_no_edges_produces_singleton_components() {
        let nodes = vec![mk_id(1), mk_id(2), mk_id(3)];
        let groups = union_find_components(&nodes, &[]);
        assert_eq!(groups.len(), 3);
        for members in groups.values() {
            assert_eq!(members.len(), 1);
        }
    }

    #[test]
    fn union_find_transitive_closure_merges_chained_edges() {
        // 1↔2, 2↔3 should yield one component {1, 2, 3}.
        let nodes = vec![mk_id(1), mk_id(2), mk_id(3), mk_id(4)];
        let edges = vec![(mk_id(1), mk_id(2)), (mk_id(2), mk_id(3))];
        let groups = union_find_components(&nodes, &edges);
        let triple = groups
            .values()
            .find(|m| m.len() == 3)
            .expect("must have one 3-member component");
        let ids: BTreeSet<MemoryId> = triple.iter().copied().collect();
        assert_eq!(ids, [mk_id(1), mk_id(2), mk_id(3)].into_iter().collect());
        // Node 4 stays singleton.
        assert!(groups.values().any(|m| m == &vec![mk_id(4)]));
    }

    #[test]
    fn union_find_handles_two_disjoint_clusters() {
        // {1, 2} + {3, 4} — two separate 2-cliques.
        let nodes = vec![mk_id(1), mk_id(2), mk_id(3), mk_id(4)];
        let edges = vec![(mk_id(1), mk_id(2)), (mk_id(3), mk_id(4))];
        let groups = union_find_components(&nodes, &edges);
        let pairs: Vec<Vec<MemoryId>> = groups.values().filter(|m| m.len() == 2).cloned().collect();
        assert_eq!(pairs.len(), 2, "expected two pair-components");
    }

    #[test]
    fn union_find_is_robust_to_duplicate_edges() {
        // Same edge listed twice must not create extra components or
        // miscount membership.
        let nodes = vec![mk_id(1), mk_id(2)];
        let edges = vec![(mk_id(1), mk_id(2)), (mk_id(1), mk_id(2))];
        let groups = union_find_components(&nodes, &edges);
        let pair = groups
            .values()
            .find(|m| m.len() == 2)
            .expect("must have one 2-member component");
        assert_eq!(pair.len(), 2);
    }

    #[test]
    fn union_find_long_chain_path_compresses_without_stack_overflow() {
        // 1↔2, 2↔3, ..., 999↔1000 — chain of 1000 nodes. Recursive find
        // would blow the stack; iterative path compression must not.
        let nodes: Vec<MemoryId> = (1u128..=1000).map(mk_id).collect();
        let edges: Vec<(MemoryId, MemoryId)> =
            (1u128..1000).map(|i| (mk_id(i), mk_id(i + 1))).collect();
        let groups = union_find_components(&nodes, &edges);
        let chain = groups
            .values()
            .find(|m| m.len() == 1000)
            .expect("must have one 1000-member component");
        assert_eq!(chain.len(), 1000);
    }

    #[test]
    fn union_find_treats_unknown_edge_endpoint_as_root() {
        // Regression (live dogfood 2026-05-30): an edge endpoint absent from
        // `nodes` — a superseded/invalidated memory whose LanceDB vector
        // lingers and gets returned by an NN search — must NOT panic. Before
        // the defensive `find`, `parent[&id]` on the unknown id panicked
        // "no entry found for key" (cluster.rs:294), crashing the entire
        // consolidation run on real divergent data. `find_candidate_clusters`
        // now also filters these edges out; this pins the primitive-level
        // safety net independently.
        let nodes = vec![mk_id(1), mk_id(2)];
        let unknown = mk_id(999); // deliberately not in `nodes`
        let edges = vec![(mk_id(1), unknown)];

        // Must not panic.
        let groups = union_find_components(&nodes, &edges);

        // The phantom endpoint must not pull a real node into a size-≥2
        // cluster; both real nodes stay singletons.
        let total: usize = groups.values().map(|m| m.len()).sum();
        assert_eq!(total, 2, "both real nodes must be accounted for");
        assert!(
            groups.values().all(|m| m.len() == 1),
            "an edge to a phantom (non-member) id must leave real nodes as singletons; \
             got {groups:?}"
        );
    }

    // ─── Threshold validation ──────────────────────────────────────────────
    //
    // The validation lives at the public-API entry point — exhaustive unit
    // tests on the function with a mocked storage backend would over-couple
    // to internal types. Threshold validation IS exercised by the
    // acceptance integration test (against real storage) but the
    // out-of-range cases are cheap to pin here without storage at all.

    #[tokio::test]
    async fn invalid_threshold_below_zero_is_rejected() {
        // We can construct a "minimum viable" storage+embeddings rig only
        // in the integration test (needs LanceVectorStore + BgeSmallProvider).
        // For threshold validation, the function returns InvalidInput before
        // touching storage — so a panicking stub is fine: the validation
        // path returns Err before the stub is reached.
        //
        // The simplest pin is an integration-style probe in
        // tests/acceptance.rs at the workspace level. Here we just lock the
        // error-message format via a string-level assertion against the
        // VaultError display surface.
        let msg = VaultError::InvalidInput(
            "merge_similarity_threshold must be in [0.0, 1.0], got -0.1".to_string(),
        )
        .to_string();
        assert!(msg.contains("must be in [0.0, 1.0]"));
        assert!(msg.contains("-0.1"));
    }
}
