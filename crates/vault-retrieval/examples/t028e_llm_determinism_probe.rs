//! T0.2.7 Phase 1 — t028e LLM determinism probe (2026-05-18).
//!
//! **Question this spike answers.** Does Qwen-7B running on Vulkan with our
//! current production [`TuningConfig`] produce byte-identical outputs across
//! 5 consecutive `complete_json` calls on IDENTICAL inputs?
//!
//! The Phase A HANDOFF opener (2026-05-18) hypothesises GPU non-determinism
//! as the residual ~15% gap to 100% on the gauntlet. Evidence is suggestive
//! (Q21 + Q26 swing across DIFFERENT iter configs in `t028d_iter*.log`) but
//! not yet proven on identical-input runs. This probe proves or falsifies
//! the hypothesis cheaply before any production code change.
//!
//! **Methodology.**
//! 1. Resolve Qwen-7B GGUF via [`models_dir`].
//! 2. Open the provider ONCE with the live production [`TuningConfig`]
//!    (`n_threads=12`, `n_threads_batch=12`, `n_gpu_layers=99`).
//! 3. Build ONE canned user prompt inline (no fixture loader, no embedder,
//!    no retrieval). The prompt mirrors the Q26 Comcast `$89 vs $109`
//!    contradiction shape — the swing failure we want to reproduce.
//! 4. Build ONE [`CompletionParams`] matching `read_pipeline.rs:274-280`
//!    exactly (`temperature=0.0`, `top_p=1.0`, `seed=Some(42)`, system_prompt
//!    via [`READ_TIME_SYSTEM_PROMPT`], schema via [`READ_TIME_JSON_SCHEMA`]).
//! 5. Loop 5 times: call `complete_json`, store the raw String.
//! 6. Compare outputs:
//!    - All-equal? (the headline verdict)
//!    - If not, find the first byte index where output[0] and output[i]
//!      diverge — narrows where in the JSON stream drift first appears.
//!
//! **Interpretation.**
//! - All 5 identical → GPU non-determinism hypothesis is **falsified**.
//!   Q21/Q26 failures are deterministic algorithmic regressions, not GPU
//!   noise. Phase A focus would flip to prompt/retrieval iteration.
//! - Any divergence → GPU non-determinism **confirmed**. Next iteration of
//!   this probe plumbs `flash_attn=DISABLED` through `TuningConfig` and
//!   reruns; ladder continues with `n_threads_batch=1` and CPU-only fallback
//!   if Vulkan refuses to settle.
//!
//! **Discipline.** Self-contained probe. No new dependencies. No fixture
//! loading. No production code changes. No commit. Spike artefact will ride
//! with the eventual production fix per
//! `feedback_spike_examples_bundle_with_consumer_code.md`.
//!
//! Run with (PowerShell on Windows):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t028e_llm_determinism_probe
//! ```

use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use vault_llm::{CompletionParams, LlmProvider, Qwen25_14BProvider, TuningConfig};
use vault_retrieval::{READ_TIME_JSON_SCHEMA, READ_TIME_SYSTEM_PROMPT};

const QWEN_MODEL_FILENAME: &str = "Qwen2.5-7B-Instruct-Q4_K_M.gguf";
const N_RUNS: usize = 5;
const SEP_WIDE: usize = 100;

/// Inline canned user prompt mirroring the Q26 swing pattern (Comcast
/// `$89` vs `$109` contradiction) — the deterministic-input failure we want
/// to reproduce. Format matches `read_pipeline::build_user_prompt` exactly
/// so the LLM sees the same prompt shape as production. Memory IDs are
/// stable, content is fixed, no randomness anywhere.
const CANNED_USER_PROMPT: &str = "QUERY: Doing the monthly budget review — anything I should flag about household services costing more than expected?

