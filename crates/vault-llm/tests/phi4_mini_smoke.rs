//! `#[ignore]` real-model integration tests for `Phi4MiniProvider`.
//!
//! Per iteration 2 §3 concern #1 CI-policy lock: these tests run on the
//! weekly `0 12 * * MON` cron OR on PRs labelled `run-llm-smoke`, **NOT**
//! on every PR. They require:
//!
//! - LLVM/libclang installed (cargo build-time)
//! - The Phi-4-mini-instruct Q4_K_M GGUF at the default model_dir, OR
//!   network access to download from the pinned unsloth mirror (~3 min,
//!   ~2.49 GB).
//! - Windows runtime (uses `APPDATA` env var); cross-platform model_dir
//!   resolution lands at production-wiring time when vault-tauri / vault-app
//!   own the Tauri-data-dir resolution.
//!
//! Run manually with:
//!
//! ```text
//! cargo test -p vault-llm -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use vault_llm::{CompletionParams, LlmProvider, Phi4MiniConfig, Phi4MiniProvider};

fn windows_models_dir() -> PathBuf {
    let appdata = std::env::var("APPDATA").expect(
        "APPDATA env var must be set (this #[ignore] smoke test currently \
         runs on Windows only; cross-platform path resolution lands at \
         production-wiring time)",
    );
    PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models")
}

const T0_2_3_MERGE_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "merge": { "type": "boolean" },
        "score": { "type": "number" },
        "merged_text": { "type": "string" }
    },
    "required": ["merge", "score"],
    "additionalProperties": false
}"#;

/// Construct a real Phi4MiniProvider against the V0.2 default config.
/// First call downloads the model (~3 min); subsequent calls hash-verify
/// the cached file (~5s).
async fn build_provider() -> Phi4MiniProvider {
    let config = Phi4MiniConfig::v0_2_default(windows_models_dir());
    Phi4MiniProvider::new(config)
        .await
        .expect("Phi4MiniProvider construction (download/verify/load)")
}

#[tokio::test]
#[ignore = "real-model smoke; needs 2.49 GB Phi-4-mini GGUF + LLVM toolchain"]
async fn identical_memories_should_merge() {
    let provider = build_provider().await;
    let prompt = "Memory A: 'Buy milk'\nMemory B: 'Buy milk'\nShould these be merged?";
    let params = CompletionParams::default();
    let output = provider
        .complete_json(prompt, T0_2_3_MERGE_SCHEMA, &params)
        .await
        .expect("inference must succeed");
    let parsed: serde_json::Value =
        serde_json::from_str(&output).expect("output must be valid JSON");
    let merge = parsed
        .get("merge")
        .and_then(|v| v.as_bool())
        .expect("output must have boolean 'merge' field");
    assert!(merge, "identical memory pair must merge — got {output}");
}

#[tokio::test]
#[ignore = "real-model smoke; needs 2.49 GB Phi-4-mini GGUF + LLVM toolchain"]
async fn unrelated_memories_should_not_merge() {
    let provider = build_provider().await;
    let prompt = "Memory A: 'Buy milk'\nMemory B: 'Tax return deadline April 15'\n\
                  Should these be merged?";
    let params = CompletionParams::default();
    let output = provider
        .complete_json(prompt, T0_2_3_MERGE_SCHEMA, &params)
        .await
        .expect("inference must succeed");
    let parsed: serde_json::Value =
        serde_json::from_str(&output).expect("output must be valid JSON");
    let merge = parsed
        .get("merge")
        .and_then(|v| v.as_bool())
        .expect("output must have boolean 'merge' field");
    assert!(
        !merge,
        "topically-unrelated memory pair must NOT merge — got {output}"
    );
}

#[tokio::test]
#[ignore = "real-model smoke; needs 2.49 GB Phi-4-mini GGUF + LLVM toolchain"]
async fn model_id_matches_v0_2_default() {
    let provider = build_provider().await;
    assert_eq!(provider.model_id(), "Phi-4-mini-instruct-Q4_K_M");
}
