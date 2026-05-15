//! Qwen2.5-14B-Instruct provider — **spike-scoped, NOT yet architecture-locked.**
//!
//! Created for the T0.2.3 read-time-pipeline architecture decision spike
//! (2026-05-14). Mirrors `Phi4MiniProvider` structurally but uses Qwen2.5's
//! ChatML format and skips the download/SHA-verification chain (caller passes
//! a pre-cached GGUF path).
//!
//! If the spike validates Qwen2.5-14B as the V0.2 stage-3 synthesis model,
//! this file gets productionized in a follow-up: download + integrity
//! verification per the ADR-043 pattern, SHA + revision pins, ADR-049
//! documenting the model selection rationale. Until then: spike code.
//!
//! Chat template — Qwen2.5 standard ChatML (newlines between role and
//! content, no `<|im_sep|>` separator — this is the load-bearing difference
//! from Phi-4-mini's variant).

use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::json_schema_to_grammar;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use crate::error::{VaultLlmError, VaultLlmResult};
use crate::phi4_mini::get_or_init_backend;
use crate::provider::{CompletionParams, LlmProvider};

/// Returns the values that `LlamaContextParams::default()` resolves to for
/// `(n_threads, n_threads_batch, n_batch, n_ubatch)`. Spike-only — used by
/// t027a to confirm what "framework default" actually means on this build
/// before tuning is applied. The llama.cpp doctest in
/// `llama-cpp-2/src/context/params/get_set.rs` asserts `n_threads == 4`
/// for the default; this probe reads the live values.
#[must_use]
pub fn framework_defaults_probe() -> (i32, i32, u32, u32) {
    let p = LlamaContextParams::default();
    (
        p.n_threads(),
        p.n_threads_batch(),
        p.n_batch(),
        p.n_ubatch(),
    )
}

/// Spike-scoped tuning knobs for the Qwen2.5 inference loop.
///
/// All fields are `Option<T>`; `None` means "use llama.cpp framework default"
/// (n_threads=4, n_threads_batch=4, KV cache K/V = f16, n_batch = n_ctx).
/// Set fields individually to override.
///
/// Created for T0.2.3 t027a latency-tuning spike (2026-05-15). If a
/// configuration becomes the locked production tuning, it is named in
/// ADR-050 and these fields move from `Option<T>` to required on the
/// productionised provider.
#[derive(Debug, Clone, Default)]
pub struct TuningConfig {
    /// Generation-phase thread count. `None` = framework default (4).
    pub n_threads: Option<i32>,
    /// Prompt-eval (batch) thread count. `None` = framework default (4).
    pub n_threads_batch: Option<i32>,
    /// Batch size for prompt-eval chunking (tokens processed per
    /// `ctx.decode()` sub-step). `None` = match n_ctx (existing behaviour —
    /// single large batch).
    pub n_batch: Option<u32>,
    /// KV cache K data type. `None` = framework default (f16).
    pub type_k: Option<KvCacheType>,
    /// KV cache V data type. `None` = framework default (f16).
    pub type_v: Option<KvCacheType>,
    /// Number of model layers to offload to GPU (Vulkan / Metal / CUDA
    /// backend, depending on which feature flags are enabled on
    /// `llama-cpp-2`). `None` = pure CPU inference (current default).
    /// `Some(99)` = "offload all" — llama.cpp internally clamps to the
    /// model's actual layer count, so 99 is the standard sentinel.
    ///
    /// **Model-level parameter — applied once at `open_with_tuning()`
    /// time.** Cannot be changed via `complete_json_with_tuning`'s
    /// per-call override; that path only touches context params. To
    /// switch offload counts at runtime, drop the provider and re-open
    /// with a different `TuningConfig`.
    pub n_gpu_layers: Option<u32>,
}

/// Spike-scoped Qwen2.5-14B-Instruct provider.
///
/// `open()` takes a pre-cached GGUF path — no download chain. Production
/// would add a `Qwen25Config` + `Qwen25Provider::new(config)` mirroring
/// `Phi4MiniProvider::new` once architecture locks.
#[allow(non_camel_case_types)]
pub struct Qwen25_14BProvider {
    backend: Arc<LlamaBackend>,
    model: Arc<LlamaModel>,
    model_id: String,
    tuning: TuningConfig,
}

