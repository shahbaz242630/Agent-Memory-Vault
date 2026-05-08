//! At-rest sealed `ObjectStore` for LanceDB — T0.2.0 (BRD §11.5.1).
//!
//! Production wiring of the integration shape Phase 0c spike v2
//! runtime-confirmed (commit `b7f1a5f`, 2026-05-08). Custom URI scheme
//! `vault-sealed://` registered with lance-io 4.0's `ObjectStoreRegistry`
//! routes ALL of Lance's I/O — both write and read paths, on every
//! supported OS — through [`SealedObjectStore`], which AEAD-encrypts
//! payloads on `put_opts` and decrypts on `get_opts` with per-file
//! AAD-binding.
//!
//! ## Why not `WrappingObjectStore` or `ObjectStoreParams.object_store`
//!
//! Spike v1 (lance-io 0.15) tried both. Both are bypassed by lance-io's
//! `LocalObjectReader` fast-path for `file://` URIs — the wrapper's
//! `wrap()` fires on writes but reads go through `std::fs` directly,
//! producing a vacuous "round-trip succeeds" over plaintext-on-disk.
//! Verified empirically; spike v1 preserved as
//! `crates/vault-storage/examples/at_rest_spike.rs.v1_fail_disabled`.
//! Spike v2 (lance-io 4.0) demonstrated that registering a provider for
//! an UNKNOWN URI scheme bypasses the fast-path because there's no
//! fast-path implementation for unknown schemes — empirically confirmed
//! with fire-counters on both flows. **Stage E 2×2 diagnostic also
//! confirmed sealed-path behavior is byte-identical to plain file:// in
//! Lance's `OptimizeStats`** — sealing is transparent to Lance's
//! compaction/prune logic.
//!
//! ## Locked decisions (consumed verbatim from V1 + V2 spikes; ADR-008 amendment text at Phase 0e)
//!
//! - **Cipher (path #1, ADR-008 archive line 681):** dryoc 0.7.2's
//!   DryocStream-as-single-message + `Tag::FINAL`. Same envelope as
//!   T0.2.9 sync. No new crypto crate.
//! - **KDF (K3):** `at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key)`.
//!   Single-source-crypto; same primitive as the AAD computation.
//! - **AAD (Finding 2(c)):** `AAD = BLAKE3("vault-at-rest-v1" || file_path_bytes)`.
//!   Per-file binding; replay-attack residual documented (ADR-008
//!   amendment, Phase 0e).
//! - **Sealing shape (iter-3 §3.4):** `version_byte (0x01) ||
//!   granularity_marker (0x00 = per-file) || dryoc_header (24 bytes) ||
//!   ciphertext`. 26-byte framing + 17-byte AEAD tag/tag-byte = 43-byte
//!   total per-file overhead.
//! - **Granularity (iter-3 §3.1):** per-file, V0.2 unconditional.
//!   Revisit at V1.0 if column-projection latency surfaces.
//! - **Sized-input quirk (extends ADR-008 line 684):** `dryoc::DryocStream`
//!   `push_to_vec` and `pull_to_vec` require `Option<&Vec<u8>>` for the
//!   AAD too — not just plaintext. Documented at the call sites.
//!
//! ## Multipart writes intentionally `NotSupported`
//!
//! Per-file granularity requires the full plaintext at seal time, which
//! streaming multipart writes don't fit without column-level granularity
//! (deferred to V1.0). Lance falls back to single-shot `put_opts` for
//! the small files we produce in V0.2 — this works in practice; if a
//! future Lance change forces multipart for our access patterns, the
//! granularity decision revisits.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use dryoc::dryocstream::{DryocStream, Header, Key, Pull, Push, Tag};
use futures::stream::{self, BoxStream, StreamExt};
use lance_core::{Error as LanceError, Result as LanceResult};
use lance_io::object_store::providers::ObjectStoreProvider;
use lance_io::object_store::{ObjectStore as LanceObjectStore, ObjectStoreParams};
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{
    Error as ObjectStoreError, GetOptions, GetResult, GetResultPayload, ListResult,
    MultipartUpload, ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload,
    PutResult,
};
use url::Url;
use zeroize::Zeroizing;

// ============================================================
//   Locked sealing-shape constants (iter-3 §3.4)
// ============================================================

/// Sealing-envelope version byte. Locked at iter-3; never changes
/// without an ADR-008 amendment + on-disk migration plan.
pub(crate) const VERSION_BYTE: u8 = 0x01;

