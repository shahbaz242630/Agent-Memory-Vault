//! Integration tests for `vault-embedding`. Maps 1:1 to the 9-test list
//! in `T0.1.7_PLAN.md` v1.2 (test strategy section), plus
//! `test_concurrent_init_succeeds` added in expanded Phase 1 to verify
//! the `OnceLock`-based ort init wrapper.
//!
//! **Phase 1 (expanded) status.** Tests 1, 2, 5 + concurrent-init are
//! ACTIVE — they exercise the runtime API surface end-to-end against
//! real fixtures (the runtime confirmation that web-research spikes
//! deferred). Tests 3, 4, 6, 7, 8, 9 stay `#[ignore]`-d for the
//! follow-up phase that lands stronger property tests + perf gate +
//! the mean-pool comparison helper.
//!
//! Tests require the bge-small fixture files in
//! `crates/vault-embedding/test-fixtures/bge-small-en-v1.5/`:
//!   - `model.onnx` (~133 MB) — official BAAI ONNX, SHA-256 in `integrity.rs`
//!   - `tokenizer.json` (~711 KB)
//!   - `onnxruntime.dll` (Windows) / `.dylib` (macOS) / `.so` (Linux)
//!
//! Run `scripts/setup-dev-env.{sh,ps1}` once per checkout to download.
//! Tests panic loudly with a clear message if fixtures are missing — they
//! never silently skip (avoids hiding regressions).

use std::path::PathBuf;
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

const FIXTURE_DIR: &str = "test-fixtures/bge-small-en-v1.5";

fn fixture_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(FIXTURE_DIR);
    p
}

fn require_fixture(name: &str) -> PathBuf {
    let p = fixture_root().join(name);
    assert!(
        p.exists(),
        "missing test fixture {p:?} — run scripts/setup-dev-env.sh (or .ps1 on Windows) first; see T0.1.7_PLAN.md test fixture section"
    );
    p
}

fn model_path() -> PathBuf {
    require_fixture("model.onnx")
}

fn tokenizer_path() -> PathBuf {
    require_fixture("tokenizer.json")
}

#[cfg(target_os = "windows")]
fn ort_lib_path() -> PathBuf {
    require_fixture("onnxruntime.dll")
}

#[cfg(target_os = "macos")]
fn ort_lib_path() -> PathBuf {
    require_fixture("libonnxruntime.dylib")
}

#[cfg(target_os = "linux")]
fn ort_lib_path() -> PathBuf {
    require_fixture("libonnxruntime.so")
}

