//! [`RerankedRetriever`] — a [`Retriever`] that wraps a base retriever with the
//! cross-encoder reranker, bringing `memory_search` up to the same relevance
//! quality the `memory_read` path already has.
//!
//! ## Why this exists (ADR-071, 2026-06-05)
//!
//! `memory_search` shipped (V0.1 → V0.2) as the RAW [`HybridRetriever`] —
//! BGE-small semantic ∪ BM25 keyword fused by Reciprocal Rank Fusion, with NO
//! reranker. That is our weakest ranking path. Cross-agent dogfood (2026-06-05,
//! Antigravity raw-tool-I/O window) showed the correct answer for "instrument"
//! ranked **#4 of 10** — behind "vintage keyboards", "structural engineer" and
//! "fluent in Mandarin" — because RRF cannot separate a relevant subject-less
//! fact from incidental keyword overlap ([[bge-small-cannot-separate-relevant]]).
//! It only produced a correct end answer because the calling agent was smart
//! enough to pick the right fact off the list; at 100+ facts the answer can fall
//! out of the returned window entirely → a wrong / empty answer. Since agents
//! pick `memory_search` unpredictably (and constantly), the weak path is the one
//! they actually hit.
//!
//! BRD §5.5 always specified the retriever as *"Multi-strategy parallel
//! retrieval **with reranking** … Reranked. No single-strategy weakness."* This
//! type is the additive step toward that `MultiStrategyRetriever`: same
//! [`Retriever`] trait, new implementer, reusing the SAME warm reranker `Arc` the
//! read pipeline already loaded (no second model, no extra startup cost).
//!
//! ## What it does
//!
//! On each `retrieve()`:
//! 1. Fetches a WIDE candidate pool from `base` (the raw hybrid) —
//!    [`SEARCH_CANDIDATE_FANOUT`], NOT the agent's small final `max_results`, so
//!    the reranker can rescue a deep-but-correct answer the agent would never see
//!    otherwise.
//! 2. Unions in the semantic channel's top-N (ADR-069 recall-union) so a strong
//!    pure-semantic match the hybrid's RRF starves is still scored.
//! 3. Reranks the pool and re-sorts by relevance DESC. **Reorder-only — nothing
//!    is dropped.** The reranker only pulls the right answer up; the result is
//!    empty ONLY when the base pool was genuinely empty.
//! 4. Truncates to the agent's `max_results`.
//!
//! ## Recall-safe: never a false-empty (ADR-071 revision, 2026-06-05)
//!
//! An earlier design dropped below-floor candidates so search could return an
//! empty "nothing found". **Three-model live dogfood killed that.** A false-empty
//! does not make a weak agent abstain — it makes the agent either (a) ABANDON the
//! vault for competing memory (Gemini-Pro-class behaviour: Opus 4.6 went hunting
//! the IDE's own brain when search returned `[]`), or (b) SPIN, re-querying with
//! ~20 keyword variations for minutes (Gemini Flash on a true no-signal query).
//! For a memory vault a false-empty is the worst failure mode: it teaches the
//! agent the vault is unreliable, so it routes around us. Recall > precision —
//! this retriever never returns empty when candidates exist. The honest "not in
//! your vault" signal is owned by `memory_read`'s structured `abstain`, which is
//! an unambiguous, actionable contract (unlike a bare empty list); the tool
//! descriptions steer question-answering there. `memory_search` is the
//! recall-first browse path: it returns the reranked candidates and lets the
//! agent judge.
//!
//! ## Score-domain contract
//!
//! The reranker emits unbounded logits (e.g. +3.2, −6.9), but [`RetrievedMemory`]
//! documents `score ∈ [-1, 1]` ([`Retriever`] invariant #4). We map the logit
//! through the logistic (sigmoid) to `[0, 1]` via [`relevance_score`] — strictly
//! monotonic, so the rank order is preserved exactly (invariant #3 holds), and
//! bounded so a returned result never reads as a "negative/irrelevant" score to
//! the agent (the earlier `tanh ∈ (-1, 1)` mapping showed the correct cello fact
//! as `-0.95`). The raw logit is kept in `explanation` for debugging. On the
//! reranked path the `score` field's meaning shifts from "BGE cosine" to "[0,1]
//! reranker relevance" — a more honest relevance signal, and an alpha-permitted
//! wire change (V0.x; frozen at V1.0).
//!
//! ## Graceful fallback
//!
//! When no reranker model is configured, `application.rs` wires the raw hybrid
//! directly (this type is simply not constructed), mirroring how the read
//! pipeline falls back to the cosine gate. So a lightweight deployment without
//! the ~1.2 GB model keeps recall-first search; the reranked path is the
//! production default.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;

