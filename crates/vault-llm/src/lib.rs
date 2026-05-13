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
//! ## Public API surface
//!
//! - [`LlmProvider`] — capability trait for any local LLM serving T0.2.3
//!   consolidator merge-decisions (or future structured-JSON workloads).
//! - [`CompletionParams`] — per-call inference parameters (seed, temperature,
//!   max_tokens, top_p).
//! - [`Phi4MiniProvider`] — V0.2 concrete implementation backed by
//!   `llama-cpp-2` (lands at T0.2.1 Phase 3, commit 2).
//! - [`VaultLlmError`] / [`VaultLlmResult`] — crate-level error surface; converts
//!   cleanly into `vault_core::VaultError` at the workspace boundary.
//! - [`MockLlmProvider`] — deterministic mock for trait-conformance tests +
//!   downstream consumer tests. Behind `#[cfg(any(test, feature = "test-utils"))]`
//!   so it's available to downstream crates' `[dev-dependencies]` via
//!   `vault-llm = { ..., features = ["test-utils"] }`.

#![forbid(unsafe_code)]

pub mod error;
pub mod model_loader;
pub mod phi4_mini;
pub mod provider;

pub use error::{VaultLlmError, VaultLlmResult};
pub use phi4_mini::{Phi4MiniConfig, Phi4MiniProvider};
pub use provider::{CompletionParams, LlmProvider};

#[cfg(any(test, feature = "test-utils"))]
pub use provider::MockLlmProvider;
