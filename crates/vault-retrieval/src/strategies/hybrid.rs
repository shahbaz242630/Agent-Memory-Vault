//! Hybrid retrieval strategy — Reciprocal Rank Fusion over a semantic
//! channel + a keyword channel.
//!
//! [`HybridRetriever`] composes two [`Retriever`]s (typically
//! [`crate::SemanticRetriever`] for dense BGE search and
//! [`crate::KeywordRetriever`] for BM25 lexical search), fires them in
//! parallel via [`tokio::try_join!`], then fuses their ranked outputs
//! using Reciprocal Rank Fusion (RRF, Cormack et al. 2009).
//!
//! ## V0.2 design (T0.2.7 Phase 2)
//!
//! **Loose coupling via `Arc<dyn Retriever>`.** The hybrid doesn't care
//! whether its inputs are `SemanticRetriever`, `KeywordRetriever`, or
//! some future `MultiStrategyRetriever` — it consumes the `Retriever`
//! trait surface only. This makes the type composable with anything
//! satisfying the same trait, and keeps the hybrid agnostic to the
//! underlying index implementations.
//!
//! **Parallel channel execution via `tokio::try_join!`.** Both channels
//! run concurrently; total latency is `max(semantic, keyword)` rather
//! than `sum`. If either channel errors, the join propagates the error
//! without waiting for the other.
//!
//! **RRF formula** — for each unique memory across both channels:
//!
//! ```text
//! rrf_score(memory) = 1/(k + sem_rank) + 1/(k + kw_rank)
//! ```
//!
//! where `sem_rank` / `kw_rank` are 1-indexed positions in each
//! channel's returned list. A memory absent from one channel
//! contributes `0` from that channel. With the default `k = 60`,
//! RRF scores fall in `[0, 2/(60+1)] ≈ [0, 0.0328]` — well inside the
//! `Retriever` trait's `[-1, 1]` score window.
//!
//! **Per-channel widening (`top_n_each`).** Each channel is asked for
//! `top_n_each` candidates regardless of the caller's `max_results`,
//! so RRF has a wider candidate pool to fuse. The default of 200
//! matches the T0.2.7 spike's empirical setting (`HYBRID_TOP_N_EACH`)
//! that surfaced Q25's Memory B at SCALE=10K diverse corpus. After
//! fusion, output is truncated to the caller's `query.max_results`.
//!
//! **Tiebreak ordering** — equal RRF scores break by `Memory::created_at
//! DESC`, matching the [`Retriever`] trait invariant #3 ordering
//! convention.
//!
//! **Boundary filtering** happens inside each child retriever (the
//! hybrid does NOT re-filter); composition inherits the contract
//! without duplicate work.
//!
//! ## What this module does NOT do (deferred to later phases)
//!
//! - No abstain gate (top-1 BM25 score check) — Phase 3.
//! - No vault-app write-path wiring — Phase 1.5 / Phase 4.
//! - No production prompt + MCP exposure — Phase 4.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;

use vault_core::{Memory, MemoryId, VaultError, VaultResult};

use crate::retriever::{
    RetrievalQuery, RetrievedMemory, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP,
};

/// Tunable knobs for [`HybridRetriever`].
///
/// Defaults match the T0.2.7 spike's tested-known-good values:
///
/// - `top_n_each = 200` — per-channel pool size BEFORE fusion. Wide
///   enough to surface Q25's Memory B at SCALE=10K diverse corpus
///   (spike empirical anchor).
/// - `rrf_k = 60` — Cormack et al. 2009 literature default. Locked
///   pending ADR-050; the spike confirmed no change needed across
///   v3a / v3b / v3c at SCALE=10K.
#[derive(Clone, Debug)]
pub struct HybridConfig {
    /// Number of candidates to pull from each child retriever before
    /// fusion. Capped at [`MAX_RESULTS_CAP`] when used.
    pub top_n_each: usize,
    /// RRF constant. Larger `k` flattens the contribution differential
    /// between ranks; `k = 60` is the literature default.
    pub rrf_k: usize,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            top_n_each: 200,
            rrf_k: 60,
        }
    }
}

/// Composes two [`Retriever`]s via Reciprocal Rank Fusion. See module
/// docs for the design + invariants.
///
/// Cheap to clone — wraps the children in `Arc` so cloning is just
/// refcount bumps. Share freely across tasks.
///
/// Per ADR-007 precedent: does NOT implement `Debug` (children may
/// hold live storage handles).
#[derive(Clone)]
pub struct HybridRetriever {
    semantic: Arc<dyn Retriever>,
    keyword: Arc<dyn Retriever>,
    config: HybridConfig,
}

impl HybridRetriever {
    /// Construct with default [`HybridConfig`] (k=60, top_n_each=200).
    pub fn new(semantic: Arc<dyn Retriever>, keyword: Arc<dyn Retriever>) -> Self {
        Self::with_config(semantic, keyword, HybridConfig::default())
    }

    /// Construct with explicit [`HybridConfig`]. Use this when tuning
    /// the fusion knobs (e.g., calibration experiments).
    pub fn with_config(
        semantic: Arc<dyn Retriever>,
        keyword: Arc<dyn Retriever>,
        config: HybridConfig,
    ) -> Self {
        Self {
            semantic,
            keyword,
            config,
        }
    }
}

