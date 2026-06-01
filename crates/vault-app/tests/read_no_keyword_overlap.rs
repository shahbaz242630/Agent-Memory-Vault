//! Bug-2 regression (live dogfood 2026-05-31): the read path must NOT
//! over-abstain on a purely-semantic query whose answer shares no keywords.
//!
//! ## The bug
//!
//! The production read stack wrapped the retriever in `AbstainingRetriever`,
//! whose top-1 BM25 (lexical) gate fired BEFORE the cross-encoder reranker (the
//! real read relevance authority, ADR-059) could judge the candidates. A query
//! with zero lexical overlap with its answer — "what does the user do for fun?"
//! vs "plays the cello in a community orchestra" — scored BM25 ~0, so the gate
//! short-circuited to an empty result and the read abstained on a fact the vault
//! holds. A memory vault that says "I don't know" about facts it has is broken
//! for the agent-read workload ([[correctness-is-the-product]]).
//!
//! ## The fix (2026-05-31)
//!
//! `Application::new` wires the `StructuredReadPipeline` against the RAW hybrid
//! retriever (BGE + Tantivy + RRF); read abstention is owned by the reranker
//! floor, not the BM25 keyword gate. The keyword gate stays on `memory_search`.
//!
//! ## What this test pins
//!
//! Real BGE + real Qwen3-Reranker over a one-fact vault:
//! - **No-keyword recall (the bug):** "what does the user do for fun?" surfaces
//!   the cello fact and does NOT abstain. FAILS on the pre-fix wiring (BM25 gate
//!   eats it), PASSES after.
//! - **A6 over-correction guard:** a genuine no-signal query
//!   ("what is the user's blood type?") still abstains — the fix must not turn
//!   the read into a never-abstain firehose ([[reference-mcp-dogfood-log-is-ground-truth]]).
//!
//! ## Running
//!
//! ```text
//! cargo test -p vault-app --test read_no_keyword_overlap -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`d: real BGE + reranker is ~12-13s/read on CPU (over the 5s test
//! budget), and needs the bge-small + qwen3-reranker fixtures
//! (run scripts/setup-dev-env.{sh,ps1}). Runs in the weekly real-model smoke.
//!
//! ## macOS deferral (ADR-033)
//!
//! Disabled on macOS — BGE/reranker transitively load ORT which SIGABRTs at
//! process exit on macOS. Linux + Windows cover the path.

#![cfg(not(target_os = "macos"))]

use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;

use vault_app::{AppConfig, Application};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_mcp::Adapter;
use vault_retrieval::ReadQuery;
use vault_storage::SqlCipherKey;

const BGE_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/bge-small-en-v1.5";
const RERANK_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/qwen3-reranker-0.6b-seq-cls";