impl Qwen25_14BProvider {
    /// Open a Qwen2.5-14B GGUF from an existing file path. Spike-only —
    /// no download, no SHA verification. Production must replace with a
    /// `Phi4MiniConfig`-shaped builder per ADR-043.
    pub async fn open(model_path: &Path) -> VaultLlmResult<Self> {
        Self::open_with_tuning(model_path, TuningConfig::default()).await
    }

    /// Open with caller-supplied tuning knobs. Spike-only.
    pub async fn open_with_tuning(model_path: &Path, tuning: TuningConfig) -> VaultLlmResult<Self> {
        let backend = get_or_init_backend()?;

        let load_path = model_path.to_path_buf();
        let backend_for_load = backend.clone();
        let n_gpu_layers = tuning.n_gpu_layers;
        let model = tokio::task::spawn_blocking(move || {
            let mut params = LlamaModelParams::default();
            if let Some(n) = n_gpu_layers {
                params = params.with_n_gpu_layers(n);
            }
            LlamaModel::load_from_file(&backend_for_load, &load_path, &params).map_err(|e| {
                VaultLlmError::ModelLoadFailed(format!("LlamaModel::load_from_file: {e}"))
            })
        })
        .await
        .map_err(|e| VaultLlmError::ModelLoadFailed(format!("spawn_blocking (load): {e}")))??;

        let model_id = model_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("qwen2.5-14b-instruct")
            .to_string();

        Ok(Self {
            backend,
            model: Arc::new(model),
            model_id,
            tuning,
        })
    }

    /// Spike-only: invoke inference with a per-call tuning override that
    /// bypasses `self.tuning`. Lets a single loaded model be re-used across
    /// many tuning configurations without paying the model-reload cost
    /// (~10-15s cold per 4.36 GB GGUF, lower with warm OS file cache).
    /// Not part of the `LlmProvider` trait surface — production callers go
    /// through `complete_json` against the provider's stored tuning.
    pub async fn complete_json_with_tuning(
        &self,
        prompt: &str,
        json_schema: &str,
        params: &CompletionParams,
        tuning: TuningConfig,
    ) -> VaultLlmResult<String> {
        let backend = self.backend.clone();
        let model = self.model.clone();
        let prompt = prompt.to_string();
        let schema = json_schema.to_string();
        let params = params.clone();
        tokio::task::spawn_blocking(move || {
            run_one_inference_qwen(&backend, &model, &prompt, &schema, &params, &tuning)
        })
        .await
        .map_err(|e| VaultLlmError::InferenceFailed(format!("spawn_blocking (inference): {e}")))?
    }
}

