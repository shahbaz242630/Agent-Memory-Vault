//! Abstain-gate decorator — short-circuits retrieval to an empty result
//! when the lexical (BM25) channel's top score falls below a calibrated
//! threshold.
//!
//! [`AbstainingRetriever`] wraps an inner [`Retriever`] (typically
//! [`crate::HybridRetriever`], but composes with anything implementing
//! the trait) plus a BM25-providing retriever (typically the same
//! [`crate::KeywordRetriever`] that powers the hybrid's lexical channel).
//! Before delegating to the inner retriever, it probes the keyword
//! channel for the top BM25 hit. If the best score is below
//! [`AbstainConfig::bm25_top_score_threshold`], the wrapper returns
//! `Ok(vec![])` — the downstream [`crate::ReadPipeline`] sees this as
//! "vault has no relevant content" and short-circuits without an LLM
//! call.
//!
//! ## V0.2 design (T0.2.7 Phase 3)
//!
//! **Why a decorator instead of inline in ReadPipeline.** The abstain
//! signal is composable — a wrapper that adds the gate works with any
//! `Retriever` impl, including future ones (e.g.,
//! `MultiStrategyRetriever`). Inlining in `ReadPipeline` would couple
//! that crate to BM25 specifics and require a custom field on `Self`;
//! the decorator approach keeps abstain a vault-retrieval-internal
//! concern and lets `ReadPipeline` remain BM25-agnostic.
//!
//! **Why top-1 score, not count.** The T0.2.7 spike's first abstain
//! design counted "hits with BM25 score above floor"; that metric is
//! scale-dependent (at SCALE=10K, every query has 200+ weak topic-
//! overlap hits, so the count never trips on hard-negs).
//!
//! **Why threshold 1.0 (not 6.0).** The spike's original 6.0 calibration
//! was derived from its synthetic corpus where hard-neg query tokens had
//! ZERO lexical overlap with any planted memory — hard-negs scored 0–5,
//! contradictions scored 8–15, 6.0 sat between. That separation does
//! NOT survive contact with hand-curated natural prose:
//!
//! - At hand-curated 100-scale (T0.2.7 Phase 5 Step 2 diagnostic
//!   2026-05-21): Q25 contradiction scored 5.09 (the GA-launch contradiction
//!   target was best found via the semantic channel, not BM25), Q21
//!   hard-neg scored 5.54 ("database migration scripts" topical-noise hit
//!   with residual content-word match). BM25 distributions for hard-negs
//!   and contradictions OVERLAP — no single BM25 threshold can pass all
//!   contradictions AND fail all hard-negs.
//! - At scale (1K → 10K), rare-anchor IDF amplification grows hard-neg
//!   BM25 scores too (e.g., "migration" appears in ~4 hand-curated memories;
//!   at 10K with no distractor matches, IDF(migration) more than doubles
//!   from ~3.1 to ~7.7). Threshold 6.0 would not abstain on Q21 at scale
//!   either — the gate stops doing useful work for hard-negs as scale grows.
//!
//! The honest architectural conclusion: BM25 top-1 alone cannot
//! reliably discriminate "real relevance" from "topical noise" across
//! the corpus mix we ship. The LLM (per the read-time system prompt's
//! explicit relevance rule) is the only correct gate.
//!
//! Threshold 1.0 therefore catches only the genuinely-empty-signal case
//! (gibberish queries with no meaningful token match anywhere). Everything
//! else proceeds to the LLM, which judges relevance per the system
//! prompt — including hard-negs like Q21 where the prompt has a Kubernetes-
//! vs-database-migration example built in. See
//! `tests/abstain_channel_diagnostic.rs` for the calibration data.
//!
//! **Score-range pass-through.** When abstain does NOT fire, the
//! wrapper delegates `inner.retrieve(...)` unchanged. The inner
//! retriever's score semantics (RRF, cosine, etc.) are preserved in
//! the output — `AbstainingRetriever` is invisible to consumers when
//! it passes through.
//!
//! **Boundary filter pass-through.** The probe query carries the
//! caller's `authorized_boundaries` slice unchanged; the keyword
//! channel filters at its layer (per the Phase 1 contract). The inner
//! retriever filters at ITS layer. Defense-in-depth, no leakage.
//!
//! ## What this module does NOT do (deferred to later phases)
//!
//! - No persistence (the gate is stateless beyond the AbstainConfig).
//! - No per-boundary or per-user threshold tuning — single global
//!   threshold for V0.2.
//! - No "explain why we abstained" telemetry field beyond
//!   `tracing::info!` events — Phase 4/5 ReadPipeline polish.

use std::sync::Arc;

use async_trait::async_trait;
use tracing::instrument;

use vault_core::{VaultError, VaultResult};

use crate::retriever::{
    RetrievalQuery, RetrievedMemory, Retriever, MAX_QUERY_BYTES, MAX_RESULTS_CAP,
};