fn open_provider() -> BgeSmallProvider {
    BgeSmallProvider::open(&model_path(), &tokenizer_path(), &ort_lib_path())
        .expect("open should succeed with valid fixtures")
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

// ---------------------------------------------------------------------------
// Test 1 — single embedding shape + dimension
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_1_single_embedding_has_expected_dimension() {
    let provider = open_provider();
    let v = provider.embed("hello world").await.expect("embed");
    assert_eq!(
        v.len(),
        EMBEDDING_DIM,
        "embedding dimension must match LanceVectorStore configuration"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — L2-normalisation invariant (single input)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_2_embed_output_is_l2_normalized_single_input() {
    let provider = open_provider();
    let v = provider.embed("hello world").await.expect("embed");
    let norm = l2_norm(&v);
    assert!(
        (norm - 1.0).abs() < 1e-6,
        "single-input L2 norm must be ~1.0; got {norm}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — determinism
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "Phase 3 — needs BgeSmallProvider::embed impl + downloaded fixtures"]
async fn test_3_embed_is_deterministic() {
    let provider = open_provider();
    let a = provider
        .embed("the cat sat on the mat")
        .await
        .expect("embed a");
    let b = provider
        .embed("the cat sat on the mat")
        .await
        .expect("embed b");
    assert_eq!(a, b, "two embeds of identical input must be byte-identical");
}

// ---------------------------------------------------------------------------
// Test 4 — cosine-similarity sanity
// ---------------------------------------------------------------------------

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[tokio::test]
#[ignore = "Phase 3 — needs BgeSmallProvider::embed impl + downloaded fixtures"]
async fn test_4_cosine_sanity_similar_vs_dissimilar() {
    let provider = open_provider();
    let a = provider.embed("the cat sat on the mat").await.expect("a");
    let b = provider.embed("a feline rested on a rug").await.expect("b");
    let c = provider.embed("quantum chromodynamics").await.expect("c");

    let ab = cosine(&a, &b);
    let ac = cosine(&a, &c);
    assert!(ab > 0.6, "similar texts must have cosine > 0.6; got {ab}");
    assert!(
        ac < 0.4,
        "dissimilar texts must have cosine < 0.4; got {ac}"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — model integrity check rejects mutated file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_5_model_integrity_check_rejects_mutated_file() {
    let original = model_path();
    let bytes = std::fs::read(&original).expect("read original");

    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let mutated_path = tmp_dir.path().join("model.onnx");
    let mut mutated = bytes.clone();
    // Mutate one byte at offset 1024 (well inside the file, away from headers
    // that might be lenient-parsed)
    mutated[1024] ^= 0xFF;
    std::fs::write(&mutated_path, &mutated).expect("write mutated");

    // Per ADR-007: BgeSmallProvider does not impl Debug (it owns runtime
    // session state); cannot use Result::expect_err. Pattern-match instead.
    let result = BgeSmallProvider::open(&mutated_path, &tokenizer_path(), &ort_lib_path());
    match result {
        Ok(_) => panic!("mutated model must fail integrity but got Ok"),
        Err(vault_core::VaultError::ModelIntegrityFailed { file, .. }) => {
            assert_eq!(file, "model", "error must name the model file");
        }
        Err(other) => panic!("expected ModelIntegrityFailed, got {other}"),
    }
}

// ---------------------------------------------------------------------------
// Test 6 — performance budget (BRD §5.3)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "perf — run with `cargo test -p vault-embedding -- --ignored`"]
async fn test_6_embed_within_100ms_budget() {
    let provider = open_provider();
    let start = std::time::Instant::now();
    let _ = provider
        .embed("a short sentence about cats")
        .await
        .expect("embed");
    let elapsed = start.elapsed();
    assert!(
        elapsed.as_millis() <= 100,
        "single embed must be ≤100ms per BRD §5.3; got {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 7 — spawn_blocking correctness (no reactor starvation)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "Phase 3 — needs BgeSmallProvider::embed impl + downloaded fixtures"]
async fn test_7_spawn_blocking_does_not_starve_reactor() {
    let provider = open_provider();
    let sleep_start = std::time::Instant::now();
    let (embed_result, _) = tokio::join!(
        provider.embed("a moderately long sentence to exercise inference time"),
        tokio::time::sleep(std::time::Duration::from_millis(50)),
    );
    let elapsed = sleep_start.elapsed();
    let _ = embed_result.expect("embed");
    // Sleep should complete in roughly 50ms, not blocked by inference time.
    // 200ms ceiling is generous (covers slow machines + scheduler jitter)
    // while still catching the failure mode (inference blocking the reactor).
    assert!(
        elapsed.as_millis() < 200,
        "tokio sleep must complete near 50ms (reactor not starved); got {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 8 — L2-normalisation invariant (broad-input property)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "Phase 3 — needs BgeSmallProvider::embed impl + downloaded fixtures"]
async fn test_8_embed_output_is_l2_normalized_across_diverse_inputs() {
    let provider = open_provider();
    // Owned long-lived strings for entries that would otherwise be temporaries.
    let long_text = "very long text ".repeat(50);
    let inputs: [&str; 20] = [
        "x",
        "hello",
        "the quick brown fox jumps over the lazy dog",
        "punctuation, lots; of: it!",
        "naïve façade café résumé", // non-ASCII
        "  whitespace-padded  ",
        "MixedCaseInput With Some CAPS",
        "1 2 3 4 5 6 7 8 9 10",
        "repeated repeated repeated repeated repeated",
        "", // empty — embed should reject or handle gracefully
        "🦀 emoji input 🚀",
        long_text.trim(),
        "single-token-y x",
        "URL-like https://example.com/path?q=1",
        "code-like fn main() { println!(\"hi\"); }",
        "newlines\nand\ttabs",
        "?",
        "    ", // all-whitespace
        "a",
        "the the the the the the the the the the",
    ];

    for (idx, text) in inputs.iter().enumerate() {
        // Empty input may legitimately error (InvalidInput); skip the norm
        // assertion in that case but capture the test intent.
        let result = provider.embed(text).await;
        match result {
            Ok(v) => {
                assert_eq!(v.len(), EMBEDDING_DIM, "input #{idx} dim");
                let norm = l2_norm(&v);
                assert!(
                    (norm - 1.0).abs() < 1e-6,
                    "input #{idx} ({text:?}) L2 norm must be ~1.0; got {norm}"
                );
            }
            Err(vault_core::VaultError::InvalidInput(_)) if text.is_empty() => {
                // Empty input rejected is acceptable — pin the contract:
                // empty input either L2-normalises to 1.0 OR returns InvalidInput.
            }
            Err(e) => panic!("input #{idx} ({text:?}) unexpected error: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Test 9 — pooling-mode contract (CLS, not mean) — load-bearing per Spike 3
// ---------------------------------------------------------------------------

/// Independently compute a mean-pooled L2-normalised embedding for the same
/// input, then assert the production output (CLS-pooled) is NOT element-wise
/// equal. This is the test that catches a future "let's switch pooling for
/// performance" regression that would silently break `LanceVectorStore` cosine
/// scoring. Test 8 (L2-norm) and test 4 (cosine sanity) would NOT catch it.
///
/// **Phase 1**: this test calls `provider.embed()` (stub → panics). Phase 3
/// implements `embed`; Phase 4 also adds a stronger sentence-transformers
/// reference cross-check for the same input.
///
/// The mean-pool comparison vector is constructed via a private test-only
/// path that mirrors the production tokenize → run-session pipeline but
/// substitutes mean-pool for CLS-pool before normalisation. This path lands
/// alongside the production code at Phase 3.
#[tokio::test]
#[ignore = "stronger version + mean-pool comparison path lands at Phase 3 — Phase 1 panics on stubbed embed"]
async fn test_9_embed_uses_cls_pooling_not_mean_pooling() {
    let provider = open_provider();
    let cls_output = provider.embed("hello world").await.expect("embed (CLS)");

    // Phase 3 will provide `vault_embedding::testing::mean_pooled_for("hello world")`
    // that runs the same tokenizer + session but mean-pools instead of CLS-pools.
    // Until then this test is `#[ignore]`-d. Phase 3 commit removes the ignore
    // attribute and lands the comparison.
    let mean_output = vec![0.0_f32; EMBEDDING_DIM]; // placeholder
    let differ = cls_output
        .iter()
        .zip(mean_output.iter())
        .any(|(a, b)| (a - b).abs() > 1e-5);
    assert!(
        differ,
        "CLS-pooled and mean-pooled outputs must differ — confirms CLS extraction is in use"
    );
}

// ---------------------------------------------------------------------------
// Concurrent-init test (added in expanded Phase 1) — proves the OnceLock
// wrapper around ort::init_from is correct.
// ---------------------------------------------------------------------------

/// Two `BgeSmallProvider::open` calls in parallel must both succeed.
///
/// The `OnceLock<Result<(), String>>` in `bge_small.rs` (`ORT_INIT`)
/// guarantees `ort::init_from` runs at most once per process; concurrent
/// callers race to the closure, but only one closure body executes — the
/// rest see the cached result. This test exercises that race: spawn two
/// `open` calls in parallel via `tokio::join!` and assert both produce a
/// `BgeSmallProvider` (not an `ort init: already initialized` error).
///
/// If the wrapper were missing, the second call would error because
/// `ort::init_from` rejects double-init by design. This test proves the
/// wrapper is wired in production code (not just documented in comments).
#[tokio::test]
async fn test_concurrent_init_succeeds() {
    let model = model_path();
    let tokenizer = tokenizer_path();
    let ort_lib = ort_lib_path();

    // tokio::task::spawn_blocking because BgeSmallProvider::open is sync
    // (does file I/O + ort init + model load); the two opens then run on
    // separate worker threads in parallel. tokio::join! waits for both.
    let m1 = model.clone();
    let t1 = tokenizer.clone();
    let o1 = ort_lib.clone();
    let h1 = tokio::task::spawn_blocking(move || BgeSmallProvider::open(&m1, &t1, &o1));
    let h2 =
        tokio::task::spawn_blocking(move || BgeSmallProvider::open(&model, &tokenizer, &ort_lib));

    let (r1, r2) = tokio::join!(h1, h2);
    let r1 = r1.expect("join first open");
    let r2 = r2.expect("join second open");

    // Per ADR-007 BgeSmallProvider has no Debug; pattern-match for Ok.
    match r1 {
        Ok(_) => {}
        Err(e) => panic!("first concurrent open failed: {e}"),
    }
    match r2 {
        Ok(_) => {}
        Err(e) => panic!("second concurrent open failed: {e}"),
    }
}
