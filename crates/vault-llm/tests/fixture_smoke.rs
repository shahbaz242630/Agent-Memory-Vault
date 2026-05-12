//! Fixture smoke test for `tests/fixtures/canned_merge_decisions.json`.
//!
//! Locks the fixture's structural shape so future commits + future model-swap
//! evaluation runs (T0.2.3+) can rely on the format. Iteration 2 amendment 2(a)
//! per `feedback_plan_iterations_inline_not_handoff.md`:
//!
//! > A new crate landing with zero tests is a CI signal void — `cargo test -p
//! > vault-llm` returns 'ok' with 0 tests run, which doesn't distinguish
//! > 'crate compiles + nothing to test' from 'crate compiles + test discovery
//! > broken.' A fixture-smoke test of ~10 lines gives commit 1 a real test-
//! > runner exercise on all 3 OS, which is exactly the CI confidence signal
//! > the two-commit split was designed to provide.
//!
//! The 5 canned merge-decisions in the fixture are seed regression material
//! for T0.2.3's consolidator integration tests and any future Phi-4-mini →
//! alternative model swap evaluation. Locking them here at commit 1 makes the
//! evidence preservation explicit (per iteration 2 observation 1(a)).

use std::path::PathBuf;

#[derive(serde::Deserialize, Debug)]
struct Fixture {
    version: u32,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize, Debug)]
struct Case {
    label: String,
    memory_a: String,
    memory_b: String,
    expected_merge: bool,
    #[allow(dead_code)] // documentary — see fixture `score_caveat`
    observed_score_spike_2: f32,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("canned_merge_decisions.json")
}

#[test]
fn canned_merge_decisions_fixture_parses_with_expected_shape() {
    let path = fixture_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read fixture at {}: {e}", path.display()));
    let fixture: Fixture = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("parse fixture at {}: {e}", path.display()));

    assert_eq!(fixture.version, 1, "fixture schema version must be 1");
    assert_eq!(
        fixture.cases.len(),
        5,
        "fixture must contain exactly 5 spike-2 canned cases (3 merge + 2 no-merge)"
    );

    let labels: Vec<&str> = fixture.cases.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "identical-A",
            "identical-B",
            "similar-A",
            "unrelated-A",
            "unrelated-B",
        ],
        "fixture cases must appear in the spike-2-captured order"
    );

    let merge_count = fixture.cases.iter().filter(|c| c.expected_merge).count();
    let nomerge_count = fixture.cases.iter().filter(|c| !c.expected_merge).count();
    assert_eq!(merge_count, 3, "fixture must have 3 expected-merge cases");
    assert_eq!(
        nomerge_count, 2,
        "fixture must have 2 expected-no-merge cases"
    );

    for case in &fixture.cases {
        assert!(!case.memory_a.is_empty(), "{}: memory_a empty", case.label);
        assert!(!case.memory_b.is_empty(), "{}: memory_b empty", case.label);
        assert!(
            (0.0..=1.0).contains(&case.observed_score_spike_2),
            "{}: observed_score_spike_2 {} out of [0, 1]",
            case.label,
            case.observed_score_spike_2
        );
    }
}