/// Granularity marker. `0x00` = per-file granularity (the V0.2 lock).
/// Reserved values for future per-column or per-block granularity.
pub(crate) const GRANULARITY_PER_FILE: u8 = 0x00;

const FRAMING_PREFIX_LEN: usize = 2;
const HEADER_LEN: usize = 24;
const TOTAL_FRAMING_LEN: usize = FRAMING_PREFIX_LEN + HEADER_LEN;

/// AEAD overhead per envelope: 16-byte Poly1305 tag + 1-byte message-tag
/// byte (per ADR-008 archive line 686; verified empirically by V1 spike
/// Stage B with a 5120-byte plaintext producing a 5163-byte sealed
/// envelope = 26 framing + 17 AEAD).
const AEAD_OVERHEAD: usize = 17;

/// Constant per-file overhead between sealed bytes and plaintext bytes.
/// `head` / `list` overrides translate inner sealed sizes back to the
/// unsealed sizes Lance expects.
const SEAL_OVERHEAD: usize = TOTAL_FRAMING_LEN + AEAD_OVERHEAD;

/// Custom URI scheme used to route I/O through [`SealedFileStoreProvider`].
/// **Must be unknown to lance-io's built-in fast-paths** — per spike v1
/// FAIL, any scheme lance-io recognises (`file`, `s3`, `az`, `gs`,
/// `memory`) is intercepted by a built-in provider before reaching our
/// registration. `vault-sealed` was chosen because it has no
/// preexisting meaning to lance-io.
pub const VAULT_SEALED_SCHEME: &str = "vault-sealed";

// ============================================================
//   Key derivation + AAD
// ============================================================

/// Derive the at-rest key from the user's master key per K3 lock:
/// `blake3::derive_key("vault memory at-rest sealing v1", &master_key)`.
///
/// BLAKE3's `derive_key` is the purpose-built domain-separated KDF
/// primitive; same security properties as HKDF-SHA256 (PRF + domain
/// separation) without adding a new crypto crate. The context string is
/// distinct from any other KDF use in the workspace — domain-separation
/// against future cross-context confusion.
///
/// Returned bytes are wrapped in [`zeroize::Zeroizing`] so they zero out
/// on drop. Callers SHOULD pass the master key by `&[u8; 32]` and keep
/// the master key's own zeroization separate.
pub fn derive_at_rest_key(master_key: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(blake3::derive_key(
        "vault memory at-rest sealing v1",
        master_key,
    ))
}

/// Compute per-file AAD per Finding 2(c) lock:
/// `AAD = BLAKE3("vault-at-rest-v1" || file_path_bytes)`.
///
/// The domain-separator `"vault-at-rest-v1"` is distinct from sync's
/// `"vault-aad-v1"` (ADR-008 archive line 700) — prevents cross-context
/// envelope confusion. No `version_id` binding in V0.2; replay-attack
/// residual documented in the ADR-008 amendment (Phase 0e).
fn compute_aad(file_path: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"vault-at-rest-v1");
    hasher.update(file_path.as_bytes());
    *hasher.finalize().as_bytes()
}

// ============================================================
//   Seal / unseal primitives
// ============================================================

/// Seal `plaintext` into the at-rest envelope:
/// `version_byte || granularity_marker || dryoc_header (24) || ciphertext`.
///
/// dryoc's `push_to_vec` requires `&Vec<u8>` (not `&[u8]`) for both
/// plaintext and AAD — sized-input quirk per ADR-008 archive line 684.
fn seal_file_bytes(plaintext: &[u8], key: &[u8; 32], aad: &[u8; 32]) -> Vec<u8> {
    let dryoc_key: Key = (*key).into();
    let (mut push, header): (DryocStream<Push>, Header) = DryocStream::init_push(&dryoc_key);

    let plaintext_vec = plaintext.to_vec();
    let aad_vec = aad.to_vec();
    let ciphertext = push
        .push_to_vec(&plaintext_vec, Some(&aad_vec), Tag::FINAL)
        .expect("dryoc push_to_vec is infallible for in-memory plaintext");

    let mut sealed = Vec::with_capacity(TOTAL_FRAMING_LEN + ciphertext.len());
    sealed.push(VERSION_BYTE);
    sealed.push(GRANULARITY_PER_FILE);
    sealed.extend_from_slice(<Header as dryoc::types::Bytes>::as_slice(&header));
    sealed.extend_from_slice(&ciphertext);
    sealed
}

