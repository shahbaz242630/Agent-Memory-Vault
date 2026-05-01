//! Trait-level invariants for `Retriever` implementers.
//!
//! The generic harness [`assert_boundary_leakage_invariant`] is the
//! load-bearing piece: any new `Retriever` impl (V0.1 = `SemanticRetriever`,
//! T0.2.7 = `MultiStrategyRetriever`) re-runs this exact harness and a
//! boundary-leak regression fails the build.
//!
//! Phase 1 scaffolds the harness shape and a `SemanticRetriever`-driven
//! call site that panics at `unimplemented!()`. Phase 2's first run
//! turns the harness from "panicking" to "real assertion firing".

mod common;

use common::{boundary, insert_memory_with_drift, make_memory, make_test_retriever, query};
use vault_retrieval::Retriever;

/// Generic boundary-leak invariant. Re-usable from any `Retriever`
/// implementer's integration test.
///
/// Loads a vault containing memories in three boundaries (`work`,
/// `personal`, `secret`), then issues two retrievals:
///
/// 1. Authorised for `work` only — every returned memory must have
///    `boundary == "work"`.
/// 2. Authorised for `work` + `personal` — every returned memory's
///    boundary must be in `{work, personal}`; no `secret` may leak.
///
/// The harness deliberately uses *real* `MetadataStore` +
/// `LanceVectorStore` (not stubs) so the SQL-layer boundary filter is
/// exercised end-to-end. Phase 2 wires the retriever body and this
/// fires for real.
pub async fn assert_boundary_leakage_invariant<R: Retriever>(retriever: R) {
    let t = make_test_retriever().await;
    let work = boundary("work");
    let personal = boundary("personal");
    let secret = boundary("secret");

    for (idx, name) in ["w1", "w2", "w3"].iter().enumerate() {
        let m = make_memory(&format!("work memory {name}"), &work);
        insert_memory_with_drift(&t, &m, idx + 1).await;
    }
    for (idx, name) in ["p1", "p2"].iter().enumerate() {
        let m = make_memory(&format!("personal memory {name}"), &personal);
        insert_memory_with_drift(&t, &m, idx + 10).await;
    }
    for (idx, name) in ["s1", "s2"].iter().enumerate() {
        let m = make_memory(&format!("secret memory {name}"), &secret);
        insert_memory_with_drift(&t, &m, idx + 20).await;
    }

    // 1. Single-boundary auth.
    let res = retriever
        .retrieve(query("anything", vec![work.clone()], 100))
        .await
        .expect("retrieve work-only");
    for r in &res {
        assert_eq!(
            r.memory.boundary, work,
            "boundary leak: returned memory {:?} has boundary {:?}, expected only 'work'",
            r.memory.id, r.memory.boundary
        );
    }

    // 2. Two-boundary auth — `secret` must never leak.
    let res = retriever
        .retrieve(query("anything", vec![work.clone(), personal.clone()], 100))
        .await
        .expect("retrieve work+personal");
    for r in &res {
        assert!(
            r.memory.boundary == work || r.memory.boundary == personal,
            "boundary leak: returned memory {:?} has boundary {:?}, expected only 'work' or 'personal'",
            r.memory.id,
            r.memory.boundary
        );
        assert_ne!(
            r.memory.boundary, secret,
            "boundary leak: secret memory {:?} returned to caller authorised for work+personal only",
            r.memory.id
        );
    }
}

/// Drive the generic invariant against `SemanticRetriever` — Phase 1
/// panics at `unimplemented!()`; Phase 2 turns this green.
#[tokio::test]
#[should_panic(expected = "T0.1.8 Phase 2")]
async fn semantic_retriever_does_not_leak_across_boundaries() {
    let t = make_test_retriever().await;
    assert_boundary_leakage_invariant(t.retriever).await;
}
