//! Trait-level invariants for `Retriever` implementers.
//!
//! The generic harness [`assert_boundary_leakage_invariant`] is the
//! load-bearing piece: any new `Retriever` impl (V0.1 = `SemanticRetriever`,
//! T0.2.7 = `MultiStrategyRetriever`) re-runs this exact harness and a
//! boundary-leak regression fails the build.
//!
//! Phase 2 wires the harness to actually populate the bundle's stores
//! (the same handles the retriever points at) and asserts the leak
//! property on real `retrieve()` output.

mod common;

use common::{boundary, insert_memory_with_drift, make_memory, make_test_retriever, TestRetriever};
use vault_retrieval::{RetrievalOptions, RetrievalQuery, Retriever};

/// Generic boundary-leak invariant. Re-usable from any `Retriever`
/// implementer's integration test. **Caller must ensure `retriever`
/// uses `b`'s stores** — in V0.1 this means passing `&b.retriever`
/// (which was constructed from `b.metadata` + `b.vectors`); at T0.2.7
/// the same pattern works for `MultiStrategyRetriever` constructed from
/// the same handles.
///
/// Loads a vault with memories in three boundaries (`work`, `personal`,
/// `secret`), then issues two retrievals:
///
/// 1. Authorised for `work` only — every returned memory must have
///    `boundary == "work"`.
/// 2. Authorised for `work` + `personal` — every returned memory's
///    boundary must be in `{work, personal}`; no `secret` may leak.
///
/// The harness uses the bundle's *real* `MetadataStore` +
/// `LanceVectorStore` (not stubs) so the SQL-layer `only_if` boundary
/// filter is exercised end-to-end.
pub async fn assert_boundary_leakage_invariant<R: Retriever>(b: &TestRetriever, retriever: &R) {
    let work = boundary("work");
    let personal = boundary("personal");
    let secret = boundary("secret");

    for (idx, name) in ["w1", "w2", "w3"].iter().enumerate() {
        let m = make_memory(&format!("work memory {name}"), &work);
        insert_memory_with_drift(b, &m, idx + 1).await;
    }
    for (idx, name) in ["p1", "p2"].iter().enumerate() {
        let m = make_memory(&format!("personal memory {name}"), &personal);
        insert_memory_with_drift(b, &m, idx + 10).await;
    }
    for (idx, name) in ["s1", "s2"].iter().enumerate() {
        let m = make_memory(&format!("secret memory {name}"), &secret);
        insert_memory_with_drift(b, &m, idx + 20).await;
    }

    let q1 = RetrievalQuery {
        query_text: "anything".into(),
        authorized_boundaries: vec![work.clone()],
        max_results: 100,
        options: RetrievalOptions::default(),
    };
    let res = retriever.retrieve(q1).await.expect("retrieve work-only");
    for r in &res {
        assert_eq!(
            r.memory.boundary, work,
            "boundary leak: returned memory {:?} has boundary {:?}, expected only 'work'",
            r.memory.id, r.memory.boundary
        );
    }

    let q2 = RetrievalQuery {
        query_text: "anything".into(),
        authorized_boundaries: vec![work.clone(), personal.clone()],
        max_results: 100,
        options: RetrievalOptions::default(),
    };
    let res = retriever
        .retrieve(q2)
        .await
        .expect("retrieve work+personal");
    for r in &res {
        assert!(
            r.memory.boundary == work || r.memory.boundary == personal,
            "boundary leak: returned memory {:?} has boundary {:?}, expected only 'work' or 'personal'",
            r.memory.id, r.memory.boundary
        );
        assert_ne!(
            r.memory.boundary, secret,
            "boundary leak: secret memory {:?} returned to caller authorised for work+personal only",
            r.memory.id
        );
    }
}

/// Drive the generic invariant against `SemanticRetriever`. T0.2.7's
/// `MultiStrategyRetriever` re-uses [`assert_boundary_leakage_invariant`]
/// without modification — that's the whole point of keeping the harness
/// generic.
#[tokio::test]
async fn semantic_retriever_does_not_leak_across_boundaries() {
    let b = make_test_retriever().await;
    assert_boundary_leakage_invariant(&b, &b.retriever).await;
}