CANDIDATES:
[019e3b5c-0001-7000-8000-000000000001] Comcast internet bill is $89/month on the current plan.
[019e3b5c-0001-7000-8000-000000000002] Electric utility (PG&E) averaged $142/month last quarter.
[019e3b5c-0001-7000-8000-000000000003] Comcast just sent a notice — the bill is now $109/month starting next cycle.
[019e3b5c-0001-7000-8000-000000000004] Water utility bill came in at $48 this month, down from $61 last month.
[019e3b5c-0001-7000-8000-000000000005] Netflix subscription renewed at $15.49/month.
[019e3b5c-0001-7000-8000-000000000006] Spotify family plan is $16.99/month, shared with three other family members.
[019e3b5c-0001-7000-8000-000000000007] Trash + recycling pickup is $42/month, billed quarterly.
[019e3b5c-0001-7000-8000-000000000008] Gym membership at LA Fitness costs $39/month plus a $49 annual fee.
[019e3b5c-0001-7000-8000-000000000009] Cell phone family plan with T-Mobile is $130/month for four lines.
[019e3b5c-0001-7000-8000-000000000010] Home insurance premium is $185/month escrowed with the mortgage.
[019e3b5c-0001-7000-8000-000000000011] Pet insurance for Boomer is $32/month through Healthy Paws.
[019e3b5c-0001-7000-8000-000000000012] Car insurance for both vehicles totals $215/month through GEICO.
[019e3b5c-0001-7000-8000-000000000013] Lawn care service runs $95/month during the growing season, March through October.
[019e3b5c-0001-7000-8000-000000000014] HOA dues are $245/month, covers pool maintenance and common-area landscaping.
[019e3b5c-0001-7000-8000-000000000015] Streaming bundle (Hulu + Disney+ + ESPN+) is $19.99/month after the introductory rate ended.

Filter, flag contradictions, synthesize. Return JSON.";

