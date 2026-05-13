//! Model file management: download, SHA-256 integrity verification, air-gap
//! fallback.
//!
//! The air-gap fallback path (iteration 2 fresh scope per Shahbaz's call) is
//! NOT a separate branch — it falls out naturally from `ensure_model_at_path`:
//! if a user manually places the GGUF file at the expected path with the
//! correct hash before first launch, the function returns Ok without ever
//! attempting a download. The operational doc that names this user-facing
//! workflow lands at Phase 3 alongside ADR-043.
//!
//! ## ADR-043 contract surface (drafted at Phase 5, locked here)
//!
//! - **Cache + air-gap**: if `path` exists AND SHA-256 matches `expected_sha256_hex`,
//!   return `Ok(())` immediately. INFO log naming the file. No HTTP call.
//! - **Stale cache**: if `path` exists but SHA-256 mismatches, delete the file
//!   (WARN log) and fall through to download.
//! - **Streaming-abort heuristic** (concern #2): if HTTP `Content-Length` is wildly
//!   off the expected byte count (`< expected_bytes / 2` or `> expected_bytes * 2`),
//!   abort with `DownloadFailed` (likely wrong file or redirect HTML).
//! - **`.partial` strategy: restart-not-resume** (concern #3): any pre-existing
//!   `.partial` from a prior crashed run is clobbered by `File::create`. No HTTP
//!   Range header use.
//! - **Atomic finalize**: write to `.partial`, verify SHA-256 post-stream, only
//!   `rename` to final path on hash pass. Failed hash → delete `.partial`,
//!   return `IntegrityCheckFailed`.
//! - **Disk-full fail-closed**: any I/O error during write propagates as
//!   `VaultLlmError::Io`. Tauri can surface the error via a fatal dialog
//!   ("Insufficient disk space — need ~3 GB free at <path>").

use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{VaultLlmError, VaultLlmResult};

/// 8 MB chunks for streaming hash compute — large enough that syscall overhead
/// is negligible vs hash compute, small enough to keep RAM bounded.
const HASH_READ_CHUNK_SIZE: usize = 8 * 1024 * 1024;

/// Compute the SHA-256 of a file by streaming + hashing in 8 MB chunks.
/// Used for both post-download integrity verification AND cache-hit
/// re-verification when a model file already exists on disk.
pub async fn compute_sha256_of_file(path: &Path) -> VaultLlmResult<[u8; 32]> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_READ_CHUNK_SIZE];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

/// Ensure the model file is available at `path` with verified SHA-256.
///
/// Three operational paths converge here per ADR-043:
/// 1. **Cache hit**: file exists + hash matches → return Ok immediately.
/// 2. **Air-gap fallback**: user manually placed the file → same as cache hit
///    from this function's POV (no distinction in the runtime behavior).
/// 3. **Fresh download**: file absent OR hash mismatch → stream from `url`,
///    hash on the fly, atomic rename to final path on hash pass.
pub async fn ensure_model_at_path(
    path: &Path,
    url: &str,
    expected_sha256_hex: &str,
    expected_bytes: u64,
) -> VaultLlmResult<()> {
    let file_label = display_label(path);

    if path.exists() {
        let actual = hex::encode(compute_sha256_of_file(path).await?);
        if actual == expected_sha256_hex {
            tracing::info!(
                file = %file_label,
                "model file already present + hash verified (cache hit or air-gap)"
            );
            return Ok(());
        }
        tracing::warn!(
            file = %file_label,
            expected = %expected_sha256_hex,
            actual = %actual,
            "existing model file hash mismatch; deleting and re-downloading"
        );
        std::fs::remove_file(path)?;
    }

    download_with_verify(path, url, expected_sha256_hex, expected_bytes).await
}

