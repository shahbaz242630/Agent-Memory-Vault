//! `vault-embedding` — embedding generation using bge-small-en-v1.5 via ONNX Runtime.
//!
//! See `Agent Build Specification.txt` §5.3 for the public API specification.
//! Real implementation lands in T0.1.7. FFI to `ort` will require an isolated
//! `unsafe` module per BRD §11.7.4 — the crate-level `forbid(unsafe_code)` here
//! will be relaxed to `deny` then.

#![forbid(unsafe_code)]
