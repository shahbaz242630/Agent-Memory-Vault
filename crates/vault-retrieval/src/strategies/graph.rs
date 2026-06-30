//! [`GraphRetriever`] — knowledge-graph retrieval channel (ADR-SEC-002 Part 2).
//!
//! ## What it does
//!
//! Surfaces memories that are *connected* to an entity named in the query, by
//! walking the knowledge graph the consolidator builds. It is the strategy that
//! answers relational questions ("where does my sister Maria work?", "what does
//! David do?") — the multi-hop questions dense + lexical search are weakest at,
//! because the answer fact often doesn't repeat the query's keywords.
//!
//! ## Deterministic — no LLM (architecture lock 2026-05-26)
//!
//! The read path holds no LLM. Query → entity resolution is pure string
//! matching: tokenise the query, and match those tokens against the *names* of
//! entities already in the graph (within the authorized boundaries). For each
//! matched entity we collect the live edges (1) directly touching it
//! (bidirectional, [`GraphStore::relationships_for_entity`]) and (2) along
//! outgoing paths up to [`MAX_HOPS`] away ([`GraphStore::traverse`]), follow
//! each edge's `source_memory_id` to the memory that produced it, and return
//! those memories. No model, no network, fully reproducible.
//!
//! ## Why two hops (ADR-SEC-002 Part 2 Amendment 1, 2026-06-28)
//!
//! A single hop only ever surfaces the source fact of an edge *directly*
//! touching the named entity — and that fact contains the entity's name by
//! construction, so lexical/semantic search already ranks it #1 (the 2026-06-28
//! dogfood measured 1-hop as redundant: graph-ON == graph-OFF, byte-identical).
//! The *second* hop reaches the answer fact two relationships away that never
//! names the query entity (e.g. "what is Maria's hospital known for?" → Maria →
//! St. Mary's → its specialty fact) — exactly what dense + lexical search miss.
//!
//! ## Additive + recall-safe
//!
//! `GraphRetriever` is wired as a union channel into
//! [`crate::strategies::hybrid`] / the reranked retriever: its hits widen the
//! candidate pool, then the cross-encoder reranker re-scores the *whole* pool.
//! So a graph hit can only ever be *offered* to the reranker — it can never
//! displace a semantic result or change ranking on its own. A noisy graph
//! candidate is simply reranked down. This is why benign over-extraction in the
//! graph is low-risk for answer quality.
//!
//! ## The "the user" hub is excluded
//!
//! Almost every fact extracts a `the user --rel--> <thing>` edge, so "the user"
//! is a hub touching nearly the whole graph. Matching it would surface the
//! entire vault and drown the signal — and user-centric queries are exactly
//! what semantic search already handles. So the hub names ([`HUB_ENTITY_NAMES`])
//! are never matched; the graph channel fires only when the query names a
//! *specific* person / org / place / project / concept.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::info;

use vault_core::{EntityId, Memory, MemoryId, VaultResult};
use vault_storage::{GraphStore, MetadataStore, TraversalOptions};

use crate::retriever::{RetrievalQuery, RetrievedMemory, Retriever};

/// Generic hub entity names that are never matched (see module docs).
const HUB_ENTITY_NAMES: &[&str] = &["the user", "user"];

/// Cap on how many query-matched entities we traverse from, bounding work on a
/// pathological query that names many entities.
const MAX_MATCHED_ENTITIES: usize = 8;

/// How many outgoing hops to walk from each matched entity. `2` is the value the
/// graph read-path dogfood (2026-06-28) showed is the real unlock: 1-hop only
/// ever surfaces the source fact of an edge directly touching the named entity —
/// which, by construction, contains that entity's name, so lexical/semantic
/// search already ranks it #1 (the channel was redundant). A *second* hop reaches
/// the answer fact that is two relationships away and never names the query
/// entity (e.g. "what is Maria's hospital known for?" → Maria → St. Mary's → its
/// specialty fact), which dense + lexical search genuinely miss. Spec-supported
/// (BRD §6 V0.1: 1–3 hops). Kept at 2 — 3+ adds noise the reranker must filter
/// for little marginal reach (ADR-SEC-002 Part 2 Amendment 1).
const MAX_HOPS: usize = 2;