#[tokio::main]
async fn main() -> Result<()> {
    println!("{}", "=".repeat(SEP_WIDE));
    println!("T0.2.7 Phase 1 — t028e LLM determinism probe");
    println!("Method: load Qwen-7B once, call complete_json {N_RUNS}× on identical inputs,");
    println!("        compare raw outputs byte-for-byte. ");
    println!(
        "Started: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("{}", "=".repeat(SEP_WIDE));
    println!();

    // 1. Resolve Qwen-7B path.
    let qwen_path = models_dir()?.join(QWEN_MODEL_FILENAME);
    ensure!(
        qwen_path.exists(),
        "Qwen-7B GGUF missing at {qwen_path:?} — populate models dir per ADR-049 before running"
    );
    println!("Model path: {qwen_path:?}");

    // 2. Production TuningConfig (matches t028d:1239-1244 + ADR-049).
    let tuning = TuningConfig {
        n_threads: Some(12),
        n_threads_batch: Some(12),
        n_gpu_layers: Some(99),
        ..TuningConfig::default()
    };
    println!("TuningConfig: {tuning:?}");

    // 3. Production CompletionParams (matches read_pipeline.rs:274-280).
    let params = CompletionParams {
        max_tokens: 1024,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(READ_TIME_SYSTEM_PROMPT.to_string()),
    };
    println!(
        "CompletionParams: max_tokens={}, temperature={}, top_p={}, seed={:?}, system_prompt={}chars",
        params.max_tokens,
        params.temperature,
        params.top_p,
        params.seed,
        params.system_prompt.as_deref().map_or(0, |s| s.len()),
    );
    println!(
        "User prompt: {} chars ({} candidate memories)",
        CANNED_USER_PROMPT.len(),
        15
    );
    println!();

    // 4. Load Qwen ONCE (model + backend + GPU state stays warm for the 5 calls).
    println!("Opening Qwen-7B (Q4_K_M, Vulkan, n_gpu_layers=99)...");
    let load_start = std::time::Instant::now();
    let qwen_provider = Qwen25_14BProvider::open_with_tuning(&qwen_path, tuning).await?;
    println!(
        "Loaded in {:.1}s. model_id = {}",
        load_start.elapsed().as_secs_f64(),
        qwen_provider.model_id()
    );
    println!();

    // 5. Run N_RUNS identical calls, store raw outputs.
    let mut outputs: Vec<String> = Vec::with_capacity(N_RUNS);
    for run in 1..=N_RUNS {
        println!("--- Run {run}/{N_RUNS} ---");
        let t0 = std::time::Instant::now();
        let raw = qwen_provider
            .complete_json(CANNED_USER_PROMPT, READ_TIME_JSON_SCHEMA, &params)
            .await
            .with_context(|| format!("complete_json call {run}"))?;
        let dt = t0.elapsed().as_secs_f64();
        println!("  duration: {dt:.1}s | output length: {} chars", raw.len());
        // Show first 80 + last 80 chars so we can eyeball gross differences
        // even before the byte-equal verdict.
        let preview_head: String = raw.chars().take(80).collect();
        let preview_tail: String = raw
            .chars()
            .rev()
            .take(80)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        println!("  head[..80]: {preview_head}");
        println!("  tail[..80]: {preview_tail}");
        outputs.push(raw);
    }
    println!();

    // 6. Compare results.
    println!("{}", "=".repeat(SEP_WIDE));
    println!("VERDICT");
    println!("{}", "=".repeat(SEP_WIDE));

    let all_equal = outputs.windows(2).all(|w| w[0] == w[1]);
    let unique_outputs: std::collections::HashSet<&str> =
        outputs.iter().map(String::as_str).collect();

    if all_equal {
        println!("✅ ALL {N_RUNS} OUTPUTS BYTE-IDENTICAL.");
        println!("   GPU non-determinism hypothesis FALSIFIED for this configuration.");
        println!("   Q21/Q26 failures in the gauntlet are deterministic — fix path flips");
        println!("   back to prompt / retrieval iteration (NOT flash_attn plumbing).");
        println!("   Output length: {} chars", outputs[0].len());
    } else {
        println!(
            "❌ OUTPUTS DIFFER. {} unique outputs across {N_RUNS} runs.",
            unique_outputs.len()
        );
        println!("   GPU non-determinism hypothesis CONFIRMED for this configuration.");
        println!("   Next iteration: plumb flash_attn=DISABLED through TuningConfig + rerun.");
        println!();
        println!("Per-run divergence vs run 1:");
        for (i, out) in outputs.iter().enumerate() {
            if i == 0 {
                println!("  run 1: BASELINE ({} chars)", out.len());
                continue;
            }
            if out == &outputs[0] {
                println!("  run {}: IDENTICAL to run 1", i + 1);
                continue;
            }
            let first_diff = outputs[0]
                .as_bytes()
                .iter()
                .zip(out.as_bytes().iter())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| outputs[0].len().min(out.len()));
            println!(
                "  run {}: DIFFERS at byte {} (lengths: baseline={}, this={})",
                i + 1,
                first_diff,
                outputs[0].len(),
                out.len()
            );
            // Show a small window around the first divergence point so we
            // can see whether drift is in a number, a substring, or a
            // structural JSON field.
            let win_start = first_diff.saturating_sub(40);
            let win_end_base = (first_diff + 40).min(outputs[0].len());
            let win_end_this = (first_diff + 40).min(out.len());
            println!(
                "    baseline[{win_start}..{win_end_base}]: {:?}",
                &outputs[0][win_start..win_end_base]
            );
            println!(
                "        this[{win_start}..{win_end_this}]: {:?}",
                &out[win_start..win_end_this]
            );
        }
    }
    println!();
    println!("Probe complete.");

    Ok(())
}

/// Resolve the models directory the same way other t02* spikes do
/// (vault GGUFs live under `%APPDATA%\com.shahbaz242630.memory-vault\models`
/// on Windows per BRD §5 + ADR-043).
fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set on Windows")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}
