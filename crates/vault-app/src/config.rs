//! [`AppConfig`] — migration target for [`Application::new`]'s seven
//! Phase 1 inline parameters per the Phase 1b migration-anchor doc-
//! comment + Phase 2 plan-paragraph pre-declaration.
//!
//! ## Migration ≠ rename (locked discipline)
//!
//! **The seven [`AppConfig`] fields preserve the Phase 1 inline param
//! names verbatim**: `metadata_path`, `vector_dir`, `graph_path`, `key`,
//! `model_path`, `tokenizer_path`, `ort_lib_path`. If any name in this
//! struct ever changes, the rename-prohibition discipline has been
//! violated. The pin test
//! `appconfig_field_names_are_locked_to_phase_1_inline_param_names`
//! enforces this at CI level — a "helpful" rename in some future
//! commit trips CI immediately rather than landing silently.
//!
//! Each field below cites [`Application::new`] as its migration anchor
//! per the audit-trail honesty discipline (provenance preserved across
//! the migration boundary).
//!
//! ## Pass-by-reference convention
//!
//! [`Application::new`] takes `&AppConfig`, not `AppConfig` by value.
//! Reasoning: AppConfig is intended to be a long-lived first-class
//! value the caller retains for downstream use — `start_with_mcp`'s
//! `authorized_boundaries`, future config-file reload paths,
//! observability metadata, crash-reporter context, etc. By-reference
//! keeps the config available to those consumers; by-value would
//! force "construct-config-just-before-construct-app" anti-patterns
//! that hide the config from later lifecycle methods.
//!
//! Where a specific field needs ownership inside `Application::new`'s
//! body (notably [`SqlCipherKey`] for `MetadataStore::open` and
//! `StorageBackend::open`), the body clones explicitly:
//! `let key = config.key.clone();`. The field-by-field clone cost is
//! cheap; restructuring to take config by value just to avoid the
//! clones would sacrifice the long-lived-config property.
//!
//! ## Debug impl is manual (not derived)
//!
//! [`SqlCipherKey`] deliberately does NOT implement [`Debug`] per its
//! own module docs (`vault-storage/src/key.rs` lines 9-10): *"The key
//! can never be accidentally logged: there is no Debug / Display impl."*
//! Therefore `#[derive(Debug)]` on [`AppConfig`] would fail to compile.
//! The manual `Debug` impl below redacts the `key` field as
//! `<redacted>` while printing the six path fields verbatim — preserving
//! debug utility for paths while honouring the upstream secrets
//! discipline (BRD §11 secrets-in-logs concern).

use std::fmt;
use std::path::PathBuf;

use vault_storage::SqlCipherKey;

/// Composition-root configuration for [`crate::Application`].
///
/// Constructed by the caller (V0.1: integration tests; V0.2+: production
/// `main()` after parsing CLI flags / config file) and passed by
/// reference to [`crate::Application::new`].
///
/// See module-level docs for migration discipline (rename-prohibition),
/// pass-by-reference convention, and Debug-redaction reasoning.
pub struct AppConfig {
    /// SQLCipher database file path.
    ///
    /// **Migration anchor:** `Application::new`'s `metadata_path: &Path`
    /// parameter from T0.1.10 Phase 1.
    pub metadata_path: PathBuf,

    /// LanceDB dataset directory path.
    ///
    /// **Migration anchor:** `Application::new`'s `vector_dir: &Path`
    /// parameter from T0.1.10 Phase 1.
    pub vector_dir: PathBuf,

    /// DuckDB graph file path.
    ///
    /// **Migration anchor:** `Application::new`'s `graph_path: &Path`
    /// parameter from T0.1.10 Phase 1.
    pub graph_path: PathBuf,

    /// SQLCipher passphrase wrapper. Zeroize-on-drop per
    /// [`SqlCipherKey`]'s contract.
    ///
    /// **Migration anchor:** `Application::new`'s `key: SqlCipherKey`
    /// parameter from T0.1.10 Phase 1.
    pub key: SqlCipherKey,

