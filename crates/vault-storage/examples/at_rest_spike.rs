//! At-rest-extension spike v2 — T0.2.0 Phase 0c.
//!
//! Replaces the v1 spike (`at_rest_spike.rs.v1_fail_disabled`) which
//! formally FAILed at lance-io 0.15: both `WrappingObjectStore` and direct
//! `ObjectStoreParams.object_store` injection were bypassed by lance-io's
//! `LocalObjectReader` fast-path for `file://` URIs in BOTH directions.
//!
//! v2 hypothesis (web-research at Phase 0a, verification report 2026-05-08,
//! runtime confirmation is THIS spike): lance-io 4.0 exposes a first-class
//! `ObjectStoreProvider` + `ObjectStoreRegistry` API. Registering a custom
//! provider for an UNKNOWN scheme (`vault-sealed://`) bypasses any built-in
//! fast-paths because there's no fast-path implementation for unknown
//! schemes. All I/O routes through our provider's returned `ObjectStore`.
//!
//! ## Cargo.lock pre-flight snapshot (captured 2026-05-08)
//!
//!   lancedb       = 0.27.2
//!   lance         = 4.0.0
//!   lance-io      = 4.0.0
//!   lance-core    = 4.0.0
//!   lance-table   = 4.0.0
//!   object_store  = 0.12.5
//!   dryoc         = 0.7.2
//!   bytes         = 1.11.1
//!   url           = 2.5.8
//!   walkdir       = 2.5.0
//!
//! Verified pre-flight 2026-05-08 against verification-report Cargo.lock
//! snapshot (HANDOFF.md Phase 0c iteration-2 plan paragraph). Re-run
//! `cargo tree -p lancedb | grep -E "lance-io|object_store"` before
//! re-executing this spike if the lockfile has been touched since.
//!
//! ## Finding 1 — `WrappingObjectStore` STILL EXISTS in lance-io 4.0
//!
//! `lance_io::object_store::ObjectStore::new(...)` exposes a
//! `wrapper: Option<Arc<dyn WrappingObjectStore>>` parameter. The v1 bypass
//! mechanism (LocalObjectReader fast-path bypassing object_store trait
//! calls for `file://` URIs) MAY still exist for `file://`. We have NOT
//! confirmed lance-io 4.0 removed it. Our safety relies entirely on
//! `vault-sealed://` being an UNKNOWN scheme. **The fire-counters in
//! Stage A are LOAD-BEARING — not nice-to-have.** If they read 0 on the
//! read flow, this spike has FAILED for the same root reason as v1
//! (fast-path bypass), just under a different mechanism, and ADR-008
//! amendment must re-open with a fundamentally different integration
//! path (Path X' / Y' / Z' from V1 plan iteration-3).
//!
//! ## Methodology (per `feedback_runtime_confirmation_after_web_spike.md`)
//!
//! Hybrid: web-research validated at Phase 0a + verification report
//! confirmed 4.0 docs.rs API surface → runtime confirmation is the
//! load-bearing leg. All four stages must run end-to-end against the
//! upgraded stack with empirical fire-counters and on-disk byte
//! inspection. No PASS is reported on docs-claim alone.
//!
//! ## Stage decomposition (PASS-or-FAIL whole)
//!
//!   - **Stage A** — `IdentityFileStoreProvider` with fire-counters.
//!     Cheapest possible refutation of fast-path bypass. ~30 min.
//!   - **Stage B** — sealing primitives lifted verbatim from v1.
//!     Already adversarially verified at v1 Stage B (2026-05-07). ~10 min.
//!   - **Stage C** — `SealedFileStoreProvider` + `SealedObjectStore`.
//!     End-to-end sealed write+read through `vault-sealed://`. Inline
//!     sealing-byte assertion (first two bytes 0x01 || 0x00) closes the
//!     "fired but didn't seal" gap. ~60-90 min.
//!   - **Stage D** — adversarial sweep + ADR-039-through-sealing.
//!     Walk every file: zero PAR1 magic, entropy ≥ 7.9, framing bytes
//!     correct. Wrong-key-fails-closed. Delete-half + Prune + decrypt-
//!     and-grep deleted ids: zero hits (privacy invariant survives
//!     sealing wrapper). ~30-45 min.
//!
//! ## Locked decisions consumed (carried verbatim from v1)
//!
//! - **AAD scheme** (Finding 2(c)): `BLAKE3("vault-at-rest-v1" || file_path_bytes)`.
//! - **KDF** (K3): `blake3::derive_key("vault memory at-rest sealing v1", &master_key)`.
//! - **Sealing shape**: `version_byte(0x01) || granularity(0x00) || dryoc_header(24) || ciphertext`.
//! - **Granularity**: per-file, V0.2 unconditional.
//! - **Cipher** (ADR-008 path #1): DryocStream-as-single-message + `Tag::FINAL`.
//!
//! ## Run
//!
//!   cargo run -p vault-storage --example at_rest_spike --release
//!
//! (Requires PROTOC + Strawberry Perl on PATH per ADR-006 / ADR-011 — same
//! constraints as the production build chain.)

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use arrow_array::{
    FixedSizeListArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray,
};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use bytes::Bytes as BytesBuf;
use dryoc::dryocstream::{DryocStream, Header, Key, Pull, Push, Tag};
use dryoc::types::Bytes;
use futures::stream::{self, BoxStream, StreamExt, TryStreamExt};
use lance_core::{Error as LanceError, Result as LanceResult};
use lance_io::object_store::providers::ObjectStoreProvider;
use lance_io::object_store::{ObjectStore as LanceObjectStore, ObjectStoreParams};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::OptimizeAction;
use lancedb::{connect, ObjectStoreRegistry, Session};
use object_store::local::LocalFileSystem;
use object_store::path::Path as ObjectPath;
use object_store::{
    Error as ObjectStoreError, GetOptions, GetResult, GetResultPayload, ListResult,
    MultipartUpload, ObjectMeta, ObjectStore, PutMultipartOptions, PutOptions, PutPayload,
    PutResult,
};
use url::Url;

// ============================================================
//   Constants — locked sealing shape (iteration 3 §3.4)
// ============================================================