use vault_core::{VaultError, VaultResult};
use vault_embedding::RerankProvider;

use crate::retriever::{
    RetrievalOptions, RetrievalQuery, RetrievedMemory, Retriever, MAX_RESULTS_CAP,
};

/// Width of the candidate pool fetched from the base retriever before
/// reranking. Mirrors the read pipeline's `DEFAULT_MAX_CANDIDATES` (20): the
/// reranker is the relevance authority and must see well beyond the agent's
/// small final `max_results` to re-order a deep-but-correct answer to the top.
pub const SEARCH_CANDIDATE_FANOUT: usize = 20;

/// Hard cap on the number of candidates handed to the reranker. Sized to the
/// hybrid([`SEARCH_CANDIDATE_FANOUT`]) ∪ semantic([`SEARCH_CANDIDATE_FANOUT`])
/// union (`2 ×`) so a unioned-in semantic match is never truncated before the
/// reranker scores it. Also bounds reranker cost (~0.39 s/candidate CPU →
/// ≤ ~16 s worst case at 40; correctness-before-latency, GPU/int8 is the
/// latency fast-follow). Mirrors the read pipeline's `RERANK_CANDIDATE_CAP`.
pub const RERANK_CANDIDATE_CAP: usize = 2 * SEARCH_CANDIDATE_FANOUT;

/// A [`Retriever`] decorator: base retrieval → recall-union → cross-encoder
/// rerank → drop-below-floor → top-K. See the module docs for the full
/// rationale (ADR-071).
pub struct RerankedRetriever {
    /// The base ranking source — production wires the raw [`HybridRetriever`].
    base: Arc<dyn Retriever>,
    /// Optional pure-semantic channel for the ADR-069 recall-union. `None`
    /// disables the union (the base pool is reranked as-is).
    semantic: Option<Arc<dyn Retriever>>,
    /// The relevance authority. Shares the same warm `Arc` the read pipeline
    /// holds, so no second model load.
    reranker: Arc<dyn RerankProvider>,
}

impl RerankedRetriever {
    /// Construct a reranked search retriever.
    ///
    /// - `base` — the ranking source (production: the raw hybrid).
    /// - `semantic` — optional semantic channel for the recall-union; pass
    ///   `None` to rerank the base pool without widening.
    /// - `reranker` — the cross-encoder relevance authority.
    pub fn new(
        base: Arc<dyn Retriever>,
        semantic: Option<Arc<dyn Retriever>>,
        reranker: Arc<dyn RerankProvider>,
    ) -> Self {
        Self {
            base,
            semantic,
            reranker,
        }
    }

    /// Union the semantic channel's top-[`SEARCH_CANDIDATE_FANOUT`] onto `hits`,
    /// deduped by id. The reranker re-sorts the whole pool and drops below-floor
    /// junk, so unioning extra candidates only widens recall — it cannot lower
    /// precision. No-op when no semantic channel is wired. Mirrors
    /// `StructuredReadPipeline::union_semantic_recall` (ADR-069).
    async fn union_semantic_recall(
        &self,
        mut hits: Vec<RetrievedMemory>,
        query_text: &str,
        boundaries: &[vault_core::Boundary],
    ) -> VaultResult<Vec<RetrievedMemory>> {
        let Some(semantic) = &self.semantic else {
            return Ok(hits);
        };
        let sem_hits = semantic
            .retrieve(RetrievalQuery {
                query_text: query_text.to_string(),
                authorized_boundaries: boundaries.to_vec(),
                max_results: SEARCH_CANDIDATE_FANOUT,
                options: RetrievalOptions::default(),
            })
            .await?;
        let present: std::collections::HashSet<vault_core::MemoryId> =
            hits.iter().map(|h| h.memory.id).collect();
        for h in sem_hits {
            if !present.contains(&h.memory.id) {
                hits.push(h);
            }
        }
        Ok(hits)
    }