fn fixture(rel: &str, name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p.push(name);
    assert!(
        p.exists(),
        "missing fixture {p:?} — run scripts/setup-dev-env.(sh|ps1) first"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib() -> PathBuf {
    fixture(BGE_FIXTURE_REL, "onnxruntime.dll")
}
#[cfg(target_os = "linux")]
fn ort_lib() -> PathBuf {
    fixture(BGE_FIXTURE_REL, "libonnxruntime.so")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real BGE + Qwen3-Reranker (~12s/read, needs fixtures); run with --ignored"]
async fn no_keyword_overlap_read_surfaces_fact_and_still_abstains_on_no_signal() {
    let tmp = TempDir::new().expect("tempdir");
    let config = AppConfig {
        metadata_path: tmp.path().join("vault.db"),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: SqlCipherKey::new("read-no-keyword-overlap-key"),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        qwen_model_path: None,
        phi4_model_path: None,
        // The reranker is the read relevance authority (ADR-059). Wiring it is
        // load-bearing for this test: it is what surfaces the no-keyword match.
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };
    let app = Application::new(&config)
        .await
        .expect("Application::new must compose the read stack (BGE + reranker)");
    let _shutdown = app.start(); // spawn the cascading retry worker
    let adapter = app.adapter();

    let boundary = Boundary::new("evalfun").expect("valid boundary");

    // Seed ONE fact, phrased so the "for fun" query shares NO content words with
    // it (no "user", no "fun") — so BM25 top-1 is ~0 and the OLD wiring would
    // have abstained here.
    let cello = NewMemory {
        content: "Plays the cello in a community orchestra on Sunday afternoons.".into(),
        memory_type: MemoryType::Semantic,
        boundary: boundary.clone(),
        source_agent: Some("claude".into()),
        confidence: 0.95,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    };
    let cello_id = adapter.write(cello).await.expect("seed write").to_string();

    // Let the cascade worker drain the write into LanceDB (semantic channel +
    // reranker candidate pool query the vector store).
    tokio::time::sleep(Duration::from_secs(5)).await;

    // ── 1. No-keyword recall (the bug). ──────────────────────────────────────
    let fun = adapter
        .read(ReadQuery {
            query_text: "what does the user do for fun?".into(),
            authorized_boundaries: vec![boundary.clone()],
        })
        .await
        .expect("read must not error");
    assert!(
        !fun.abstain,
        "no-keyword-overlap read MUST NOT abstain — the reranker should surface the cello fact \
         (this fails on the pre-fix BM25-gated wiring)"
    );
    assert!(
        fun.relevant_facts.iter().any(|f| f.memory_id == cello_id),
        "the cello fact MUST surface for 'what does the user do for fun?'; got {:?}",
        fun.relevant_facts
            .iter()
            .map(|f| &f.fact)
            .collect::<Vec<_>>()
    );

    // ── 2. A6 over-correction guard. ─────────────────────────────────────────
    // A genuine no-signal query against the same one-fact vault must still
    // abstain — removing the keyword gate must not make the read never-abstain.
    let blood = adapter
        .read(ReadQuery {
            query_text: "what is the user's blood type?".into(),
            authorized_boundaries: vec![boundary.clone()],
        })
        .await
        .expect("read must not error");
    assert!(
        blood.abstain,
        "no-signal read MUST still abstain (A6 guard); the reranker should score the unrelated \
         cello fact below its floor. got facts: {:?}",
        blood
            .relevant_facts
            .iter()
            .map(|f| &f.fact)
            .collect::<Vec<_>>()
    );
}

/// Multi-fact A7 regression — the gap the one-fact test above MISSED, found in
/// the §7 live dogfood (2026-06-01). With the subject-less cello fact buried
/// among ~12 distractors, BGE-small ranks it below the old `RERANK_CANDIDATE_CAP
/// = 8` for a loosely-phrased query, so it was truncated away BEFORE the
/// (correctly-framed, ADR-064) reranker could score it — and `memory_read`
/// abstained on a fact the vault held. The fix reranks the full retrieved pool
/// (cap = `DEFAULT_MAX_CANDIDATES`). This test pins that: the cello surfaces for
/// two loosely-phrased reads even amid distractors, AND the no-signal guard
/// still abstains. It FAILS on the cap-8 build, PASSES after.
///
/// `#[ignore]` for the same reason as the test above (real BGE + reranker over
/// the 5s budget); runs in the weekly real-model smoke.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real BGE + Qwen3-Reranker over a 13-fact vault (~slow); run with --ignored"]
async fn cello_surfaces_amid_distractors_and_still_abstains_on_no_signal() {
    let tmp = TempDir::new().expect("tempdir");
    let config = AppConfig {
        metadata_path: tmp.path().join("vault.db"),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: SqlCipherKey::new("read-cello-distractors-key"),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        qwen_model_path: None,
        phi4_model_path: None,
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };
    let app = Application::new(&config)
        .await
        .expect("Application::new must compose the read stack (BGE + reranker)");
    let _shutdown = app.start();
    let adapter = app.adapter();

    let boundary = Boundary::new("evalfun").expect("valid boundary");

    // The subject-less hobby fact (the dogfood repro) + 12 distractors mirroring
    // the §7 B-seed + Part-A leftovers. The cello has NO lexical overlap with the
    // "fun"/"music" queries, so BGE ranks it low amid these — exactly the case
    // that the cap-8 truncation dropped.
    const CELLO: &str = "Plays the cello in a community orchestra on Sunday afternoons.";
    let seeds = [
        CELLO,
        "The user drives a Tesla Model 3.",
        "The user sold the Tesla and now drives a Rivian R1T.",
        "The user works as a data scientist at Helix Labs.",
        "The user's favourite cuisine is Japanese.",
        "The user's dog is a Labrador named Biscuit.",
        "The user's favorite color is amber.",
        "The user relocated to Lisbon in March 2026 for a fresh start.",
        "The user prefers per-action commit approvals and four definition-of-done gates.",
        "The user works primarily in a dark-themed editor and finds light themes straining.",
        "The user collects vintage mechanical keyboards.",
        "The user enjoys trail running in the foothills on weekends.",
        "The user is checking normalization determinism.",
    ];
    let mut cello_id = String::new();
    for content in seeds {
        let nm = NewMemory {
            content: content.into(),
            memory_type: MemoryType::Semantic,
            boundary: boundary.clone(),
            source_agent: Some("claude".into()),
            confidence: 0.95,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        let id = adapter.write(nm).await.expect("seed write").to_string();
        if content == CELLO {
            cello_id = id;
        }
    }

    // Drain all 13 writes into LanceDB before reading (cascade is async). Longer
    // than the one-fact test: every seed must be embedded + queryable so the
    // assertion isolates RANKING (the cap), not cascade timing.
    tokio::time::sleep(Duration::from_secs(12)).await;

    // ── Two loosely-phrased reads — both MUST surface the cello amid distractors.
    for query in [
        "what does the user do for fun?",
        "what music does the user play?",
    ] {
        let resp = adapter
            .read(ReadQuery {
                query_text: query.into(),
                authorized_boundaries: vec![boundary.clone()],
            })
            .await
            .expect("read must not error");
        assert!(
            !resp.abstain,
            "{query:?} MUST NOT abstain — the cello is present; this FAILS on the cap-8 build \
             (BGE ranks the cello below 8 so the reranker never scores it)"
        );
        assert!(
            resp.relevant_facts.iter().any(|f| f.memory_id == cello_id),
            "the cello fact MUST surface for {query:?} amid distractors; got {:?}",
            resp.relevant_facts
                .iter()
                .map(|f| &f.fact)
                .collect::<Vec<_>>()
        );
    }

    // ── No-signal guard still holds against the populated vault.
    let blood = adapter
        .read(ReadQuery {
            query_text: "what is the user's blood type?".into(),
            authorized_boundaries: vec![boundary.clone()],
        })
        .await
        .expect("read must not error");
    assert!(
        blood.abstain,
        "no-signal read MUST still abstain even on a 13-fact vault (no firehose); got {:?}",
        blood
            .relevant_facts
            .iter()
            .map(|f| &f.fact)
            .collect::<Vec<_>>()
    );
}
