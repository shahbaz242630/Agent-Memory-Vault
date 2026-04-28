//! `vault-tauri` — Tauri shell. Bridges the frontend (UI) to `vault-app` via
//! Tauri commands. Owns first-run setup (model download + verification) and
//! installer concerns.
//!
//! See `Agent Build Specification.txt` §5.11 for the public API specification.
//! Real implementation lands in T0.1.11; this skeleton currently exposes nothing.
//!
//! Note: per BRD §5.11 this crate becomes a binary in T0.1.11 (`src/main.rs`
//! Tauri entry point). Today it is a library skeleton so the workspace builds.

#![forbid(unsafe_code)]
