//! `vault-embedding` — embedding generation using bge-small-en-v1.5 via
//! ONNX Runtime. See `Agent Build Specification.txt` §5.3 + `T0.1.7_PLAN.md`
//! for the full design context.
//!
//! Public surface:
//! - [`EmbeddingProvider`] — the abstract trait consumed by vault-storage's
//!   write-path and vault-retrieval's search-path.
//! - [`BgeSmallProvider`] — production implementation backed by `ort` 2.x +
//!   `tokenizers`.
//! - [`integrity`] — SHA-256 verification of model + tokenizer files at
//!   provider construction (ADR-020).
//!
//! Phase 1 ships the trait, the constructor signature, the integrity
//! verifier, and the failing tests. Phase 2 fleshes out model loading;
//! Phase 3 wires inference. See `T0.1.7_PLAN.md` for the rhythm.
//!
//! `forbid(unsafe_code)` will be relaxed to `deny` in Phase 2 when the
//! `ort` FFI module lands; only that module gets `#[allow(unsafe_code)]`
//! per ADR-002.

#![forbid(unsafe_code)]

pub mod bge_small;
pub mod integrity;
pub mod provider;

pub use bge_small::BgeSmallProvider;
pub use integrity::{
    verify_file_sha256, BGE_SMALL_EN_V1_5_MODEL_SHA256, BGE_SMALL_EN_V1_5_TOKENIZER_SHA256,
};
pub use provider::{EmbeddingProvider, EMBEDDING_DIM};
