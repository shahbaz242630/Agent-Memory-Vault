//! ADR-058 — per-boundary REPORT generation wiring.
//!
//! These tests pin the gap surfaced by the first live consolidation dogfood
//! (2026-05-29): `Consolidator::run_consolidation` produced the run-audit
//! `summary_markdown` but the per-boundary REPORT artifact (the curated
//! "what is currently true, grouped by topic" view the structured read
//! pipeline serves from) was NEVER built or persisted. The producer functions
//! (`discover_topics` / `generate_report` / `write_report_atomic`) existed and
//! were unit-tested in isolation, but nothing in the run path called them, so
//! `REPORT_MISSING` could never clear and `topic` was null on every read.
//!
//! `Consolidator::generate_reports` is the new wiring method. Test 1 pins that
//! it produces a topical REPORT per boundary; test 2 pins that the produced
//! REPORT survives the atomic-write + JSON round-trip at the on-disk path the
//! read pipeline's `FilesystemReportLoader` reads from.
//!
//! The full end-to-end "run_consolidation_with_safety leaves a REPORT file on
//! disk" assertion needs a real Phi-4 + the full Application (no
//! mock-consolidator injection seam on `Application` yet — logged as
//! tech-debt under ADR-058 in HANDOFF.md); it is proven by the live dogfood
//! re-run. These two tests run on every CI cycle (Linux + Windows, BGE-gated
//! against macOS) with mocks.
//!
//! ## macOS deferral
//!
//! Gated `#![cfg(not(target_os = "macos"))]` per ADR-033 — BGE provider
//! transitively depends on ONNX Runtime which has a known macOS process-exit
//! SIGABRT. Linux + Windows CI covers the embedding path.

#![cfg(not(target_os = "macos"))]

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::Utc;
use uuid::Uuid;
use vault_consolidator::{write_report_atomic, Consolidator, ConsolidatorConfig, Report};
use vault_core::Boundary;
use vault_llm::MockLlmProvider;

mod common;
use common::{
    insert_and_drain, make_memory_with_content, open_bge_provider, open_sealed_storage_for_test,
};

/// Build a Consolidator over a temp sealed store seeded with `contents` in
/// one boundary, embedded via real BGE, with a MockLlmProvider. The mock
/// returns a non-label string so topic naming falls back to placeholder
/// labels (we assert on the *grouping*, not the label text — label quality
/// is a Phi-4 concern exercised by the cron-gated real-model test).
async fn seed_consolidator(
    passphrase: &str,
    boundary: &Boundary,
    contents: &[&str],
) -> (Consolidator, tempfile::TempDir) {
    let embedder = open_bge_provider();
    let (storage, dir) = open_sealed_storage_for_test(passphrase).await;

    let mut pairs = Vec::with_capacity(contents.len());
    for content in contents {
        let memory = make_memory_with_content(content, boundary);
        let embedding = embedder.embed(content).await.expect("embed must succeed");
        pairs.push((memory, embedding));
    }
    insert_and_drain(&storage, pairs).await;

    let llm = Arc::new(MockLlmProvider::new(
        "mock-topic-label",
        "not-json".to_string(),
    ));
    let consolidator = Consolidator::new(
        Arc::new(storage),
        llm,
        embedder,
        ConsolidatorConfig::default(),
    );
    (consolidator, dir)
}

/// Test 1 — `generate_reports` produces one topical REPORT per boundary, and
/// every seeded fact is present in the REPORT exactly once (no loss, no
/// invention). This is the core of the wiring that was missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generate_reports_produces_topical_report_per_boundary() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let boundary = Boundary::new("personal").expect("valid boundary");
    // 4 distinct facts → discover_topics runs connected-components topic
    // discovery (ADR-068, N >= 3) and emits >= 1 topic. The facts are mostly
    // unrelated, so we don't depend on any particular cluster count.
    let contents = [
        "The user's blood pressure was 132 over 85 on Tuesday morning.",
        "The user is learning Spanish using a flashcard app every evening.",
        "The user prefers the Rust programming language for backend work.",
        "The user's flight to Berlin departs at 7am on the 14th.",
    ];

    let (consolidator, _dir) =
        seed_consolidator("report-gen-per-boundary", &boundary, &contents).await;

    let reports = consolidator
        .generate_reports(Uuid::new_v4())
        .await
        .expect("generate_reports must succeed");

    assert_eq!(
        reports.len(),
        1,
        "exactly one boundary was seeded, so exactly one REPORT is expected"
    );
    let report = &reports[0];
    assert_eq!(
        report.boundary, boundary,
        "REPORT boundary must match the seeded boundary"
    );
    assert!(
        !report.facts_by_topic.is_empty(),
        "REPORT must carry at least one topic; an empty facts_by_topic is the \
         exact REPORT_MISSING-never-clears bug this method fixes"
    );

    // Every seeded fact appears exactly once across all topics — no memory
    // lost, none invented.
    let facts_in_report: Vec<&str> = report
        .facts_by_topic
        .values()
        .flat_map(|facts| facts.iter().map(|f| f.fact.as_str()))
        .collect();
    let unique_facts: BTreeSet<&str> = facts_in_report.iter().copied().collect();
    assert_eq!(
        facts_in_report.len(),
        unique_facts.len(),
        "no fact may appear in more than one topic"
    );
    let expected: BTreeSet<&str> = contents.iter().copied().collect();
    assert_eq!(
        unique_facts, expected,
        "the union of all topics' facts MUST equal the seeded set exactly \
         (no loss, no invention)"
    );
}

