//! Error types shared across the Memory Vault workspace.
//!
//! Every fallible operation in `vault-*` crates returns [`VaultResult<T>`].
//! The error variants are organised by failure category, not by source crate,
//! so callers can pattern-match on intent rather than implementation detail.
//!
//! See `Agent Build Specification.txt` §5.1 for the canonical error catalogue.

use thiserror::Error;

/// The single error type used across the entire Memory Vault workspace.
///
/// Variants describe failure *categories*. Each variant carries a `String`
/// message rather than nesting source-crate error types — this keeps
/// downstream crates from depending on, say, `rusqlite::Error` just to
/// match on a storage failure.
///
/// New variants must be added when a genuinely new failure category emerges.
/// Avoid "OtherError(String)" or similar grab-bags — they make pattern
/// matching meaningless.
#[derive(Error, Debug)]
pub enum VaultError {
    /// Persistent storage failure (SQLite, LanceDB, DuckDB, encrypted blobs).
    #[error("storage error: {0}")]
    Storage(String),

    /// Embedding generation failed (model loading, tokenisation, inference).
    #[error("embedding error: {0}")]
    Embedding(String),

    /// Local LLM inference failed (model loading, prompt execution, structured output).
    #[error("llm error: {0}")]
    Llm(String),

    /// Retrieval pipeline failure (query classification, strategy execution, reranking).
    #[error("retrieval error: {0}")]
    Retrieval(String),

    /// Sleep-cycle consolidation failed (clustering, merging, decay, checkpointing).
    #[error("consolidation error: {0}")]
    Consolidation(String),

    /// MCP protocol or transport failure (adapter, server, tool dispatch).
    #[error("mcp error: {0}")]
    Mcp(String),

    /// Cross-device sync failure (CRDT, encryption, cloud API).
    #[error("sync error: {0}")]
    Sync(String),

    /// Third-party connector failure (OAuth, fetch, extraction).
    #[error("connector error: {0}")]
    Connector(String),

    /// Authentication failure (Clerk session, capability token, device pairing).
    #[error("authentication error: {0}")]
    Auth(String),

    /// Mandatory access control denied the operation. The boundary check
    /// failed — see BRD §11.4.3. **Always returned generically** (no info leak).
    #[error("access denied: {0}")]
    AccessDenied(String),

    /// Input failed validation at a public API boundary (length, charset,
    /// schema, range). See BRD §11.7.1.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// Embedding vector dimensionality did not match the configured store
    /// dimension. Carried as a structured variant (rather than as a string
    /// inside [`Self::Storage`]) because the cascading retry queue's
    /// `is_permanent` classifier in `vault-storage` matches on it
    /// exhaustively to dead-letter on attempt 1 — a dimension mismatch is
    /// always a contract / config error, never transient. See T0.1.6_PLAN
    /// Q2 and ADR-009 amendment.
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch {
        /// Dimension the store was configured for.
        expected: usize,
        /// Dimension actually presented by the caller.
        actual: usize,
    },

    /// The requested resource (memory, entity, boundary) does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// Cryptographic operation failed (key derivation, encryption, decryption,
    /// signature verification). Authentication-tag failures land here.
    #[error("crypto error: {0}")]
    Crypto(String),

    /// Configuration is missing or malformed (config file, env vars, model paths).
    #[error("config error: {0}")]
    Config(String),

    /// Underlying I/O failure (filesystem, network) that doesn't fit a more
    /// specific category. Wraps [`std::io::Error`] for context.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization or deserialization failed (JSON, CRDT payload).
    #[error("serde error: {0}")]
    Serde(String),
}

/// Standard result alias used throughout the workspace.
pub type VaultResult<T> = Result<T, VaultError>;

impl From<serde_json::Error> for VaultError {
    fn from(value: serde_json::Error) -> Self {
        Self::Serde(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_messages_are_prefixed_by_category() {
        assert!(VaultError::Storage("disk full".into())
            .to_string()
            .starts_with("storage error:"));
        assert!(
            VaultError::AccessDenied("boundary 'work' not authorized".into())
                .to_string()
                .starts_with("access denied:")
        );
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::other("simulated");
        let vault_err: VaultError = io_err.into();
        assert!(matches!(vault_err, VaultError::Io(_)));
    }

    #[test]
    fn serde_error_converts_via_from() {
        let serde_err = serde_json::from_str::<serde_json::Value>("{invalid").unwrap_err();
        let vault_err: VaultError = serde_err.into();
        assert!(matches!(vault_err, VaultError::Serde(_)));
    }

    #[test]
    fn dimension_mismatch_is_structured_and_displays_both_dims() {
        let err = VaultError::DimensionMismatch {
            expected: 384,
            actual: 256,
        };
        let s = err.to_string();
        assert!(s.contains("384"), "display should mention expected: {s}");
        assert!(s.contains("256"), "display should mention actual: {s}");
        assert!(matches!(
            err,
            VaultError::DimensionMismatch {
                expected: 384,
                actual: 256
            }
        ));
    }
}
