//! Test-only fault injection for adversarial tests of the cascading
//! orchestrator (T0.1.6 Phase A Q5, Phase C1b implementation).
//!
//! Used by `cascading.rs`'s adversarial tests in C1b to drive specific
//! failure scenarios deterministically:
//! - Persistent LanceDB write failure → dead-letter (Q5 test 2)
//! - Retry queue overflow under sustained LanceDB failure (Q5 test 5)
//! - Mid-cascade abort + recovery (Q5 test 1) — requires injecting between
//!   the SQLite ack and the downstream write
//!
//! ## Cost in production
//!
//! Zero. The whole file is gated `#![cfg(test)]` — production builds don't
//! compile it. The orchestrator's fault-injection hook in `cascading.rs`
//! will be `#[cfg(test)]`-gated too (decided in C1b alongside the
//! adversarial tests); production paths use direct calls into
//! `LanceVectorStore` / `DuckDbGraphStore` with no indirection.
//!
//! ## Why a trait, not an enum?
//!
//! Adversarial tests need to express *stateful* fault patterns ("fail the
//! first 3 attempts, then allow", "panic on the next call only"). A trait
//! gives each test the freedom to construct an `Arc<dyn FaultInjector>`
//! with whatever state shape it needs without bloating an enum that has
//! to serve every scenario.

#![cfg(test)]

use vault_core::{VaultError, VaultResult};

/// What the orchestrator's fault hook should do for the next operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FaultDecision {
    /// Allow the operation through normally — this is the production path.
    Allow,
    /// Substitute a [`VaultError::Storage`] with the given message.
    /// Used to drive transient-failure cascades into the retry queue.
    Fail(String),
}

/// Test-only trait. C1b's `cascading.rs` accepts an `Arc<dyn FaultInjector>`
/// in its test-only constructor (`new_for_test`) and consults it before
/// each downstream store call.
pub trait FaultInjector: Send + Sync {
    /// Decision for the next vector-store operation.
    fn vector_decision(&self) -> FaultDecision;
    /// Decision for the next graph-store operation.
    fn graph_decision(&self) -> FaultDecision;
}

/// No-op injector. Always allows. Used as the default when a test wants
/// the orchestrator to behave normally except where it explicitly injects.
pub struct NoFault;

impl FaultInjector for NoFault {
    fn vector_decision(&self) -> FaultDecision {
        FaultDecision::Allow
    }
    fn graph_decision(&self) -> FaultDecision {
        FaultDecision::Allow
    }
}

/// Always fail the vector-store op with the given message; allow graph-store
/// ops through. Drives "persistent LanceDB failure" scenarios (Q5 test 2 + 5).
pub struct AlwaysFailVector(pub String);

impl FaultInjector for AlwaysFailVector {
    fn vector_decision(&self) -> FaultDecision {
        FaultDecision::Fail(self.0.clone())
    }
    fn graph_decision(&self) -> FaultDecision {
        FaultDecision::Allow
    }
}

/// Always fail the graph-store op with the given message; allow vector-store
/// ops through. Symmetric counterpart for DuckDB-side failure scenarios.
pub struct AlwaysFailGraph(pub String);

impl FaultInjector for AlwaysFailGraph {
    fn vector_decision(&self) -> FaultDecision {
        FaultDecision::Allow
    }
    fn graph_decision(&self) -> FaultDecision {
        FaultDecision::Fail(self.0.clone())
    }
}

/// Translate a [`FaultDecision`] into a `Result` the orchestrator can
/// `?` against. The error variant is `VaultError::Storage` because that's
/// the realistic shape for transient downstream-store failures (and what
/// the retry queue's `is_permanent` classifier expects on the transient
/// path).
pub(crate) fn into_result(decision: FaultDecision) -> VaultResult<()> {
    match decision {
        FaultDecision::Allow => Ok(()),
        FaultDecision::Fail(msg) => Err(VaultError::Storage(msg)),
    }
}

// -----------------------------------------------------------------------
// Self-validation tests (cargo test will compile + run these in the
// test profile; the cfg(test) gate at the top of the file already
// excludes everything from production)
// -----------------------------------------------------------------------

mod tests {
    use super::*;

    #[test]
    fn no_fault_allows_both_sides() {
        let f = NoFault;
        assert_eq!(f.vector_decision(), FaultDecision::Allow);
        assert_eq!(f.graph_decision(), FaultDecision::Allow);
    }

    #[test]
    fn always_fail_vector_only_fails_vector() {
        let f = AlwaysFailVector("simulated lance io".into());
        assert_eq!(
            f.vector_decision(),
            FaultDecision::Fail("simulated lance io".into())
        );
        assert_eq!(f.graph_decision(), FaultDecision::Allow);
    }

    #[test]
    fn always_fail_graph_only_fails_graph() {
        let f = AlwaysFailGraph("simulated duckdb io".into());
        assert_eq!(f.vector_decision(), FaultDecision::Allow);
        assert_eq!(
            f.graph_decision(),
            FaultDecision::Fail("simulated duckdb io".into())
        );
    }

    #[test]
    fn into_result_translates_allow_to_ok() {
        assert!(into_result(FaultDecision::Allow).is_ok());
    }

    #[test]
    fn into_result_translates_fail_to_storage_error() {
        let err = into_result(FaultDecision::Fail("boom".into())).unwrap_err();
        assert!(matches!(err, VaultError::Storage(s) if s == "boom"));
    }
}
