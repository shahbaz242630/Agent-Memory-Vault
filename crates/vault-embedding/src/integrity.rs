//! Model + tokenizer integrity verification (ADR-020 / T0.1.7_PLAN.md Q5).
//!
//! Both the bge-small ONNX model and its tokenizer config are SHA-256-verified
//! against compiled-in expected hashes at provider construction. A mismatch on
//! either is fatal at startup — see [`vault_core::VaultError::ModelIntegrityFailed`].
//!
//! **Why hash both, not just the model.** A mismatched tokenizer paired with a
//! matched model produces vectors of correct shape and L2 norm but with shifted
//! semantics — the most insidious failure class. Both files are part of the
//! signed-binary chain (BRD §11.7.5); both are verified.
//!
//! **Why hash file bytes, not parsed content.** Defends against any
//! whitespace / encoding normalisation on disk (e.g., CRLF conversion on
//! Windows). The bytes-on-disk are what `tokenizers` and `ort` actually
//! consume.

use sha2::{Digest, Sha256};
use std::path::Path;
use vault_core::{VaultError, VaultResult};

/// Canonical SHA-256 of `bge-small-en-v1.5/onnx/model.onnx` as published by
/// BAAI on Hugging Face (commit `5c38ec7`, MIT license, 133 MB FP32).
///
/// Captured via Spike 3 (T0.1.7_PLAN.md amendment block — 2026-04-30) from
/// the file metadata at <https://huggingface.co/BAAI/bge-small-en-v1.5/blob/main/onnx/model.onnx>.
pub const BGE_SMALL_EN_V1_5_MODEL_SHA256: &str =
    "828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35";

/// Canonical SHA-256 of `bge-small-en-v1.5/tokenizer.json`.
///
/// Captured at first fixture download via `scripts/setup-dev-env.{sh,ps1}`
/// against the upstream BAAI tokenizer.json (commit `9b09f79`, MIT license).
/// Hugging Face's metadata UI doesn't surface SHA-256 for sub-LFS-threshold
/// files; this is the bytes-on-disk hash of the file we ship.
pub const BGE_SMALL_EN_V1_5_TOKENIZER_SHA256: &str =
    "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66";

/// Canonical SHA-256 of the Qwen3-Reranker-0.6B seq-cls ONNX model
/// (`qwen3-reranker-0.6b-seq-cls/model.onnx`, f16, ~1.19 GB).
///
/// Source: `shawnw3i/Qwen3-Reranker-0.6B-seq-cls-ONNX` (Apache-2.0), a
/// SequenceClassification conversion of `Qwen/Qwen3-Reranker-0.6B`
/// (base_model `tomaarsen/Qwen3-Reranker-0.6B-seq-cls`). Captured at first
/// fixture download 2026-05-29 — the model is community-converted
/// (per spike-playbook: provenance noted, integrity-pinned to the bytes we
/// validated). Selected via the `reranker_spike` model-fit evaluation.
pub const QWEN3_RERANKER_MODEL_SHA256: &str =
    "bd3ee3865c63d1bec518d0785ff551f7c0b78af0d4bf4855653a0c765a2a1292";

/// Canonical SHA-256 of the Qwen3-Reranker-0.6B seq-cls tokenizer
/// (`qwen3-reranker-0.6b-seq-cls/tokenizer.json`). Captured 2026-05-29.
pub const QWEN3_RERANKER_TOKENIZER_SHA256: &str =
    "aeb13307a71acd8fe81861d94ad54ab689df773318809eed3cbe794b4492dae4";

/// Verify a file's bytes against an expected SHA-256 hex string.
///
/// `file_label` is the logical name surfaced in the error (`"model"`,
/// `"tokenizer"`) so a fatal-error dialog can be specific. The on-disk
/// path is included via tracing for operator diagnosis but NOT in the
/// error message (paths can leak local layout in user-facing surfaces).
///
/// # Errors
///
/// - [`VaultError::Io`] on read failure.
/// - [`VaultError::ModelIntegrityFailed`] on hash mismatch — fatal at startup.
#[tracing::instrument(level = "debug", skip(path, expected), fields(file_label = %file_label))]
pub fn verify_file_sha256(path: &Path, expected: &str, file_label: &str) -> VaultResult<()> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = format!("{:x}", hasher.finalize());

    if actual == expected {
        tracing::debug!(file_label, "integrity verified");
        Ok(())
    } else {
        Err(VaultError::ModelIntegrityFailed {
            file: file_label.to_string(),
            expected: expected.to_string(),
            actual,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// SHA-256 of an empty byte string — a known reference value.
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn verify_empty_file_against_known_empty_sha256_passes() {
        let f = NamedTempFile::new().expect("temp file");
        verify_file_sha256(f.path(), EMPTY_SHA256, "model")
            .expect("empty file must hash to known empty SHA-256");
    }

    #[test]
    fn verify_returns_model_integrity_failed_on_mismatch() {
        let mut f = NamedTempFile::new().expect("temp file");
        f.write_all(b"not what was expected").expect("write");
        let bogus = "0000000000000000000000000000000000000000000000000000000000000000";

        let err =
            verify_file_sha256(f.path(), bogus, "model").expect_err("hash mismatch must error");

        match err {
            VaultError::ModelIntegrityFailed {
                file,
                expected,
                actual,
            } => {
                assert_eq!(file, "model");
                assert_eq!(expected, bogus);
                assert_ne!(actual, bogus, "actual must reflect the real hash");
                assert_eq!(actual.len(), 64, "SHA-256 hex must be 64 chars");
            }
            other => panic!("expected ModelIntegrityFailed, got {other}"),
        }
    }

    #[test]
    fn verify_returns_io_error_when_file_missing() {
        let path = Path::new("definitely_does_not_exist_42.bin");
        let err =
            verify_file_sha256(path, EMPTY_SHA256, "model").expect_err("missing file must error");
        match err {
            VaultError::Io(_) => {}
            other => panic!("expected Io, got {other}"),
        }
    }

    #[test]
    fn file_label_propagates_to_error_for_dialog_specificity() {
        let mut f = NamedTempFile::new().expect("temp file");
        f.write_all(b"bytes").expect("write");
        let bogus = "0000000000000000000000000000000000000000000000000000000000000000";

        let err =
            verify_file_sha256(f.path(), bogus, "tokenizer").expect_err("hash mismatch must error");
        match err {
            VaultError::ModelIntegrityFailed { file, .. } => {
                assert_eq!(
                    file, "tokenizer",
                    "label must propagate so dialog can name it"
                );
            }
            other => panic!("expected ModelIntegrityFailed, got {other}"),
        }
    }
}