/// Unseal envelope. Returns Err on framing mismatch, AEAD authenticity
/// failure (wrong key, tampered ciphertext, wrong AAD), or malformed
/// framing. Error string is bounded; not for end-user display.
fn unseal_file_bytes(sealed: &[u8], key: &[u8; 32], aad: &[u8; 32]) -> Result<Vec<u8>, String> {
    if sealed.len() < TOTAL_FRAMING_LEN {
        return Err(format!(
            "sealed envelope too short ({} bytes < {} framing)",
            sealed.len(),
            TOTAL_FRAMING_LEN
        ));
    }
    if sealed[0] != VERSION_BYTE {
        return Err(format!("unexpected version_byte: {:#x}", sealed[0]));
    }
    if sealed[1] != GRANULARITY_PER_FILE {
        return Err(format!("unexpected granularity_marker: {:#x}", sealed[1]));
    }

    let header_bytes = &sealed[FRAMING_PREFIX_LEN..FRAMING_PREFIX_LEN + HEADER_LEN];
    let header: Header = header_bytes
        .try_into()
        .map_err(|e: dryoc::Error| format!("header recovery: {e}"))?;

    let ciphertext: Vec<u8> = sealed[TOTAL_FRAMING_LEN..].to_vec();

    let dryoc_key: Key = (*key).into();
    let mut pull: DryocStream<Pull> = DryocStream::init_pull(&dryoc_key, &header);

    let aad_vec = aad.to_vec();
    let (plaintext, tag) = pull
        .pull_to_vec(&ciphertext, Some(&aad_vec))
        .map_err(|e| format!("dryoc pull_to_vec: {e}"))?;

    if !matches!(tag, Tag::FINAL) {
        return Err(format!("unexpected tag: {tag:?}"));
    }

    Ok(plaintext)
}

// ============================================================
//   URL helpers — vault-sealed:/// scheme construction
// ============================================================

/// Build a `vault-sealed:///<abs-path>` URL from a local directory path.
/// Constructs a `file://` URL via `Url::from_directory_path` (which has
/// well-defined cross-platform percent-encoding) then swaps the scheme.
///
/// Panics if `local_dir` is not absolute (an absolute path is required
/// for `Url::from_directory_path`; callers must canonicalise first).
pub fn make_vault_sealed_uri(local_dir: &std::path::Path) -> String {
    let file_url =
        Url::from_directory_path(local_dir).expect("from_directory_path requires absolute path");
    format!(
        "{VAULT_SEALED_SCHEME}:{}",
        &file_url.as_str()["file:".len()..]
    )
}

/// Translate a `vault-sealed:///<path>` URL back to a local filesystem
/// path by scheme substitution + `Url::to_file_path`.
fn vault_sealed_to_local_path(url: &Url) -> Result<std::path::PathBuf, String> {
    let s = url.as_str();
    let after_scheme = s
        .strip_prefix(&format!("{VAULT_SEALED_SCHEME}:"))
        .ok_or_else(|| format!("not a vault-sealed:// url: {s}"))?;
    let file_str = format!("file:{after_scheme}");
    let file_url = Url::parse(&file_str).map_err(|e| format!("scheme-swap reparse: {e}"))?;
    file_url
        .to_file_path()
        .map_err(|_| format!("to_file_path failed for {file_str}"))
}

// ============================================================
//   SealedObjectStore — wraps any inner ObjectStore
// ============================================================

/// AEAD-sealing wrapper around an inner [`object_store::ObjectStore`].
/// `put_opts` aggregates the payload, seals with per-file AAD, and
/// forwards sealed bytes to the inner store. `get_opts` fetches sealed
/// bytes, unseals, and returns plaintext as a single-chunk
/// `GetResult::Stream`.
///
/// `head` / `list` / `list_with_delimiter` adjust the reported size by
/// the constant 43-byte sealing overhead so Lance sees plaintext-shaped
/// metadata (it expects sizes consistent with what `get_opts` returns).
///
/// `copy` / `copy_if_not_exists` forward to the inner store. Note: a
/// raw byte-copy of a sealed file produces an AAD-mismatched blob at
/// the destination (AAD is bound to the source path); reads from the
/// destination will fail closed. Memory Vault's V0.2 access patterns
/// don't exercise copy on sealed files; if a future code path does,
/// it must unseal-reseal.
///
/// `put_multipart_opts` returns `NotSupported` — per-file granularity
/// requires the full plaintext at seal time.
struct SealedObjectStore {
    inner: Arc<dyn ObjectStore>,
    key: Arc<Zeroizing<[u8; 32]>>,
}

