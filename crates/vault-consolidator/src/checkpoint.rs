//! Checkpoint capture (T0.2.5 / A2, BRD §5.6 line 1032 slot).
//!
//! Turns "what one consolidation run changed" into the
//! [`vault_storage::CheckpointEntry`] list that
//! [`vault_storage::StorageBackend::create_checkpoint`] persists as an
//! undo-log.
//!
//! ## Why a before/after diff (not per-mutation hooks)
//!
//! Every mutation inside [`crate::consolidator::Consolidator::run_consolidation`]
//! — merge-supersede, deterministic dedup, contradiction `invalidate`, and
//! Phase-4 decay — is **metadata-only on an existing row** (none re-embed; only
//! the separate `enrich_facts` pass changes a stored vector). The new merged
//! rows are the only insertions. So the complete set of changes a run made is
//! exactly the diff between a snapshot taken *before* the run and one taken
//! *after*:
//!
//! - present **before & after** with a changed row → [`CheckpointEntry::Modified`]
//!   (pre-image = the before-row + its stored embedding, reconstructed via
//!   [`stored_embed_text`]);
//! - present **only after** → [`CheckpointEntry::Created`] (a merged row;
//!   rollback deletes it).
//!
//! The consolidator never hard-deletes (supersede / `valid_until` / decay are
//! soft markers), so there is no "present only before" (deleted) case; if one
//! ever appears it is logged and skipped (it cannot be restored via
//! `update_memory`).
//!
//! This keeps capture to two enumerations + a diff, with zero changes to the
//! mutation sites themselves.

use std::collections::HashMap;

use vault_core::{Memory, MemoryId, VaultResult};
use vault_embedding::EmbeddingProvider;
use vault_storage::CheckpointEntry;

use crate::phases::enrich::stored_embed_text;

/// Build the checkpoint entries describing what changed between the pre-run
/// snapshot `pre` and the post-run state `post`.
///
/// Both slices MUST be full enumerations (`include_superseded = true`) so a
/// memory that transitioned active → superseded during the run is present in
/// both and detected as `Modified` rather than mis-classified.
///
/// `Modified` pre-images carry the memory's stored embedding, reconstructed by
/// re-embedding [`stored_embed_text`] of the *before* row (exact, since the
/// embedder is deterministic and the run's mutations never re-embed).
pub(crate) async fn diff_to_entries(
    pre: &[Memory],
    post: &[Memory],
    embeddings: &dyn EmbeddingProvider,
) -> VaultResult<Vec<CheckpointEntry>> {
    let pre_by_id: HashMap<MemoryId, &Memory> = pre.iter().map(|m| (m.id, m)).collect();
    let post_by_id: HashMap<MemoryId, &Memory> = post.iter().map(|m| (m.id, m)).collect();

    let mut entries = Vec::new();

    // Modified: present in both, row changed. (Memory's PartialEq ignores the
    // always-None `embedding` field on both sides — vectors live in LanceDB —
    // so this compares content / confidence / valid_until / superseded_by /
    // metadata, i.e. exactly the fields a run mutates.)
    for (id, &before) in &pre_by_id {
        match post_by_id.get(id) {
            Some(&after) if after != before => {
                let embedding = embeddings.embed(&stored_embed_text(before)).await?;
                entries.push(CheckpointEntry::Modified {
                    memory: Box::new(before.clone()),
                    embedding,
                });
            }
            Some(_) => { /* unchanged — nothing to capture */ }
            None => {
                tracing::warn!(
                    target: "vault_consolidator::checkpoint",
                    memory_id = %id,
                    "memory present pre-run but absent post-run — the consolidator should \
                     never hard-delete; cannot checkpoint a delete, skipping this entry"
                );
            }
        }
    }

    // Created: present only in the post-run state (new merged rows).
    for memory in post {
        if !pre_by_id.contains_key(&memory.id) {
            entries.push(CheckpointEntry::Created {
                memory_id: memory.id,
                boundary: memory.boundary.clone(),
            });
        }
    }

    Ok(entries)
}
