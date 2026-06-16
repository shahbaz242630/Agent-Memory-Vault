//! T0.2.3 commit 3 — BRD §6.2 line 1441 + §5.6 lines 977-980 acceptance
//! integration tests for vault-consolidator's Phase 1 + Phase 2 + Phase 3
//! pipeline plus the `generate_summary_markdown` output.
//!
//! Three tests in this file:
//!
//! 1. **`merge_acceptance_phase_1_to_3_end_to_end_against_100_fixture`** —
//!    real Phi-4-mini, cron-gated via `#[ignore]` + `cfg(target_os =
//!    "windows")` (model-path resolution is currently Windows-only per
//!    `vault-llm/tests/phi4_mini_smoke.rs`). Loads the 100-memory acceptance
//!    fixture (`merge_acceptance_100.json`), embeds via BGE, runs the full
//!    `Consolidator::run_consolidation` pipeline, and verifies the
//!    structural acceptance criteria from BRD §6.2 line 1441: merges produce
//!    consolidated memories, originals preserved as superseded, contradictions
//!    surfaced for user review, and `summary_markdown` contains all required
//!    sections.
//!
//! 2. **`rollback_restores_pre_consolidation_state_exactly`** +
//!    **`rollback_reverts_combined_dedup_and_decay`** — T0.2.5 (Checkpoint &
//!    Rollback) acceptance, every CI cycle. Real BGE + `MockLlmProvider`; the
//!    deterministic dedup + Phase-4 decay paths exercise rollback end-to-end
//!    (no real Phi-4). Snapshot pre-state → `run_consolidation` → rollback →
//!    assert post-rollback state == pre-state EXACTLY.
//!
//! 3. **`summary_markdown_is_non_empty_and_contains_required_sections`** —
//!    runs on every CI cycle (Linux + Windows, BGE-gated against macOS). Tiny
//!    fixture (4 memories) with `MockLlmProvider` so the test is fast +
//!    deterministic. Asserts the structural BRD §5.6 line 980 contract:
//!    summary_markdown is non-empty and contains all 5 required section
//!    headers.
//!
//! ## macOS deferral
//!
//! Gated `#![cfg(not(target_os = "macos"))]` per ADR-033 — BGE
//! provider transitively depends on ONNX Runtime which has a known
//! macOS process-exit SIGABRT. Linux + Windows CI matrix covers the
//! embedding path; macOS coverage lands when the upstream issue resolves.

#![cfg(not(target_os = "macos"))]

use std::sync::Arc;

use vault_consolidator::{Consolidator, ConsolidatorConfig};
use vault_core::{Boundary, Memory};
use vault_llm::MockLlmProvider;
use vault_storage::{MemoryFilter, StorageBackend};

// Imports used only by the Windows-only real-Phi-4 test (#1) and its
// classification-quality helper. Gated to keep non-Windows CI under
// `-D warnings` from flagging unused imports.
#[cfg(target_os = "windows")]
use std::collections::HashMap;
#[cfg(target_os = "windows")]
use vault_core::MemoryId;
#[cfg(target_os = "windows")]
use vault_embedding::EMBEDDING_DIM;

mod common;
use common::{
    insert_and_drain, load_canned_response_as_string, make_memory_with_content, open_bge_provider,
    open_sealed_storage_for_test,
};
#[cfg(target_os = "windows")]
use common::{load_merge_acceptance_fixture, make_memory_from_fixture};

// ─────────────────────────────────────────────────────────────────────────
// Test 1 — end-to-end against the 100-memory fixture, real Phi-4
// ─────────────────────────────────────────────────────────────────────────