    /// Rerank the pool and re-sort by relevance DESC. **Reorder-only** — every
    /// candidate is kept; none are dropped. The reranker's job here is purely to
    /// pull the right answer up; the relevance/abstain judgment belongs to the
    /// calling agent (and to `memory_read`, the primary answer path).
    ///
    /// **Why never drop (ADR-071 revision, 2026-06-05).** The earlier design
    /// dropped below-`relevance_floor` candidates so search could return an empty
    /// "nothing found". Three-model live dogfood killed that: a false-empty does
    /// NOT make a weak agent abstain — it makes the agent (a) abandon the vault
    /// for competing memory (Opus 4.6 went hunting the IDE's brain) or (b) spin
    /// re-querying with 20 keyword variations for minutes (Flash on a true
    /// no-signal query). For a memory vault, a false-empty is the worst failure:
    /// it teaches the agent the vault is unreliable. Recall > precision — search
    /// never returns empty when candidates exist; `memory_read`'s structured
    /// `abstain` owns the honest "not in vault" signal instead.
    async fn rerank_pool(
        &self,
        query_text: &str,
        mut candidates: Vec<RetrievedMemory>,
    ) -> VaultResult<Vec<RetrievedMemory>> {
        if candidates.is_empty() {
            return Ok(candidates);
        }
        candidates.truncate(RERANK_CANDIDATE_CAP);

        let docs: Vec<String> = candidates
            .iter()
            .map(|c| c.memory.content.clone())
            .collect();
        let scores = self.reranker.rerank(query_text, &docs).await?;
        if scores.len() != candidates.len() {
            return Err(VaultError::Embedding(format!(
                "reranker returned {} scores for {} candidates",
                scores.len(),
                candidates.len()
            )));
        }

        let mut reordered: Vec<RetrievedMemory> = candidates
            .into_iter()
            .zip(scores)
            .map(|(mut c, logit)| {
                c.score = relevance_score(logit);
                c.explanation = format!("reranked: logit={logit:.4} → relevance={:.4}", c.score);
                c
            })
            .collect();
        // Re-sort by score DESC (the reranker, not the base, owns ordering now).
        // `relevance_score` is strictly monotonic in the logit, so this is the
        // logit order; NaN-free by construction (see `relevance_score`).
        reordered.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(reordered)
    }
}

/// Map an unbounded cross-encoder logit to a `[0, 1]` relevance score via the
/// logistic (sigmoid) function. Strictly monotonic (preserves the reranker's
/// rank order) and bounded, so a returned result never reads as a
/// "negative/irrelevant" score to the calling agent — replacing the earlier
/// `tanh ∈ (-1, 1)` mapping that showed the correct-but-subject-less cello fact
/// as `-0.95` (live dogfood, 2026-06-05). Sigmoid handles ±∞ correctly
/// (→ 1.0 / 0.0); a NaN logit (should never occur) maps to 0.0 to keep the
/// score finite and in range (`Retriever` invariant #4).
fn relevance_score(logit: f32) -> f32 {
    if logit.is_nan() {
        return 0.0;
    }
    1.0 / (1.0 + (-logit).exp())
}

#[async_trait]
impl Retriever for RerankedRetriever {
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        let started = Instant::now();
        let query_length = query.query_text.trim().len();
        let boundary_count = query.authorized_boundaries.len();
        let max_results = query.max_results;
        let score_threshold = query.options.score_threshold;
        let include_archived = query.options.include_archived;