/// Finding D — a retired (invalidated) fact must NOT appear in the generated
/// REPORT. The REPORT is the "current truth, grouped by topic" view the read
/// pipeline serves; a fact retired by contradiction/expiry has left the current
/// truth, so surfacing it would present stale knowledge as live. Before the fix,
/// `generate_reports` listed every non-superseded row (including `valid_until`-
/// invalidated ones), so retired facts leaked into the REPORT.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generate_reports_excludes_invalidated_facts() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let boundary = Boundary::new("personal").expect("valid boundary");
    let embedder = open_bge_provider();
    let (storage, _dir) = open_sealed_storage_for_test("report-excludes-invalidated").await;

    let keep = "The user prefers the Rust programming language for backend work.";
    let retire = "The user's flight to Berlin departs at 7am on the 14th.";

    let keep_mem = make_memory_with_content(keep, &boundary);
    let retire_mem = make_memory_with_content(retire, &boundary);
    let retire_id = retire_mem.id;
    let keep_emb = embedder.embed(keep).await.expect("embed keep");
    let retire_emb = embedder.embed(retire).await.expect("embed retire");
    insert_and_drain(
        &storage,
        vec![(keep_mem, keep_emb), (retire_mem, retire_emb)],
    )
    .await;

    // Retire one fact via the bi-temporal invalidate API (ADR-051, metadata-only).
    storage
        .invalidate(retire_id, Utc::now(), "test: retired".to_string())
        .await
        .expect("invalidate must succeed");

    let llm = Arc::new(MockLlmProvider::new(
        "mock-topic-label",
        "not-json".to_string(),
    ));
    let consolidator = Consolidator::new(
        Arc::new(storage),
        llm,
        embedder,
        ConsolidatorConfig::default(),
    );

    let reports = consolidator
        .generate_reports(Uuid::new_v4())
        .await
        .expect("generate_reports must succeed");

    let facts: Vec<&str> = reports
        .iter()
        .flat_map(|r| r.facts_by_topic.values())
        .flat_map(|fs| fs.iter().map(|f| f.fact.as_str()))
        .collect();

    assert!(
        facts.contains(&keep),
        "the live fact must appear in the REPORT; got {facts:?}"
    );
    assert!(
        !facts.contains(&retire),
        "the RETIRED fact must NOT appear in the REPORT — a retired fact has left \
         the current truth (Finding D); got {facts:?}"
    );
}

/// Test 2 — a REPORT produced by `generate_reports`, written via
/// `write_report_atomic`, lands at `<vault_root>/reports/<boundary>.report.json`
/// and round-trips back through JSON deserialization unchanged. This pins the
/// producer → disk contract at the exact path the read pipeline's
/// `FilesystemReportLoader` reads from.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn generated_report_round_trips_to_disk_at_expected_path() {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();

    let boundary = Boundary::new("personal").expect("valid boundary");
    let contents = [
        "The user's car is a blue 2019 hatchback.",
        "The user takes oat milk in coffee, never dairy.",
        "The user's dentist appointment is on the 3rd of next month.",
    ];

    let (consolidator, _dir) =
        seed_consolidator("report-gen-roundtrip", &boundary, &contents).await;

    let reports = consolidator
        .generate_reports(Uuid::new_v4())
        .await
        .expect("generate_reports must succeed");
    let report = reports.into_iter().next().expect("one REPORT expected");

    // Write to a separate temp vault_root (the writer creates reports/ lazily).
    let vault_root = tempfile::tempdir().expect("vault_root tempdir");
    let path = write_report_atomic(&report, vault_root.path()).expect("write_report_atomic");

    assert_eq!(
        path,
        vault_root
            .path()
            .join("reports")
            .join("personal.report.json"),
        "REPORT must land at the path FilesystemReportLoader reads from"
    );
    assert!(path.exists(), "REPORT file must exist after write");

    let restored: Report =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read REPORT file"))
            .expect("REPORT JSON must deserialize back into Report");
    assert_eq!(
        restored, report,
        "REPORT must round-trip through atomic-write + JSON parse unchanged"
    );
}
