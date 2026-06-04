//! ADR-067 regression (live dogfood 2026-06-02): `memory_search` must NOT
//! short-circuit to an empty result on single-token / no-lexical-overlap
//! queries whose answer the vault holds.
//!
//! ## The bug
//!
//! The `memory_search` path wrapped the hybrid retriever in an
//! `AbstainingRetriever` whose top-1 BM25 (lexical) gate returned `[]` whenever
//! the keyword channel scored below 1.0. On a single-token query (`"amber"`,
//! `"C7UNI"`) or a no-lexical-overlap query, the gate fired in ~0-2ms BEFORE the
//! semantic channel ran, dropping facts that were present and semantically
//! findable. The live log showed `memory_search "amber"` → `[]` (2ms) while a
//! `memory_read "what is the user's favorite color?"` one call earlier surfaced
//! the very same "amber" fact ([[reference-mcp-dogfood-log-is-ground-truth]]).
//! A search tool that returns nothing for content it holds is broken for the
//! agent workload ([[correctness-is-the-product]]).
//!
//! ## The fix (ADR-067, 2026-06-04)
//!
//! `Application::new` wires `memory_search` against the RAW hybrid retriever
//! (BGE + Tantivy + RRF), dropping the hard BM25 gate — mirroring the read
//! recall-first stance (ADR-066). Search returns the semantic-backed ranked
//! candidates and the calling agent judges relevance.
//!
//! ## What this test pins
//!
//! Real BGE over a small vault, through the real `Adapter::search`:
//! - **No-lexical-overlap recall (the bug):** `"food"` surfaces the Japanese-
//!   cuisine fact (zero shared content words). FAILS on the pre-fix BM25 gate.
//! - **Single-token recall (the live repro):** `"amber"` surfaces the favorite-
//!   color fact. FAILS on the pre-fix gate (BM25 top-1 below 1.0 → abstained).
//!
//! ## Running
//!
//! ```text
//! cargo test -p vault-app --test search_recall_first -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`d: real BGE needs the bge-small fixtures (run
//! scripts/setup-dev-env.{sh,ps1}) and the cascade drain pushes it over the 5s
//! test budget. Runs in the weekly real-model smoke. Search uses the hybrid
//! only (no reranker), so no reranker fixture is required.
//!
//! ## macOS deferral (ADR-033)
//!
//! Disabled on macOS — BGE transitively loads ORT which SIGABRTs at process
//! exit on macOS. Linux + Windows cover the path.

#![cfg(not(target_os = "macos"))]

use std::path::PathBuf;
use std::time::Duration;

use tempfile::TempDir;

use vault_app::{AppConfig, Application};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_mcp::Adapter;
use vault_retrieval::{RetrievalOptions, RetrievalQuery};
use vault_storage::SqlCipherKey;

const BGE_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/bge-small-en-v1.5";

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
#[ignore = "real BGE hybrid search recall-first regression (needs fixtures); run with --ignored"]
async fn search_surfaces_facts_on_single_token_and_no_overlap_queries() {
    let tmp = TempDir::new().expect("tempdir");
    let config = AppConfig {
        metadata_path: tmp.path().join("vault.db"),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: SqlCipherKey::new("search-recall-first-key"),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        qwen_model_path: None,
        phi4_model_path: None,
        // Search uses the hybrid only — no reranker required.
        rerank_model_path: None,
        rerank_tokenizer_path: None,
    };
    let app = Application::new(&config)
        .await
        .expect("Application::new must compose the search stack (BGE)");
    let _shutdown = app.start(); // spawn the cascading retry worker
    let adapter = app.adapter();

    let boundary = Boundary::new("evalsearch").expect("valid boundary");

    // Seed a small diverse vault. The two probed facts are phrased so the query
    // shares NO content words with them (the exact case the BM25 gate ate).
    const CUISINE: &str = "The user's favourite cuisine is Japanese.";
    const COLOR: &str = "The user's favorite color is amber.";
    let seeds = [
        CUISINE,
        COLOR,
        "The user works as a data scientist at Helix Labs.",
        "The user drives a Rivian R1T.",
        "The user has a golden retriever named Biscuit.",
        "The user collects vintage mechanical keyboards.",
    ];
    let mut ids: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
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
        ids.insert(content, id);
    }

    // Drain the writes into LanceDB before searching (cascade is async).
    tokio::time::sleep(Duration::from_secs(8)).await;

    let search = |query: &str| {
        let q = RetrievalQuery {
            query_text: query.to_string(),
            authorized_boundaries: vec![boundary.clone()],
            max_results: 10,
            options: RetrievalOptions {
                score_threshold: None,
                include_archived: false,
            },
        };
        adapter.search(q)
    };

    // ── 1. No-lexical-overlap recall (the bug). ──────────────────────────────
    // "food" shares no content word with "favourite cuisine is Japanese"; the
    // pre-fix BM25 gate scored ~0 and returned [] before the semantic channel.
    let food = search("food").await.expect("search must not error");
    assert!(
        food.iter().any(|m| m.memory.id.to_string() == ids[CUISINE]),
        "search 'food' MUST surface the Japanese-cuisine fact via the semantic \
         channel (FAILS on the pre-fix BM25 gate); got {:?}",
        food.iter().map(|m| &m.memory.content).collect::<Vec<_>>()
    );

    // ── 2. Single-token recall (the live "amber" repro). ─────────────────────
    let amber = search("amber").await.expect("search must not error");
    assert!(
        amber.iter().any(|m| m.memory.id.to_string() == ids[COLOR]),
        "search 'amber' MUST surface the favorite-color fact (FAILS on the \
         pre-fix gate, which abstained in ~2ms on the single-token BM25 probe); \
         got {:?}",
        amber.iter().map(|m| &m.memory.content).collect::<Vec<_>>()
    );
}
