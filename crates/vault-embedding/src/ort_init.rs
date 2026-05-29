//! Process-global ONNX Runtime initialisation gate, shared by every
//! provider in this crate (`BgeSmallProvider`, `Qwen3RerankerProvider`).
//!
//! `ort::init_from(dylib).commit()` sets the ONE process-global ORT
//! environment and may run at most once per process. Both the embedder and
//! the reranker load `ort` under `load-dynamic` from the same bundled dylib,
//! so they MUST share a single init — a second `commit()` would conflict.
//! The first caller wins; subsequent callers (including concurrent ones from
//! parallel cargo test threads) observe the same cached outcome. The
//! `Result<(), String>` payload preserves the first-call result — error or
//! success — so a degraded process surfaces the original failure on every
//! later open attempt rather than silently retrying.

use std::path::Path;
use std::sync::OnceLock;
use vault_core::{VaultError, VaultResult};

static ORT_INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// Initialise ORT under `load-dynamic` with the given dylib path, at most
/// once per process. Idempotent and concurrency-safe; all callers observe
/// the first-call outcome.
///
/// # Errors
///
/// [`VaultError::Embedding`] if the first-call `ort::init_from(...).commit()`
/// failed (the cached failure is returned on every subsequent call).
pub(crate) fn ensure_ort_initialised(dylib_path: &Path) -> VaultResult<()> {
    let result = ORT_INIT.get_or_init(|| {
        let path_str = dylib_path
            .to_str()
            .ok_or_else(|| "ort dylib path is not valid UTF-8".to_string())?;
        ort::init_from(path_str)
            .commit()
            .map(|_| ())
            .map_err(|e| format!("ort init_from: {e}"))
    });

    match result {
        Ok(()) => Ok(()),
        Err(s) => Err(VaultError::Embedding(format!("ort init: {s}"))),
    }
}
