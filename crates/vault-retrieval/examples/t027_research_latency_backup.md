# T0.2.3 — Qwen2.5-7B Latency Backup Playbook

**Authored:** 2026-05-15 — research-mode brief, not yet promoted to ADR.
**Status:** Contingency. Only read if `t027a` (threads + KV-cache q8_0 sweep) leaves us above the 120s hard ceiling.
**Owner:** Shahbaz (product) + me (architect, senior dev). Solo two-person bootstrap.

This document is the **map**. Numbers labelled "speculative" are educated guesses anchored to adjacent benchmarks — they need a spike to confirm before any production lock-in. Numbers labelled "verified by source X" are from cited sources I trust enough to plan around.

---

## 1. Executive summary

Baseline (t026): **mean 187s, p99 224s** on Qwen2.5-7B-Q4_K_M, framework defaults (n_threads=4 — confirmed via `framework_defaults_probe()` doctest in `llama-cpp-2/src/context/params/get_set.rs`), 8K-token prompts with GBNF-constrained JSON.

Hard ceiling: **120s/query.** Target: 180s → 120s = **~35% reduction needed past t027a**.

If t027a's thread sweep alone delivers the standard 2-3x speedup (4 → 10/12 threads on a 16T CPU), we already land at ~70-90s and this document is filed. **The three highest-leverage backup levers, in order, if t027a underperforms:**