/// Tunable knobs for [`AbstainingRetriever`].
///
/// Default `bm25_top_score_threshold = 1.0` — see module docs for the
/// rationale ("Why threshold 1.0"). Briefly: the spike's 6.0 calibration
/// didn't survive hand-curated natural prose at any scale, so the gate
/// is dialed down to catch only genuine-zero-signal queries (gibberish,
/// no matching tokens). All non-trivial queries proceed to the LLM,
/// which is the only correct relevance judge per the read-time system
/// prompt.
#[derive(Clone, Debug)]
pub struct AbstainConfig {
    /// Top-1 BM25 score required to NOT abstain. If the best BM25 hit
    /// across the user's authorized boundaries scores strictly below
    /// this, the wrapper returns an empty `Vec` without invoking the
    /// inner retriever.
    pub bm25_top_score_threshold: f32,
}

impl Default for AbstainConfig {
    fn default() -> Self {
        Self {
            bm25_top_score_threshold: 1.0,
        }
    }
}

/// Wraps an inner [`Retriever`] with a top-1 BM25 abstain gate. See
/// module docs for the design + invariants.
///
/// Cheap to clone — wraps the children in `Arc` so cloning is just
/// refcount bumps. Share freely across tasks.
///
/// Per ADR-007 precedent: does NOT implement `Debug` (children may
/// hold live storage handles).
#[derive(Clone)]
pub struct AbstainingRetriever {
    inner: Arc<dyn Retriever>,
    keyword: Arc<dyn Retriever>,
    config: AbstainConfig,
}

impl AbstainingRetriever {
    /// Construct with default [`AbstainConfig`] (threshold = 6.0).
    pub fn new(inner: Arc<dyn Retriever>, keyword: Arc<dyn Retriever>) -> Self {
        Self::with_config(inner, keyword, AbstainConfig::default())
    }

    /// Construct with an explicit [`AbstainConfig`]. Use this when
    /// tuning the threshold (e.g., calibration experiments).
    pub fn with_config(
        inner: Arc<dyn Retriever>,
        keyword: Arc<dyn Retriever>,
        config: AbstainConfig,
    ) -> Self {
        Self {
            inner,
            keyword,
            config,
        }
    }
}

#[async_trait]
impl Retriever for AbstainingRetriever {
    #[instrument(
        skip(self, query),
        fields(
            query_len = query.query_text.len(),
            boundary_count = query.authorized_boundaries.len(),
            max_results = query.max_results,
            threshold = self.config.bm25_top_score_threshold,
        )
    )]
    async fn retrieve(&self, query: RetrievalQuery) -> VaultResult<Vec<RetrievedMemory>> {
        // ── Q2 / Q3 validation at this layer for consistent error
        // surface (child retrievers also validate; we want errors to
        // surface BEFORE either round-trip).
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
        // Empty boundaries → no probe round-trip needed; the inner
        // retriever also short-circuits, but skipping the probe saves
        // a round-trip.
        if query.authorized_boundaries.is_empty() {
            return Ok(Vec::new());
        }

        // ── Probe the keyword channel for the top BM25 score. ───────
        // The probe carries the SAME authorized_boundaries +
        // query_text + options as the user's request, but uses a
        // generous pool size. Pool > 1 is load-bearing: the keyword
        // channel applies boundary filtering POST-hydration, so the
        // raw Tantivy top-1 hit might be in a non-authorized boundary
        // and get filtered out. A pool of [`MAX_RESULTS_CAP`] (200)
        // matches the T0.2.7 spike's `top_n_each` and gives plenty
        // of headroom for cross-boundary BM25 noise without measurable
        // cost (Tantivy in-RAM search at limit=200 is sub-millisecond).
        // We only read `.first()` from the result; the rest is just
        // headroom.
        let probe_query = RetrievalQuery {
            max_results: MAX_RESULTS_CAP,
            ..query.clone()
        };
        let probe_hits = self.keyword.retrieve(probe_query).await?;
        let top_score = probe_hits.first().map(|m| m.score).unwrap_or(0.0);

        if top_score < self.config.bm25_top_score_threshold {
            tracing::info!(
                target: "vault_retrieval::abstain",
                top_bm25_score = top_score,
                threshold = self.config.bm25_top_score_threshold,
                "abstain fired: top BM25 score below threshold; returning empty result"
            );
            return Ok(Vec::new());
        }

        // ── Pass-through: delegate to inner. ────────────────────────
        // The inner retriever's full output is returned unchanged —
        // score range, ordering, and explanation strings all preserved.
        self.inner.retrieve(query).await
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn default_threshold_matches_v0_2_calibration() {
        // V0.2 calibration: 1.0, dialed down from the spike's 6.0 after
        // hand-curated fixture diagnostics surfaced the BM25-distribution
        // overlap between hard-negs and contradictions. See module docs.
        let cfg = AbstainConfig::default();
        assert!((cfg.bm25_top_score_threshold - 1.0).abs() < 1e-6);
    }

    #[test]
    fn custom_threshold_round_trips() {
        let cfg = AbstainConfig {
            bm25_top_score_threshold: 9.5,
        };
        assert!((cfg.bm25_top_score_threshold - 9.5).abs() < 1e-6);
    }
}
