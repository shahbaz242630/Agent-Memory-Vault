//! T0.2.1 Phase 1 spike-2: Phi-4-mini load + JSON-output runtime confirmation.
//!
//! Per `feedback_runtime_confirmation_after_web_spike.md` discipline: the prior
//! research spike picked Phi-4-mini-instruct (MIT, Q4_K_M, ~2.49 GB) over
//! alternatives via web research only; this spike-2 is the empirical
//! confirmation on real hardware before locking the `LlmProvider` trait shape.
//!
//! Five stages run sequentially (Stage F dropped per locked CPU-only on
//! Shahbaz's i7-13620H Lenovo IdeaPad Slim 5 — no NVIDIA GPU present).
//!
//! - **Stage A** — model fetch from HuggingFace + SHA-256 hash compute over the
//!   downloaded bytes. Idempotent: re-running skips download if the file is
//!   already present and just re-hashes (so we can confirm pinned-hash later).
//! - **Stage B** — `LlamaBackend::init` + `LlamaModel::load_from_file`. Captures
//!   cold-load wall time.
//! - **Stage C** — JSON schema for T0.2.3 consolidator merge-decision is
//!   converted to GBNF via `llama_cpp_2::json_schema_to_grammar`, then a
//!   `LlamaSampler::grammar(...)` is constructed against it (adversarial probe
//!   for [llama.cpp#18173](https://github.com/ggml-org/llama.cpp/issues/18173)
//!   "Unexpected empty grammar stack after accepting piece" — if it fires here,
//!   it fires at construction time and we abort the spike).
//! - **Stage D** — 5 canned merge-decision prompts → run through Phi-4-mini with
//!   grammar-constrained sampling → assert each output parses as valid JSON.
//! - **Stage E** — 100-call latency bench. Reports p50 + p99 + mean. Pass gate:
//!   p50 fits T0.2.3 budget of 5-30 min for 100-1000 nightly merge-decisions
//!   (i.e. 1.8-18 s per call upper bound).
//!
//! Per iteration 2 item 3 lock, downloads land at the **production cache
//! location** (`%APPDATA%\com.shahbaz242630.memory-vault\models\`) so Phase 4
//! dogfood + the air-gap fallback path share the same file + same hash.
//!
//! Run with:
//!
//! ```text
//! cargo run -p vault-llm --example phi4_load_and_json_spike --release
//! ```
//!
//! Release profile recommended — debug builds make llama.cpp's CPU inference
//! 10-50× slower; latency bench in Stage E would not reflect production.

use anyhow::{anyhow, bail, Context, Result};
use futures::StreamExt;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::json_schema_to_grammar;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;
use sha2::{Digest, Sha256};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// Spike-2 finding (iteration 2 absorption): Microsoft's official
// `microsoft/Phi-4-mini-instruct-GGUF` repo is **gated** — returns HTTP 401
// without an HF auth token + accepted-license click-through. Production
// auto-download flow must point at a non-gated community redistribution.
// `unsloth/Phi-4-mini-instruct-GGUF` and `bartowski/microsoft_Phi-4-mini-instruct-GGUF`
// are the canonical MIT-license-preserving mirrors (verified 2026-05-13 at
// spike-2 Stage A retry — both return 302 to public xethub.hf.co URLs without
// auth). ADR-043 (drafted at Phase 5) locks the redistribution-source +
// revision-pin discipline; iteration 2's concern #7 lock now applies to
// the unsloth-or-bartowski commit hash, not floating /resolve/main/.
const MODEL_URL: &str = "https://huggingface.co/unsloth/Phi-4-mini-instruct-GGUF/resolve/main/Phi-4-mini-instruct-Q4_K_M.gguf";
const MODEL_FILENAME: &str = "Phi-4-mini-instruct-Q4_K_M.gguf";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    println!("=== T0.2.1 spike-2: Phi-4-mini load + JSON-output runtime confirmation ===\n");

    let model_dir = models_dir();
    std::fs::create_dir_all(&model_dir).context("create models dir")?;
    let model_path = model_dir.join(MODEL_FILENAME);
    println!("Target model path: {}\n", model_path.display());

    stage_a_download_and_hash(&model_path).await?;
    let (backend, model) = stage_b_init_and_load(&model_path)?;
    let gbnf = stage_c_schema_to_grammar(&model)?;
    stage_d_sample_inference(&backend, &model, &gbnf)?;
    stage_e_latency_bench(&backend, &model, &gbnf)?;

    println!("\n=== Spike-2 ALL STAGES PASS ===");
    Ok(())
}

