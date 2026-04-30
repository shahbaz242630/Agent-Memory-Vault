//! `EmbeddingProvider` — the abstract embedding contract consumed by
//! `vault-storage::cascading::StorageBackend::write_memory` (T0.1.6) and the
//! retrieval search path (T0.1.8). One implementation lives in this crate
//! today (`BgeSmallProvider`); the trait keeps the rest of the workspace
//! decoupled from the specific model + runtime choice.
//!
//! V0.1 intentionally exposes a single-input `embed` method only. Batch
//! inference (`embed_batch`) is deferred to T0.2.x when the consolidator
//! becomes the first real batching caller — see T0.1.7_PLAN.md Q4 (push-back
//! on speculative scaffolding per CLAUDE.md). The trait is additive: an
//! `embed_batch` default-impl method can be added later without breaking
//! existing callers.

use async_trait::async_trait;
use vault_core::VaultResult;

/// Embedding dimension produced by every implementor in this crate.
///
/// Locked to **384** because:
/// - `LanceVectorStore` (T0.1.4) is constructed with this exact dimension and
///   its cosine-distance scoring assumes L2-normalised vectors of this length.
/// - The bundled model (`bge-small-en-v1.5`) has `hidden_size: 384` per its
///   `config.json` (verified via Spike 3 — see T0.1.7_PLAN.md amendment block).
///
/// Changing this constant requires a coordinated change across vault-storage
/// (re-create the LanceDB table at the new dimension) AND a model swap. The
/// model integrity check (ADR-020) defends against silent model swaps that
/// would change this dimension.
pub const EMBEDDING_DIM: usize = 384;

/// Abstract embedding provider. Sole production caller is the cascading
/// orchestrator's write-path (`vault-storage::cascading::StorageBackend::write_memory`)
/// and the retrieval search path (T0.1.8).
///
/// Implementations MUST:
/// 1. Return a `Vec<f32>` of length [`EMBEDDING_DIM`] (= 384).
/// 2. Return an L2-normalised vector (unit norm within `1e-6`) — `LanceVectorStore`'s
///    cosine scoring is calibrated for this. Test 8 (`embed_output_is_l2_normalized`)
///    enforces this contract across diverse inputs.
/// 3. Be deterministic: same input → byte-identical output across calls.
/// 4. Be safe to call concurrently from many tasks (Send + Sync).
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Embed a single text input. Returns a `Vec<f32>` of length [`EMBEDDING_DIM`],
    /// L2-normalised.
    ///
    /// # Errors
    ///
    /// Returns [`vault_core::VaultError::Embedding`] if tokenisation, inference,
    /// or post-processing fails. A length-zero or non-UTF-8 input returns
    /// [`vault_core::VaultError::InvalidInput`].
    async fn embed(&self, text: &str) -> VaultResult<Vec<f32>>;
}