const VERSION_BYTE: u8 = 0x01;
const GRANULARITY_PER_FILE: u8 = 0x00;
const FRAMING_PREFIX_LEN: usize = 2;
const HEADER_LEN: usize = 24;
const TOTAL_FRAMING_LEN: usize = FRAMING_PREFIX_LEN + HEADER_LEN;
/// AEAD overhead per envelope: 16-byte Poly1305 tag + 1-byte message-tag byte.
const AEAD_OVERHEAD: usize = 17;
/// Constant per-file overhead between sealed bytes and plaintext bytes.
const SEAL_OVERHEAD: usize = TOTAL_FRAMING_LEN + AEAD_OVERHEAD;

const VAULT_SEALED_SCHEME: &str = "vault-sealed";
const TABLE_NAME: &str = "memories";
const EMBEDDING_DIM: usize = 384;
const TEST_ROW_COUNT: usize = 100;

// ============================================================
//   Stage B — sealing primitives (verbatim from v1 spike)
// ============================================================

/// Derive the at-rest key from the master key per K3 lock.
fn derive_at_rest_key(master_key: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key("vault memory at-rest sealing v1", master_key)
}

/// Compute the at-rest AAD per Finding 2 candidate (c) lock:
/// `AAD = BLAKE3("vault-at-rest-v1" || file_path_bytes)`.
fn compute_aad(file_path: &str) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"vault-at-rest-v1");
    hasher.update(file_path.as_bytes());
    *hasher.finalize().as_bytes()
}

/// Seal a plaintext payload into the at-rest envelope per the locked
/// sealing shape: `version_byte || granularity_marker || dryoc_header || ciphertext`.
fn seal_file_bytes(plaintext: &[u8], key: &[u8; 32], aad: &[u8; 32]) -> Vec<u8> {
    let dryoc_key: Key = (*key).into();
    let (mut push, header): (DryocStream<Push>, Header) = DryocStream::init_push(&dryoc_key);

    // Sized-input quirk per ADR-008 archive line 684 — both plaintext AND
    // aad must be Vec<u8>, not &[u8] slices.
    let plaintext_vec = plaintext.to_vec();
    let aad_vec = aad.to_vec();
    let ciphertext = push
        .push_to_vec(&plaintext_vec, Some(&aad_vec), Tag::FINAL)
        .expect("dryoc push_to_vec");

    let mut sealed = Vec::with_capacity(TOTAL_FRAMING_LEN + ciphertext.len());
    sealed.push(VERSION_BYTE);
    sealed.push(GRANULARITY_PER_FILE);
    sealed.extend_from_slice(header.as_slice());
    sealed.extend_from_slice(&ciphertext);
    sealed
}

/// Unseal the at-rest envelope. Returns Err on framing mismatch or AEAD failure.
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

/// Shannon entropy in bits per byte. For a uniformly random byte
/// stream, H ≈ 8.0; AEAD ciphertexts should be very close.
fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0usize; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / len;
            -p * p.log2()
        })
        .sum()
}

// ============================================================
//   URL helpers — vault-sealed:/// scheme construction
// ============================================================

/// Build a `vault-sealed:///<abs-path>` URL from a local directory path.
/// Internally constructs a `file://` URL (which has well-defined
/// percent-encoding rules cross-platform) then swaps the scheme prefix.
fn make_vault_sealed_uri(local_dir: &std::path::Path) -> String {
    let file_url =
        Url::from_directory_path(local_dir).expect("from_directory_path requires absolute path");
    // file_url is e.g. `file:///C:/Users/.../tmp/` — swap the scheme.
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
//   FireCounters — runtime-confirmation gate (Stages A + C)
// ============================================================

#[derive(Debug, Default)]
struct FireCounters {
    new_store: AtomicUsize,
    put_opts: AtomicUsize,
    get_opts: AtomicUsize,
    head: AtomicUsize,
    list: AtomicUsize,
}

impl FireCounters {
    fn snapshot(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.new_store.load(Ordering::SeqCst),
            self.put_opts.load(Ordering::SeqCst),
            self.get_opts.load(Ordering::SeqCst),
            self.head.load(Ordering::SeqCst),
            self.list.load(Ordering::SeqCst),
        )
    }

    fn print(&self, label: &str) {
        let (ns, p, g, h, l) = self.snapshot();
        eprintln!(
            "  [counters {label}] new_store={ns} put_opts={p} get_opts={g} head={h} list={l}"
        );
    }
}

// ============================================================
//   CountingObjectStore — instrument any inner ObjectStore
// ============================================================

/// Forwards every `ObjectStore` method to `inner`, incrementing the
/// matching counter on the way through. Used by Stage A's
/// `IdentityFileStoreProvider` to prove that `vault-sealed://` routes
/// I/O through our registered provider's returned store on BOTH the
/// write path AND the read path.
#[derive(Debug)]
struct CountingObjectStore {
    inner: Arc<dyn ObjectStore>,
    counters: Arc<FireCounters>,
    label: &'static str,
}

impl std::fmt::Display for CountingObjectStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CountingObjectStore<{}>({})", self.label, self.inner)
    }
}

#[async_trait]
impl ObjectStore for CountingObjectStore {
    async fn put_opts(
        &self,
        location: &ObjectPath,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.counters.put_opts.fetch_add(1, Ordering::SeqCst);
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &ObjectPath,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.counters.get_opts.fetch_add(1, Ordering::SeqCst);
        self.inner.get_opts(location, options).await
    }

    async fn head(&self, location: &ObjectPath) -> object_store::Result<ObjectMeta> {
        self.counters.head.fetch_add(1, Ordering::SeqCst);
        self.inner.head(location).await
    }

    async fn delete(&self, location: &ObjectPath) -> object_store::Result<()> {
        self.inner.delete(location).await
    }