        let result = self.retrieve_inner(query).await;

        // Invariant #5: emit exactly one structured diagnostic event, success or
        // error, with the canonical shape.
        match &result {
            Ok(out) => tracing::info!(
                target: "vault_retrieval::query",
                query_length,
                boundary_count,
                result_count = out.len(),
                max_results,
                score_threshold = ?score_threshold,
                include_archived,
                latency_ms = started.elapsed().as_millis() as u64,
                retriever = "reranked",
                "reranked search complete"
            ),
            Err(e) => tracing::info!(
                target: "vault_retrieval::query",
                query_length,
                boundary_count,
                result_count = 0_usize,
                max_results,
                score_threshold = ?score_threshold,
                include_archived,
                latency_ms = started.elapsed().as_millis() as u64,
                retriever = "reranked",
                error = %e,
                "reranked search failed"
            ),
        }
        result
    }
}

impl RerankedRetriever {
    /// The work, split out so [`Retriever::retrieve`] can wrap it in the
    /// single-event tracing contract on both success and error paths.
    async fn retrieve_inner(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // Result-limit contract (Q3): reject 0 / over-cap up front. The base
        // fetch below uses our own fan-out, so the base would never see — and
        // never reject — the caller's invalid value.
        if query.max_results == 0 || query.max_results > MAX_RESULTS_CAP {
            return Err(VaultError::InvalidInput(format!(
                "max_results must be in 1..={MAX_RESULTS_CAP}, got {}",
                query.max_results
            )));
        }

        let final_cap = query.max_results;
        // The caller's cosine `score_threshold` is meaningless once the reranker
        // owns relevance (scores become [0,1] reranker-relevance, not cosine), so
        // it is NOT passed to the base fetch. If the caller explicitly set one we
        // re-interpret it as a [0,1] relevance floor and apply it AFTER reranking
        // (their explicit filter — distinct from our own never-false-empty rule).
        let user_threshold = query.options.score_threshold;
        let boundaries = query.authorized_boundaries;
        let query_text = query.query_text;

        // 1. WIDE base fetch — the reranker needs a real pool, not the agent's
        //    small final top-K. `score_threshold` is dropped (see above);
        //    `include_archived` is preserved. Query-text validation propagates
        //    from the base.
        let hits = self
            .base
            .retrieve(RetrievalQuery {
                query_text: query_text.clone(),
                authorized_boundaries: boundaries.clone(),
                max_results: SEARCH_CANDIDATE_FANOUT,
                options: RetrievalOptions {
                    score_threshold: None,
                    include_archived: query.options.include_archived,
                },
            })
            .await?;

        // 2. Recall-union (ADR-069): rescue strong semantic matches RRF starves.
        let pool = self
            .union_semantic_recall(hits, &query_text, &boundaries)
            .await?;

        // 3. Rerank + re-sort by relevance. REORDER-ONLY — nothing is dropped, so
        //    the result is empty ONLY when the base pool was empty (genuinely no
        //    candidates / empty boundary). We never manufacture a false-empty.
        let mut ranked = self.rerank_pool(&query_text, pool).await?;

        // 4. Honor an EXPLICIT caller relevance threshold on the [0,1] scale (no
        //    floor by default → no false-empty from our side).
        if let Some(t) = user_threshold {
            ranked.retain(|r| r.score >= t);
        }

        // 5. Truncate to the agent's requested count (trait invariant #2 allows
        //    fewer).
        ranked.truncate(final_cap);
        Ok(ranked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use chrono::{DateTime, Utc};
    use vault_core::{Boundary, Memory, MemoryId, MemoryType, NewMemory};

    // ---------------------------------------------------------------------
    // Builders (local — mirror the pipeline test module's helpers).
    // ---------------------------------------------------------------------

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("valid test boundary")
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-05T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn fake_memory(id_n: u128, content: &str, boundary_name: &str) -> Memory {
        let mut m = Memory::try_new(NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary(boundary_name),
            source_agent: None,
            confidence: 0.9,
            valid_from: Some(now()),
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("static-valid test memory");
        m.id = MemoryId(uuid::Uuid::from_u128(id_n));
        m
    }

    fn retrieved(memory: Memory, score: f32) -> RetrievedMemory {
        RetrievedMemory {
            memory,
            score,
            explanation: format!("base: score={score:.4}"),
        }
    }

    /// Mock retriever — returns canned candidates, records the last query so a
    /// test can assert the fan-out width.
    struct MockRetriever {
        canned: Vec<RetrievedMemory>,
        last_query: Mutex<Option<RetrievalQuery>>,
    }

    impl MockRetriever {
        fn new(canned: Vec<RetrievedMemory>) -> Arc<Self> {
            Arc::new(Self {
                canned,
                last_query: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl Retriever for MockRetriever {
        async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
            *self.last_query.lock().unwrap() = Some(query);
            Ok(self.canned.clone())
        }
    }

    /// Mock reranker — content→logit map (unknown → `default`), configurable floor.
    struct MockReranker {
        scores: HashMap<String, f32>,
        default: f32,
        floor: f32,
    }

    impl MockReranker {
        fn new(scores: Vec<(&str, f32)>, default: f32, floor: f32) -> Arc<Self> {
            Arc::new(Self {
                scores: scores
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                default,
                floor,
            })
        }
    }

    #[async_trait]
    impl RerankProvider for MockReranker {
        async fn rerank(&self, _query: &str, docs: &[String]) -> VaultResult<Vec<f32>> {
            Ok(docs
                .iter()
                .map(|d| self.scores.get(d).copied().unwrap_or(self.default))
                .collect())
        }
        fn relevance_floor(&self) -> f32 {
            self.floor
        }
    }

    fn query(text: &str, max_results: usize) -> RetrievalQuery {
        RetrievalQuery {
            query_text: text.to_string(),
            authorized_boundaries: vec![boundary("personal")],
            max_results,
            options: RetrievalOptions::default(),
        }
    }

    // ---------------------------------------------------------------------
    // Tests
    // ---------------------------------------------------------------------

    /// The core promise: the reranker re-orders a deep-but-correct answer to
    /// the top. Base ranks the right fact LAST; the reranker scores it highest;
    /// it comes back at rank 1. This is the "cello #4 → #1" fix in miniature.
    #[tokio::test]
    async fn reranker_promotes_the_correct_answer_to_rank_one() {
        let keyboards = fake_memory(1, "collects vintage mechanical keyboards", "personal");
        let engineer = fake_memory(2, "works as a structural engineer", "personal");
        let cello = fake_memory(3, "plays the cello in a community orchestra", "personal");

        // Base puts the right answer (cello) LAST — the RRF-starved case.
        let base = MockRetriever::new(vec![
            retrieved(keyboards, 0.62),
            retrieved(engineer, 0.59),
            retrieved(cello, 0.40),
        ]);
        // Reranker scores the cello far above the distractors.
        let reranker = MockReranker::new(
            vec![
                ("plays the cello in a community orchestra", 6.5),
                ("collects vintage mechanical keyboards", -4.0),
                ("works as a structural engineer", -3.5),
            ],
            -10.0,
            -2.5,
        );
        let retriever = RerankedRetriever::new(base, None, reranker);

        let out = retriever
            .retrieve(query("what instrument does the user play", 10))
            .await
            .expect("retrieve ok");

        assert_eq!(
            out[0].memory.content, "plays the cello in a community orchestra",
            "the reranker must promote the correct answer to rank 1"
        );
        // REORDER-ONLY: the distractors are NOT dropped — they are returned,
        // just ranked below the cello.
        assert_eq!(out.len(), 3, "reorder-only must keep all candidates");
        // Score-domain: sigmoid(logit) ∈ (0, 1).
        assert!(
            (0.0..=1.0).contains(&out[0].score),
            "score must be in [0, 1], got {}",
            out[0].score
        );
        assert!(out[0].score > 0.99, "a +6.5 logit should map near 1.0");
    }

    /// Recall-safety (ADR-071 revision): even when EVERY candidate scores deeply
    /// negative (a true no-signal query like blood type), the facts are returned
    /// reordered — NEVER a false-empty. A false-empty makes weak agents abandon
    /// the vault or spin re-querying; `memory_read`'s `abstain` owns the honest
    /// "not in vault" signal instead.
    #[tokio::test]
    async fn never_returns_false_empty_even_when_all_score_negative() {
        let a = fake_memory(1, "owns a labrador named biscuit", "personal");
        let b = fake_memory(2, "drives a rivian", "personal");
        let base = MockRetriever::new(vec![retrieved(a, 0.5), retrieved(b, 0.45)]);
        // No content matches → everything gets a deeply negative default logit.
        let reranker = MockReranker::new(vec![], -6.0, -2.5);
        let retriever = RerankedRetriever::new(base, None, reranker);

        let out = retriever
            .retrieve(query("what is the user's blood type", 10))
            .await
            .expect("retrieve ok");

        assert_eq!(
            out.len(),
            2,
            "reorder-only must return the candidates, never a false-empty"
        );
        for r in &out {
            assert!(
                (0.0..=1.0).contains(&r.score),
                "even a deeply-negative logit maps into [0, 1], got {}",
                r.score
            );
        }
    }

    /// Recall-union: a strong semantic match the base pool MISSES entirely is
    /// unioned in, reranked, and surfaces. Without the union it could never be
    /// scored. (ADR-069 on the search path.)
    #[tokio::test]
    async fn recall_union_rescues_a_base_missed_semantic_match() {
        let distractor = fake_memory(1, "collects vintage mechanical keyboards", "personal");
        let cello = fake_memory(3, "plays the cello in a community orchestra", "personal");

        // Base returns ONLY the distractor (cello starved out of the hybrid).
        let base = MockRetriever::new(vec![retrieved(distractor, 0.6)]);
        // Semantic channel surfaces the cello. Annotated `dyn` so `Some(..)`
        // matches the `Option<Arc<dyn Retriever>>` param (unsizing does not
        // propagate through `Option`).
        let semantic: Arc<dyn Retriever> = MockRetriever::new(vec![retrieved(cello, 0.55)]);
        let reranker = MockReranker::new(
            vec![
                ("plays the cello in a community orchestra", 5.0),
                ("collects vintage mechanical keyboards", -4.0),
            ],
            -10.0,
            -2.5,
        );
        let retriever = RerankedRetriever::new(base, Some(semantic), reranker);

        let out = retriever
            .retrieve(query("what instrument does the user play", 10))
            .await
            .expect("retrieve ok");

        // Reorder-only: both are returned; the unioned-in cello is reranked to
        // the top despite the base missing it entirely.
        assert_eq!(out.len(), 2, "both candidates returned (reorder-only)");
        assert_eq!(
            out[0].memory.content, "plays the cello in a community orchestra",
            "the recall-union must rescue the base-missed semantic match to rank 1"
        );
    }

    /// The base is fetched WIDE (fan-out), not at the agent's small final cap —
    /// otherwise the reranker could not rescue a deep answer.
    #[tokio::test]
    async fn base_is_fetched_at_fanout_width_not_final_cap() {
        let base = MockRetriever::new(vec![]);
        let reranker = MockReranker::new(vec![], 1.0, -2.5);
        let retriever = RerankedRetriever::new(base.clone(), None, reranker);

        // Agent asks for just 3, but the base must be probed at the fan-out.
        let _ = retriever.retrieve(query("anything", 3)).await.unwrap();

        let seen = base.last_query.lock().unwrap().clone().unwrap();
        assert_eq!(
            seen.max_results, SEARCH_CANDIDATE_FANOUT,
            "base must be fetched at fan-out width, not the agent's final cap"
        );
    }

    /// The final result is truncated to the agent's `max_results` even when more
    /// candidates clear the floor.
    #[tokio::test]
    async fn truncates_to_requested_max_results() {
        let mems: Vec<RetrievedMemory> = (1..=5)
            .map(|i| retrieved(fake_memory(i, &format!("fact number {i}"), "personal"), 0.5))
            .collect();
        let base = MockRetriever::new(mems);
        // Reorder-only keeps all 5; the final truncate must cut to max_results=2.
        let reranker = MockReranker::new(vec![], 2.0, -2.5);
        let retriever = RerankedRetriever::new(base, None, reranker);

        let out = retriever.retrieve(query("facts", 2)).await.unwrap();
        assert_eq!(out.len(), 2, "must truncate to the requested max_results");
    }

    /// Result-limit contract (Q3): max_results == 0 and over-cap are rejected.
    #[tokio::test]
    async fn rejects_invalid_max_results() {
        let base = MockRetriever::new(vec![]);
        let reranker = MockReranker::new(vec![], 1.0, -2.5);
        let retriever = RerankedRetriever::new(base, None, reranker);

        assert!(
            retriever.retrieve(query("x", 0)).await.is_err(),
            "max_results == 0 must be rejected"
        );
        assert!(
            retriever
                .retrieve(query("x", MAX_RESULTS_CAP + 1))
                .await
                .is_err(),
            "over-cap max_results must be rejected"
        );
    }

    /// Scores are sorted DESC and every score is finite + in [0, 1] (Retriever
    /// invariants #3 and #4, with the sigmoid relevance mapping).
    #[tokio::test]
    async fn scores_are_sorted_desc_and_in_unit_range() {
        let hi = fake_memory(1, "high relevance fact", "personal");
        let mid = fake_memory(2, "middling relevance fact", "personal");
        let lo = fake_memory(3, "low relevance fact", "personal");
        let base = MockRetriever::new(vec![
            retrieved(lo, 0.4),
            retrieved(hi, 0.3),
            retrieved(mid, 0.2),
        ]);
        let reranker = MockReranker::new(
            vec![
                ("high relevance fact", 7.0),
                ("middling relevance fact", 1.0),
                ("low relevance fact", -1.0),
            ],
            -10.0,
            -2.5,
        );
        let retriever = RerankedRetriever::new(base, None, reranker);

        let out = retriever.retrieve(query("relevance", 10)).await.unwrap();
        assert_eq!(out.len(), 3);
        for w in out.windows(2) {
            assert!(w[0].score >= w[1].score, "scores must be sorted DESC");
        }
        for r in &out {
            assert!(
                r.score.is_finite() && (0.0..=1.0).contains(&r.score),
                "score {} must be finite and in [0, 1]",
                r.score
            );
        }
        assert_eq!(out[0].memory.content, "high relevance fact");
    }

    /// An EXPLICIT caller `score_threshold` is honored on the [0,1] relevance
    /// scale (their filter) — distinct from our never-false-empty default.
    #[tokio::test]
    async fn explicit_score_threshold_filters_on_relevance_scale() {
        let strong = fake_memory(1, "strong match", "personal");
        let weak = fake_memory(2, "weak match", "personal");
        let base = MockRetriever::new(vec![retrieved(strong, 0.5), retrieved(weak, 0.4)]);
        // sigmoid(3.0) ≈ 0.95 ; sigmoid(-1.0) ≈ 0.27.
        let reranker = MockReranker::new(
            vec![("strong match", 3.0), ("weak match", -1.0)],
            -10.0,
            -2.5,
        );
        let retriever = RerankedRetriever::new(base, None, reranker);

        let mut q = query("match", 10);
        q.options.score_threshold = Some(0.5);
        let out = retriever.retrieve(q).await.unwrap();

        assert_eq!(out.len(), 1, "only the above-threshold result survives");
        assert_eq!(out[0].memory.content, "strong match");
    }
}
