//! Test-verification helpers — gated behind the `testing` cargo feature.
//!
//! **NOT for production use.** This module exists solely to enable cross-
//! verification tests (notably `test_9_embed_uses_cls_pooling_not_mean_pooling`
//! in `tests/embedding_tests.rs`) that need to compare the production CLS-pooled
//! `embed` output against a mean-pooled variant of the same input + same
//! tokenizer + same ort session. Without this helper, the CLS-pooling contract
//! could only be verified by manual code review of `bge_small.rs`'s slice
//! extraction — exactly the silent-failure scenario Spike 3 exposed.
//!
//! Production builds without `--features testing` do NOT compile this module.
//! The `[dev-dependencies] vault-embedding = { path = ".", features =
//! ["testing"] }` self-reference in `Cargo.toml` auto-enables the feature
//! for integration tests so `cargo test` works without a manual feature flag.

use crate::bge_small::BgeSmallProvider;
use crate::provider::EMBEDDING_DIM;
use std::sync::Arc;
use vault_core::{VaultError, VaultResult};

impl BgeSmallProvider {
    /// **TEST HELPER (gated `testing` feature).** Produces a mean-pooled,
    /// L2-normalised embedding for the given input — same tokenizer, same
    /// ort session as production `embed()`, but mean-pools across all
    /// non-padding token positions instead of extracting the [CLS] vector.
    ///
    /// Used by `test_9_embed_uses_cls_pooling_not_mean_pooling` to assert
    /// that production output (CLS-pool) differs element-wise from
    /// mean-pool — the test that pins the CLS-pooling contract from
    /// Spike 3 (`1_Pooling/config.json`: `pooling_mode_cls_token: true`).
    ///
    /// **Why mean-pool isn't right for production:** bge-small-en-v1.5 was
    /// trained with CLS-token pooling per BAAI's `1_Pooling/config.json`.
    /// Mean-pooling produces vectors of correct shape and L2 norm but with
    /// shifted semantics — the most insidious failure class. This helper
    /// exists to PROVE we're not doing it in production, not to enable it.
    ///
    /// # Errors
    ///
    /// Same shape as `embed`: `VaultError::Embedding` on tokenize / ort /
    /// extraction failure; `VaultError::InvalidInput` on empty input
    /// (matching production behaviour).
    pub async fn mean_pooled_for(&self, text: &str) -> VaultResult<Vec<f32>> {
        // Mirror embed()'s tokenize → truncate → cast pipeline.
        let mut encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| VaultError::Embedding(format!("tokenize: {e}")))?;

        if encoding.len() > 512 {
            encoding.truncate(512, 0, tokenizers::TruncationDirection::Right);
        }

        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        let mask_u32: Vec<u32> = encoding.get_attention_mask().to_vec();
        let mask: Vec<i64> = mask_u32.iter().map(|&x| x as i64).collect();
        let type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&x| x as i64).collect();
        let seq_len = ids.len();

        let session = Arc::clone(&self.session);

        tokio::task::spawn_blocking(move || -> VaultResult<Vec<f32>> {
            let input_ids = ort::value::Tensor::from_array(([1_usize, seq_len], ids))
                .map_err(|e| VaultError::Embedding(format!("input_ids tensor: {e}")))?;
            let attention_mask = ort::value::Tensor::from_array(([1_usize, seq_len], mask))
                .map_err(|e| VaultError::Embedding(format!("attention_mask tensor: {e}")))?;
            let token_type_ids = ort::value::Tensor::from_array(([1_usize, seq_len], type_ids))
                .map_err(|e| VaultError::Embedding(format!("token_type_ids tensor: {e}")))?;

            let mut session_guard = session
                .lock()
                .map_err(|e| VaultError::Embedding(format!("session lock poisoned: {e}")))?;

            let outputs = session_guard
                .run(ort::inputs![
                    "input_ids" => input_ids,
                    "attention_mask" => attention_mask,
                    "token_type_ids" => token_type_ids,
                ])
                .map_err(|e| VaultError::Embedding(format!("session run: {e}")))?;

            let last_hidden_state = outputs
                .get("last_hidden_state")
                .ok_or_else(|| VaultError::Embedding("missing 'last_hidden_state'".into()))?;

            let (_shape, data) = last_hidden_state
                .try_extract_tensor::<f32>()
                .map_err(|e| VaultError::Embedding(format!("extract tensor: {e}")))?;

            // Mean-pool across sequence positions (attention-mask weighted).
            // Memory layout: [batch=1, seq_len, hidden=384] in row-major →
            // hidden_size floats per position, seq_len positions per batch row.
            let expected_len = seq_len * EMBEDDING_DIM;
            if data.len() < expected_len {
                return Err(VaultError::Embedding(format!(
                    "output too small for mean-pool: {} floats < expected {}",
                    data.len(),
                    expected_len
                )));
            }

            let mut sum = vec![0.0_f32; EMBEDDING_DIM];
            let mut total_weight = 0.0_f32;
            for (pos, &mask_val) in mask_u32.iter().enumerate().take(seq_len) {
                let weight = mask_val as f32;
                if weight == 0.0 {
                    continue;
                }
                let offset = pos * EMBEDDING_DIM;
                for (h, sum_h) in sum.iter_mut().enumerate() {
                    *sum_h += data[offset + h] * weight;
                }
                total_weight += weight;
            }
            if total_weight == 0.0 {
                return Err(VaultError::Embedding(
                    "all-zero attention mask in mean-pool".into(),
                ));
            }
            let mean: Vec<f32> = sum.iter().map(|s| s / total_weight).collect();

            // L2-normalize (matches production's post-pool normalisation so
            // the comparison isolates pooling-mode difference, not norm difference).
            let norm: f32 = mean.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm == 0.0 {
                return Err(VaultError::Embedding("zero-norm mean vector".into()));
            }
            Ok(mean.iter().map(|x| x / norm).collect())
        })
        .await
        .map_err(|e| VaultError::Embedding(format!("spawn_blocking join: {e}")))?
    }
}