1. **Vulkan iGPU offload on Iris Xe** (research question 3). The `vulkan` Cargo feature is already wired in `llama-cpp-2 0.1.146`. Empirical 7B Q4 numbers on Iris Xe via Vulkan: **~6-10 tok/s generation, 50-100+ tok/s prompt processing** vs ~3-7 tok/s CPU. Expected aggregate latency: **80-130s** range. Caveat: Windows driver fragility on Intel iGPUs is real and documented (GitHub #17106, #17056, #20201). **Confidence: medium.** This is the highest single-shot upside if the spike succeeds on Shahbaz's Iris Xe driver stack.
2. **Drop to Qwen2.5-3B-Instruct** (research question 5). Same tokenizer family as Qwen2.5-7B (151,646 vocab — verified), Apache 2.0, MMLU ~65 vs 7B's 74.2. Expected latency: **~75-90s** linearly scaled by parameter count (speculative — 3B/7B = 0.43). **Quality risk: not yet validated for our contradiction-surfacing workload.** Need a t027c quality-replay spike on the 8 t026 queries before committing. **Confidence: medium-high on latency, low on quality without the spike.**
3. **Speculative decoding with Qwen2.5-0.5B as draft + Qwen2.5-7B as target** (research question 1). Same vocabulary across the Qwen2.5 family (verified — research-mode #4). Reported speedups on CPU: **1.5x–2.5x with same-family drafts and code-shaped output**; our GBNF-constrained JSON output is closer to "code" than "open chat" so acceptance rate should be on the high end. **Critical blocker:** `llama-cpp-2 0.1.146` has **no safe-wrapper speculative API** (confirmed by reading `sampling.rs` and `context/kv_cache.rs`). Build effort: significant — would need to compose `kv_cache_seq_*` primitives + raw `llama-cpp-sys-2` FFI, or fork the crate. **Confidence: high on the technique's potential, low on the integration cost. This is a 2-3 week build, not a weekend spike.**

Three more techniques are worth running as cheap-and-fast experiments before the heavier levers:

4. **Prefix caching of the system prompt** (research question 7). The 8K system prompt is shared across queries; processing it once and reusing the KV-cache state across calls could save **~20-50% of prompt-eval time** (speculative, scaled from `--cache-reuse` reports). `llama-cpp-2` exposes `copy_kv_cache_seq`, `kv_cache_seq_add`, `clear_kv_cache_seq` (verified at `context/kv_cache.rs:30-194`) — the primitives exist; the orchestration is new code. **Confidence: high on technique, medium on our ability to ship cleanly in 1-2 days.**
5. **Drop GBNF grammar; parse free-form JSON instead** (research question 6). GBNF overhead on simple schemas is typically small (low single-digit %), but our schema has nested arrays/objects which is where grammars stall. Worst case is a meaningful slowdown that disappears when we remove the constraint. **Quality risk: medium-high** — without grammar, we lose the "valid JSON guaranteed" property and add a parse-retry path. **Confidence: low net win, but cheap to measure (1 day spike).**
6. **Compress top-20 to top-12 retrieval** (research question 6). At 8K-token prompts, dropping 8 candidates removes ~30-40% of prompt tokens. Speculative latency reduction: **~15-25%**. Recall risk on multi-cluster queries like Q19 (8 memories across 3 clusters) is the biggest concern. **Confidence: high latency, medium recall risk.**

**Hardware paths to defer:**
- Apple Silicon directional ballpark: M2 Pro / M3 / M4 should deliver **~30-60s** for the same workload (verified pattern from Apple Silicon benchmarks). Not relevant for the founder's i7-13620H baseline but matters for macOS users.
- Intel OpenVINO / IPEX-LLM: real but messy. OpenVINO 2026 supports Qwen-2.5-7B on Iris Xe but the integration cost into our pure-Rust stack is high (Python/C++ shim layer). **Defer to V1.0**.
- NPU on Intel Core Ultra: not Shahbaz's CPU; Raptor Lake-H has no NPU. Skip.

**Smaller-model paths:**
- Qwen2.5-3B: most promising. Need quality spike.
- Qwen2.5-1.5B: probably too small (Qwen3 paper shows 1B-class drops ~10% on MMLU under 4-bit quant; we're already at Q4_K_M).
- Gemma-3-4B: viable alternative; IFEval 90.2 (better than Llama-3.2-3B's 77.4). Worth a quality spike alongside Qwen-3B.
- Llama-3.2-3B: weaker than Gemma-3-4B on instruction-following per llm-stats comparison.
- Phi-4-mini (3.8B): **already ruled out** at t025 (1/8 contradiction surfacing).

---

## 2. Per-technique deep dive

### 2.1 Speculative decoding (Qwen-7B target + Qwen-0.5B/1.5B draft)

**(a) What it is:** A small "draft" model proposes K tokens ahead; the large "target" verifies all K in a single forward pass. Accepted tokens skip the target's serial decode loop. Speedup = (accepted tokens) / (target forward passes) × hardware-batch efficiency.

**(b) Expected latency gain on our setup:**
- **Same-family draft (Qwen2.5-0.5B with Qwen2.5-7B):** 1.5x–2.5x reported on CPU for code/structured-output workloads ([discussion #10466](https://github.com/ggml-org/llama.cpp/discussions/10466), reported ~80% acceptance for code). Our GBNF-constrained JSON output behaves more like code than free chat → **expect the high end, 2x–2.5x on the generation phase**.
- Important asymmetry: prompt-eval is unchanged (still one 8K-token forward pass). Speculative decoding only helps the ~500-token generation phase. In t026, generation likely dominates (a Q4_K_M 7B at 4 threads on Raptor Lake-H runs ~3-5 tok/s gen → ~100-170s for 500 tokens). **Net wall-clock reduction: ~30-50%, i.e., 180s → 90-125s**.
- After threads sweep already cuts generation cost, speculative's marginal gain shrinks. The two techniques **don't compose linearly**.

**(c) Quality risk:** Speculative decoding with correct rejection sampling is **mathematically lossless** — the target distribution is preserved exactly. Greedy speculative (which we'd use, given temperature=0) is exactly identical to non-speculative output. **Quality risk: zero, by construction.** This is the technique's biggest selling point.

**(d) Implementation effort in our stack:** **HIGH — this is the blocker.**
- I read `llama-cpp-2 0.1.146` source. Confirmed:
  - **No `speculative` module, no `LlamaSampler::speculative`, no draft-model helper** (`sampling.rs` lines 28-612 enumerated; only `chain`, `temp`, `top_k`, `top_p`, `grammar`, `dist`, `greedy`, etc.).
  - **The primitives exist** (`context/kv_cache.rs:30-194`): `copy_cache`, `copy_kv_cache_seq`, `clear_kv_cache_seq`, `llama_kv_cache_seq_keep`, `kv_cache_seq_add`, `kv_cache_seq_div`, `kv_cache_seq_pos_max`. These match `llama_kv_cache_seq_*` in the C API needed for parallel-sequence batches.
  - **`LlamaBatch` supports multi-sequence tokens** via `add(token, pos, &[seq_ids], logits)` (per our current call site in `qwen25.rs:253-255`).
- Build paths in increasing scope:
  1. **Use `llama-cpp-sys-2` raw FFI** to call `llama_decode` with a draft-then-verify pattern, hand-roll rejection sampling against the target's logits, manage two `LlamaContext`s (one per model). **~2-3 weeks, plus debugging.**
  2. **Fork llama-cpp-rs and add a `SpeculativeContext` safe wrapper.** Same engineering work but reusable + upstreamable. **~3-4 weeks.**
  3. **Wait for upstream.** No active speculative-decoding PR was found in `utilityai/llama-cpp-rs` (search returned no results 2026-05-15). The C++ `llama.cpp` has had speculative in `llama-cli` for years; the Rust wrapper just hasn't surfaced it.

**(e) License / privacy implications:**
- Qwen2.5-0.5B / 1.5B GGUFs are Apache 2.0. Same as Qwen2.5-7B. **No new license risk.**
- Adds ~400 MB (Qwen2.5-0.5B-Q4_K_M) or ~1 GB (1.5B-Q4_K_M) to the model bundle. Both fit our model directory.
- **Memory pressure on 16 GB systems:** at runtime, both models share `LlamaBackend` but each has its own KV cache. Qwen-7B Q4_K_M @ 8K ctx ≈ 4.36 GB weights + ~1 GB KV cache. Qwen-0.5B Q4_K_M adds ~400 MB + ~100 MB KV. Total ~6 GB resident, fits 16 GB host comfortably alongside the OS + LanceDB + SQLCipher + BGE.
- **Privacy:** unchanged. Both models run locally; no network traffic.

**(f) Confidence:** **High on the technique's potential** (well-validated theoretical and empirical foundation). **Low on integration cost** (the safe-wrapper hole is genuine, not a documentation gap). If t027a delivers the threads speedup and we land at 100s, **defer speculative entirely until V1.1** because the integration ROI flips negative. Only worth pursuing if we're stuck above 150s after every other lever is exhausted.

**Phi-4-mini as draft model — RULED OUT.** Phi-4-mini's vocabulary is 200,064 tokens; Qwen2.5's is 151,646. Speculative decoding requires identical tokenizer/vocab (else the draft's token IDs can't be verified against the target's logits). [LM Studio docs](https://lmstudio.ai/docs/app/advanced/speculative-decoding) confirm this hard requirement. **Cross-family doesn't work**; this isn't a "low acceptance rate" problem, it's a "cannot run" problem.

---

### 2.2 Lower quantizations of Qwen2.5-7B

**(a) What it is:** Replace Q4_K_M (~4.5 bpw, 4.36 GB) with smaller per-weight encodings: Q3_K_M (~3.9 bpw), Q3_K_S (~3.5 bpw), IQ4_XS (~4.25 bpw), IQ4_NL (~4.5 bpw), IQ3_XS (~3.3 bpw).

**(b) Expected latency gain:**
- **Q4_K_M → IQ4_XS:** ~3% smaller weights but on **Apple Silicon Metal**, IQ-quants are ~3.8x **slower** than K-quants due to upstream llama.cpp kernel regressions ([HF discussion #5617](https://github.com/ggml-org/llama.cpp/discussions/5617)). On CPU AVX2, IQ4_XS performance varies by kernel; older reports show parity or marginal win. **Expected gain on our CPU: 0-10%, with real risk of regression.**
- **Q4_K_M → Q3_K_M / Q3_K_S:** ~15-25% smaller weights, proportional reduction in memory-bandwidth pressure (which is the bottleneck on CPU inference). **Expected gain: 10-20%.**
- **Q4_K_M → IQ3_XS:** ~25% smaller. **Expected gain: 15-25%** but with the IQ-kernel performance caveat.

**(c) Quality risk: HIGH and the biggest concern of this whole document.**
- The "Empirical Study of Qwen3 Quantization" ([arxiv 2505.02214](https://arxiv.org/abs/2505.02214)) — most relevant published study — finds Qwen3-14B loses only ~1% MMLU at 4-bit GPTQ, but Qwen3-0.6B drops ~10%. Qwen2.5-7B falls between; my speculative estimate is **2-5% MMLU drop at Q3_K_M**, **5-10% at Q3_K_S/IQ3_XS**.
- **MMLU and perplexity DO NOT measure our quality signal.** Our t026 result is `4/4 contradiction surfacing on oblique queries` — a nuanced reasoning task with structured-output constraints. Standard benchmarks systematically under-report quality cliffs on this kind of task.
- The contradictions we caught (Q26: Comcast $89 vs $109; Q25: GA in Q1 vs Q2) required the model to read both numbers in a 20-candidate context, recognize they refer to the same fact, and emit them both in JSON. **This is exactly the failure mode where lower quants regress without showing on MMLU.**
- **Quality cliff intuition:** Q4_K_M is the community sweet spot. Below 4-bit, instruction-following tends to crack first, followed by structured-output reliability. We currently sit at the safe edge.

**(d) Implementation effort:** **LOW.** Drop in a new GGUF file. ~30 min including download + SHA pin update + ADR-043-style integrity record.

**(e) License / privacy:** Unchanged. All Qwen2.5 GGUFs are Apache 2.0.

**(f) Confidence:** **Medium-low.** A focused spike (replay 8 t026 queries on Q3_K_M and IQ4_XS, assert 4/4 contradictions + 2/2 hard-negatives) is **cheap (1 day) and load-bearing**. But I would NOT promote a lower quant without that quality replay. The downside (losing our validated quality bar) is bigger than the upside (10-20% latency).

---

### 2.3 Vulkan / OpenCL / SYCL GPU offload via Iris Xe

**(a) What it is:** Compile llama.cpp with Vulkan support; offload matmul kernels to the Intel Iris Xe iGPU (96 EUs, shares system RAM). The `vulkan` Cargo feature in `llama-cpp-2 0.1.146` is wired (verified at `Cargo.toml:55`).

**(b) Expected latency gain:**
- **Verified numbers** from llama.cpp Vulkan benchmark thread [discussion #10879](https://github.com/ggml-org/llama.cpp/discussions/10879) for **i7-1185G7 + Iris Xe (TGL GT2)** running 7B Q4 models:
  - Jan 2025: pp512 ~42 tok/s, tg128 ~7.3 tok/s
  - Feb 2025: pp512 ~51 tok/s, tg128 ~8.3 tok/s
  - Jun 2025: pp512 ~106 tok/s, tg128 ~5.9 tok/s (driver/kernel regression in tg!)
- **For our i7-13620H + Iris Xe** (Raptor Lake-H, same 96 EU class, slightly higher base clocks): expect **pp ~80-120 tok/s, tg ~6-10 tok/s** (speculative scaling).
- **Our wall-clock math:**
  - Prompt eval: 8K tokens at 100 tok/s = **80s** (vs ~80-120s CPU at 4 threads, ~30-40s CPU at 10 threads).
  - Generation: 500 tokens at 8 tok/s = **62s** (vs ~125s CPU at 4 threads, ~50-60s CPU at 10 threads).
  - **Total: ~140s.** That's BETTER than baseline 187s but probably NOT better than the t027a 10-thread CPU result. **Vulkan may underperform a properly-threaded CPU on this hardware class.**
- Anonymous blog post [Local AI on Integrated Graphics](https://zenvanriel.com/ai-engineer-blog/local-ai-integrated-graphics-vulkan-offload/) reports the same shape: Iris Xe Vulkan = 8-12 pp, 6-10 tg for a 7B Q4 model.

**(c) Quality risk:**
- **Driver-dependent garbage output is documented.** [llama.cpp #17106](https://github.com/ggml-org/llama.cpp/issues/17106) — "Vulkan output is gibberish on Intel GPU"; mitigation is `GGML_VK_DISABLE_F16=1`. [#17056](https://github.com/ggml-org/llama.cpp/issues/17056) — Vulkan not working on Intel GPU. [#19327](https://github.com/ggml-org/llama.cpp/issues/19327) — crashes on Arrow Lake iGPU with larger models. [#20201](https://github.com/ggml-org/llama.cpp/issues/20201) — Intel iGPU + Vulkan crashes since b8148.
- Windows-side: Intel iGPU driver ≥ 31.0.101.5522 is the documented minimum for non-garbage output on recent llama.cpp. Beta-user-laptop driver variance is a real distribution problem.
- **For our quality signal (4/4 contradictions),** garbage output is a catastrophic failure mode, not a degradation. The probability is non-trivial across 30+ beta users with different driver vintages.

**(d) Implementation effort:**
- **Build side:** trivial. Add `features = ["vulkan"]` to `vault-llm`'s `llama-cpp-2` dep + ship the Vulkan SDK runtime. **~2 hours.**
- **Runtime side:** add `n_gpu_layers` to `LlamaModelParams` (need to verify the safe wrapper exposes this — I'd bet it does; `LlamaModelParams` is the standard surface). Add a config knob for CPU/Vulkan switching with fallback. **~1 day.**
- **Distribution:** Vulkan SDK runtime DLLs (Windows) or shared libs (Linux) need bundling. The MoltenVK shim on macOS is **questionable** — Apple users would be better off with Metal (which is also a `llama-cpp-2` feature). So Vulkan is a Windows + Linux strategy.
- **CI side:** matrix runners don't have Iris Xe GPUs; can compile-test but can't runtime-test. **Quality regressions only surface in beta-user telemetry.**

**(e) License / privacy:** Unchanged. Local inference.

**(f) Confidence:** **Medium.** The mean-case gain is real but **probably smaller than well-threaded CPU**. The tail risk (driver garbage) is real. **Recommendation: only spike this if t027a + speculative both underperform.** Even then, gate behind a feature flag with auto-fallback to CPU on quality assertion failure.

**OpenCL:** llama.cpp's OpenCL backend exists but is in maintenance mode; Vulkan is the recommended Intel iGPU path. Skip.

**SYCL:** Intel's first-party path (oneAPI-based). Better performance than Vulkan on Intel hardware, but build complexity is high (oneAPI compiler toolchain). **Defer to V1.0** unless we hit a wall.

---

### 2.4 Alternative inference engines

**Filter criteria (re-stated):** (i) mature, (ii) Win+Linux+macOS, (iii) Rust-callable without massive integration, (iv) permissive license.

| Engine | Maturity | Win | Linux | macOS | Rust-native | License | Verdict |
|---|---|---|---|---|---|---|---|
| **candle** | Mature, HF-backed | Y | Y | Y (Metal) | Native | Apache 2.0 / MIT | **Viable** |
| **mistral.rs** | Active, individual maintainer | Y | Y | Y (Metal) | Native | MIT | **Viable** |
| **ExLlamaV2** | Mature | Linux-only realistically | Y | N | Python | MIT | **Rule out** (Python, GPU-focused) |
| **MLC-LLM** | Mature, mobile/edge focus | Y | Y | Y | C++ via FFI | Apache 2.0 | **Defer** (integration cost) |
| **ONNX Runtime** (with Qwen ONNX) | Mature, MS-backed | Y | Y | Y | Crate exists | MIT | **Possible but odd** for LLMs |
| **vLLM** | Mature | N (Linux only) | Y | N | Python | Apache 2.0 | **Rule out** (server, GPU) |

**candle (a)** — Hugging Face's pure-Rust ML framework. Supports quantized LLaMA, Mistral, Phi, Qwen via GGUF.

**candle (b)** — On Apple M1, candle was within ~10-15% of llama.cpp on Mistral-7B Q4 ([advanced-stack benchmark](https://advanced-stack.com/resources/inference-performance-benchmark-of-mistral-ai-instruct-using-llama-cpp.html)). **On CPU Windows, candle's matmul kernels are less aggressively tuned than llama.cpp's `tinyBLAS`** (justine.lol's work) — expect parity or 10-20% **slower**, not faster.

**candle (c)** — Quality matches reference (same weights, same numerics). Zero risk.

**candle (d)** — Migration cost is **significant**: full provider rewrite (~1-2 weeks), new GBNF/grammar story (candle's grammar support is weaker than llama.cpp's). The GBNF dependency alone is load-bearing for our JSON-output guarantee.

**candle (e)** — Apache 2.0/MIT. Privacy unchanged.

**candle (f)** — **Confidence: LOW that it's faster.** The only reason to switch would be if we needed a candle-specific feature (e.g., better Vulkan story, or some pre-baked model llama.cpp doesn't load). **Skip.**

---

**mistral.rs (a)** — Eric Buehler's Rust-native inference engine. PagedAttention, continuous batching, **speculative decoding via draft models is documented as in-development** ([GitHub disc #245](https://github.com/EricLBuehler/mistral.rs/discussions/245)).

**mistral.rs (b)** — Claims faster-than-llama.cpp on CUDA in some benchmarks. **CPU benchmarks are sparse** — I couldn't find a head-to-head Mistral-7B Q4 CPU comparison. Likely parity-or-slower than llama.cpp on CPU AVX2.

**mistral.rs (c)** — Same weights → same quality. Zero risk on the model side. But its **GGUF + grammar story is less proven** than llama.cpp's — may need to migrate grammar definitions.

**mistral.rs (d)** — **Higher integration cost than candle**: less HF-shaped API, smaller community, newer surface. **~2-3 weeks** if everything works. **High risk of hitting an undocumented edge case.**

**mistral.rs (e)** — MIT. Privacy unchanged.

**mistral.rs (f)** — **Confidence: LOW-MEDIUM.** The one reason to consider it is if it ships native speculative decoding before llama-cpp-rs does. **Monitor, don't migrate.** Re-evaluate at V0.2 → V1.0 boundary.

---

**ONNX Runtime (with Qwen2.5-7B-INT8 ONNX export):**
- We already use `ort` for BGE embeddings; the dependency is in our tree.
- Microsoft maintains Phi-4 / Qwen2.5 ONNX exports for OpenVINO/DirectML/CUDA EPs.
- **CPU performance on ORT for 7B LLMs is typically slower than llama.cpp's hand-tuned matmul** — ORT is optimized for transformer encoders and CNN classifiers, not autoregressive 7B decoding.
- **GBNF/grammar support: nonexistent.** We'd have to either: (a) drop constrained decoding, (b) build a custom logit-mask layer on top of ORT, (c) post-validate JSON. None are appealing.
- **Verdict: rule out for the read-time path.** ORT stays in our BGE story; not a fit for the synthesis path.

---

### 2.5 Smaller models that could pass our quality bar

**The whole question hinges on what we don't know empirically.** Our 4/4 contradiction surfacing on Qwen-7B was measured. None of these has been measured on our 8-query benchmark. Until that spike runs, claims about which model "could pass" are unfalsifiable.

**Recommended quality-replay spike (t027c, 1 day):** Re-run the t026 8-query fixture against each candidate model. Pass criterion: 4/4 contradictions + 2/2 hard-negatives, just like Qwen-7B.

#### Qwen2.5-3B-Instruct
- **Latency speculative:** 3B/7B = 0.43; expect **~75-90s** at framework defaults, **~30-45s** after threads sweep. **Comfortably under 120s.**
- **Quality:** Apache 2.0, same Qwen tokenizer (151,646 vocab, verified), same instruction-tuning regime. Qwen2.5 technical report: "Qwen2.5-3B-Instruct outperforms Phi3.5-mini-Instruct and MiniCPM3-4B in mathematics and coding tasks while delivering competitive results in language understanding." [Qwen blog](https://qwenlm.github.io/blog/qwen2.5-llm/). No published contradiction-detection benchmark.
- **Speculative quality assessment:** the contradiction-surfacing task at Qwen-7B's bar may degrade by ~15-25% (e.g., 3/4 instead of 4/4 on oblique queries like Q25, Q26). **Unknown until the spike.**
- **Confidence: HIGH on latency, MEDIUM on quality.** Top candidate after t027a.

#### Qwen2.5-1.5B-Instruct
- **Latency speculative:** 1.5B/7B = 0.21; expect **~40-50s** at framework defaults, **~15-25s** after threads sweep.
- **Quality:** Apache 2.0. Significantly weaker on instruction-following per Qwen technical report (parameter scale matters a lot under 2B). Q4_K_M further compresses. The arxiv 2505.02214 Qwen3 quantization study showed ~10% MMLU drop at 4-bit for 0.6B-class models — **1.5B at Q4_K_M is in the danger zone**.
- **Speculative quality assessment:** **probably falls below our bar.** 2/4 contradiction surfacing likely.
- **Confidence: HIGH on latency, LOW on quality.** Spike only after 3B fails.

#### Phi-4-mini (3.8B)
- **Ruled out at t025** (1/8 contradiction surfacing on this workload). Filed.

#### Llama-3.2-3B-Instruct
- **Latency speculative:** ~3B class, similar to Qwen-3B: **~75-90s** baseline, **~30-45s** threaded.
- **Quality:** Llama 3 license (community license, permissive but with usage restrictions — check ToS for V1.0 commercial path). Instruction-following weaker than Qwen2.5-3B per [llm-stats comparison](https://llm-stats.com/models/compare/gemma-3-4b-it-vs-llama-3.2-3b-instruct): IFEval 77.4 vs Gemma-3-4B's 90.2.
- **Confidence: MEDIUM on latency, LOW on quality.** Probably weaker than Qwen-3B on our task.

#### Gemma-3-4B
- **Latency speculative:** ~4B class, **~100-110s** baseline, **~40-55s** threaded.
- **Quality:** Gemma license (Google, permissive with use-policy restrictions — re-check for V1.0 commercial). IFEval 90.2, GSM8k 89.2, MATH 75.6 (per [llm-stats](https://llm-stats.com/models/compare/gemma-3-4b-it-vs-llama-3.2-3b-instruct)). 131K context.
- **Speculative quality assessment:** the IFEval score suggests strong structured-output and instruction-following. **This is the strongest 3-5B alternative on paper.**
- **Confidence: MEDIUM on latency, MEDIUM-HIGH on quality. Worth spiking alongside Qwen-3B.**

**Recommended spike priority for t027c:**
1. Qwen2.5-3B-Instruct
2. Gemma-3-4B
3. Qwen2.5-1.5B (only if 3B+4B both pass)

---

### 2.6 Prompt-shape optimizations

#### 2.6a — Reduce top-20 retrieval to top-K (K=10–14)

**(a) What:** Cut the BGE retrieve cap from 20 to 10/12/14 candidates. Each candidate is 50-300 tokens; 8 fewer candidates ≈ **~1500-2400 fewer tokens** in the 8K prompt.

**(b) Latency gain:** Prompt eval scales ~linearly with token count on CPU. Removing 25-35% of prompt = **~25-35% prompt-eval reduction** ≈ **15-25% wall-clock reduction**, depending on how much of total latency is prompt vs generation.

**(c) Quality risk: HIGH on multi-cluster queries.**
- Q19 ("Where are we with the launch timeline?") spans 3 clusters across 8 memories. If those memories are ranked positions 15-20 by BGE, top-12 drops them.
- Q26's contradiction pair is unlikely to BOTH be in positions 15-20 — typically contradictions are semantically similar, so they cluster in top-10. **Lower risk for contradiction surfacing.**
- Hard-negatives (Q21, Q22) are unchanged — the verdict is `vault_has_no_relevant_content=true` regardless of K.

**(d) Effort:** Trivial. Change `max_results: 20` to `max_results: 12` in the retrieval call. ~5 min plus quality replay.

**(e) License / privacy:** Unchanged.

**(f) Confidence: HIGH latency, MEDIUM recall. Cheap to measure.** Recommended as part of t027c's spike.

#### 2.6b — Compress retrieved chunks (dedupe, drop boilerplate)

Speculative gain: 10-15% prompt size reduction with mild quality risk. **Implementation cost (writing a chunk-compressor) outweighs the gain.** Skip for V0.2.

#### 2.6c — Restructure system prompt for token-efficiency

The current `STANDALONE_SYSTEM_PROMPT` is ~250 tokens. Cutting in half saves 125 tokens out of 8K = ~1.5% prompt reduction. **Not load-bearing. Skip.**

#### 2.6d — Drop GBNF grammar; parse free-form JSON instead

**(a) What:** Remove `LlamaSampler::grammar(...)` from the sampler chain; let the model emit free-form text; parse JSON with `serde_json::from_str`; retry with corrective prompt on parse failure.

**(b) Latency gain:** GBNF overhead depends on grammar complexity. Our schema has nested arrays and optional fields (a known slow pattern per [llama.cpp grammars README](https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md)). **Speculative gain: 5-15%.** Mostly per-token sampling overhead, ~constant per generated token.

**(c) Quality risk: MEDIUM-HIGH.**
- We lose the "always-valid JSON" guarantee. Need a retry loop with a corrective prompt ("you emitted invalid JSON; here was your output [X], please re-emit valid JSON matching schema [Y]").
- Qwen2.5-7B-Instruct is well-tuned for structured output and usually emits valid JSON without grammar, but **the failure mode (parse error → retry → +180s latency for that query) is worse than the success mode (5-15% faster)**.
- Tail risk: a query that retries 2-3 times can blow past 500s.

**(d) Effort:** Remove grammar sampler (~10 lines). Add retry-with-corrective-prompt loop (~50 lines). Add parse-failure telemetry. ~1 day.

**(e) License / privacy:** Unchanged.

**(f) Confidence: LOW net win.** The mean speeds up modestly, but the tail latency gets worse. **Don't recommend** unless we're desperate AND we have observability to catch retry storms.

---

### 2.7 Caching / batching strategies

#### 2.7a — Prefix caching of the shared system prompt

**(a) What:** Compute the KV-cache state for the system prompt + GBNF once at startup; clone it into the working KV cache before each query so we skip re-tokenizing and re-decoding those tokens.

**(b) Latency gain:**
- Our 250-token system prompt is ~3% of an 8K-token prompt. At face value, prefix cache saves ~3% of prompt-eval time = ~2% wall-clock. **Not worth it for system prompt alone.**
- **BUT** if we also cache the GBNF grammar compilation (which currently re-runs per query in `qwen25.rs:216-217`), we save the `json_schema_to_grammar` cost too. Speculative gain: ~1-3 seconds per query.
- **The bigger win** is per-agent session prefix caching: if an agent fires 5 queries in a row, queries 2-5 share more than just the system prompt — they may share retrieval-cached candidates. But that's a different architecture (memoization across queries) and goes beyond this technique.

**(c) Quality risk: zero by construction** if the cached prefix is byte-identical to the live prompt prefix.

**(d) Implementation effort:**
- `llama-cpp-2 0.1.146` exposes `copy_kv_cache_seq`, `kv_cache_seq_add`, `clear_kv_cache_seq`, `llama_kv_cache_seq_keep` (verified at `context/kv_cache.rs:30-194`). The primitives are there.
- We don't currently keep a `LlamaContext` between queries (each `run_one_inference_qwen` call creates a fresh one at `qwen25.rs:246-248`). To cache, we'd need to: (i) hold a long-lived `LlamaContext` in the provider; (ii) on each query, clear sequence 0 except for the cached system-prompt tokens; (iii) append the new user prompt tokens and decode.
- **Refactor cost: ~2-3 days,** including a `KvCacheCheckpoint` abstraction that survives the spawn_blocking boundary.

**(e) License / privacy:** Unchanged.

**(f) Confidence: HIGH technique correctness, LOW absolute gain.** For ~2-3% wall-clock saving, the refactor cost is not worth it in isolation. **Becomes interesting if combined with speculative decoding** (which also needs a long-lived `LlamaContext`) — the architecture work amortizes.

#### 2.7b — Batch parallelism for concurrent reads

llama.cpp's continuous batching is designed for `llama-server` serving multiple users. **For a local single-user MCP server, queries from agents in a session are typically serial** (agent A asks, gets response, asks again). Concurrent parallel reads from multiple agents simultaneously sharing one model is a V1.0 multi-agent scenario.

**Verdict: defer to V1.0.** For V0.2 single-user beta, this is over-engineering.

---

### 2.8 Hardware paths

#### 2.8a — Intel OpenVINO / IPEX-LLM

OpenVINO 2026 supports Qwen-2.5-7B on Iris Xe and Intel NPU; IPEX-LLM provides a `llama.cpp`-compatible backend with Intel-optimized kernels.

**Realistic speedup vs llama.cpp Vulkan on Iris Xe:** **likely 1.5-2x faster** on prompt eval (Intel's first-party kernels beat generic Vulkan). Generation speed: similar (memory-bandwidth bound).

**Integration cost in our pure-Rust stack: HIGH.** OpenVINO's Rust bindings are immature; IPEX-LLM is a Python+C++ ecosystem. Either path adds a heavy native dependency.

**License:** OpenVINO is Apache 2.0, IPEX-LLM is Apache 2.0. **Permissive — no privacy/license blocker.**

**Verdict: Defer to V1.0.** Worth the engineering investment when we have paying customers funding the dev time. **Not for V0.2.**

#### 2.8b — MLX on Apple Silicon

MLX is Apple-only. **Important for macOS users.** llama.cpp with Metal feature already covers macOS competitively; MLX is ~1.5-3x faster on token generation per [GitHub issue #19366](https://github.com/ggml-org/llama.cpp/issues/19366) for some models.

But `llama-cpp-2 0.1.146` already has the `metal` Cargo feature wired (Cargo.toml:48). **For macOS, just enable `metal`.** No need to add MLX as a separate engine.

#### 2.8c — Apple Silicon directional ballpark for macOS users

- M1 / M1 Pro / M1 Max: 18-50 tok/s tg on Llama-3.1-8B Q4_K_M
- M2 Pro: ~28 tok/s tg
- M3 Pro / M4 Pro: 60-120 tok/s tg
- M-series memory bandwidth (200-400 GB/s) is **5-10x our Iris Xe's effective bandwidth from shared DDR5**.

**Speculative wall-clock for our 8K + 500-token workload on M2 Pro:** ~30-60s. On M3+: ~20-40s. **macOS users get massively better performance for free** because of memory bandwidth. The product strategy implication: Apple Silicon users are unlikely to hit our 120s ceiling; the i7-13620H is the worst-case target.

---

### 2.9 Other load-bearing levers

#### 2.9a — Build flag: tinyBLAS and AVX2 kernels

llama.cpp's tinyBLAS work (Justine Tunney, [justine.lol/matmul](https://justine.lol/matmul/)) delivers **2-5x faster prompt eval on f16** on modern Intel CPUs. Our Q4_K_M weights aren't f16, but the same matmul kernels apply to the f16 KV-cache path (which is still our default in t026 baseline).

**Check:** is `llama-cpp-sys-2` building with the latest llama.cpp commit that has these kernels, and is AVX2 detected at build time on our Windows CI? The `Cargo.lock` for `llama-cpp-sys-2` and the upstream sha would tell us. **Low effort to verify.**

#### 2.9b — OpenMP feature

`llama-cpp-2` defaults include `openmp` (Cargo.toml:39). Good — OpenMP delivers consistent threading. **No action.**

#### 2.9c — FlashAttention

llama.cpp's `-fa` flag (FlashAttention) is GPU-focused but **available on CPU** in recent builds. On CPU it sometimes helps, sometimes hurts (memory-bandwidth dependent). Worth a 1-knob sweep alongside t027a.

#### 2.9d — Run inference in `--release` with native target CPU

The spike uses `cargo run --release` which is correct. Ensure `RUSTFLAGS="-C target-cpu=native"` is set when building locally; on CI use `target-cpu=x86-64-v3` (AVX2 baseline) for distribution. **Worth verifying in the build.rs / .cargo/config.toml.**

---

## 3. Recommended next-spike sequence

**Assumption:** t027a (threads + KV q8_0 sweep) completes and we know our post-tune latency. Decide tree:

### Branch A — t027a lands at ≤120s
**Done.** File this document under "did not need." Move to T0.2.4.

### Branch B — t027a lands at 120-150s
**Cheap wins first.**
1. **t027b — Prompt-shape spike (1 day).** Top-12 retrieval + maybe top-14, replay 8 queries. Pass: 4/4 contradictions + 2/2 hard-negatives. Expected gain: 15-25%. If pass + ≤120s → ship.
2. **t027c — Quality replay of Qwen2.5-3B and Gemma-3-4B (1 day).** Same fixture, pass criterion same. If 3B passes → ship (expected ~45-60s after threads). If only 4B passes → expected ~60-80s, also ships.
3. If neither (1) nor (2) lands us at ≤120s **with** quality, fall through to Branch C.

### Branch C — t027a lands at 150-200s
Cheap wins likely insufficient on their own. **Combine:**
1. **t027b + t027c** (run them in parallel: top-12 retrieval AND smaller model). Replay 8 queries on each combination. If Qwen-3B + top-12 passes 4/4 + 2/2 → ship.
2. **t027d — Vulkan iGPU offload spike (3-5 days).** Build with `vulkan` feature, run 8-query replay, measure latency AND quality (driver-garbage check). Mandatory CPU-fallback path. If passes → ship behind a feature flag with auto-fallback.

### Branch D — t027a still above 200s
Combinatorial spike. **In order, escalating cost:**
1. t027b + t027c (Qwen-3B + top-12). If that doesn't land at ≤120s, the 3B model probably isn't enough either and we have a deeper problem.
2. t027d (Vulkan).
3. **t027e — Prefix caching architecture (3-5 days).** Long-lived `LlamaContext`, system-prompt KV-cache reuse. Modest gain alone; sets up t027f.
4. **t027f — Speculative decoding via llama-cpp-sys-2 FFI (2-3 weeks).** Build the Qwen-0.5B-draft + Qwen-7B-target pipeline. Highest theoretical upside but biggest investment.

### Do-not-do unless desperate
- Lower quantization than Q4_K_M (quality risk too high without a deeper quality study).
- Drop GBNF grammar (tail latency risk).
- Switch inference engines (candle / mistral.rs — high cost, uncertain gain).
- OpenVINO / SYCL integration (V1.0 work, not V0.2).

---

## 4. Sources

### Speculative decoding
- [llama.cpp speculative.md (master)](https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md) — canonical doc on llama.cpp's speculative-decoding implementation.
- [Speculative decoding potential for running big LLMs on consumer grade GPUs (discussion #10466)](https://github.com/ggml-org/llama.cpp/discussions/10466) — 0.5B draft + 14B target = 2.5x throughput; acceptance ~80% code / ~50% chat.
- [Research: Speculative Decoding for Low-Latency CPU Inference (issue #21453)](https://github.com/ggml-org/llama.cpp/issues/21453) — open research proposal, hypothesized 1.5-3x CPU.
- [thc1006/qwen3.6-speculative-decoding-rtx3090](https://github.com/thc1006/qwen3.6-speculative-decoding-rtx3090) — empirical "no net speedup" on Qwen3.6-MoE + RTX 3090 (cautionary).
- [Speculative Decoding: 20-50% Speed Boost (InsiderLLM)](https://insiderllm.com/guides/speculative-decoding-explained/) — Mac M1 11→16 tok/s with llama-160m + llama-7b.
- [LM Studio Speculative Decoding docs](https://lmstudio.ai/docs/app/advanced/speculative-decoding) — vocab-match requirement (locks out Phi-4-mini as Qwen draft).

### Quantization
- [An Empirical Study of Qwen3 Quantization (arxiv 2505.02214)](https://arxiv.org/abs/2505.02214) — Qwen3-14B drops 1% MMLU @ 4-bit GPTQ; Qwen3-0.6B drops 10%.
- [Demystifying LLM Quantization Suffixes (Paul Ilvez)](https://medium.com/@paul.ilvez/demystifying-llm-quantization-suffixes-what-q4-k-m-q8-0-and-q6-k-really-mean-0ec2770f17d3) — Q4_K_M is the community sweet spot.
- [Very slow IQ quant performance on Apple Silicon (discussion #5617)](https://github.com/ggml-org/llama.cpp/discussions/5617) — IQ4_XS ~3.8x slower than Q4_K_M on M-series Metal.
- [TurboQuant - Extreme KV Cache Quantization (discussion #20969)](https://github.com/ggml-org/llama.cpp/discussions/20969) — Q4_0 KV is lossless on Qwen3.5-9B at 4x compression.
- [KV Cache Quantization Benchmarks on DGX Spark](https://forums.developer.nvidia.com/t/kv-cache-quantization-benchmarks-on-dgx-spark-q4-0-vs-q8-0-vs-f16-llama-cpp-nemotron-30b-128k-context/365138) — Q8_0 KV 47% smaller, near-zero throughput cost at moderate ctx.

### Vulkan / Iris Xe
- [Performance of llama.cpp with Vulkan (discussion #10879)](https://github.com/ggml-org/llama.cpp/discussions/10879) — i7-1185G7 + Iris Xe pp/tg numbers across 2025 driver versions.
- [Misc. bug: Vulkan output is gibberish on Intel GPU (#17106)](https://github.com/ggml-org/llama.cpp/issues/17106) — mitigation: `GGML_VK_DISABLE_F16=1`.
- [Eval bug: Vulkan not working on Intel GPU (#17056)](https://github.com/ggml-org/llama.cpp/issues/17056) — Intel iGPU driver compatibility failure mode.
- [Vulkan backend crashes on Intel Arrow Lake iGPU (#19327)](https://github.com/ggml-org/llama.cpp/issues/19327) — model-size-dependent crashes.
- [Intel iGPU + Vulkan - crashes since b8148 (#20201)](https://github.com/ggml-org/llama.cpp/issues/20201) — driver-version regression.
- [Local AI Performance on Integrated Graphics with Vulkan Offload](https://zenvanriel.com/ai-engineer-blog/local-ai-integrated-graphics-vulkan-offload/) — Iris Xe Q4 7B numbers.
- [Intel IPEX-LLM (intel/ipex-llm)](https://github.com/intel/ipex-llm) — first-party Intel iGPU/NPU llama.cpp backend.

### Alternative engines
- [Hugging Face Candle](https://github.com/huggingface/candle) — Rust-native ML.
- [EricLBuehler/mistral.rs](https://github.com/EricLBuehler/mistral.rs) — Rust LLM inference, in-development speculative.
- [Advanced inference engine features (mistral.rs disc #245)](https://github.com/EricLBuehler/mistral.rs/discussions/245) — feature roadmap.
- [Performance benchmark of Mistral AI using llama.cpp](https://advanced-stack.com/resources/inference-performance-benchmark-of-mistral-ai-instruct-using-llama-cpp.html) — llama.cpp vs Candle vs MLX on Apple M1.

### Smaller models
- [Qwen/Qwen2.5-3B-Instruct (HF)](https://huggingface.co/Qwen/Qwen2.5-3B-Instruct) — model card.
- [Qwen2.5: A Party of Foundation Models](https://qwenlm.github.io/blog/qwen2.5-llm/) — 3B / 1.5B / 7B comparison narrative.
- [Gemma 3 4B vs Llama 3.2 3B Instruct (llm-stats)](https://llm-stats.com/models/compare/gemma-3-4b-it-vs-llama-3.2-3b-instruct) — IFEval, GSM8k, MATH side-by-side.
- [Phi-4 Technical Report (arxiv 2412.08905)](https://arxiv.org/html/2412.08905v1) — Phi-4 family capabilities and limits.

### Prompt shape / GBNF / caching
- [llama.cpp grammars README](https://github.com/ggml-org/llama.cpp/blob/master/grammars/README.md) — known slow patterns (repeated optionals, many-state automata).
- [Lost in Space: Optimizing Tokens for Grammar-Constrained Decoding (arxiv 2502.14969)](https://arxiv.org/html/2502.14969v1) — token-efficiency for GCD.
- [Tutorial: KV cache reuse with llama-server (discussion #13606)](https://github.com/ggml-org/llama.cpp/discussions/13606) — cache_n / prompt_n diagnostic.
- [Mastering Host-Memory Prompt Caching (discussion #20574)](https://github.com/ggml-org/llama.cpp/discussions/20574) — `--cache-reuse 256`, `--cache-ram`.
- [How to cache system prompt? (discussion #8947)](https://github.com/ggml-org/llama.cpp/discussions/8947) — `--system-prompt-file`.
- [Parallelization / Batching Explanation (discussion #4130)](https://github.com/ggml-org/llama.cpp/discussions/4130) — continuous batching primitives.
- [Optimal parameters for parallel inference (discussion #18308)](https://github.com/ggml-org/llama.cpp/discussions/18308) — multi-slot tuning.

### Hardware / matmul
- [LLaMA Now Goes Faster on CPUs (justine.lol)](https://justine.lol/matmul/) — tinyBLAS 2-5x speedup on Intel CPUs.
- [Improve cpu prompt eval speed (PR #6414)](https://github.com/ggml-org/llama.cpp/pull/6414) — upstream of tinyBLAS into llama.cpp.
- [Performance of llama.cpp on Apple Silicon M-series (discussion #4167)](https://github.com/ggml-org/llama.cpp/discussions/4167) — M1/M2/M3/M4 baseline numbers.
- [llama.cpp token gen vs MLX on Apple Silicon (#19366)](https://github.com/ggml-org/llama.cpp/issues/19366) — MLX ~3x faster on token-gen.
- [Intel OpenVINO 2026 release notes (Phoronix)](https://www.phoronix.com/news/Intel-OpenVINO-2026.0-Released) — Qwen-2.5-7B + NPU support.
- [Intel: Run LLMs on Intel GPUs Using llama.cpp](https://www.intel.com/content/www/us/en/developer/articles/technical/run-llms-on-gpus-using-llama-cpp.html) — first-party Intel iGPU llama.cpp story.

### RAG / retrieval
- [RAG Recall vs Precision: A Practical Diagnostic (OptyxStack)](https://optyxstack.com/rag-reliability/rag-recall-vs-precision-diagnostic) — top-K tradeoff intuition.
- [Metrics for Evaluation of Retrieval in RAG (Deconvolute Labs)](https://deconvoluteai.com/blog/rag/metrics-retrieval) — recall@k vs precision@k framework.

### llama-cpp-2 crate (source-verified at 0.1.146)
- File: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/llama-cpp-2-0.1.146/src/sampling.rs` — sampler API enumeration (no speculative helper).
- File: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/llama-cpp-2-0.1.146/src/context/kv_cache.rs` — KV-cache seq primitives exposed.
- File: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/llama-cpp-2-0.1.146/Cargo.toml` — `vulkan`, `metal`, `cuda`, `rocm`, `openmp` Cargo features confirmed.

---

## Appendix — quick-reference latency arithmetic

**t026 baseline decomposition (speculative, no profiling):**
- 8K-token prompt eval at 4 threads on Q4_K_M ≈ 60-100s
- 500-token generation at 4 threads ≈ 100-150s
- Other (model load amortized, retrieval, JSON parse) ≈ 5-10s
- Sum ≈ 165-260s (consistent with measured 156-224s range)

**Wall-clock targets and what gets us there:**
| Path | Prompt eval | Generation | Total | Confidence |
|---|---|---|---|---|
| Baseline (t026) | 80s | 100s | **180s** | measured |
| Threads → 10 (t027a expected) | 35s | 45s | **80s** | medium |
| Threads + KV q8_0 | 32s | 40s | **72s** | medium |
| Threads + top-12 retrieval | 22s | 45s | **67s** | medium-low |
| Threads + Qwen-3B | 15s | 20s | **35s** | medium (latency); LOW (quality) |
| Threads + Vulkan | 25s | 50s | **75s** | low-medium |
| Threads + speculative (2x) | 35s | 22s | **57s** | low (build cost) |

The single biggest unknown is **how much t027a alone delivers**. Everything else is a multiplier on whatever's left.