/// Cap on graph-surfaced candidate memories handed upward, mirroring the
/// semantic union fan-out — the reranker is the relevance authority and dedups.
const MAX_CANDIDATE_MEMORIES: usize = 32;

/// Placeholder relevance for a graph hit before reranking. The union path's
/// reranker overwrites `score` with a real cross-encoder relevance; this value
/// only needs to be finite and in `[-1, 1]` (trait invariant 4) and is the same
/// for every graph hit (ties tiebreak on recency).
const GRAPH_CHANNEL_SCORE: f32 = 0.5;

/// Knowledge-graph retrieval channel. Cheap to clone (holds `Arc`s).
pub struct GraphRetriever {
    graph: Arc<dyn GraphStore>,
    metadata_store: Arc<MetadataStore>,
}

impl GraphRetriever {
    /// Construct the graph retrieval channel.
    ///
    /// - `graph` — the knowledge graph (entity + relationship store).
    /// - `metadata_store` — hydrates the connected memories by id
    ///   (`get_memories_batch`).
    #[must_use]
    pub fn new(graph: Arc<dyn GraphStore>, metadata_store: Arc<MetadataStore>) -> Self {
        Self {
            graph,
            metadata_store,
        }
    }
}

/// Split text into lowercase alphanumeric word tokens (drops punctuation).
fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// `true` when `name_tokens` appears as a contiguous run inside `query_tokens`
/// (whole-word phrase match). An empty or over-long name never matches.
fn mentions(query_tokens: &[String], name_tokens: &[String]) -> bool {
    if name_tokens.is_empty() || name_tokens.len() > query_tokens.len() {
        return false;
    }
    query_tokens
        .windows(name_tokens.len())
        .any(|w| w == name_tokens)
}

/// `true` when `name` is a generic hub name we never match on.
fn is_hub(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    HUB_ENTITY_NAMES.contains(&n.as_str())
}

/// A memory is part of *current* knowledge — not superseded, retired, or
/// archived. The graph must never surface a stale fact.
fn is_current(m: &Memory) -> bool {
    m.superseded_by.is_none() && m.valid_until.is_none() && !m.is_archived()
}

#[async_trait]
impl Retriever for GraphRetriever {
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        let start = std::time::Instant::now();
        let boundaries = &query.authorized_boundaries;

        // Empty authorization → empty (compile-time-safe access control, no
        // store round-trip). Also short-circuit an empty query.
        let query_tokens = tokenize(&query.query_text);
        if boundaries.is_empty() || query_tokens.is_empty() {
            info!(
                target: "vault_retrieval::query",
                strategy = "graph",
                boundary_count = boundaries.len(),
                result_count = 0usize,
                latency_ms = start.elapsed().as_millis() as u64,
                "graph channel short-circuit (empty boundary or query)"
            );
            return Ok(Vec::new());
        }

        // 1) Resolve query → specific named entities (never the hub).
        let entities = self.graph.list_entities(boundaries).await?;
        let name_tokens_cache: Vec<(EntityId, String, Vec<String>)> = entities
            .into_iter()
            .filter(|e| !is_hub(&e.name))
            .map(|e| {
                let toks = tokenize(&e.name);
                (e.id, e.name, toks)
            })
            .collect();
        let matched: Vec<(EntityId, String)> = name_tokens_cache
            .iter()
            .filter(|(_, _, toks)| mentions(&query_tokens, toks))
            .map(|(id, name, _)| (*id, name.clone()))
            .take(MAX_MATCHED_ENTITIES)
            .collect();