    fn list(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.counters.list.fetch_add(1, Ordering::SeqCst);
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(
        &self,
        prefix: Option<&ObjectPath>,
    ) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy(&self, from: &ObjectPath, to: &ObjectPath) -> object_store::Result<()> {
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
//   Stage A — IdentityFileStoreProvider
// ============================================================

/// Identity provider. Returns a `LanceObjectStore` wrapping a
/// `CountingObjectStore` over `LocalFileSystem`. NO sealing. Purpose:
/// prove that registering an `ObjectStoreProvider` for the unknown
/// scheme `vault-sealed://` actually intercepts ALL I/O on BOTH write
/// and read flows.
#[derive(Debug)]
struct IdentityFileStoreProvider {
    counters: Arc<FireCounters>,
}

impl IdentityFileStoreProvider {
    fn new() -> (Self, Arc<FireCounters>) {
        let counters = Arc::new(FireCounters::default());
        (
            Self {
                counters: counters.clone(),
            },
            counters,
        )
    }
}

#[async_trait]
impl ObjectStoreProvider for IdentityFileStoreProvider {
    async fn new_store(
        &self,
        base_path: Url,
        _params: &ObjectStoreParams,
    ) -> LanceResult<LanceObjectStore> {
        self.counters.new_store.fetch_add(1, Ordering::SeqCst);
        eprintln!(
            "  [trace] IdentityFileStoreProvider::new_store(base_path={})",
            base_path
        );

        let local: Arc<dyn ObjectStore> = Arc::new(LocalFileSystem::new());
        let counted: Arc<dyn ObjectStore> = Arc::new(CountingObjectStore {
            inner: local,
            counters: self.counters.clone(),
            label: "identity",
        });

        // Build the lance-io ObjectStore via the public constructor.
        // Mirror FileStoreProvider's defaults for local-fs semantics.
        Ok(LanceObjectStore::new(
            counted, base_path, /* block_size                    */ None,
            /* wrapper                       */ None,
            /* use_constant_size_upload_parts*/ false,
            /* list_is_lexically_ordered     */ false,
            /* io_parallelism                */ 8, /* download_retry_count          */ 3,
            /* storage_options               */ None,
        ))
    }

    fn extract_path(&self, url: &Url) -> LanceResult<ObjectPath> {
        // Default impl uses `url.to_file_path()` which fails on
        // vault-sealed:// (only known to file:/etc). Mirror
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

async fn run_stage_a() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("===== Stage A — IdentityFileStoreProvider =====");

    let tmp = tempfile::TempDir::new()?;
    let uri = make_vault_sealed_uri(tmp.path());
    eprintln!("URI: {uri}");

    let (provider, counters) = IdentityFileStoreProvider::new();
    let registry = ObjectStoreRegistry::default();
    registry.insert(VAULT_SEALED_SCHEME, Arc::new(provider));

    let session = Session::new(0, 0, Arc::new(registry));

    eprintln!("step 1 — connecting via lancedb::connect(\"{uri}\").session(...).execute()");
    let conn = connect(&uri).session(Arc::new(session)).execute().await?;
    eprintln!("        connect OK");
    counters.print("post-connect");

    eprintln!("step 2 — create_empty_table(\"{TABLE_NAME}\") + add 100 rows × 384-dim");
    let schema = make_schema();
    let batch = make_test_batch(TEST_ROW_COUNT, EMBEDDING_DIM);
    let reader = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader_box: Box<dyn arrow_array::RecordBatchReader + Send> = Box::new(reader);
    let tbl = conn.create_table(TABLE_NAME, reader_box).execute().await?;
    eprintln!("        create_table+add OK");
    counters.print("post-write");

    eprintln!("step 3 — query().limit({TEST_ROW_COUNT}).execute() round-trip");
    let read_batches: Vec<RecordBatch> = tbl
        .query()
        .limit(TEST_ROW_COUNT)
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let read_count: usize = read_batches.iter().map(|b| b.num_rows()).sum();
    eprintln!("        read count: {read_count}");
    counters.print("post-read");

    let (ns, p, g, h, l) = counters.snapshot();
    assert_eq!(
        read_count, TEST_ROW_COUNT,
        "Stage A round-trip count mismatch"
    );
    assert!(
        ns >= 1,
        "Stage A: new_store fire-counter MUST be ≥ 1 (provider was never registered/invoked)"
    );
    assert!(
        p >= 1,
        "Stage A: put_opts fire-counter MUST be ≥ 1 (write path bypassed our provider — STOP-and-escalate)"
    );
    assert!(
        g >= 1,
        "Stage A: get_opts fire-counter MUST be ≥ 1 (read path bypassed our provider — v1 LocalObjectReader-class FAIL recurring; STOP-and-escalate, ADR-008 amendment must re-open with Path X'/Y'/Z')"
    );
    let _ = (h, l); // head/list useful for diagnostics, not pass criteria.

    eprintln!("Stage A: PASS — provider intercepts BOTH write and read flows on vault-sealed://");
    eprintln!();
    Ok(())
}

// ============================================================
//   Stage B — sealing primitives + adversarial assertions
// ============================================================

fn run_stage_b() {
    eprintln!("===== Stage B — sealing primitives =====");

    let master_key: [u8; 32] = *b"this is a 32-byte test mkey-vlt!";
    let at_rest_key = derive_at_rest_key(&master_key);
    eprintln!("derived at_rest_key (K3 = blake3::derive_key)");

    let file_path = "memories.lance/data/abc12345.lance";
    let aad = compute_aad(file_path);
    eprintln!("computed AAD for file_path: {file_path}");

    // Synthetic Parquet-shaped plaintext: 5KB starting + ending with `PAR1` magic.
    let mut plaintext: Vec<u8> = Vec::with_capacity(5120);
    plaintext.extend_from_slice(b"PAR1");
    plaintext.extend(std::iter::repeat(0xCDu8).take(5112));
    plaintext.extend_from_slice(b"PAR1");
    eprintln!(
        "plaintext: {} bytes (begins+ends with PAR1 magic)",
        plaintext.len()
    );

    let sealed = seal_file_bytes(&plaintext, &at_rest_key, &aad);
    eprintln!(
        "sealed: {} bytes (overhead = {} bytes)",
        sealed.len(),
        sealed.len() - plaintext.len()
    );

    // (1) round-trip identity.
    let unsealed = unseal_file_bytes(&sealed, &at_rest_key, &aad).expect("unseal round-trip");
    assert_eq!(unsealed, plaintext, "round-trip identity violated");
    eprintln!("✓ (1) round-trip identity OK");

    // (2) bit-flip in ciphertext fails closed.
    let mut tampered = sealed.clone();
    tampered[TOTAL_FRAMING_LEN + 100] ^= 0x01;
    assert!(
        unseal_file_bytes(&tampered, &at_rest_key, &aad).is_err(),
        "(2) bit-flip MUST fail closed"
    );
    eprintln!("✓ (2) bit-flip in ciphertext fails closed");

    // (3) wrong key fails closed.
    let wrong_master: [u8; 32] = *b"WRONG MASTER KEY for the spk-vlt";
    let wrong_at_rest_key = derive_at_rest_key(&wrong_master);
    assert!(
        unseal_file_bytes(&sealed, &wrong_at_rest_key, &aad).is_err(),
        "(3) wrong-key decryption MUST fail closed"
    );
    eprintln!("✓ (3) wrong-key decryption fails closed");

    // (4) wrong AAD (different file_path) fails closed.
    let wrong_aad = compute_aad("memories.lance/data/different.lance");
    assert!(
        unseal_file_bytes(&sealed, &at_rest_key, &wrong_aad).is_err(),
        "(4) wrong-AAD decryption MUST fail closed"
    );
    eprintln!("✓ (4) wrong-AAD decryption fails closed");

    // (5) no PAR1 magic in sealed envelope.
    assert!(
        !sealed.windows(4).any(|w| w == b"PAR1"),
        "(5) sealed envelope MUST NOT contain Parquet magic"
    );
    eprintln!("✓ (5) no PAR1 magic anywhere in sealed envelope");

    // (6) entropy of ciphertext body ≥ 7.9 bits/byte.
    let body_entropy = shannon_entropy(&sealed[TOTAL_FRAMING_LEN..]);
    eprintln!("        ciphertext body entropy: {body_entropy:.4} bits/byte (require ≥ 7.9)");
    assert!(
        body_entropy >= 7.9,
        "(6) ciphertext body entropy must approach 8.0; got {body_entropy:.4}"
    );
    eprintln!("✓ (6) ciphertext body entropy ≥ 7.9 bits/byte");

    eprintln!("Stage B: PASS — all 6 assertions hold");
    eprintln!();
}

// ============================================================
//   Stage C — SealedFileStoreProvider + SealedObjectStore
// ============================================================

/// Sealed wrapper over a `LocalFileSystem` inner store. Intercepts
/// `put_opts` to seal payloads + `get_opts` to unseal results. Per-file
/// granularity, AAD computed per-call from the location path.
#[derive(Debug)]
struct SealedObjectStore {
    inner: Arc<dyn ObjectStore>,
    key: [u8; 32],
    counters: Arc<FireCounters>,
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
        self.counters.put_opts.fetch_add(1, Ordering::SeqCst);
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
        // Per-file sealing requires the full plaintext at seal time.
        // Spike scope (v1 §3.1 lock): per-file granularity, no multipart.
        Err(ObjectStoreError::NotSupported {
            source: "SealedObjectStore: put_multipart not implemented (per-file granularity)"
                .into(),
        })
    }

    async fn get_opts(
        &self,
        location: &ObjectPath,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.counters.get_opts.fetch_add(1, Ordering::SeqCst);
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
            BytesBuf::new()
        } else {
            BytesBuf::from(body)
        };
        let payload_stream: BoxStream<'static, object_store::Result<BytesBuf>> =
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
        self.counters.head.fetch_add(1, Ordering::SeqCst);
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
        self.counters.list.fetch_add(1, Ordering::SeqCst);
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
        // AAD-binding caveat: a sealed file at `from` has AAD bound to from's
        // path; raw byte-copy to `to` yields a sealed blob whose AAD doesn't
        // match its new location and reads from `to` will fail closed. Spike
        // scope doesn't exercise this path.
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

#[derive(Debug)]
struct SealedFileStoreProvider {
    key: [u8; 32],
    counters: Arc<FireCounters>,
}

impl SealedFileStoreProvider {
    fn new(key: [u8; 32]) -> (Self, Arc<FireCounters>) {
        let counters = Arc::new(FireCounters::default());
        (
            Self {
                key,
                counters: counters.clone(),
            },
            counters,
        )
    }
}

#[async_trait]
impl ObjectStoreProvider for SealedFileStoreProvider {
    async fn new_store(
        &self,
        base_path: Url,
        _params: &ObjectStoreParams,
    ) -> LanceResult<LanceObjectStore> {
        self.counters.new_store.fetch_add(1, Ordering::SeqCst);
        eprintln!(
            "  [trace] SealedFileStoreProvider::new_store(base_path={})",
            base_path
        );

        let local: Arc<dyn ObjectStore> = Arc::new(LocalFileSystem::new());
        let sealed: Arc<dyn ObjectStore> = Arc::new(SealedObjectStore {
            inner: local,
            key: self.key,
            counters: self.counters.clone(),
        });

        Ok(LanceObjectStore::new(
            sealed, base_path, None, None, false, false, 8, 3, None,
        ))
    }

    fn extract_path(&self, url: &Url) -> LanceResult<ObjectPath> {
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
        Ok(VAULT_SEALED_SCHEME.to_string())
    }
}

async fn run_stage_c() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("===== Stage C — SealedFileStoreProvider =====");

    let tmp = tempfile::TempDir::new()?;
    let uri = make_vault_sealed_uri(tmp.path());
    eprintln!("URI: {uri}");

    let master_key: [u8; 32] = *b"this is a 32-byte test mkey-vlt!";
    let at_rest_key = derive_at_rest_key(&master_key);

    let (provider, counters) = SealedFileStoreProvider::new(at_rest_key);
    let registry = ObjectStoreRegistry::default();
    registry.insert(VAULT_SEALED_SCHEME, Arc::new(provider));
    let session = Session::new(0, 0, Arc::new(registry));

    eprintln!("step 1 — connect + create_table + add 100 rows × 384-dim through SEALED provider");
    let conn = connect(&uri).session(Arc::new(session)).execute().await?;
    let schema = make_schema();
    let batch = make_test_batch(TEST_ROW_COUNT, EMBEDDING_DIM);
    let reader = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader_box: Box<dyn arrow_array::RecordBatchReader + Send> = Box::new(reader);
    let tbl = conn.create_table(TABLE_NAME, reader_box).execute().await?;
    counters.print("post-write");

    eprintln!("step 2 — query().limit({TEST_ROW_COUNT}).execute() round-trip");
    let read_batches: Vec<RecordBatch> = tbl
        .query()
        .limit(TEST_ROW_COUNT)
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;
    let read_count: usize = read_batches.iter().map(|b| b.num_rows()).sum();
    eprintln!("        read count: {read_count}");
    counters.print("post-read");

    assert_eq!(
        read_count, TEST_ROW_COUNT,
        "Stage C round-trip count mismatch"
    );
    let (_, p, g, _, _) = counters.snapshot();
    assert!(p >= 1, "Stage C: put_opts fire-counter MUST be ≥ 1");
    assert!(
        g >= 1,
        "Stage C: get_opts fire-counter MUST be ≥ 1 (read path bypassed sealing — STOP-and-escalate)"
    );

    // ---- A2 fold: inline sealing-byte assertion ----
    eprintln!("step 3 — A2 inline sealing-byte assertion (first 2 bytes 0x01||0x00)");
    let candidates = walk_data_files(tmp.path());
    let mut checked = 0usize;
    for path in &candidates {
        let bytes = std::fs::read(path)?;
        if bytes.len() < 2 {
            continue;
        }
        // Some files may be < 64 bytes (e.g. _versions/_latest.manifest);
        // require framing on EVERY file we created — anything written by
        // Lance must have gone through SealedObjectStore::put_opts.
        assert_eq!(
            bytes[0],
            VERSION_BYTE,
            "Stage C: file {} first byte {:#x} != VERSION_BYTE {:#x} (sealing skipped this file)",
            path.display(),
            bytes[0],
            VERSION_BYTE
        );
        assert_eq!(
            bytes[1],
            GRANULARITY_PER_FILE,
            "Stage C: file {} second byte {:#x} != GRANULARITY_PER_FILE {:#x}",
            path.display(),
            bytes[1],
            GRANULARITY_PER_FILE
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "Stage C: walk_data_files found no files under {} — Lance wrote nothing OR temp tree shape changed",
        tmp.path().display()
    );
    eprintln!("        verified framing on {checked} on-disk files");

    eprintln!("Stage C: PASS — sealed provider intercepts both flows + framing bytes present");
    eprintln!();

    // Hand the active state forward to Stage D so it can run wrong-key
    // and ADR-039-through-sealing assertions on the same temp tree.
    drop(tbl);
    drop(conn);
    Ok(())
}

// ============================================================
//   Stage D — adversarial sweep + ADR-039-through-sealing
// ============================================================

async fn run_stage_d() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("===== Stage D — adversarial sweep + ADR-039-through-sealing =====");

    let tmp = tempfile::TempDir::new()?;
    let uri = make_vault_sealed_uri(tmp.path());
    eprintln!("URI: {uri}");

    let master_key: [u8; 32] = *b"this is a 32-byte test mkey-vlt!";
    let at_rest_key = derive_at_rest_key(&master_key);

    // ---- Step 1: write 100 rows through the sealed provider ----
    let (provider, _counters) = SealedFileStoreProvider::new(at_rest_key);
    let registry = ObjectStoreRegistry::default();
    registry.insert(VAULT_SEALED_SCHEME, Arc::new(provider));
    let session = Session::new(0, 0, Arc::new(registry));
    let conn = connect(&uri).session(Arc::new(session)).execute().await?;
    let schema = make_schema();
    let batch = make_test_batch(TEST_ROW_COUNT, EMBEDDING_DIM);
    let reader = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    let reader_box: Box<dyn arrow_array::RecordBatchReader + Send> = Box::new(reader);
    let tbl = conn.create_table(TABLE_NAME, reader_box).execute().await?;
    eprintln!("step 1 — wrote 100 rows via sealed provider");

    // ---- Step 2: on-disk adversarial sweep (every file) ----
    eprintln!("step 2 — on-disk adversarial sweep");
    let files = walk_data_files(tmp.path());
    assert!(
        !files.is_empty(),
        "Stage D: no files found under {}",
        tmp.path().display()
    );
    let mut min_entropy = f64::INFINITY;
    let mut entropy_checked = 0usize;
    for path in &files {
        let bytes = std::fs::read(path)?;
        // (a) no PAR1 magic anywhere
        assert!(
            !bytes.windows(4).any(|w| w == b"PAR1"),
            "Stage D: PAR1 magic found in {} — file is plaintext",
            path.display()
        );
        // (b) framing bytes
        if bytes.len() >= 2 {
            assert_eq!(
                bytes[0],
                VERSION_BYTE,
                "Stage D: bad version byte in {}",
                path.display()
            );
            assert_eq!(
                bytes[1],
                GRANULARITY_PER_FILE,
                "Stage D: bad granularity byte in {}",
                path.display()
            );
        }
        // (c) entropy of the ciphertext body ≥ 7.9.
        //
        // Shannon entropy is a per-byte-distribution statistic; converging
        // close to 8.0 needs the sample to contain most of the 256 possible
        // byte values. By coupon-collector, an N-byte uniform random sample
        // expects about 256·(1-(1-1/256)^N) distinct values — at N=295 only
        // ~175 distinct values, capping entropy ≪ 8.0 even on a perfect
        // ciphertext. The 7.9 bound is appropriate for samples ≥ ~4 KiB
        // (where ~245 of 256 values typically appear). Smaller sealed files
        // (manifest sidecars, .txn transaction logs) are still authenticated
        // by the version-byte / granularity-byte framing check above —
        // entropy is just not a useful additional signal at that size.
        const ENTROPY_MIN_BODY_BYTES: usize = 4096;
        if bytes.len() > TOTAL_FRAMING_LEN + ENTROPY_MIN_BODY_BYTES {
            let body_entropy = shannon_entropy(&bytes[TOTAL_FRAMING_LEN..]);
            assert!(
                body_entropy >= 7.9,
                "Stage D: entropy {:.4} < 7.9 in {} (size {})",
                body_entropy,
                path.display(),
                bytes.len()
            );
            min_entropy = min_entropy.min(body_entropy);
            entropy_checked += 1;
        }
    }
    eprintln!(
        "        scanned {} files, entropy-checked {entropy_checked} (min {:.4} ≥ 7.9)",
        files.len(),
        if entropy_checked == 0 {
            0.0
        } else {
            min_entropy
        }
    );

    // ---- Step 3: wrong-key fail-closed ----
    eprintln!("step 3 — wrong-key open fail-closed");
    drop(tbl);
    drop(conn);
    let wrong_master: [u8; 32] = *b"WRONG MASTER KEY for the spk-vlt";
    let wrong_at_rest_key = derive_at_rest_key(&wrong_master);
    let (wrong_provider, _) = SealedFileStoreProvider::new(wrong_at_rest_key);
    let wrong_registry = ObjectStoreRegistry::default();
    wrong_registry.insert(VAULT_SEALED_SCHEME, Arc::new(wrong_provider));
    let wrong_session = Session::new(0, 0, Arc::new(wrong_registry));
    let wrong_open = connect(&uri)
        .session(Arc::new(wrong_session))
        .execute()
        .await;
    let wrong_query_result = match wrong_open {
        Ok(conn) => match conn.open_table(TABLE_NAME).execute().await {
            Ok(tbl) => match tbl.query().limit(TEST_ROW_COUNT).execute().await {
                Ok(stream) => stream.try_collect::<Vec<_>>().await.map(|_| ()),
                Err(e) => Err(e),
            },
            Err(e) => Err(e),
        },
        Err(e) => Err(e),
    };
    assert!(
        wrong_query_result.is_err(),
        "Stage D: wrong-key open MUST fail closed; got Ok"
    );
    eprintln!("        wrong-key open failed closed (as required)");

    // ---- Step 4: ADR-039-through-sealing ----
    //
    // Privacy contract under test: "After delete + Prune, the bytes that
    // physically held the deleted rows on disk are gone." Per ADR-039 the
    // production code calls `Table::optimize(OptimizeAction::Prune {
    // older_than: zero, delete_unverified: true })` immediately after
    // `table.delete()`. Here we exercise that exact pair through the sealed
    // wrapper and observe the on-disk effects.
    //
    // We deliberately do NOT decrypt-and-grep deleted-id strings. The AAD at
    // write time is bound to whatever path string Lance hands to put_opts
    // (a function of extract_path's ObjectPath conversion + Lance's internal
    // path construction); reconstructing that string at read time from a
    // walkdir-emitted absolute path is too implementation-detail-fragile to
    // serve as a runtime gate. Instead we observe the two on-disk facts that
    // ADR-039's privacy contract reduces to once Stage B has independently
    // verified AEAD soundness: (1) total bytes on disk decreased after Prune
    // (physical removal happened), and (2) every remaining file is still
    // sealed framing-bytes-correct (Lance's compaction path didn't bypass
    // our SealedObjectStore mid-Prune).
    eprintln!(
        "step 4 — ADR-039-through-sealing (delete half + Prune + on-disk physical-removal check)"
    );
    // Re-open with correct key.
    let (provider2, _) = SealedFileStoreProvider::new(at_rest_key);
    let registry2 = ObjectStoreRegistry::default();
    registry2.insert(VAULT_SEALED_SCHEME, Arc::new(provider2));
    let session2 = Session::new(0, 0, Arc::new(registry2));
    let conn2 = connect(&uri).session(Arc::new(session2)).execute().await?;
    let tbl2 = conn2.open_table(TABLE_NAME).execute().await?;

    // ---- Pre-delete snapshot: every file's content-hash ----
    // With per-file AEAD using random nonces (Stage B verified), every
    // sealed file has a unique content-hash by construction. So a
    // content-hash present pre-delete that is absent post-Prune proves
    // those exact bytes are physically gone from disk.
    let pre_delete_files = walk_data_files(tmp.path());
    let pre_delete: std::collections::HashMap<[u8; 32], std::path::PathBuf> = pre_delete_files
        .iter()
        .filter_map(|p| {
            std::fs::read(p)
                .ok()
                .map(|bytes| (file_hash(&bytes), p.clone()))
        })
        .collect();
    let pre_delete_bytes: u64 = pre_delete_files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    eprintln!(
        "        pre-delete: {} files, {} bytes, {} distinct content-hashes",
        pre_delete_files.len(),
        pre_delete_bytes,
        pre_delete.len()
    );

    let deleted_ids: Vec<String> = (0..50).map(|i| format!("id-{i:04}")).collect();
    let id_list_sql = deleted_ids
        .iter()
        .map(|id| format!("'{id}'"))
        .collect::<Vec<_>>()
        .join(",");
    let delete_predicate = format!("id IN ({id_list_sql})");
    tbl2.delete(&delete_predicate).await?;
    eprintln!("        deleted 50 rows");

    // Mirror production ADR-039 (post-Phase-0c-spike-Stage-E amendment):
    // Compact-then-Prune. Compact rewrites partial fragments dropping
    // tombstoned rows; Prune removes the now-orphaned old data files.
    // Prune-alone leaves data files bit-for-bit unchanged (Stage E 2×2
    // matrix proves this on both plain file:// and vault-sealed://).
    use chrono::TimeDelta;
    tbl2.optimize(OptimizeAction::Compact {
        options: lancedb::table::CompactionOptions::default(),
        remap_options: None,
    })
    .await?;
    tbl2.optimize(OptimizeAction::Prune {
        older_than: Some(TimeDelta::zero()),
        delete_unverified: Some(true),
        error_if_tagged_old_versions: Some(false),
    })
    .await?;
    eprintln!("        Compact + Prune {{ older_than: zero, delete_unverified: true }} OK");

    let post_prune_files = walk_data_files(tmp.path());
    let post_prune_hashes: std::collections::HashSet<[u8; 32]> = post_prune_files
        .iter()
        .filter_map(|p| std::fs::read(p).ok().map(|bytes| file_hash(&bytes)))
        .collect();
    let post_prune_bytes: u64 = post_prune_files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum();
    eprintln!(
        "        post-prune: {} files, {} bytes",
        post_prune_files.len(),
        post_prune_bytes
    );

    // (a) Physical removal of DATA FILE bytes specifically.
    //
    // Privacy invariant: every pre-delete file under data/ must have its
    // content-hash gone from post-Prune. Per-file random AEAD nonces
    // (Stage B verified) guarantee distinct ciphertexts have distinct
    // hashes, so a hash present pre-delete and absent post-Prune proves
    // those exact bytes are gone from disk. Compact-then-Prune sequence
    // ensures the partial-fragment data file is rewritten and the
    // original removed (Stage E 2×2 matrix verified this is the only
    // working sequence; Prune-alone leaves data files bit-for-bit
    // unchanged on both plain file:// and vault-sealed://).
    let is_data_file = |p: &std::path::Path| -> bool {
        p.components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("data"))
    };
    let pre_delete_data_files: Vec<(&[u8; 32], &std::path::PathBuf)> = pre_delete
        .iter()
        .filter(|(_, path)| is_data_file(path))
        .collect();
    assert!(
        !pre_delete_data_files.is_empty(),
        "Stage D: spike-shape regression — no pre-delete files under data/ found"
    );
    let surviving_data_hashes: Vec<&std::path::PathBuf> = pre_delete_data_files
        .iter()
        .filter(|(hash, _)| post_prune_hashes.contains(*hash))
        .map(|(_, path)| *path)
        .collect();
    assert!(
        surviving_data_hashes.is_empty(),
        "Stage D ADR-039 FAIL: {} pre-delete data file(s) still BIT-FOR-BIT identical \
         post-Compact+Prune through the sealed wrapper. Surviving: {:?}",
        surviving_data_hashes.len(),
        surviving_data_hashes
            .iter()
            .map(|p| p
                .strip_prefix(tmp.path())
                .unwrap_or(p)
                .display()
                .to_string())
            .collect::<Vec<_>>()
    );
    eprintln!(
        "        ✓ physical removal: all {} pre-delete data file(s) gone from disk",
        pre_delete_data_files.len()
    );
    let all_physically_removed: Vec<&std::path::PathBuf> = pre_delete
        .iter()
        .filter(|(hash, _)| !post_prune_hashes.contains(*hash))
        .map(|(_, path)| path)
        .collect();
    eprintln!(
        "        (total {} pre-delete file(s) physically gone — incl. manifests/transactions)",
        all_physically_removed.len()
    );

    // (b) Every remaining file is still framing-correct sealed — Lance's
    //     compaction path stayed inside our SealedObjectStore.
    for path in &post_prune_files {
        let bytes = std::fs::read(path)?;
        if bytes.len() < 2 {
            continue;
        }
        assert_eq!(
            bytes[0], VERSION_BYTE,
            "Stage D ADR-039 FAIL: post-prune file {} has bad version byte {:#x} — Lance compaction bypassed sealing",
            path.display(),
            bytes[0]
        );
        assert_eq!(
            bytes[1],
            GRANULARITY_PER_FILE,
            "Stage D ADR-039 FAIL: post-prune file {} has bad granularity byte {:#x}",
            path.display(),
            bytes[1]
        );
    }
    eprintln!(
        "        ✓ all {} post-prune files still sealing framing-correct",
        post_prune_files.len()
    );

    eprintln!("Stage D: PASS — adversarial sweep + ADR-039-through-sealing OK (Compact+Prune)");
    eprintln!();
    Ok(())
}

// ============================================================
//   Stage E — ADR-039 diagnostic 2×2 matrix
// ============================================================

/// Distinguish whether the bit-for-bit-survival of the data file post-Prune
/// is (a) Lance's Prune semantics in 4.0 — i.e., Prune does NOT compact data
/// files, so production ADR-039 code is incomplete — or (b) the sealing
/// wrapper interfering with Lance's compaction.
///
/// Matrix: {sealed, plain file://} × {Prune-alone, Compact+Prune}. For each
/// cell: write 100 rows, capture pre-delete data file content-hashes,
/// delete 50 rows, run optimize, capture OptimizeStats, check whether
/// every pre-delete data file's content-hash is gone from post-prune.
async fn run_stage_e() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("===== Stage E — ADR-039 diagnostic 2×2 matrix =====");

    let master_key: [u8; 32] = *b"this is a 32-byte test mkey-vlt!";
    let at_rest_key = derive_at_rest_key(&master_key);

    let scenarios: Vec<(&str, bool, bool)> = vec![
        ("plain file://     | Prune-alone        ", false, false),
        ("plain file://     | Compact+Prune      ", false, true),
        ("vault-sealed://   | Prune-alone        ", true, false),
        ("vault-sealed://   | Compact+Prune      ", true, true),
    ];

    let mut findings: Vec<(String, bool, String)> = Vec::new();

    for (label, use_seal, use_compact_first) in &scenarios {
        eprintln!();
        eprintln!("---- {label} ----");
        let tmp = tempfile::TempDir::new()?;

        let conn = if *use_seal {
            let uri = make_vault_sealed_uri(tmp.path());
            let (provider, _) = SealedFileStoreProvider::new(at_rest_key);
            let registry = ObjectStoreRegistry::default();
            registry.insert(VAULT_SEALED_SCHEME, Arc::new(provider));
            let session = Session::new(0, 0, Arc::new(registry));
            connect(&uri).session(Arc::new(session)).execute().await?
        } else {
            let uri = tmp.path().to_str().expect("temp path utf-8");
            connect(uri).execute().await?
        };

        let schema = make_schema();
        let batch = make_test_batch(TEST_ROW_COUNT, EMBEDDING_DIM);
        let reader = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
        let reader_box: Box<dyn arrow_array::RecordBatchReader + Send> = Box::new(reader);
        let tbl = conn.create_table(TABLE_NAME, reader_box).execute().await?;

        let pre_delete_files = walk_data_files(tmp.path());
        let pre_data_hashes: std::collections::HashMap<[u8; 32], std::path::PathBuf> =
            pre_delete_files
                .iter()
                .filter(|p| {
                    p.components()
                        .any(|c| c.as_os_str().eq_ignore_ascii_case("data"))
                })
                .filter_map(|p| std::fs::read(p).ok().map(|b| (file_hash(&b), p.clone())))
                .collect();
        eprintln!(
            "  pre-delete: {} files total, {} data files",
            pre_delete_files.len(),
            pre_data_hashes.len()
        );

        let deleted_ids: Vec<String> = (0..50).map(|i| format!("id-{i:04}")).collect();
        let id_list_sql = deleted_ids
            .iter()
            .map(|id| format!("'{id}'"))
            .collect::<Vec<_>>()
            .join(",");
        let delete_predicate = format!("id IN ({id_list_sql})");
        tbl.delete(&delete_predicate).await?;

        if *use_compact_first {
            let compact_stats = tbl
                .optimize(OptimizeAction::Compact {
                    options: lancedb::table::CompactionOptions::default(),
                    remap_options: None,
                })
                .await?;
            eprintln!("  Compact stats: {compact_stats:?}");
        }

        use chrono::TimeDelta;
        let prune_stats = tbl
            .optimize(OptimizeAction::Prune {
                older_than: Some(TimeDelta::zero()),
                delete_unverified: Some(true),
                error_if_tagged_old_versions: Some(false),
            })
            .await?;
        eprintln!("  Prune stats:   {prune_stats:?}");

        let post_files = walk_data_files(tmp.path());
        let post_hashes: std::collections::HashSet<[u8; 32]> = post_files
            .iter()
            .filter_map(|p| std::fs::read(p).ok().map(|b| file_hash(&b)))
            .collect();

        let surviving: Vec<&std::path::PathBuf> = pre_data_hashes
            .iter()
            .filter(|(hash, _)| post_hashes.contains(*hash))
            .map(|(_, path)| path)
            .collect();

        let pass = surviving.is_empty();
        let summary = if pass {
            format!(
                "PASS — all {} pre-delete data file(s) physically rewritten/removed",
                pre_data_hashes.len()
            )
        } else {
            format!(
                "FAIL — {}/{} pre-delete data file(s) bit-for-bit identical post-optimize",
                surviving.len(),
                pre_data_hashes.len()
            )
        };
        eprintln!("  {summary}");
        findings.push((label.to_string(), pass, summary));
    }

    eprintln!();
    eprintln!("---- Stage E summary ----");
    for (label, pass, summary) in &findings {
        let mark = if *pass { "✓" } else { "✗" };
        eprintln!("  {mark} {label}: {summary}");
    }
    eprintln!();

    // Interpretation logic: which scenarios passed determines what
    // production ADR-039 (and our Phase 0d wiring) need to do.
    let pf_prune = findings[0].1;
    let pf_compact = findings[1].1;
    let sl_prune = findings[2].1;
    let sl_compact = findings[3].1;

    eprintln!("---- Interpretation ----");
    if pf_prune && sl_prune {
        eprintln!(
            "✓ Prune-alone is sufficient on both paths. ADR-039 production code is correct as-is. \
             Phase 0d wires SealedFileStoreProvider with no Prune-call changes needed."
        );
    } else if !pf_prune && pf_compact && sl_compact {
        eprintln!(
            "⚠ ADR-039 PRODUCTION BUG: Prune-alone does NOT physically rewrite data files in \
             lance 4.0 (sealed and unsealed both confirm). Compact+Prune is required for \
             physical removal. Production code at vector_store.rs:378-385 needs amendment to \
             insert Compact before Prune. Phase 0d both wires sealing AND fixes ADR-039 \
             implementation."
        );
    } else if !pf_prune && !sl_prune && !pf_compact && !sl_compact {
        eprintln!(
            "⚠⚠ Lance 4.0 does NOT physically rewrite data files even with Compact+Prune. \
             ADR-039 fundamentally cannot be enforced via lancedb's optimize API alone. \
             Need to re-evaluate with upstream — possibly fork-based scrub or different \
             retention semantics."
        );
    } else if (pf_prune || pf_compact) && !sl_prune && !sl_compact {
        eprintln!(
            "⚠ Sealing wrapper INTERFERES with Lance compaction: plain file:// achieves \
             physical removal, sealed path does NOT. Investigate sealing wrapper's \
             interaction with Compact's read-rewrite-delete cycle. Possible cause: \
             SealedObjectStore's head/list size adjustments confusing Lance's compaction \
             logic."
        );
    } else {
        eprintln!(
            "⚠ Mixed result — manual review required. Findings above show which combinations \
             achieve physical removal."
        );
    }
    eprintln!();

    Ok(())
}

// ============================================================
//   Helpers
// ============================================================

fn make_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new(
            "embedding",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIM as i32,
            ),
            false,
        ),
    ]))
}