/// Production cache location per iteration 2 item 3 lock. Spike-2 is Windows-only
/// (CPU-only locked); production code at Phase 3 uses the `directories` crate for
/// cross-platform support.
fn models_dir() -> PathBuf {
    let appdata = std::env::var("APPDATA").expect("APPDATA env var must be set on Windows");
    PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models")
}

// ─── Stage A ─────────────────────────────────────────────────────────────────

async fn stage_a_download_and_hash(model_path: &Path) -> Result<()> {
    println!("─── Stage A: model download + SHA-256 ───");
    let start = Instant::now();

    if model_path.exists() {
        let bytes = std::fs::metadata(model_path)?.len();
        println!("  Existing file at {}", model_path.display());
        println!(
            "  Size on disk: {:.2} GB ({} bytes)",
            bytes as f64 / 1e9,
            bytes
        );
        let hash = compute_sha256_of_file(model_path).await?;
        println!("  SHA-256: {}", hex::encode(hash));
        println!(
            "  Stage A elapsed: {:.2}s (skipped download — file already present)\n",
            start.elapsed().as_secs_f64()
        );
        return Ok(());
    }

    println!("  Downloading from: {MODEL_URL}");
    println!("  Expected size: ~2.49 GB (5-15 min on typical home broadband)");

    let resp = reqwest::get(MODEL_URL)
        .await
        .context("HTTP GET initial request")?
        .error_for_status()
        .context("HTTP non-2xx response status")?;

    let content_length = resp.content_length();
    if let Some(cl) = content_length {
        println!("  Content-Length: {:.2} GB ({} bytes)", cl as f64 / 1e9, cl);
        // Streaming-abort heuristic per iteration 2 concern #2 — if HF served
        // a redirect HTML page or a different quantization, the size will be
        // wildly off the expected ~2.49 GB. Cheap early reject.
        if !(1_000_000_000..=5_000_000_000).contains(&cl) {
            bail!(
                "Content-Length {} bytes is wildly off expected ~2.49 GB — \
                 aborting (likely wrong file or redirect HTML)",
                cl
            );
        }
    }

    let partial_path = model_path.with_extension("gguf.partial");
    // Restart-not-resume per iteration 2 concern #3: any .partial from a prior
    // failed run gets clobbered. SHA-256 verifies integrity post-stream, so a
    // restart-from-byte-0 strategy is correct and simple.
    let mut file = tokio::fs::File::create(&partial_path)
        .await
        .context("create .partial file")?;
    let mut hasher = Sha256::new();
    let mut bytes_total: u64 = 0;
    let mut last_log_ts = Instant::now();
    let mut stream = resp.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.context("HTTP stream chunk")?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.context(".partial write")?;
        bytes_total += chunk.len() as u64;
        if last_log_ts.elapsed().as_secs_f64() >= 5.0 {
            let mb = bytes_total as f64 / 1e6;
            let mbps = mb / start.elapsed().as_secs_f64();
            match content_length {
                Some(cl) => {
                    let pct = (bytes_total as f64 / cl as f64) * 100.0;
                    println!(
                        "    Progress: {:.0} MB / {:.0} MB ({:.1}%) at {:.1} MB/s",
                        mb,
                        cl as f64 / 1e6,
                        pct,
                        mbps
                    );
                }
                None => println!("    Progress: {mb:.0} MB at {mbps:.1} MB/s"),
            }
            last_log_ts = Instant::now();
        }
    }
    file.flush().await.context("flush .partial")?;
    drop(file);

    let hash = hasher.finalize();
    let hash_hex = hex::encode(hash);
    println!(
        "  Downloaded {} bytes ({:.2} GB)",
        bytes_total,
        bytes_total as f64 / 1e9
    );
    println!("  SHA-256: {hash_hex}");

    tokio::fs::rename(&partial_path, model_path)
        .await
        .context("atomic rename .partial → final")?;

    println!("  Stage A elapsed: {:.2}s\n", start.elapsed().as_secs_f64());
    Ok(())
}