/// **Real-model smoke; cron-gated.** Runs the full Phase 1 → 2 → 3 pipeline
/// against the 100-memory acceptance fixture with real Phi-4-mini inference.
/// Phi-4 path resolution is currently Windows-only (`APPDATA`) per
/// `vault-llm/tests/phi4_mini_smoke.rs:24-32`; cross-platform model_dir
/// resolution lands when vault-tauri / vault-app own the user-data-dir
/// resolution.
///
/// Acceptance per BRD §6.2 line 1441 verbatim: *"Merge produces consolidated
/// memories, originals preserved as superseded, retrieval surfaces merged
/// version, AND summary_markdown contains all required sections in scannable
/// form for a 100-memory test run with known duplicates and one contradiction."*
///
/// The fixture contains 100 entries with mixed cluster sizes, 2 contradiction
/// pairs, and 3 BGE-truncation entries (2000+ chars) per T0.2.3 commit 3 plan
/// iteration 2 — see `tests/fixtures/merge_acceptance_100.json` and the
/// commit 3 deliverables block in HANDOFF.md for content-length distribution
/// rationale + within-cluster variance design.
#[cfg(target_os = "windows")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real-model smoke; needs 2.49 GB Phi-4-mini GGUF + LLVM toolchain + Windows APPDATA"]
async fn merge_acceptance_phase_1_to_3_end_to_end_against_100_fixture() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_test_writer()
        .try_init();

    // ── Step 1: load + shape-assert fixture ──────────────────────────────
    let fixture = load_merge_acceptance_fixture();
    assert_eq!(
        fixture.len(),
        100,
        "fixture must contain exactly 100 entries"
    );

    // Sanity: at least 1 contradiction (BRD §6.2 line 1441 floor).
    let contradiction_count = fixture
        .iter()
        .filter(|e| e.ground_truth.outcome == "contradiction")
        .count();
    assert!(
        contradiction_count >= 2,
        "fixture must contain ≥2 contradiction entries (2 pairs); got {contradiction_count}"
    );

    // ── Step 2: embed via BGE + write through cascading path ─────────────
    let embedder = open_bge_provider();
    let (storage, _dir) = open_sealed_storage_for_test("acceptance-merge-passphrase").await;

    let mut memory_id_to_ground_truth: HashMap<MemoryId, (String, Option<String>)> = HashMap::new();

    let mut pairs = Vec::with_capacity(fixture.len());
    for entry in &fixture {
        let memory = make_memory_from_fixture(entry);
        memory_id_to_ground_truth.insert(
            memory.id,
            (
                entry.ground_truth.outcome.clone(),
                entry.ground_truth.cluster.clone(),
            ),
        );
        let embedding = embedder
            .embed(&entry.content)
            .await
            .unwrap_or_else(|e| panic!("embed failed on {:?}: {e}", entry.id));
        assert_eq!(embedding.len(), EMBEDDING_DIM);
        pairs.push((memory, embedding));
    }
    insert_and_drain(&storage, pairs).await;

    // ── Step 3: build Phi-4-mini provider + Consolidator ────────────────
    // Phi-4 model path resolution mirrors `vault-llm/tests/phi4_mini_smoke.rs:24-32`.
    // First call downloads the GGUF (~3 min, ~2.49 GB); subsequent calls
    // hash-verify the cached file (~5s).
    let appdata = std::env::var("APPDATA").expect(
        "APPDATA env var must be set (#[ignore] real-model test runs on \
         Windows; cross-platform model_dir resolution lands at production \
         wiring time per phi4_mini_smoke.rs)",
    );
    let models_dir = std::path::PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models");
    let phi4_config = vault_llm::Phi4MiniConfig::v0_2_default(models_dir);
    let llm = Arc::new(
        vault_llm::Phi4MiniProvider::new(phi4_config)
            .await
            .expect("Phi4MiniProvider construction (download/verify/load)"),
    );

    let storage_arc = Arc::new(storage);
    let consolidator = Consolidator::new(
        storage_arc.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );

    // ── Step 4: run the full consolidation pipeline ──────────────────────
    let report = consolidator
        .run_consolidation()
        .await
        .expect("run_consolidation must succeed");

    tracing::info!(
        memories_processed = report.memories_processed,
        memories_merged = report.memories_merged,
        contradictions_resolved = report.contradictions_resolved,
        conflicts_count = report.conflicts_for_user_review.len(),
        summary_md_chars = report.summary_markdown.len(),
        "Phase 1+2+3 acceptance run complete"
    );

    // ── Step 5: structural acceptance — BRD §6.2 line 1441 ───────────────

    // (5a) Merge produces consolidated memories.
    assert_eq!(
        report.memories_processed, 100,
        "all 100 memories enumerated"
    );
    assert!(
        report.memories_merged > 0,
        "Phi-4 should have detected at least one merge in the 100-memory \
         fixture; the fixture has 15 ground-truth merge clusters. Got: \
         memories_merged={}",
        report.memories_merged
    );

    // (5b) Originals preserved as superseded — default filter excludes them.
    let active_after = storage_arc
        .list_memories(MemoryFilter::default(), None)
        .await
        .expect("list_memories after consolidation");
    let all_filter = MemoryFilter {
        include_superseded: true,
        ..MemoryFilter::default()
    };
    let all_after = storage_arc
        .list_memories(all_filter, None)
        .await
        .expect("list_memories including superseded");
    assert!(
        all_after.len() > active_after.len(),
        "superseded memories should still exist in storage (provenance \
         preserved per BRD §5.6 line 948); active={}, all={}",
        active_after.len(),
        all_after.len()
    );
    let superseded_count = all_after
        .iter()
        .filter(|m| m.superseded_by.is_some())
        .count();
    assert_eq!(
        superseded_count, report.memories_merged,
        "each merged cluster member must have superseded_by set; superseded \
         in storage={}, report.memories_merged={}",
        superseded_count, report.memories_merged
    );

    // (5c) Contradictions surfaced for user review — BRD §6.2 line 1441
    // requires at least one contradiction detected for the 100-memory run.
    assert!(
        !report.conflicts_for_user_review.is_empty(),
        "Phi-4 should have detected at least one of the 2 ground-truth \
         contradictions in the fixture; got 0"
    );

    // (5d) summary_markdown structural sections — BRD §5.6 lines 959-973.
    let md = &report.summary_markdown;
    assert!(
        !md.is_empty(),
        "summary_markdown must be non-empty for a non-trivial run"
    );
    assert!(md.contains("# Consolidation Run —"), "Run header missing");
    assert!(md.contains("## Merges"), "Merges section header missing");
    assert!(
        md.contains("## Contradictions"),
        "Contradictions section header missing"
    );
    assert!(md.contains("## Decay"), "Decay section header missing");
    assert!(md.contains("## Footer"), "Footer section header missing");

    // (5e) Quality observability — log precision/recall against ground truth.
    // Per plan iteration 2 forward-pointer: not a hard gate at T0.2.3
    // (Phi-4-mini judgment on long content is a known unknown — ADR-042
    // revisit trigger if quality degrades materially). Logged so cron-gated
    // run history surfaces the trend.
    log_classification_quality(&report, &memory_id_to_ground_truth, &all_after);
}