        // 2) For each matched entity, collect the source memories of:
        //    (a) the edges DIRECTLY touching it — bidirectional 1-hop
        //        ([`GraphStore::relationships_for_entity`]); this catches the
        //        incoming `the user --rel--> <entity>` edge a query naming
        //        `<entity>` must still see; and
        //    (b) the edges along OUTGOING paths up to [`MAX_HOPS`] away
        //        ([`GraphStore::traverse`]) — the multi-hop reach that lets the
        //        graph answer questions whose answer fact is two relationships
        //        away and never names the query entity. Outgoing-only traversal
        //        from a *specific* named entity walks AWAY from the "the user"
        //        hub, so it does not re-introduce the hub explosion the design
        //        avoids (D4). The union dedups by source memory id, so the hop-1
        //        overlap between (a) and (b) is harmless.
        //    Track which entity connected each memory for the explanation string.
        let mut connectors: HashMap<MemoryId, HashSet<String>> = HashMap::new();
        'outer: for (entity_id, entity_name) in &matched {
            let direct = self
                .graph
                .relationships_for_entity(entity_id, boundaries)
                .await?;
            let paths = self
                .graph
                .traverse(
                    entity_id,
                    boundaries,
                    TraversalOptions {
                        max_hops: MAX_HOPS,
                        relation_filter: None,
                        follow_aliases: false,
                    },
                )
                .await?;
            let path_rels = paths.into_iter().flat_map(|(_entity, rels)| rels);
            for r in direct.into_iter().chain(path_rels) {
                if let Some(src) = r.source_memory_id {
                    connectors
                        .entry(src)
                        .or_default()
                        .insert(entity_name.clone());
                }
                if connectors.len() >= MAX_CANDIDATE_MEMORIES {
                    break 'outer;
                }
            }
        }

        if connectors.is_empty() {
            info!(
                target: "vault_retrieval::query",
                strategy = "graph",
                boundary_count = boundaries.len(),
                matched_entities = matched.len(),
                result_count = 0usize,
                latency_ms = start.elapsed().as_millis() as u64,
                "graph channel: no connected memories"
            );
            return Ok(Vec::new());
        }

        // 3) Hydrate the connected memories, then keep only those that are in an
        //    authorized boundary (defense in depth — the edges were already
        //    boundary-scoped) AND are current knowledge.
        let ids: Vec<MemoryId> = connectors.keys().copied().collect();
        let memories = self.metadata_store.get_memories_batch(&ids).await?;

        let mut hits: Vec<RetrievedMemory> = memories
            .into_iter()
            .filter(|m| boundaries.contains(&m.boundary) && is_current(m))
            .map(|m| {
                let via = connectors
                    .get(&m.id)
                    .map(|set| {
                        let mut names: Vec<&str> = set.iter().map(String::as_str).collect();
                        names.sort_unstable();
                        names.join(", ")
                    })
                    .unwrap_or_default();
                RetrievedMemory {
                    score: GRAPH_CHANNEL_SCORE,
                    explanation: format!("graph: connected via {via}"),
                    memory: m,
                }
            })
            .collect();

        // Deterministic order: recency DESC (scores are all equal pre-rerank).
        hits.sort_by(|a, b| {
            b.memory
                .created_at
                .cmp(&a.memory.created_at)
                .then_with(|| b.memory.id.0.cmp(&a.memory.id.0))
        });
        hits.truncate(query.max_results);

        info!(
            target: "vault_retrieval::query",
            strategy = "graph",
            boundary_count = boundaries.len(),
            matched_entities = matched.len(),
            result_count = hits.len(),
            latency_ms = start.elapsed().as_millis() as u64,
            "graph channel retrieval complete"
        );
        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use vault_core::{
        Boundary, Entity, EntityType, MemoryType, NewEntity, NewMemory, Relationship,
    };
    use vault_storage::{DuckDbGraphStore, SqlCipherKey};

    // ─── pure-helper unit tests (no stores) ──────────────────────────────────

    #[test]
    fn tokenize_lowercases_and_drops_punctuation() {
        assert_eq!(
            tokenize("Where does Maria work?"),
            ["where", "does", "maria", "work"]
        );
        assert_eq!(tokenize("Acme Corp."), ["acme", "corp"]);
        assert!(tokenize("   !!!  ").is_empty());
    }

    #[test]
    fn mentions_matches_whole_word_phrases_only() {
        let q = tokenize("where does maria work at acme corp");
        assert!(mentions(&q, &tokenize("maria")));
        assert!(mentions(&q, &tokenize("Acme Corp")));
        assert!(!mentions(&q, &tokenize("Mari"))); // not a whole-word run
        assert!(!mentions(&q, &tokenize("corp acme"))); // order matters
        assert!(!mentions(&q, &[])); // empty name never matches
    }

    #[test]
    fn hub_names_are_excluded() {
        assert!(is_hub("the user"));
        assert!(is_hub("The User"));
        assert!(is_hub("user"));
        assert!(!is_hub("Maria"));
    }

    // ─── integration: real graph + metadata stores ──────────────────────────
    // (the graph runs ephemeral in-memory, so no at-rest key is needed here)

    fn boundary(s: &str) -> Boundary {
        Boundary::new(s).unwrap()
    }

    async fn make_memory(meta: &MetadataStore, content: &str, b: &str) -> MemoryId {
        let m = Memory::try_new(NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: boundary(b),
            source_agent: None,
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .unwrap();
        let id = m.id;
        meta.create_memory(&m).await.unwrap();
        id
    }

    struct Fixture {
        retriever: GraphRetriever,
        _dir: tempfile::TempDir,
    }

    /// Build a small graph: `the user --works_at--> Acme` (mem A),
    /// `the user --sibling_of--> Maria` (mem B), `Maria --works_at--> Hospital`
    /// (mem C), all in `work`; plus a `personal`-boundary `the user --owns-->
    /// Rex` (mem D) to prove isolation. Returns `(fixture, mem_a..mem_d)`.
    async fn fixture() -> (Fixture, MemoryId, MemoryId, MemoryId, MemoryId) {
        let dir = tempdir().unwrap();
        let key = SqlCipherKey::new("graph-retriever-test");
        let meta = Arc::new(
            MetadataStore::open(dir.path().join("m.db"), key)
                .await
                .unwrap(),
        );
        // ONE shared graph store drives both setup and retrieval.
        let graph: Arc<dyn GraphStore> =
            Arc::new(DuckDbGraphStore::open_ephemeral().await.unwrap());

        let mem_a = make_memory(&meta, "The user works at Acme Corp.", "work").await;
        let mem_b = make_memory(&meta, "The user's sister is Maria.", "work").await;
        let mem_c = make_memory(&meta, "Maria works at the General Hospital.", "work").await;
        let mem_d = make_memory(&meta, "The user owns a dog named Rex.", "personal").await;

        // work-boundary entities + edges.
        let user = make_entity_via(&graph, "the user", EntityType::Person, "work").await;
        let acme = make_entity_via(&graph, "Acme Corp", EntityType::Organization, "work").await;
        let maria = make_entity_via(&graph, "Maria", EntityType::Person, "work").await;
        let hospital =
            make_entity_via(&graph, "General Hospital", EntityType::Organization, "work").await;
        // personal-boundary entities + edge (isolation probe).
        let user_p = make_entity_via(&graph, "the user", EntityType::Person, "personal").await;
        let rex = make_entity_via(&graph, "Rex", EntityType::Concept, "personal").await;

        link_via(&graph, &user, &acme, "works_at", mem_a).await;
        link_via(&graph, &user, &maria, "sibling_of", mem_b).await;
        link_via(&graph, &maria, &hospital, "works_at", mem_c).await;
        link_via(&graph, &user_p, &rex, "owns", mem_d).await;

        // A 2-hop chain off Maria: General Hospital --specializes_in--> pediatric
        // cardiology (mem E). The answer fact never names Maria, so it is reachable
        // ONLY by walking Maria → Hospital → specialty (the multi-hop case).
        let mem_e = make_memory(
            &meta,
            "The General Hospital specializes in pediatric cardiology.",
            "work",
        )
        .await;
        let pediatrics =
            make_entity_via(&graph, "pediatric cardiology", EntityType::Concept, "work").await;
        link_via(&graph, &hospital, &pediatrics, "specializes_in", mem_e).await;

        let retriever = GraphRetriever::new(graph, meta);
        (
            Fixture {
                retriever,
                _dir: dir,
            },
            mem_a,
            mem_b,
            mem_c,
            mem_d,
        )
    }

    async fn make_entity_via(
        g: &Arc<dyn GraphStore>,
        name: &str,
        et: EntityType,
        b: &str,
    ) -> Entity {
        let e = Entity::try_new(NewEntity {
            name: name.into(),
            entity_type: et,
            boundary: boundary(b),
        })
        .unwrap();
        g.create_entity(&e).await.unwrap();
        e
    }

    async fn link_via(
        g: &Arc<dyn GraphStore>,
        from: &Entity,
        to: &Entity,
        rel: &str,
        src: MemoryId,
    ) {
        let r = Relationship::try_new(from.id, to.id, rel, 0.9, Some(src)).unwrap();
        g.create_relationship(&r).await.unwrap();
    }

    fn query(text: &str, boundaries: &[&str]) -> RetrievalQuery {
        RetrievalQuery {
            query_text: text.into(),
            authorized_boundaries: boundaries.iter().map(|b| boundary(b)).collect(),
            max_results: 10,
            options: crate::retriever::RetrievalOptions::default(),
        }
    }

    #[tokio::test]
    async fn surfaces_memories_connected_to_a_named_entity() {
        let (fx, mem_a, mem_b, mem_c, _mem_d) = fixture().await;
        // Query names "Maria" → her edges: user→Maria (mem B) + Maria→Hospital
        // (mem C). Acme (mem A) is NOT connected to Maria → not surfaced.
        let hits = fx
            .retriever
            .retrieve(query("what does Maria do", &["work"]))
            .await
            .unwrap();
        let ids: HashSet<MemoryId> = hits.iter().map(|h| h.memory.id).collect();
        assert!(ids.contains(&mem_b), "user→Maria edge's memory surfaces");
        assert!(
            ids.contains(&mem_c),
            "Maria→Hospital edge's memory surfaces (the answer fact)"
        );
        assert!(
            !ids.contains(&mem_a),
            "an unrelated entity's memory is not surfaced"
        );
    }

    #[tokio::test]
    async fn surfaces_two_hop_connected_memory() {
        let (fx, _a, _b, _c, _d) = fixture().await;
        // "Maria" is named; the answer fact ("...specializes in pediatric
        // cardiology") is TWO hops away (Maria → General Hospital → specialty) and
        // never mentions Maria — the case 1-hop + lexical search both miss.
        let hits = fx
            .retriever
            .retrieve(query("what is Maria's hospital known for", &["work"]))
            .await
            .unwrap();
        assert!(
            hits.iter()
                .any(|h| h.memory.content.contains("pediatric cardiology")),
            "the 2-hop answer fact must surface via Maria → Hospital → specialty"
        );
    }

    #[tokio::test]
    async fn dogfood_exact_two_hop_maria_delgado_st_marys() {
        // DIAGNOSTIC (2026-06-29): mirrors the graph_readpath_dogfood corpus
        // EXACTLY — entity names "Maria Delgado" / "St. Mary's Hospital" and the
        // possessive query "What is Maria Delgado's hospital known for?" — to
        // localize why the hard dogfood saw graph-ON == graph-OFF (the specialty
        // fact never surfaced). PASS here ⇒ the GraphRetriever injects correctly
        // and the loss is DOWNSTREAM (rerank-pool truncation / burial). FAIL ⇒
        // entity resolution misses this exact phrasing.
        let dir = tempdir().unwrap();
        let key = SqlCipherKey::new("graph-dogfood-mirror");
        let meta = Arc::new(
            MetadataStore::open(dir.path().join("m.db"), key)
                .await
                .unwrap(),
        );
        let graph: Arc<dyn GraphStore> =
            Arc::new(DuckDbGraphStore::open_ephemeral().await.unwrap());

        let m_sib = make_memory(
            &meta,
            "The user's sister is named Maria Delgado.",
            "personal",
        )
        .await;
        let m_works = make_memory(
            &meta,
            "Maria Delgado works as a cardiac nurse at St. Mary's Hospital.",
            "personal",
        )
        .await;
        let m_spec = make_memory(
            &meta,
            "St. Mary's Hospital specializes in pediatric cardiology and neonatal care.",
            "personal",
        )
        .await;

        let user = make_entity_via(&graph, "the user", EntityType::Person, "personal").await;
        let maria = make_entity_via(&graph, "Maria Delgado", EntityType::Person, "personal").await;
        let hospital = make_entity_via(
            &graph,
            "St. Mary's Hospital",
            EntityType::Organization,
            "personal",
        )
        .await;
        let pediatrics = make_entity_via(
            &graph,
            "pediatric cardiology",
            EntityType::Concept,
            "personal",
        )
        .await;

        link_via(&graph, &user, &maria, "sibling_of", m_sib).await;
        link_via(&graph, &maria, &hospital, "works_at", m_works).await;
        link_via(&graph, &hospital, &pediatrics, "specializes_in", m_spec).await;

        let retriever = GraphRetriever::new(graph, meta);
        let hits = retriever
            .retrieve(query(
                "What is Maria Delgado's hospital known for?",
                &["personal"],
            ))
            .await
            .unwrap();
        assert!(
            hits.iter()
                .any(|h| h.memory.content.contains("pediatric cardiology")),
            "the 2-hop specialty fact must surface from the EXACT dogfood corpus; got: {:?}",
            hits.iter().map(|h| &h.memory.content).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn the_user_hub_is_never_matched() {
        let (fx, _a, _b, _c, _d) = fixture().await;
        // "the user" appears in the query but is the hub → no traversal from it,
        // and no other entity is named → empty.
        let hits = fx
            .retriever
            .retrieve(query("tell me about the user", &["work"]))
            .await
            .unwrap();
        assert!(hits.is_empty(), "the hub must not surface the whole graph");
    }

    #[tokio::test]
    async fn never_crosses_a_boundary() {
        let (fx, _a, _b, _c, mem_d) = fixture().await;
        // "Rex" lives in the personal boundary; a work-only query must never
        // surface it, even though it names Rex.
        let hits = fx
            .retriever
            .retrieve(query("who is Rex", &["work"]))
            .await
            .unwrap();
        let ids: HashSet<MemoryId> = hits.iter().map(|h| h.memory.id).collect();
        assert!(
            !ids.contains(&mem_d),
            "a personal-boundary memory must never surface in a work-only query"
        );
        assert!(hits.is_empty());

        // With personal authorized, Rex's memory surfaces.
        let hits = fx
            .retriever
            .retrieve(query("who is Rex", &["personal"]))
            .await
            .unwrap();
        assert!(hits.iter().any(|h| h.memory.id == mem_d));
    }

    #[tokio::test]
    async fn no_named_entity_returns_empty() {
        let (fx, _a, _b, _c, _d) = fixture().await;
        let hits = fx
            .retriever
            .retrieve(query("what is the meaning of life", &["work"]))
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn empty_boundary_short_circuits() {
        let (fx, _a, _b, _c, _d) = fixture().await;
        let hits = fx
            .retriever
            .retrieve(query("what does Maria do", &[]))
            .await
            .unwrap();
        assert!(hits.is_empty());
    }
}