impl std::fmt::Debug for SealedObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Per ADR-007: do not emit anything that could leak the key. The
        // key is a `Zeroizing<[u8; 32]>` so the `Debug` derive on it
        // would print bytes — we manually opt out.
        f.debug_struct("SealedObjectStore")
            .field("inner", &self.inner)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl std::fmt::Display for SealedObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SealedObjectStore({})", self.inner)
    }
}

impl SealedObjectStore {
    fn aad_for_path(&self, location: &ObjectPath) -> [u8; 32] {
        compute_aad(location.as_ref())
    }
}

#[async_trait]
impl ObjectStore for SealedObjectStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        let plaintext: Vec<u8> = payload.iter().flat_map(|b| b.iter().copied()).collect();
        let aad = self.aad_for_path(location);
        let sealed = seal_file_bytes(&plaintext, &self.key, &aad);
        let sealed_payload: PutPayload = sealed.into();
        self.inner.put_opts(location, sealed_payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        _location: &ObjectPath,
        _opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        Err(ObjectStoreError::NotSupported {
            source:
                "SealedObjectStore: put_multipart not implemented (per-file granularity, V0.2 lock)"
                    .into(),
        })
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        // Always fetch the WHOLE sealed file — per-file granularity
        // requires whole-file decryption. If Lance requested a byte
        // range OR head-only, fetch full body anyway and slice/strip
        // from the unsealed plaintext below. V1.0 mitigation: store
        // unsealed_size in the framing or a sidecar.
        let mut full_options = options;
        let requested_range = full_options.range.take();
        let was_head_only = full_options.head;
        full_options.head = false;

        let sealed_get = self.inner.get_opts(location, full_options).await?;
        let sealed_meta = sealed_get.meta.clone();
        let sealed_attributes = sealed_get.attributes.clone();
        let sealed_bytes = sealed_get.bytes().await?;

        let aad = self.aad_for_path(location);
        let plaintext = unseal_file_bytes(&sealed_bytes, &self.key, &aad).map_err(|e| {
            ObjectStoreError::Generic {
                store: "SealedObjectStore",
                source: e.into(),
            }
        })?;

        let plaintext_len = plaintext.len();
        let (range, body): (std::ops::Range<u64>, Vec<u8>) = match requested_range {
            None => (0..plaintext_len as u64, plaintext),
            Some(object_store::GetRange::Bounded(r)) => {
                let start = (r.start as usize).min(plaintext_len);
                let end = (r.end as usize).min(plaintext_len);
                let slice = plaintext[start..end].to_vec();
                (start as u64..end as u64, slice)
            }
            Some(object_store::GetRange::Offset(o)) => {
                let start = (o as usize).min(plaintext_len);
                let slice = plaintext[start..].to_vec();
                (start as u64..plaintext_len as u64, slice)
            }
            Some(object_store::GetRange::Suffix(suffix)) => {
                let start = plaintext_len.saturating_sub(suffix as usize);
                let slice = plaintext[start..].to_vec();
                (start as u64..plaintext_len as u64, slice)
            }
        };

        let body_bytes = if was_head_only {
            Bytes::new()
        } else {
            Bytes::from(body)
        };
        let payload_stream: BoxStream<'static, object_store::Result<Bytes>> =
            stream::once(async move { Ok(body_bytes) }).boxed();

        let unsealed_meta = ObjectMeta {
            location: sealed_meta.location,
            last_modified: sealed_meta.last_modified,
            size: plaintext_len as u64,
            e_tag: sealed_meta.e_tag,
            version: sealed_meta.version,
        };

        Ok(GetResult {
            payload: GetResultPayload::Stream(payload_stream),
            meta: unsealed_meta,
            range,
            attributes: sealed_attributes,
        })
    }

    async fn head(&self, location: &ObjectPath) -> object_store::Result<ObjectMeta> {
        let mut meta = self.inner.head(location).await?;
        meta.size = meta.size.saturating_sub(SEAL_OVERHEAD as u64);
        Ok(meta)
    }

    async fn delete(&self, location: &ObjectPath) -> object_store::Result<()> {
        self.inner.delete(location).await
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.inner
            .list(prefix)
            .map(|res| {
                res.map(|mut meta| {
                    meta.size = meta.size.saturating_sub(SEAL_OVERHEAD as u64);
                    meta
                })
            })
            .boxed()
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> object_store::Result<ListResult> {
        let mut result = self.inner.list_with_delimiter(prefix).await?;
        for meta in &mut result.objects {
            meta.size = meta.size.saturating_sub(SEAL_OVERHEAD as u64);
        }
        Ok(result)
    }

    async fn copy(&self, from: &ObjectPath, to: &ObjectPath) -> object_store::Result<()> {
        // AAD-binding caveat documented at struct-level. Memory Vault
        // V0.2 access patterns don't exercise this path through Lance.
        self.inner.copy(from, to).await
    }

    async fn copy_if_not_exists(
        &self,
        from: &ObjectPath,
        to: &ObjectPath,
    ) -> object_store::Result<()> {
        self.inner.copy_if_not_exists(from, to).await
    }
}

