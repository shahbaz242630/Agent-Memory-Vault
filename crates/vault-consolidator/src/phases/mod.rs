//! Sleep-cycle phase primitives.
//!
//! Each phase of the consolidation cycle (BRD §5.6 lines 933-955) lives in its
//! own sub-module so the orchestrator (`crate::consolidator::Consolidator`) is
//! a thin glue layer over per-phase primitives that are individually unit-
//! testable.
//!
//! | Phase | Module | Lands at |
//! |---|---|---|
//! | 1 — Identify candidate clusters | [`cluster`] | T0.2.2 (shipped at `a889931` + `a53e3a5`) |
//! | 2 — LLM merge decisions | `merge` (not yet created) | T0.2.3 commit 1 |
//! | 3 — Apply merges | `merge` (not yet created) | T0.2.3 commit 2 |
//! | 4 — Decay and archive | `decay` (not yet created) | T0.2.4 |
//!
//! File layout matches BRD §5.6 lines 987-989 verbatim (T0.2.3 commit 1
//! refactor — T0.2.2 commit 1 shipped clustering as `src/clustering.rs`
//! flat-layout; corrected here).

pub mod cluster;
pub mod merge;
