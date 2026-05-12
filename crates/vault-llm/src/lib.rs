//! `vault-llm` — local LLM inference for Memory Vault.
//!
//! V0.2 default model: **Phi-4-mini-instruct Q4_K_M GGUF** (Microsoft, MIT license,
//! ~2.49 GB on disk). Inference via [`llama-cpp-2`](https://crates.io/crates/llama-cpp-2)
//! — a safe Rust wrapper over llama.cpp. The underlying `llama-cpp-sys-2` is
//! unsafe C++ FFI but is fully walled off behind the safe wrapper API, so
//! `#![forbid(unsafe_code)]` cleanly applies in this crate (BRD §2.7).
//!
//! Structured JSON output via GBNF grammar (llama.cpp native, no third-party
//! constrained-decoding crate). Designed for T0.2.3 consolidator merge-decisions
//! as the only V0.2 consumer; the [`LlmProvider`] trait (lands in commit 2 of
//! T0.2.1) stays minimal and model-agnostic so a Qwen3-4B-Instruct swap (or any
//! future provider) is a single config-flag change.
//!
//! See `Agent_Build_Specification.txt` §6.2 T0.2.1 for the canonical task spec,
//! and HANDOFF.md ADR-042 / ADR-043 / ADR-044 (drafted at T0.2.1 Phase 5) for the
//! locked decision record.
//!
//! ## Crate state (commit 1, T0.2.1 Phase 1 spike-2)
//!
//! Scaffold + workspace dep wiring only. The `examples/phi4_load_and_json_spike`
//! binary carries the runtime-confirmation evidence per
//! `feedback_runtime_confirmation_after_web_spike.md`. Production code
//! (`LlmProvider` trait, `Phi4MiniProvider`, `MockLlmProvider`, model downloader)
//! lands in commit 2 (Phase 2) of T0.2.1.

#![forbid(unsafe_code)]