impl std::fmt::Debug for Qwen25_14BProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Same redaction as Phi4MiniProvider — backend + model hold raw FFI handles.
        f.debug_struct("Qwen25_14BProvider")
            .field("model_id", &self.model_id)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl LlmProvider for Qwen25_14BProvider {
    async fn complete_json(
        &self,
        prompt: &str,
        json_schema: &str,
        params: &CompletionParams,
    ) -> VaultLlmResult<String> {
        let backend = self.backend.clone();
        let model = self.model.clone();
        let prompt = prompt.to_string();
        let schema = json_schema.to_string();
        let params = params.clone();
        let tuning = self.tuning.clone();
        tokio::task::spawn_blocking(move || {
            run_one_inference_qwen(&backend, &model, &prompt, &schema, &params, &tuning)
        })
        .await
        .map_err(|e| VaultLlmError::InferenceFailed(format!("spawn_blocking (inference): {e}")))?
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// Qwen2.5 standard ChatML chat-template builder.
///
/// Differs from Phi-4-mini's variant: newlines between role and content,
/// no `<|im_sep|>` separator. Qwen2.5 documentation specifies this format
/// verbatim.
fn build_chatml_prompt_qwen(user_msg: &str, system_override: Option<&str>) -> String {
    let system_msg = system_override.unwrap_or("You are a helpful assistant.");
    format!(
        "<|im_start|>system\n{system_msg}<|im_end|>\n\
         <|im_start|>user\n{user_msg}<|im_end|>\n\
         <|im_start|>assistant\n"
    )
}

/// Inner inference loop — structurally identical to `phi4_mini::run_one_inference`
/// modulo the chat-template call. Duplication accepted for spike-speed; if
/// architecture locks with both providers shipping, a shared `run_one_inference`
/// extracted into a `inference.rs` module is the refactor.
fn run_one_inference_qwen(
    backend: &LlamaBackend,
    model: &LlamaModel,
    user_prompt: &str,
    json_schema: &str,
    params: &CompletionParams,
    tuning: &TuningConfig,
) -> VaultLlmResult<String> {
    let full_prompt = build_chatml_prompt_qwen(user_prompt, params.system_prompt.as_deref());
    let gbnf = json_schema_to_grammar(json_schema)
        .map_err(|e| VaultLlmError::GrammarCompilation(format!("json_schema_to_grammar: {e}")))?;

    let tokens = model
        .str_to_token(&full_prompt, AddBos::Always)
        .map_err(|e| VaultLlmError::InferenceFailed(format!("str_to_token: {e}")))?;
    let n_predict = params.max_tokens as i32;
    // Larger context than Phi-4: Qwen2.5-14B supports 128K natively, and
    // stage-3 synthesis prompts will run 10-40K tokens with 20 candidates.
    // We size n_ctx to (prompt + predict) but cap at a reasonable upper
    // bound to avoid runaway KV-cache memory.
    let needed = tokens.len() as u32 + params.max_tokens;
    let n_ctx = needed.clamp(2048, 32_768);

    let n_batch = tuning.n_batch.unwrap_or(n_ctx);
    let mut ctx_params = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(n_ctx))
        .with_n_batch(n_batch);
    if let Some(nt) = tuning.n_threads {
        ctx_params = ctx_params.with_n_threads(nt);
    }
    if let Some(ntb) = tuning.n_threads_batch {
        ctx_params = ctx_params.with_n_threads_batch(ntb);
    }
    if let Some(tk) = tuning.type_k {
        ctx_params = ctx_params.with_type_k(tk);
    }
    if let Some(tv) = tuning.type_v {
        ctx_params = ctx_params.with_type_v(tv);
    }
    let mut ctx = model
        .new_context(backend, ctx_params)
        .map_err(|e| VaultLlmError::InferenceFailed(format!("new_context: {e}")))?;

    let mut batch = LlamaBatch::new(n_ctx as usize, 1);
    let last_idx = tokens.len().saturating_sub(1) as i32;
    for (i, token) in (0_i32..).zip(tokens.into_iter()) {
        batch
            .add(token, i, &[0], i == last_idx)
            .map_err(|e| VaultLlmError::InferenceFailed(format!("batch.add prompt: {e}")))?;
    }
    ctx.decode(&mut batch)
        .map_err(|e| VaultLlmError::InferenceFailed(format!("decode initial: {e}")))?;

    // Greedy sampling under GBNF — same posture as Phi4MiniProvider for V0.2.
    // temperature/top_p/seed are forward-compat; ignored at inference time.
    if params.temperature != 0.0 || params.top_p != 1.0 || params.seed.is_some() {
        tracing::warn!(
            temperature = params.temperature,
            top_p = params.top_p,
            seed = ?params.seed,
            "Qwen25_14BProvider uses greedy sampling under GBNF (spike); \
             non-default temperature/top_p/seed ignored."
        );
    }
    let grammar_sampler = LlamaSampler::grammar(model, &gbnf, "root")
        .map_err(|e| VaultLlmError::GrammarCompilation(format!("LlamaSampler::grammar: {e}")))?;
    let mut sampler = LlamaSampler::chain_simple([grammar_sampler, LlamaSampler::greedy()]);

    let mut n_cur = batch.n_tokens();
    let max_tokens = n_cur + n_predict;
    let mut output = String::new();
    while n_cur <= max_tokens {
        let token = sampler.sample(&ctx, batch.n_tokens() - 1);
        if model.is_eog_token(token) {
            break;
        }
        let bytes = model
            .token_to_piece_bytes(token, 64, false, None)
            .map_err(|e| VaultLlmError::InferenceFailed(format!("token_to_piece_bytes: {e}")))?;
        output.push_str(&String::from_utf8_lossy(&bytes));

        batch.clear();
        batch
            .add(token, n_cur, &[0], true)
            .map_err(|e| VaultLlmError::InferenceFailed(format!("batch.add loop: {e}")))?;
        n_cur += 1;
        ctx.decode(&mut batch)
            .map_err(|e| VaultLlmError::InferenceFailed(format!("decode loop: {e}")))?;
    }
    Ok(output)
}
