//! T0.2.3 commit 3 — BRD §5.6 lines 981-982 property tests for
//! `Consolidator::run_consolidation`:
//!
//! 1. **`consolidation_is_idempotent`** — BRD §5.6 line 981 verbatim:
//!    *"running it twice on same data produces same result."* Interpretation:
//!    after the first run stabilizes state (merges applied, originals
//!    superseded), the second run is a no-op — Phase 1 finds zero clusters
//!    because non-superseded duplicates are gone, Phase 2 + 3 never fire,
//!    no further state change. Asserted via `report.memories_merged == 0`
//!    on run 2.
//!
//! 2. **`no_memory_is_ever_lost`** — BRD §5.6 line 982 verbatim: *"all input
//!    memories appear in output as either active, superseded, or archived."*
//!    Decay/archive lands at T0.2.4; at T0.2.3 the only operations are
//!    Phase 3 merge (which marks originals `superseded_by` and writes a new
//!    merged memory). Every input memory ID must be findable in storage
//!    after consolidation — either with `superseded_by = None` (singleton
//!    or merged-output) or `superseded_by = Some(_)` (merged-input).
//!
//! Both tests use `MockLlmProvider` with the canned `merge_size_2` response
//! from `canned_merge_decisions_nary.json` so the LLM dispatch is fast +
//! deterministic. Property semantics hold under "always-merge" mock because
//! the structural invariants (idempotence after stabilization; no-loss on
//! the merge-write-cascade) are properties of the orchestrator, not of the
//! classifier's specific decisions.
//!
//! ## macOS deferral
//!
//! Gated `#![cfg(not(target_os = "macos"))]` per ADR-033 — BGE
//! provider transitively depends on ONNX Runtime which has a known
//! macOS process-exit SIGABRT. Linux + Windows CI matrix covers these
//! properties; macOS coverage lands when the upstream issue resolves.

#![cfg(not(target_os = "macos"))]

use std::collections::HashSet;
use std::sync::Arc;

use vault_consolidator::{Consolidator, ConsolidatorConfig};
use vault_core::{Boundary, MemoryId};
use vault_llm::MockLlmProvider;
use vault_storage::MemoryFilter;

mod common;
use common::{
    insert_and_drain, load_canned_response_as_string, make_memory_with_content, open_bge_provider,
    open_sealed_storage_for_test,
};

// ─────────────────────────────────────────────────────────────────────────
// Property 1 — idempotence on stabilized state
// ─────────────────────────────────────────────────────────────────────────