/// Compute + log precision/recall for the cron-gated acceptance test against
/// the fixture's ground-truth labels. Not a hard gate per T0.2.3 commit 3
/// plan iteration 2 forward-pointer (Phi-4 quality on long content is the
/// known unknown the cron run is measuring).
#[cfg(target_os = "windows")]
fn log_classification_quality(
    report: &vault_consolidator::ConsolidationReport,
    memory_id_to_ground_truth: &HashMap<MemoryId, (String, Option<String>)>,
    all_after: &[Memory],
) {
    // Build "predicted merge group" per memory: the new_memory_id their
    // superseded_by points at. Singletons stay as their own group.
    let mut predicted_group: HashMap<MemoryId, MemoryId> = HashMap::new();
    for m in all_after {
        match m.superseded_by {
            Some(new_id) => {
                predicted_group.insert(m.id, new_id);
            }
            None => {
                predicted_group.insert(m.id, m.id);
            }
        }
    }

    // Pair-counting precision/recall over the original 100 fixture members
    // only (skip the newly-written merged memories which aren't ground-truth
    // entries).
    let original_ids: Vec<MemoryId> = memory_id_to_ground_truth.keys().copied().collect();
    let mut tp = 0u64;
    let mut fp = 0u64;
    let mut fn_ = 0u64;
    for i in 0..original_ids.len() {
        for j in (i + 1)..original_ids.len() {
            let a = original_ids[i];
            let b = original_ids[j];
            let (a_outcome, a_cluster) = &memory_id_to_ground_truth[&a];
            let (b_outcome, b_cluster) = &memory_id_to_ground_truth[&b];
            let gt_same_merge_group = a_outcome == "merge"
                && b_outcome == "merge"
                && a_cluster == b_cluster
                && a_cluster.is_some();
            let pred_same_merge_group = predicted_group.get(&a) == predicted_group.get(&b)
                && predicted_group.contains_key(&a);
            match (pred_same_merge_group, gt_same_merge_group) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => fn_ += 1,
                (false, false) => {}
            }
        }
    }
    let precision = if tp + fp == 0 {
        1.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let recall = if tp + fn_ == 0 {
        1.0
    } else {
        tp as f64 / (tp + fn_) as f64
    };
    tracing::info!(
        precision = precision,
        recall = recall,
        tp = tp,
        fp = fp,
        fn_ = fn_,
        contradictions_detected = report.conflicts_for_user_review.len(),
        "Phi-4 merge classification quality (observability only — no hard gate at T0.2.3)"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test 2 — rollback (T0.2.5 dependency stub)
// ─────────────────────────────────────────────────────────────────────────

/// Full enumeration (incl. superseded) of the vault's memories, sorted by id
/// so two snapshots are directly comparable. The rollback acceptance bar is
/// `post == pre`, byte-for-byte across every row.
async fn snapshot(storage: &StorageBackend) -> Vec<Memory> {
    let mut all = storage
        .list_memories(
            MemoryFilter {
                include_superseded: true,
                ..MemoryFilter::default()
            },
            None,
        )
        .await
        .expect("list_memories");
    all.sort_by_key(|m| m.id);
    all
}

/// **BRD §6.2 line 1451 acceptance — runs every CI cycle.** Real BGE +
/// `MockLlmProvider`; the deterministic dedup pass (ADR-063) collapses a
/// near-identical pair (no real Phi-4 needed), which is the supersession
/// mutation rollback must reverse.
///
/// Snapshot pre-state → `run_consolidation` (T0.2.5 captures a checkpoint) →
/// `rollback_checkpoint` → assert the post-rollback state equals the pre-run
/// state EXACTLY. Also pins "no memory ever lost" (every original id survives
/// the run) and the double-rollback guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_restores_pre_consolidation_state_exactly() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let embedder = open_bge_provider();
    let (storage, _dir) = open_sealed_storage_for_test("rollback-exact-passphrase").await;

    // A near-identical pair (collapses via dedup) + an unrelated singleton.
    // Same paraphrases the summary-sections test proves cluster at 0.92.
    let work = Boundary::new("work").expect("valid boundary");
    let memories = vec![
        make_memory_with_content("Daily standup moved to 10am from 9am", &work),
        make_memory_with_content("Standup moved to 10am from 9am", &work),
        make_memory_with_content("Annual offsite scheduled for the week of June 12", &work),
    ];
    let mut pairs = Vec::with_capacity(memories.len());
    for memory in &memories {
        let embedding = embedder.embed(&memory.content).await.expect("embed");
        pairs.push((memory.clone(), embedding));
    }
    insert_and_drain(&storage, pairs).await;

    let storage = Arc::new(storage);
    let pre = snapshot(storage.as_ref()).await;
    assert_eq!(pre.len(), 3, "three memories written");

    let llm = Arc::new(MockLlmProvider::new(
        "mock-merge-canned",
        load_canned_response_as_string("merge_size_2"),
    ));
    let consolidator = Consolidator::new(
        storage.clone(),
        llm,
        embedder.clone(),
        ConsolidatorConfig::default(),
    );
    let report = consolidator
        .run_consolidation()
        .await
        .expect("run_consolidation");

    // The dedup collapsed the pair → the run changed state → a checkpoint exists.
    let checkpoint_id = report
        .checkpoint_id
        .expect("a state-changing run must produce a checkpoint");

    // "No memory ever lost": every original id is still present after the run
    // (the loser is superseded, not deleted).
    let after_run = snapshot(storage.as_ref()).await;
    for m in &pre {
        assert!(
            after_run.iter().any(|q| q.id == m.id),
            "memory {} vanished during the run — consolidator must never hard-delete",
            m.id
        );
    }
    assert!(
        after_run.iter().any(|m| m.superseded_by.is_some()),
        "expected the dedup to supersede one of the pair"
    );

    // Roll back and assert EXACT restoration of the pre-run state.
    let rb = storage
        .rollback_checkpoint(checkpoint_id)
        .await
        .expect("rollback");
    assert!(
        rb.restored + rb.deleted >= 1,
        "rollback must touch at least one memory; got {rb:?}"
    );

    let post = snapshot(storage.as_ref()).await;
    assert_eq!(
        post, pre,
        "rollback must restore the pre-consolidation state EXACTLY"
    );
    assert!(
        post.iter().all(|m| m.superseded_by.is_none()),
        "every memory must be active again after rollback"
    );

    // Double-rollback is rejected (the checkpoint is spent).
    assert!(
        storage.rollback_checkpoint(checkpoint_id).await.is_err(),
        "a second rollback of the same checkpoint must error"
    );
}

/// **Adversarial combined-mutation rollback — runs every CI cycle.** One run
/// that BOTH dedups a near-identical pair AND decays every active fact
/// (`decay_after_days = 0` forces decay without waiting). Rollback must revert
/// the supersession AND every confidence change AND remove the decay markers —
/// i.e. restore the exact pre-run state across two different mutation kinds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rollback_reverts_combined_dedup_and_decay() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let embedder = open_bge_provider();
    let (storage, _dir) = open_sealed_storage_for_test("rollback-combined-passphrase").await;

    let work = Boundary::new("work").expect("valid boundary");
    let memories = vec![
        make_memory_with_content("Daily standup moved to 10am from 9am", &work),
        make_memory_with_content("Standup moved to 10am from 9am", &work),
        make_memory_with_content("Annual offsite scheduled for the week of June 12", &work),
        make_memory_with_content("Quarterly reviews due by end of next month", &work),
    ];
    let mut pairs = Vec::with_capacity(memories.len());
    for memory in &memories {
        let embedding = embedder.embed(&memory.content).await.expect("embed");
        pairs.push((memory.clone(), embedding));
    }
    insert_and_drain(&storage, pairs).await;

    let storage = Arc::new(storage);
    let pre = snapshot(storage.as_ref()).await;

    let llm = Arc::new(MockLlmProvider::new(
        "mock-merge-canned",
        load_canned_response_as_string("merge_size_2"),
    ));
    // decay_after_days = 0 → every active fact decays this run.
    let config = ConsolidatorConfig {
        decay_after_days: 0,
        ..ConsolidatorConfig::default()
    };
    let consolidator = Consolidator::new(storage.clone(), llm, embedder.clone(), config);
    let report = consolidator
        .run_consolidation()
        .await
        .expect("run_consolidation");
    let checkpoint_id = report
        .checkpoint_id
        .expect("a state-changing run must produce a checkpoint");

    // Sanity: the run actually decayed at least one fact (confidence dropped
    // below the 0.9 it was written with) — so the test exercises decay-revert.
    let after_run = snapshot(storage.as_ref()).await;
    assert!(
        after_run
            .iter()
            .any(|m| m.superseded_by.is_none() && m.confidence < 0.9),
        "expected decay to lower at least one active fact's confidence"
    );

    storage
        .rollback_checkpoint(checkpoint_id)
        .await
        .expect("rollback");

    let post = snapshot(storage.as_ref()).await;
    assert_eq!(
        post, pre,
        "rollback must revert BOTH the dedup supersession AND the decay exactly"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Test 3 — summary_markdown sections (every-cycle CI)
// ─────────────────────────────────────────────────────────────────────────

/// **Runs on every CI cycle (Linux + Windows).** Tiny fixture, MockLlmProvider,
/// fast + deterministic. Pins BRD §5.6 line 980's structural contract:
/// `summary_markdown` non-empty + contains all 5 required section headers
/// (Run header, Merges, Contradictions, Decay, Footer).
///
/// Boundary-separation invariant is tested at the unit level by
/// `summary::tests::boundary_separation_no_cross_boundary_content_leak`; this
/// integration test exercises the orchestrator → markdown path end-to-end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn summary_markdown_is_non_empty_and_contains_required_sections() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let embedder = open_bge_provider();
    let (storage, _dir) = open_sealed_storage_for_test("summary-sections-test-passphrase").await;

    // 4 memories in one boundary — 2 form a tight, near-identical cluster
    // (BGE clusters them at the 0.92 default threshold), 2 are unrelated
    // singletons. Because the pair is near-identical, the deterministic dedup
    // pass (ADR-063) — which runs BEFORE the LLM merge step — collapses them
    // (keep canonical survivor, supersede the rest) and SKIPS the LLM. So this
    // run is non-trivial via dedup, not merge. The LLM-merge path is exercised
    // by the #[ignore]d real-Phi-4 acceptance test in this file.
    //
    // Paraphrases mirror T0.2.2 `clustering_acceptance_100.json:38-42` (the
    // standup-time-change cluster) — those are proven to cluster at the
    // 0.92 threshold by T0.2.2's acceptance test.
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
        let embedding = embedder
            .embed(&memory.content)
            .await
            .expect("embed must succeed");
        pairs.push((memory.clone(), embedding));
    }
    insert_and_drain(&storage, pairs).await;

    let merge_canned = load_canned_response_as_string("merge_size_2");
    let llm = Arc::new(MockLlmProvider::new("mock-merge-canned", merge_canned));

    let storage_arc = Arc::new(storage);
    let consolidator = Consolidator::new(
        storage_arc,
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
        summary_md_chars = report.summary_markdown.len(),
        "summary_markdown sections test run complete"
    );

    let md = &report.summary_markdown;

    // BRD §5.6 line 980 — non-empty for a non-trivial run.
    assert!(
        !md.is_empty(),
        "summary_markdown must be non-empty for a non-trivial run"
    );
    assert!(
        md.len() > 200,
        "summary_markdown should have meaningful content (>200 chars); got {} chars",
        md.len()
    );

    // BRD §5.6 line 980 — all 5 required section headers present.
    assert!(
        md.contains("# Consolidation Run —"),
        "Run header missing:\n{md}"
    );
    assert!(
        md.contains("## Merges"),
        "Merges section header missing:\n{md}"
    );
    assert!(
        md.contains("## Contradictions"),
        "Contradictions section header missing:\n{md}"
    );
    assert!(
        md.contains("## Decay"),
        "Decay section header missing:\n{md}"
    );
    assert!(
        md.contains("## Footer"),
        "Footer section header missing:\n{md}"
    );

    // Footer pins (matches summary.rs unit test #5). T0.2.5 shipped: this
    // non-trivial run (the dedup collapses the near-identical pair) creates a
    // real checkpoint, so the footer carries a real id — never the old
    // "pending-T0.2.5" placeholder — plus the rollback command hint.
    assert!(
        md.contains("**Checkpoint ID:** "),
        "Footer checkpoint-ID line missing:\n{md}"
    );
    assert!(
        !md.contains("pending-T0.2.5"),
        "Footer still shows the pre-T0.2.5 placeholder; real checkpoint id expected:\n{md}"
    );
    assert!(
        md.contains("vault-cli checkpoint rollback"),
        "Footer rollback command hint missing:\n{md}"
    );

    // Sanity: the run was non-trivial. Phase 1 clustered the near-identical
    // standup pair, and the deterministic dedup pass (ADR-063, which runs
    // before the LLM merge step) collapsed it — so the pair is DEDUPED, not
    // merged. memories_merged stays 0; memories_deduped is what proves the
    // run did real work on the cluster.
    assert!(
        report.memories_deduped > 0,
        "expected the near-identical standup pair to be collapsed by deterministic \
         dedup (ADR-063 runs before the LLM merge step, so it is deduped not merged); \
         got memories_deduped=0"
    );
}
