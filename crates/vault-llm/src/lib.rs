//! `vault-llm` — local LLM inference using Phi-4-mini via llama.cpp bindings.
//!
//! See `Agent Build Specification.txt` §5.4 for the public API specification.
//! Real implementation lands in T0.2.1 (V0.2). FFI to llama.cpp will require an
//! isolated `unsafe` module per BRD §11.7.4 — the crate-level `forbid(unsafe_code)`
//! here will be relaxed to `deny` then.

#![forbid(unsafe_code)]
