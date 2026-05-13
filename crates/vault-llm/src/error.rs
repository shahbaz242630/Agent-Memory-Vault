//! Error types for `vault-llm`.
//!
//! Per BRD §2.4 each crate carries its own `thiserror`-based error enum and
//! converts to [`VaultError`] at the workspace boundary. `VaultLlmError`
//! captures the LLM-specific failure categories — model load / inference /
//! grammar / download / integrity-check — and provides a `From` impl that
//! maps cleanly into [`VaultError`]'s catalogue so vault-app + vault-tauri +
//! vault-consolidator callers see a single workspace-wide error surface.

use thiserror::Error;
use vault_core::VaultError;

/// LLM-specific failure categories.
#[derive(Debug, Error)]
pub enum VaultLlmError {
    /// The underlying llama.cpp backend reported a model-loading failure
    /// (missing GGUF file, corrupted weights, incompatible architecture).
    #[error("model load failed: {0}")]
    ModelLoadFailed(String),

    /// Inference invocation through `llama-cpp-2` failed (context creation,
    /// batch.add, decode, sampler.sample, token_to_piece_bytes, etc).
    #[error("inference failed: {0}")]
    InferenceFailed(String),

    /// JSON schema → GBNF grammar compilation failed, or the constructed
    /// `LlamaSampler::grammar` returned a `GrammarError` (the llama.cpp#18173
    /// adversarial probe class). Spike-2's T0.2.3-shape schema does NOT trip
    /// this; tracked for surface integrity of future schema additions.
    #[error("grammar compilation failed: {0}")]
    GrammarCompilation(String),

    /// Model file download from the pinned HuggingFace mirror failed
    /// (network, HTTP non-2xx, mid-stream interruption, atomic-rename).
    /// Per ADR-043 (drafted at Phase 5), this is fail-closed — no
    /// half-downloaded files left behind.
    #[error("model download failed: {0}")]
    DownloadFailed(String),

    /// Downloaded model bytes do not match the pinned SHA-256. Fail-closed
    /// per ADR-043; do not retry; surface to user via Tauri dialog.
    /// `file` names the artefact (e.g. "phi-4-mini-instruct-Q4_K_M.gguf"),
    /// `expected` and `actual` are hex-encoded SHA-256 strings.
    #[error("model integrity check failed for {file}: expected {expected}, got {actual}")]
    IntegrityCheckFailed {
        file: String,
        expected: String,
        actual: String,
    },

    /// Caller-supplied prompt or schema failed validation at the trait API
    /// boundary (empty prompt, schema not valid JSON, etc).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Underlying I/O failure during model load, download, or cache I/O.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Standard result alias used throughout `vault-llm`.
pub type VaultLlmResult<T> = Result<T, VaultLlmError>;

impl From<VaultLlmError> for VaultError {
    fn from(value: VaultLlmError) -> Self {
        match value {
            // The four llm-inference categories collapse into the workspace's
            // single `Llm(String)` variant — vault-core deliberately keeps the
            // category-level granularity coarse (per its module doc), and the
            // wrapped message already carries the specific category prefix
            // via thiserror's display.
            VaultLlmError::ModelLoadFailed(_)
            | VaultLlmError::InferenceFailed(_)
            | VaultLlmError::GrammarCompilation(_)
            | VaultLlmError::DownloadFailed(_) => VaultError::Llm(value.to_string()),

            // Integrity check maps onto vault-core's pre-existing structured
            // variant — the dialog flow in vault-tauri matches on this variant
            // exhaustively to surface a SHA-mismatch-specific message.
            VaultLlmError::IntegrityCheckFailed {
                file,
                expected,
                actual,
            } => VaultError::ModelIntegrityFailed {
                file,
                expected,
                actual,
            },

            VaultLlmError::InvalidInput(msg) => VaultError::InvalidInput(msg),
            VaultLlmError::Io(err) => VaultError::Io(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_prefixed_by_category() {
        assert!(VaultLlmError::ModelLoadFailed("file not found".into())
            .to_string()
            .starts_with("model load failed:"));
        assert!(VaultLlmError::InferenceFailed("ctx.decode".into())
            .to_string()
            .starts_with("inference failed:"));
        assert!(VaultLlmError::GrammarCompilation("empty stack".into())
            .to_string()
            .starts_with("grammar compilation failed:"));
    }

    #[test]
    fn llm_categories_collapse_to_vault_llm_variant() {
        let load: VaultError = VaultLlmError::ModelLoadFailed("x".into()).into();
        assert!(matches!(load, VaultError::Llm(_)));
        let infer: VaultError = VaultLlmError::InferenceFailed("y".into()).into();
        assert!(matches!(infer, VaultError::Llm(_)));
        let grammar: VaultError = VaultLlmError::GrammarCompilation("z".into()).into();
        assert!(matches!(grammar, VaultError::Llm(_)));
        let download: VaultError = VaultLlmError::DownloadFailed("w".into()).into();
        assert!(matches!(download, VaultError::Llm(_)));
    }

    #[test]
    fn integrity_check_failure_maps_to_structured_vault_variant() {
        let err = VaultLlmError::IntegrityCheckFailed {
            file: "phi-4.gguf".into(),
            expected: "abc".into(),
            actual: "def".into(),
        };
        let converted: VaultError = err.into();
        match converted {
            VaultError::ModelIntegrityFailed {
                file,
                expected,
                actual,
            } => {
                assert_eq!(file, "phi-4.gguf");
                assert_eq!(expected, "abc");
                assert_eq!(actual, "def");
            }
            other => panic!("expected ModelIntegrityFailed, got {other:?}"),
        }
    }

    #[test]
    fn io_error_round_trips_through_both_layers() {
        let io_err = std::io::Error::other("simulated");
        let llm_err: VaultLlmError = io_err.into();
        assert!(matches!(llm_err, VaultLlmError::Io(_)));
        let vault_err: VaultError = llm_err.into();
        assert!(matches!(vault_err, VaultError::Io(_)));
    }
}
