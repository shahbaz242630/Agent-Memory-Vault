//! Live end-to-end proof that first-run reranker acquisition actually works
//! (ADR-087).
//!
//! # Why this exists
//!
//! Every other test around `model_fetch` is offline: they check path
//! composition, URL shape, and that the pinned hashes are well-formed. None of
//! them proves the one thing that matters to a new user — that pointing our
//! code at an empty directory results in a working reranker on disk.
//!
//! The gap is not theoretical. Before this test existed, the acquisition path
//! had been "verified" only by reading a Hub directory listing and by fetching
//! the small tokenizer with a shell command. Neither exercised
//! `ensure_reranker` itself. A wrong URL, a redirect that serves HTML, a
//! mis-transcribed hash constant, or a `.partial` rename bug would all have
//! passed the offline suite and then failed on every beta user's first launch.
//!
//! # Why it is `#[ignore]`
//!
//! It downloads ~1.15 GB from Hugging Face. Project rule: no test in the normal
//! suite makes network calls, and no test takes longer than 5 seconds. This
//! follows the precedent already set by `vault-llm`'s Phi-4 smoke test —
//! `#[ignore]`, run deliberately:
//!
//! ```text
//! cargo test -p vault-tauri --test reranker_download -- --ignored --nocapture
//! ```
//!
//! Run it when the acquisition path changes: URLs, hashes, the downloader, or
//! the `.partial` strategy. It is the only test that would catch a dead mirror.

use std::time::Instant;

use vault_tauri::model_fetch::{ensure_reranker, RERANKER_MODEL_BYTES, RERANKER_TOKENIZER_BYTES};

/// Full cold-start acquisition against an empty directory, exactly as a new
/// install experiences it — then a second call proving the cache short-circuit.
#[tokio::test]
#[ignore = "downloads ~1.15 GB from Hugging Face; run with --ignored"]
async fn cold_download_fetches_and_verifies_both_reranker_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let models_dir = tmp.path();

    // ── Cold path: nothing on disk ──────────────────────────────────────
    let started = Instant::now();
    let paths = ensure_reranker(models_dir)
        .await
        .expect("cold reranker download must succeed");
    let cold_elapsed = started.elapsed();

    println!(
        "cold download completed in {:.1}s",
        cold_elapsed.as_secs_f64()
    );

    // Both files landed at the paths the app will look in.
    assert!(paths.model.exists(), "model must exist after download");
    assert!(
        paths.tokenizer.exists(),
        "tokenizer must exist after download"
    );

    // Exact byte counts. `ensure_model_at_path` already verified SHA-256 —
    // if these sizes match AND it returned Ok, the bytes are right. Asserting
    // size independently catches the case where a future refactor drops the
    // hash check: size alone is weak, but size-mismatch is conclusive.
    let model_len = std::fs::metadata(&paths.model)
        .expect("model metadata")
        .len();
    let tok_len = std::fs::metadata(&paths.tokenizer)
        .expect("tokenizer metadata")
        .len();
    assert_eq!(
        model_len, RERANKER_MODEL_BYTES,
        "downloaded model size must match the pinned constant"
    );
    assert_eq!(
        tok_len, RERANKER_TOKENIZER_BYTES,
        "downloaded tokenizer size must match the pinned constant"
    );

    // No `.partial` files left behind. The atomic-rename contract says a
    // partial only survives a FAILED download; a successful one must leave the
    // directory clean. This is also the regression guard for the
    // `with_extension("gguf.partial")` bug — under the old code the ONNX
    // download would have renamed from `model.gguf.partial`, and any stray
    // partial would surface here.
    let dir = paths.model.parent().expect("model parent dir");
    let strays: Vec<_> = std::fs::read_dir(dir)
        .expect("read models dir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".partial"))
        .collect();
    assert!(
        strays.is_empty(),
        "successful download must leave no .partial files, found: {strays:?}"
    );

    // ── Warm path: the ADR-043 cache-hit / air-gap short-circuit ─────────
    // A second call must verify the existing files and return WITHOUT
    // re-downloading. This is what stops every app launch re-fetching 1.15 GB,
    // so it is not an optimisation — it is load-bearing. Hashing 1.15 GB takes
    // a couple of seconds; re-downloading takes minutes, so the bound is
    // generous but still decisive.
    let started = Instant::now();
    let again = ensure_reranker(models_dir)
        .await
        .expect("warm reranker check must succeed");
    let warm_elapsed = started.elapsed();

    println!(
        "warm cache check completed in {:.1}s",
        warm_elapsed.as_secs_f64()
    );

    assert_eq!(again, paths, "warm call must resolve the same paths");
    assert!(
        warm_elapsed < cold_elapsed,
        "warm call ({warm_elapsed:?}) must be faster than cold ({cold_elapsed:?}) \
         — if not, the cache short-circuit is not firing and every launch re-downloads"
    );
}

/// A corrupt on-disk model must be detected and replaced, not loaded.
///
/// This is the failure mode a user hits after a disk error or an interrupted
/// copy. Silently loading a corrupt model would produce plausible-looking but
/// meaningless rankings — far worse than an error.
#[tokio::test]
#[ignore = "downloads ~1.15 GB from Hugging Face; run with --ignored"]
async fn corrupt_cached_file_is_detected_and_refetched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let models_dir = tmp.path();

    let paths = ensure_reranker(models_dir)
        .await
        .expect("initial download must succeed");

    // Corrupt the tokenizer (small, so the refetch is quick) by truncating it.
    std::fs::write(&paths.tokenizer, b"this is not a tokenizer").expect("corrupt tokenizer");
    assert_ne!(
        std::fs::metadata(&paths.tokenizer).expect("metadata").len(),
        RERANKER_TOKENIZER_BYTES,
        "precondition: tokenizer is now corrupt"
    );

    ensure_reranker(models_dir)
        .await
        .expect("corrupt file must be detected and refetched, not accepted");

    assert_eq!(
        std::fs::metadata(&paths.tokenizer).expect("metadata").len(),
        RERANKER_TOKENIZER_BYTES,
        "corrupt tokenizer must have been replaced with the real file"
    );
}