    /// `bge-small-en-v1.5/model.onnx` path. Verified against pinned
    /// SHA-256 at `Application::new` time per ADR-019/020 — startup-
    /// fatal on mismatch.
    ///
    /// **Migration anchor:** `Application::new`'s `model_path: &Path`
    /// parameter from T0.1.10 Phase 1.
    pub model_path: PathBuf,

    /// `bge-small-en-v1.5/tokenizer.json` path. Verified against pinned
    /// SHA-256 alongside the model.
    ///
    /// **Migration anchor:** `Application::new`'s `tokenizer_path: &Path`
    /// parameter from T0.1.10 Phase 1.
    pub tokenizer_path: PathBuf,

    /// `libonnxruntime.{dll,dylib,so}` path for the host platform per
    /// ADR-019 `load-dynamic` strategy.
    ///
    /// **Migration anchor:** `Application::new`'s `ort_lib_path: &Path`
    /// parameter from T0.1.10 Phase 1.
    pub ort_lib_path: PathBuf,
}

impl fmt::Debug for AppConfig {
    /// Manual `Debug` impl that redacts the [`SqlCipherKey`] field
    /// while printing paths verbatim. Required because
    /// [`SqlCipherKey`] does not implement `Debug` (deliberate upstream
    /// secrets discipline at `vault-storage/src/key.rs`).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field("metadata_path", &self.metadata_path)
            .field("vector_dir", &self.vector_dir)
            .field("graph_path", &self.graph_path)
            .field("key", &"<redacted>")
            .field("model_path", &self.model_path)
            .field("tokenizer_path", &self.tokenizer_path)
            .field("ort_lib_path", &self.ort_lib_path)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Field-name preservation pin** — locks the rename-prohibition
    /// discipline at CI level. If any field name in [`AppConfig`] ever
    /// changes, this struct-init pattern fails to compile, tripping
    /// CI immediately rather than letting a "helpful" rename land
    /// silently.
    ///
    /// Pre-declared at T0.1.10 Phase 2 plan-paragraph review:
    /// *"AppConfig should preserve the seven Phase 1 inline param
    /// names verbatim (...) — clean-slate redesign of names belongs
    /// to a future refactor phase, not the migration phase. Migration
    /// ≠ rename."*
    #[test]
    fn appconfig_field_names_are_locked_to_phase_1_inline_param_names() {
        // Construct an AppConfig naming all seven fields explicitly.
        // The variable is unused; what matters is that the struct
        // literal compiles — which proves the field names match the
        // expected set verbatim.
        let _config = AppConfig {
            metadata_path: PathBuf::new(),
            vector_dir: PathBuf::new(),
            graph_path: PathBuf::new(),
            key: SqlCipherKey::new(""),
            model_path: PathBuf::new(),
            tokenizer_path: PathBuf::new(),
            ort_lib_path: PathBuf::new(),
        };
    }

    /// Manual `Debug` impl must redact the `key` field. Regression
    /// check that the redaction text is present and the raw key is
    /// not — defensive pin against future "helpful" Debug derive
    /// reintroduction or redaction-text drift.
    #[test]
    fn appconfig_debug_redacts_sqlcipher_key() {
        let config = AppConfig {
            metadata_path: PathBuf::from("/tmp/vault.db"),
            vector_dir: PathBuf::from("/tmp/lance"),
            graph_path: PathBuf::from("/tmp/graph.duckdb"),
            key: SqlCipherKey::new("super-secret-passphrase-xyz"),
            model_path: PathBuf::from("/tmp/model.onnx"),
            tokenizer_path: PathBuf::from("/tmp/tokenizer.json"),
            ort_lib_path: PathBuf::from("/tmp/libonnxruntime.so"),
        };
        let dbg_str = format!("{config:?}");
        assert!(
            dbg_str.contains("<redacted>"),
            "AppConfig Debug output MUST contain the <redacted> marker for the key field; got: {dbg_str}"
        );
        assert!(
            !dbg_str.contains("super-secret-passphrase-xyz"),
            "AppConfig Debug output MUST NOT leak the SqlCipherKey contents; got: {dbg_str}"
        );
    }
}