async fn compute_sha256_of_file(path: &Path) -> Result<[u8; 32]> {
    let mut file = tokio::fs::File::open(path)
        .await
        .context("open existing file for hashing")?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 8 * 1024 * 1024]; // 8 MB chunks
    loop {
        let n = file.read(&mut buf).await.context("read chunk")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

// ─── Stage B ─────────────────────────────────────────────────────────────────

fn stage_b_init_and_load(model_path: &Path) -> Result<(LlamaBackend, LlamaModel)> {
    println!("─── Stage B: backend init + model load ───");
    let stage_start = Instant::now();

    let init_start = Instant::now();
    let backend = LlamaBackend::init().map_err(|e| anyhow!("LlamaBackend::init: {e}"))?;
    println!(
        "  LlamaBackend::init OK ({} ms)",
        init_start.elapsed().as_millis()
    );

    let load_start = Instant::now();
    let model_params = LlamaModelParams::default();
    let model = LlamaModel::load_from_file(&backend, model_path, &model_params)
        .map_err(|e| anyhow!("LlamaModel::load_from_file: {e}"))?;
    println!(
        "  LlamaModel::load_from_file OK ({:.2}s)",
        load_start.elapsed().as_secs_f64()
    );
    println!("  model.n_ctx_train(): {}", model.n_ctx_train());

    println!(
        "  Stage B elapsed: {:.2}s\n",
        stage_start.elapsed().as_secs_f64()
    );
    Ok((backend, model))
}

// ─── Stage C ─────────────────────────────────────────────────────────────────

fn stage_c_schema_to_grammar(model: &LlamaModel) -> Result<String> {
    println!("─── Stage C: JSON schema → GBNF + #18173 adversarial probe ───");
    let start = Instant::now();

    // T0.2.3 consolidator merge-decision schema:
    //   merge       — bool, mandatory
    //   score       — float in [0, 1], mandatory
    //   merged_text — string, optional (only meaningful if merge=true)
    let schema_str = r#"{
  "type": "object",
  "properties": {
    "merge": { "type": "boolean" },
    "score": { "type": "number" },
    "merged_text": { "type": "string" }
  },
  "required": ["merge", "score"],
  "additionalProperties": false
}"#;
    println!("  Schema:\n{schema_str}\n");

    let gbnf =
        json_schema_to_grammar(schema_str).map_err(|e| anyhow!("json_schema_to_grammar: {e}"))?;
    println!("  Generated GBNF ({} bytes):", gbnf.len());
    println!("{gbnf}");

    // Adversarial probe: construct LlamaSampler::grammar — if llama.cpp#18173
    // "Unexpected empty grammar stack after accepting piece" is going to fire
    // for our schema shape, it fires here at construction time. If it does, we
    // abort and iteration 2 picks up the rewrite question.
    println!("  #18173 probe: constructing LlamaSampler::grammar(model, gbnf, \"root\") ...");
    let _probe = LlamaSampler::grammar(model, &gbnf, "root")
        .map_err(|e| anyhow!("LlamaSampler::grammar — potential #18173 fired: {e}"))?;
    println!("  #18173 probe PASS — sampler constructed without GrammarError");

    println!("  Stage C elapsed: {:.2}s\n", start.elapsed().as_secs_f64());
    Ok(gbnf)
}