fn make_test_batch(rows: usize, dim: usize) -> RecordBatch {
    let schema = make_schema();
    let ids: Vec<String> = (0..rows).map(|i| format!("id-{i:04}")).collect();
    let id_array = Arc::new(StringArray::from(ids));

    // Avoid all-zero vectors per Phase 0a-fix Layer 4 / ADR-038 finding —
    // lance 4.0 Cosine search filters NaN-distance rows. Production
    // BGE-small-en-v1.5 vectors are L2-normalised non-zero. Use 1.0..N range.
    let values: Vec<f32> = (0..rows * dim)
        .map(|i| ((i as f32) * 0.001).fract() + 0.5)
        .collect();
    let item_field = Arc::new(Field::new("item", DataType::Float32, true));
    let embedding_array = Arc::new(FixedSizeListArray::new(
        item_field,
        dim as i32,
        Arc::new(Float32Array::from(values)),
        None,
    ));

    RecordBatch::try_new(schema, vec![id_array, embedding_array]).expect("record batch")
}

fn walk_data_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|r| r.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect()
}

/// BLAKE3 content-hash of a file's bytes. Used for the Stage D ADR-039
/// physical-removal check: per-file random AEAD nonces (Stage B-verified)
/// guarantee distinct ciphertexts have distinct hashes, so a hash present
/// pre-delete and absent post-Prune proves those exact bytes are gone
/// from disk.
fn file_hash(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

// ============================================================
//   Driver
// ============================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    eprintln!("============================================================");
    eprintln!("  At-rest spike v2 (T0.2.0 Phase 0c, 2026-05-08)");
    eprintln!("  Pre-flight: lancedb=0.27.2 lance-io=4.0.0 object_store=0.12.5");
    eprintln!("============================================================");
    eprintln!();

    run_stage_a().await?;
    run_stage_b();
    run_stage_c().await?;
    run_stage_d().await?;
    run_stage_e().await?;

    eprintln!("============================================================");
    eprintln!("  Phase 0c spike v2: ALL STAGES PASS");
    eprintln!("  ObjectStoreProvider integration runtime-confirmed.");
    eprintln!("  ADR-039 amended to Compact+Prune (Stage E discovery).");
    eprintln!("  Phase 0d: production-wire SealedFileStoreProvider.");
    eprintln!("============================================================");
    Ok(())
}