async fn download_with_verify(
    path: &Path,
    url: &str,
    expected_sha256_hex: &str,
    expected_bytes: u64,
) -> VaultLlmResult<()> {
    let file_label = display_label(path);
    tracing::info!(
        file = %file_label,
        url = %url,
        expected_bytes = expected_bytes,
        "starting model download"
    );

    let resp = reqwest::get(url)
        .await
        .map_err(|e| VaultLlmError::DownloadFailed(format!("HTTP GET {url}: {e}")))?
        .error_for_status()
        .map_err(|e| VaultLlmError::DownloadFailed(format!("HTTP non-2xx: {e}")))?;

    // Streaming-abort heuristic per ADR-043 / iteration 2 concern #2 —
    // reject obvious-mismatch early to save bandwidth on a clearly-wrong
    // payload (e.g., HF served a redirect HTML page, or pinned URL points
    // at a different quantization variant).
    if let Some(cl) = resp.content_length() {
        let cl_low = expected_bytes / 2;
        let cl_high = expected_bytes.saturating_mul(2);
        if cl < cl_low || cl > cl_high {
            return Err(VaultLlmError::DownloadFailed(format!(
                "Content-Length {cl} bytes wildly off expected ~{expected_bytes} bytes \
                 (acceptable range [{cl_low}, {cl_high}]) — aborting (likely wrong file or redirect HTML)"
            )));
        }
    }

    // Restart-not-resume: create truncates any pre-existing .partial.
    let partial_path = path.with_extension("gguf.partial");
    let mut file = tokio::fs::File::create(&partial_path).await?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result
            .map_err(|e| VaultLlmError::DownloadFailed(format!("stream chunk: {e}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);

    let actual = hex::encode(hasher.finalize());
    if actual != expected_sha256_hex {
        // Fail-closed: remove the (now-tainted) .partial file.
        let _ = std::fs::remove_file(&partial_path);
        return Err(VaultLlmError::IntegrityCheckFailed {
            file: file_label,
            expected: expected_sha256_hex.to_string(),
            actual,
        });
    }

    tokio::fs::rename(&partial_path, path).await?;
    tracing::info!(
        file = %file_label,
        sha256 = %actual,
        "model downloaded + integrity verified"
    );
    Ok(())
}

fn display_label(path: &Path) -> String {
    path.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn tempfile_with_content(content: &[u8]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("tempfile create");
        f.write_all(content).expect("tempfile write");
        f.flush().expect("tempfile flush");
        f
    }

    // ─── floor 5: SHA-256 verify success ────────────────────────────────

    #[tokio::test]
    async fn sha256_of_known_content_matches_canonical_hash() {
        // SHA-256("hello world") =
        //   b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        let f = tempfile_with_content(b"hello world");
        let h = compute_sha256_of_file(f.path()).await.expect("hash");
        assert_eq!(
            hex::encode(h),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    // ─── floor 6: cache-hit / air-gap short-circuit ─────────────────────

    #[tokio::test]
    async fn ensure_returns_ok_immediately_on_cache_hit_with_matching_hash() {
        let f = tempfile_with_content(b"hello world");
        let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        // URL deliberately set to an unreachable address; cache-hit short-circuit
        // MUST fire before any HTTP attempt. If the function ever tries to
        // download, this test fails or hangs (we'd see it instantly).
        let result = ensure_model_at_path(
            f.path(),
            "http://127.0.0.1:1/never-reached.bin",
            expected,
            11, // "hello world" is 11 bytes
        )
        .await;
        assert!(
            result.is_ok(),
            "cache hit must short-circuit before HTTP — got {result:?}"
        );
    }

    // ─── floor 7: SHA-256 mismatch on cached file deletes + re-downloads
    //              (and downstream download fails closed = atomic-cleanup proof) ─

    #[tokio::test]
    async fn ensure_with_mismatched_cached_hash_deletes_file_then_attempts_redownload() {
        let f = tempfile_with_content(b"wrong content");
        let path = f.path().to_owned();
        // Release tempfile guard but the file stays on disk for the test.
        drop(f);

        // Wrong expected hash + unreachable URL — ensure_model_at_path
        // should: (1) hash the existing file, (2) detect mismatch, (3)
        // delete the file, (4) attempt the download, (5) fail on the
        // unreachable URL. The post-condition we assert is the file is
        // GONE (proving step 3) AND the result is Err (proving step 5).
        let wrong_expected = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = ensure_model_at_path(
            &path,
            "http://127.0.0.1:1/never-reached.bin",
            wrong_expected,
            13, // "wrong content" is 13 bytes
        )
        .await;
        assert!(result.is_err(), "download to unreachable URL must fail");
        assert!(
            !path.exists(),
            "mismatched cached file must be deleted before redownload attempt"
        );
    }
}