// ─── Stage D ─────────────────────────────────────────────────────────────────

fn stage_d_sample_inference(backend: &LlamaBackend, model: &LlamaModel, gbnf: &str) -> Result<()> {
    println!("─── Stage D: 5 canned merge-decision inferences ───");

    let prompts: Vec<(&str, &str)> = vec![
        (
            "identical-A",
            "Memory A: 'Buy milk'\nMemory B: 'Buy milk'\nShould these be merged?",
        ),
        (
            "identical-B",
            "Memory A: 'Meeting at 3pm Friday'\nMemory B: 'Meeting at 3pm Friday'\nShould these be merged?",
        ),
        (
            "similar-A",
            "Memory A: 'Bought groceries today'\nMemory B: 'Picked up groceries this afternoon'\nShould these be merged?",
        ),
        (
            "unrelated-A",
            "Memory A: 'Buy milk'\nMemory B: 'Tax return deadline April 15'\nShould these be merged?",
        ),
        (
            "unrelated-B",
            "Memory A: 'Doctor appointment Tuesday'\nMemory B: 'Send invoice to client X'\nShould these be merged?",
        ),
    ];

    for (label, prompt_user) in prompts {
        let start = Instant::now();
        let full_prompt = build_chatml_prompt(prompt_user);
        let output = run_one_inference(backend, model, &full_prompt, gbnf, 256, 1024)?;
        let elapsed = start.elapsed();

        let parsed: std::result::Result<serde_json::Value, _> = serde_json::from_str(&output);
        let json_ok = parsed.is_ok();
        let preview: String = output.trim().chars().take(160).collect();
        println!(
            "  [{label}] {:.2}s — JSON valid: {json_ok} — output: {preview}",
            elapsed.as_secs_f64()
        );
        if !json_ok {
            bail!("Stage D: prompt [{label}] returned non-JSON output: {output}");
        }
    }

    println!();
    Ok(())
}

fn build_chatml_prompt(user_msg: &str) -> String {
    // Phi-4-mini-instruct expects a ChatML-style prompt template.
    // For the spike we hand-craft it; production code (Phase 3) will use
    // `model.chat_template(None)` + `model.apply_chat_template_oaicompat(...)`
    // for robustness across model swaps.
    format!(
        "<|im_start|>system<|im_sep|>You are a JSON-only memory-merge classifier. \
         Respond with strict JSON matching this schema: \
         {{\"merge\": bool, \"score\": float between 0 and 1, \"merged_text\": optional string}}. \
         If merge is true, set merged_text to the consolidated string; if false, set it to empty string.<|im_end|>\
         <|im_start|>user<|im_sep|>{user_msg}<|im_end|>\
         <|im_start|>assistant<|im_sep|>"
    )
}

// ─── Stage E ─────────────────────────────────────────────────────────────────