/// BRD §5.6 line 981: *consolidation is idempotent* — after run 1
/// stabilizes state, run 2 produces no further state change.
///
/// Mechanics: Phase 1 reads non-superseded memories (default filter); after
/// run 1 marks cluster members `superseded_by = Some(new_id)`, run 2's
/// Phase 1 enumeration excludes them, so the only surviving rows are
/// singletons + newly-written merged memories. No new duplicate pairs exist
/// → no clusters → no Phase 2/3 → `report.memories_merged == 0`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consolidation_is_idempotent() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let embedder = open_bge_provider();
    let (storage, _dir) = open_sealed_storage_for_test("idempotence-property-test").await;

    // Small fixture with one clusterable pair + two singletons. Paraphrases
    // mirror T0.2.2 line 38-39 (proven to cluster at 0.92 threshold).
    let work = Boundary::new("work").expect("valid boundary");
    let memories = vec![
        make_memory_with_content("Daily standup moved to 10am from 9am", &work),
        make_memory_with_content("Standup moved to 10am from 9am", &work),
        make_memory_with_content(
            "Quarterly performance reviews due by end of next month",
            &work,
        ),
        make_memory_with_content("Annual offsite scheduled for the week of June 12", &work),
    ];

    let mut pairs = Vec::with_capacity(memories.len());
    for memory in &memories {
        let embedding = embedder.embed(&memory.content).await.expect("embed");
        pairs.push((memory.clone(), embedding));
    }
    insert_and_drain(&storage, pairs).await;

    let merge_canned = load_canned_response_as_string("merge_size_2");
    let llm = Arc::new(MockLlmProvider::new("idempotence-mock", merge_canned));

    let storage_arc = Arc::new(storage);
    let consolidator = Consolidator::new(
        storage_arc.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    // Run 1: should apply the one merge cluster.
    let report1 = consolidator
        .run_consolidation()
        .await
        .expect("run 1 must succeed");

    tracing::info!(
        run = 1,
        memories_processed = report1.memories_processed,
        memories_merged = report1.memories_merged,
        "first consolidation run"
    );
    assert!(
        report1.memories_merged > 0,
        "run 1 should merge the standup cluster (2 paraphrases at 0.92 \
         cosine threshold); got memories_merged=0"
    );

    // Run 2: state is stabilized. No new clusters should form because the
    // merged-cluster originals are superseded (excluded by default filter)
    // and the new merged memory is alone in its semantic neighbourhood.
    let report2 = consolidator
        .run_consolidation()
        .await
        .expect("run 2 must succeed");

    tracing::info!(
        run = 2,
        memories_processed = report2.memories_processed,
        memories_merged = report2.memories_merged,
        contradictions_resolved = report2.contradictions_resolved,
        "second consolidation run (should be no-op)"
    );

    // Property: run 2 is a no-op on stabilized state.
    assert_eq!(
        report2.memories_merged, 0,
        "BRD §5.6 line 981 violated: run 2 produced {} additional merges \
         on stabilized state — consolidation is not idempotent",
        report2.memories_merged
    );
    assert_eq!(
        report2.contradictions_resolved, 0,
        "BRD §5.6 line 981 violated: run 2 flagged {} additional \
         contradictions on stabilized state",
        report2.contradictions_resolved
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Property 2 — no memory is ever lost
// ─────────────────────────────────────────────────────────────────────────

/// BRD §5.6 line 982: *no memory is ever lost* — every input memory ID is
/// findable in storage after consolidation, either active (own row,
/// `superseded_by = None`) or superseded (own row, `superseded_by =
/// Some(_)`). Decay/archive lands at T0.2.4; at T0.2.3 there's no archive
/// path, so "active OR superseded" exhausts the post-state outcomes.
///
/// Mechanics: Phase 3 `apply_merge` (via ADR-046 `mark_superseded`) is
/// metadata-only — original rows persist with `superseded_by` set; the new
/// merged memory is written as a fresh active row. Total row count after
/// consolidation = input count + (1 per merge cluster). The property test
/// asserts every input ID is still present in storage (no silent drops),
/// AND that the union of active-IDs ∪ superseded-IDs covers every input ID.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_memory_is_ever_lost() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let embedder = open_bge_provider();
    let (storage, _dir) = open_sealed_storage_for_test("no-loss-property-test").await;

    let work = Boundary::new("work").expect("valid boundary");
    let memories = vec![
        make_memory_with_content("Daily standup moved to 10am from 9am", &work),
        make_memory_with_content("Standup moved to 10am from 9am", &work),
        make_memory_with_content(
            "Quarterly performance reviews due by end of next month",
            &work,
        ),
        make_memory_with_content("Annual offsite scheduled for the week of June 12", &work),
    ];
    let input_ids: HashSet<MemoryId> = memories.iter().map(|m| m.id).collect();
    let input_count = memories.len();

    let mut pairs = Vec::with_capacity(memories.len());
    for memory in &memories {
        let embedding = embedder.embed(&memory.content).await.expect("embed");
        pairs.push((memory.clone(), embedding));
    }
    insert_and_drain(&storage, pairs).await;

    let merge_canned = load_canned_response_as_string("merge_size_2");
    let llm = Arc::new(MockLlmProvider::new("no-loss-mock", merge_canned));

    let storage_arc = Arc::new(storage);
    let consolidator = Consolidator::new(
        storage_arc.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    let report = consolidator
        .run_consolidation()
        .await
        .expect("run_consolidation must succeed");

    tracing::info!(
        memories_processed = report.memories_processed,
        memories_merged = report.memories_merged,
        "no-memory-lost property test consolidation complete"
    );

    // Read post-state including superseded rows so we can audit each input
    // ID's disposition.
    let all_filter = MemoryFilter {
        include_superseded: true,
        ..MemoryFilter::default()
    };
    let all_after = storage_arc
        .list_memories(all_filter, None)
        .await
        .expect("list_memories with include_superseded");
    let all_ids_after: HashSet<MemoryId> = all_after.iter().map(|m| m.id).collect();

    // Property assertion 1: every input ID is still in storage.
    for input_id in &input_ids {
        assert!(
            all_ids_after.contains(input_id),
            "BRD §5.6 line 982 violated: input memory {input_id} is missing \
             from post-consolidation storage — memory was silently dropped"
        );
    }

    // Property assertion 2: partition every input ID into active (own row,
    // superseded_by None) OR superseded (own row, superseded_by Some). At
    // T0.2.3 there's no archive state yet (Phase 4 ships at T0.2.4); the
    // binary partition exhausts the post-state outcomes per BRD §5.6
    // line 982.
    let by_id: std::collections::HashMap<MemoryId, &vault_core::Memory> =
        all_after.iter().map(|m| (m.id, m)).collect();
    let mut active_input_count = 0;
    let mut superseded_input_count = 0;
    for input_id in &input_ids {
        let memory = by_id
            .get(input_id)
            .expect("input ID present in post-state per assertion 1");
        if memory.superseded_by.is_none() {
            active_input_count += 1;
        } else {
            superseded_input_count += 1;
        }
    }
    assert_eq!(
        active_input_count + superseded_input_count,
        input_count,
        "BRD §5.6 line 982 partition check: every input ID must be active \
         or superseded; got active={active_input_count}, \
         superseded={superseded_input_count}, input_count={input_count}"
    );
    assert!(
        superseded_input_count > 0,
        "expected at least one input to be superseded after merge; got 0"
    );

    // Property assertion 3: storage row count is non-decreasing — at least
    // as many rows post-consolidation as pre-consolidation. (Phase 3 adds
    // 1 new merged row per merge cluster; never deletes.)
    assert!(
        all_after.len() >= input_count,
        "row count decreased after consolidation: input={input_count}, \
         post={} — apply_merge must NEVER delete rows (BRD §5.6 line 948 \
         'do not delete — preserve provenance')",
        all_after.len()
    );

    // Property assertion 4: merge produced new active rows. The report's
    // memories_merged field counts SUPERSEDED rows (cluster members); each
    // merge cluster also yields exactly 1 new active row. Net new active
    // rows = total active rows - (input rows that stayed active as
    // singletons).
    if report.memories_merged > 0 {
        let active_after_count = all_after
            .iter()
            .filter(|m| m.superseded_by.is_none())
            .count();
        let new_merged_rows = active_after_count - active_input_count;
        assert!(
            new_merged_rows > 0,
            "merge applied {} supersessions but produced zero new merged \
             rows — Phase 3 invariant broken (apply_merge must write a new \
             merged Memory row per cluster)",
            report.memories_merged
        );
    }
}