// ============================================================
//   SealedFileStoreProvider — registered for vault-sealed:// scheme
// ============================================================

/// `lance_io::object_store::providers::ObjectStoreProvider` impl that
/// returns a [`SealedObjectStore`] over `LocalFileSystem` for any
/// `vault-sealed://<abs-path>` URL.
///
/// Constructed once at [`crate::vector_store::LanceVectorStore::open_with_at_rest_key`]
/// and registered in an [`lance_io::object_store::providers::ObjectStoreRegistry`]
/// passed to a [`lancedb::Session`]. Lance calls `new_store` lazily
/// on first I/O, then reuses the returned `ObjectStore` for the
/// lifetime of the connection.
pub struct SealedFileStoreProvider {
    key: Arc<Zeroizing<[u8; 32]>>,
}

impl std::fmt::Debug for SealedFileStoreProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // ADR-007: never leak the key via Debug.
        f.debug_struct("SealedFileStoreProvider")
            .field("key", &"<redacted>")
            .finish()
    }
}

impl SealedFileStoreProvider {
    /// Build a provider holding `at_rest_key` (the K3-derived key, NOT
    /// the user's master key). The key is wrapped in [`zeroize::Zeroizing`]
    /// inside an `Arc` so it zeroes on drop and is cheap to share with
    /// the [`SealedObjectStore`] instances `new_store` returns.
    pub fn new(at_rest_key: Zeroizing<[u8; 32]>) -> Self {
        Self {
            key: Arc::new(at_rest_key),
        }
    }
}

#[async_trait]
impl ObjectStoreProvider for SealedFileStoreProvider {
    async fn new_store(
        &self,
        base_path: Url,
        _params: &ObjectStoreParams,
    ) -> LanceResult<LanceObjectStore> {
        let local: Arc<dyn ObjectStore> = Arc::new(LocalFileSystem::new());
        let sealed: Arc<dyn ObjectStore> = Arc::new(SealedObjectStore {
            inner: local,
            key: self.key.clone(),
        });

        // 9-param `new` constructor — defaults mirror lance-io 4.0
        // FileStoreProvider's body (verified 2026-05-08 by source-read
        // of lance-io 4.0.0/src/object_store/providers/local.rs).
        Ok(LanceObjectStore::new(
            sealed, base_path, /* block_size                     */ None,
            /* wrapper                        */ None,
            /* use_constant_size_upload_parts */ false,
            /* list_is_lexically_ordered      */ false,
            /* io_parallelism                 */ 8,
            /* download_retry_count           */ 3,
            /* storage_options                */ None,
        ))
    }

    fn extract_path(&self, url: &Url) -> LanceResult<ObjectPath> {
        // Default impl uses `url.to_file_path()` which fails on
        // vault-sealed:// (only known to file/ftp). Mirror
        // FileStoreProvider's body but scheme-substitute first.
        let local = vault_sealed_to_local_path(url)
            .map_err(|e| LanceError::invalid_input(format!("vault-sealed scheme-swap: {e}")))?;
        ObjectPath::from_absolute_path(&local).map_err(|e| {
            LanceError::invalid_input(format!(
                "Path::from_absolute_path({}): {e}",
                local.display()
            ))
        })
    }

    fn calculate_object_store_prefix(
        &self,
        _url: &Url,
        _storage_options: Option<&HashMap<String, String>>,
    ) -> LanceResult<String> {
        // Mirror FileStoreProvider — return the scheme name as the prefix.
        Ok(VAULT_SEALED_SCHEME.to_string())
    }
}