fn stage_e_latency_bench(backend: &LlamaBackend, model: &LlamaModel, gbnf: &str) -> Result<()> {
    println!("─── Stage E: 100-call latency bench ───");

    let prompt = build_chatml_prompt(
        "Memory A: 'Buy milk'\nMemory B: 'Get milk from store'\nShould these be merged?",
    );

    let n = 100usize;
    let mut latencies_ms = Vec::with_capacity(n);
    let bench_start = Instant::now();
    println!("  Running {n} calls (no warmup separation — first call counts) ...");
    for i in 0..n {
        let call_start = Instant::now();
        let _output = run_one_inference(backend, model, &prompt, gbnf, 128, 1024)?;
        latencies_ms.push(call_start.elapsed().as_millis() as u64);
        if (i + 1) % 10 == 0 {
            println!(
                "    progress: {}/{n} (cumulative {:.1}s)",
                i + 1,
                bench_start.elapsed().as_secs_f64()
            );
        }
    }
    latencies_ms.sort_unstable();
    let p50 = latencies_ms[n / 2];
    let p99 = latencies_ms[(n * 99) / 100];
    let mean = latencies_ms.iter().sum::<u64>() as f64 / n as f64;
    println!("  Latency over {n} calls:  p50={p50} ms  p99={p99} ms  mean={mean:.0} ms");

    // T0.2.3 budget: 5-30 min for 100-1000 nightly merge-decisions →
    // upper-bound 30 min / 100 = 18 s/call; lower-bound 5 min / 1000 = 0.3 s/call.
    // Spike-pass criterion: p50 ≤ 18 s (we don't enforce the lower bound;
    // faster is always fine).
    const T0_2_3_UPPER_BOUND_MS: u64 = 18_000;
    if p50 > T0_2_3_UPPER_BOUND_MS {
        bail!(
            "Stage E: p50 {p50} ms exceeds T0.2.3 consolidator upper-bound \
             {T0_2_3_UPPER_BOUND_MS} ms (5-30 min / 100-1000 pairs band)"
        );
    }
    println!("  T0.2.3 consolidator budget check: PASS (p50 within band)\n");
    Ok(())
}

// ─── inference inner loop (shared across Stages D + E) ───────────────────────

fn run_one_inference(
    backend: &LlamaBackend,
    model: &LlamaModel,
    prompt: &str,
    gbnf: &str,
    n_predict: i32,
    n_ctx_min: u32,
) -> Result<String> {
    let tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| anyhow!("model.str_to_token: {e}"))?;
    let n_ctx = n_ctx_min.max(tokens.len() as u32 + n_predict as u32);

    let ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(n_ctx))
        .with_n_batch(n_ctx);
    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| anyhow!("model.new_context: {e}"))?;

    let mut batch = LlamaBatch::new(n_ctx as usize, 1);
    let last_idx = tokens.len().saturating_sub(1) as i32;
    for (i, token) in (0_i32..).zip(tokens.into_iter()) {
        batch
            .add(token, i, &[0], i == last_idx)
            .map_err(|e| anyhow!("batch.add (prompt): {e}"))?;
    }
    ctx.decode(&mut batch)
        .map_err(|e| anyhow!("ctx.decode (initial prompt): {e}"))?;

    let grammar_sampler = LlamaSampler::grammar(model, gbnf, "root")
        .map_err(|e| anyhow!("LlamaSampler::grammar (per-call): {e}"))?;
    let mut sampler = LlamaSampler::chain_simple([grammar_sampler, LlamaSampler::greedy()]);

    let mut n_cur = batch.n_tokens();
    let max_tokens = n_cur + n_predict;
    let mut output = String::new();
    while n_cur <= max_tokens {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        if model.is_eog_token(token) {
            break;
        }
        // `token_to_piece_bytes(token, buffer_size_hint, special=false, lstrip=None)`
        // — non-deprecated replacement for `token_to_bytes(token, Special::Plaintext)`.
        // `special=false` matches `Special::Plaintext` semantics (don't render
        // special tokens like `<|im_end|>` verbatim into the output).
        // Buffer hint = 64 bytes: the deprecated `token_to_bytes` defaults to 8
        // but spike-2 Stage D first run surfaced `Insufficient Buffer Space -10`
        // on a multi-byte token. 64 bytes is safely above the longest reasonable
        // single-token UTF-8 sequence for this tokenizer family; iteration 2
        // absorbs this as a phi4_mini provider implementation lock.
        let bytes = model
            .token_to_piece_bytes(token, 64, false, None)
            .map_err(|e| anyhow!("model.token_to_piece_bytes: {e}"))?;
        output.push_str(&String::from_utf8_lossy(&bytes));

        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| anyhow!("batch.add (decode loop): {e}"))?;
        n_cur += 1;
        ctx.decode(&mut batch)
            .map_err(|e| anyhow!("ctx.decode (decode loop): {e}"))?;
    }
    Ok(output)
}
