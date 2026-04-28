//! `vault-consolidator` — the sleep cycle. Nightly job that merges duplicates,
//! decays old memories, resolves contradictions, and produces summaries.
//! The product gets better with use.
//!
//! See `Agent Build Specification.txt` §5.6 for the public API specification.
//! Real implementation lands across T0.2.2 → T0.2.6 (V0.2).

#![forbid(unsafe_code)]
