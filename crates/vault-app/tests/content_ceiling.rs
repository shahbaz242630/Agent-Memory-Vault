//! C4 content-ceiling probe (§7 Part A) — Claude Code's automated execution of
//! the test Claude Desktop cannot run (it won't emit a literal 50K+ payload).
//!
//! Writes bracketed payloads of increasing size through the SAME write path the
//! MCP `memory_write` tool uses (`Adapter::write`), then reads each back from the
//! metadata store by id and asserts the stored content is byte-length-intact with
//! BOTH bracket tokens present — i.e. the server either stores the full payload or
//! rejects cleanly, never silently truncating while returning success. Records
//! the true ceiling. Read-back is by id (synchronous metadata), NOT search, so it
//! does not depend on the async embedding cascade.
//!
//! `#[ignore]`d (real BGE init; needs the bge fixtures). Run:
//!   cargo test -p vault-app --test content_ceiling -- --ignored --nocapture
//!
//! macOS deferral (ADR-033): ORT SIGABRTs at process exit on macOS.

#![cfg(not(target_os = "macos"))]

use std::path::PathBuf;

use tempfile::TempDir;

use vault_app::{AppConfig, Application};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_mcp::Adapter;
use vault_storage::{MetadataStore, SqlCipherKey};

const BGE_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/bge-small-en-v1.5";
const VAULT_KEY: &str = "content-ceiling-key";

fn bge_fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(BGE_FIXTURE_REL);
    p.push(name);
    assert!(
        p.exists(),
        "missing bge fixture {p:?} — run scripts/setup-dev-env"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib() -> PathBuf {
    bge_fixture("onnxruntime.dll")
}
#[cfg(target_os = "linux")]
fn ort_lib() -> PathBuf {
    bge_fixture("libonnxruntime.so")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "C4 content-ceiling probe; real BGE; run with --ignored --nocapture"]
async fn content_ceiling_stores_intact_or_rejects_cleanly() {
    let tmp = TempDir::new().expect("tempdir");
    let vault_db = tmp.path().join("vault.db");
    let config = AppConfig {
        metadata_path: vault_db.clone(),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: SqlCipherKey::new(VAULT_KEY),
        model_path: bge_fixture("model.onnx"),
        tokenizer_path: bge_fixture("tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        qwen_model_path: None,
        phi4_model_path: None,
        rerank_model_path: None,
        rerank_tokenizer_path: None,
    };
    let app = Application::new(&config).await.expect("app");
    let _shutdown = app.start();
    let adapter = app.adapter();
    let boundary = Boundary::new("testeval").expect("boundary");

    // Second metadata handle on the same DB (deterministic key) for synchronous
    // read-back by id — independent of the async embedding cascade and the
    // search/BM25 path.
    let metadata = MetadataStore::open(&vault_db, SqlCipherKey::new(VAULT_KEY))
        .await
        .expect("open metadata for read-back");

    // content = CAP_OK_<n>_START + ("ABCDEFGHIJ" * reps) + CAP_OK_<n>_END.
    let make = |reps: usize| -> String {
        let n = reps * 10;
        format!(
            "CAP_OK_{n}_START{}CAP_OK_{n}_END",
            "ABCDEFGHIJ".repeat(reps)
        )
    };

    println!("\n================ C4 CONTENT-CEILING PROBE ================\n");
    let mut max_stored = 0usize;
    // 5K, 10K, 50K, 100K filler chars (+ ~30 char brackets).
    for reps in [500usize, 1_000, 5_000, 10_000] {
        let content = make(reps);
        let want_len = content.chars().count();
        let n = reps * 10;
        let start_tok = format!("CAP_OK_{n}_START");
        let end_tok = format!("CAP_OK_{n}_END");

        match adapter
            .write(NewMemory {
                content: content.clone(),
                memory_type: MemoryType::Semantic,
                boundary: boundary.clone(),
                source_agent: Some("claude-code".into()),
                confidence: 0.95,
                valid_from: None,
                valid_until: None,
                metadata: serde_json::json!({}),
            })
            .await
        {
            Err(e) => {
                println!("  [{want_len:>7} chars]  REJECTED cleanly → {e}");
            }
            Ok(id) => {
                let stored = metadata
                    .get_memory(&id)
                    .await
                    .expect("metadata get_memory")
                    .unwrap_or_else(|| {
                        panic!("C4: wrote {want_len}-char fact but it's absent from metadata")
                    });
                let got_len = stored.content.chars().count();
                let both_brackets =
                    stored.content.contains(&start_tok) && stored.content.contains(&end_tok);
                // The whole written payload must be present (the full string is a
                // prefix; storage appends at most a documented trailing period —
                // see C7/C9). That, plus both brackets, rules out mid/tail
                // truncation. We assert PRESENCE, not exact length, so the
                // canonical trailing-period normalization isn't misread as loss.
                let intact = stored.content.starts_with(&content);
                assert!(
                    intact && both_brackets,
                    "C4: written payload MUST be stored intact at {want_len} chars \
                     (got len {got_len}, starts_with_written={intact}, both_brackets={both_brackets}) — \
                     a shorter stored length or a missing bracket = SILENT TRUNCATION"
                );
                assert!(
                    got_len <= want_len + 2,
                    "C4: stored len {got_len} exceeds written {want_len} by >2 — unexpected padding"
                );
                max_stored = max_stored.max(want_len);
                println!("  [{want_len:>7} chars]  STORED INTACT ✅ (stored len {got_len}, both brackets, +{} normalization)", got_len - want_len);
            }
        }
    }
    println!("\n  → largest payload stored intact: {max_stored} chars");
    println!("=========================================================\n");
    assert!(
        max_stored >= 5_000,
        "C4: vault must store at least a 5K payload intact (got {max_stored})"
    );
}