#[async_trait]
impl Retriever for HybridRetriever {
    #[instrument(
        skip(self, query),
        fields(
            query_len = query.query_text.len(),
            boundary_count = query.authorized_boundaries.len(),
            max_results = query.max_results,
            rrf_k = self.config.rrf_k,
            top_n_each = self.config.top_n_each,
        )
    )]
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // ── Q2 / Q3 validation at the hybrid layer for consistent
        // error surface (child retrievers do the same; we want the
        // failure to surface BEFORE we round-trip to both channels).
        let trimmed = query.query_text.trim();
        if trimmed.is_empty() {
            return Err(VaultError::InvalidInput(
                "query_text must be non-empty after trim".into(),
            ));
        }
        if query.query_text.len() > MAX_QUERY_BYTES {
            return Err(VaultError::InvalidInput(format!(
                "query_text exceeds MAX_QUERY_BYTES={MAX_QUERY_BYTES}"
            )));
        }
        if query.max_results == 0 || query.max_results > MAX_RESULTS_CAP {
            return Err(VaultError::InvalidInput(format!(
                "max_results must be in 1..={MAX_RESULTS_CAP}"
            )));
        }
        // ── Q1 short-circuit ────────────────────────────────────────
        if query.authorized_boundaries.is_empty() {
            return Ok(Vec::new());
        }

        // ── Widen per-channel pool to `top_n_each`, capped by the
        // trait's MAX_RESULTS_CAP. We always pull at least 1.
        let top_n = self.config.top_n_each.clamp(1, MAX_RESULTS_CAP);
        let widened = RetrievalQuery {
            max_results: top_n,
            ..query.clone()
        };

        // ── Fire both channels in parallel. ─────────────────────────
        let (sem_results, kw_results) = tokio::try_join!(
            self.semantic.retrieve(widened.clone()),
            self.keyword.retrieve(widened),
        )?;

        // ── Build 1-indexed rank maps. ──────────────────────────────
        let mut sem_rank: HashMap<MemoryId, usize> = HashMap::with_capacity(sem_results.len());
        for (i, m) in sem_results.iter().enumerate() {
            sem_rank.insert(m.memory.id, i + 1);
        }
        let mut kw_rank: HashMap<MemoryId, usize> = HashMap::with_capacity(kw_results.len());
        for (i, m) in kw_results.iter().enumerate() {
            kw_rank.insert(m.memory.id, i + 1);
        }

        // ── Union of IDs + hydration map. Either channel's
        // `RetrievedMemory` carries a fully-hydrated `Memory`; we
        // prefer the semantic-side copy when both have it (arbitrary
        // but deterministic — they should be byte-equal anyway since
        // both channels hydrate from the same metadata store).
        let mut all_ids: HashSet<MemoryId> = HashSet::new();
        all_ids.extend(sem_rank.keys().copied());
        all_ids.extend(kw_rank.keys().copied());

        let mut by_id: HashMap<MemoryId, Memory> = HashMap::with_capacity(all_ids.len());
        for r in sem_results.iter().chain(kw_results.iter()) {
            by_id.entry(r.memory.id).or_insert_with(|| r.memory.clone());
        }

        // ── Compute RRF score per unique memory + track contribution
        // ranks for the explanation.
        let k_f = self.config.rrf_k as f32;
        let mut scored: Vec<(Memory, f32, Option<usize>, Option<usize>)> =
            Vec::with_capacity(all_ids.len());
        for id in all_ids {
            let Some(memory) = by_id.remove(&id) else {
                continue;
            };
            let sr = sem_rank.get(&id).copied();
            let kr = kw_rank.get(&id).copied();
            let s_sem = sr.map_or(0.0, |r| 1.0 / (k_f + r as f32));
            let s_kw = kr.map_or(0.0, |r| 1.0 / (k_f + r as f32));
            scored.push((memory, s_sem + s_kw, sr, kr));
        }

        // ── Sort by RRF DESC, tiebreak created_at DESC. ─────────────
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| b.0.created_at.cmp(&a.0.created_at))
        });
        scored.truncate(query.max_results);

        // ── Emit RetrievedMemory with hybrid explanation. ───────────
        let out: Vec<RetrievedMemory> = scored
            .into_iter()
            .map(|(memory, rrf, sr, kr)| {
                let explanation = format!(
                    "hybrid: rrf={:.4} (sem_rank={} · kw_rank={})",
                    rrf,
                    sr.map(|r| r.to_string()).unwrap_or_else(|| "—".into()),
                    kr.map(|r| r.to_string()).unwrap_or_else(|| "—".into()),
                );
                RetrievedMemory {
                    memory,
                    score: rrf,
                    explanation,
                }
            })
            .collect();

        Ok(out)
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn default_config_matches_spike() {
        let cfg = HybridConfig::default();
        assert_eq!(cfg.top_n_each, 200);
        assert_eq!(cfg.rrf_k, 60);
    }

    #[test]
    fn rrf_score_upper_bound_default_k() {
        // Both channels rank a memory at #1 → max possible RRF score.
        let k = 60.0_f32;
        let max = 2.0 / (k + 1.0);
        assert!((max - 0.0327868).abs() < 1e-5, "got {max}");
    }
}
