//! T0.2.0 BRD §6 acceptance suite — sub-task (f).
//!
//! Five tests, one per BRD §6 T0.2.0 acceptance criterion
//! (Agent_Build_Specification.txt lines 1418-1422):
//!
//! (a) Vector data dir contains no plaintext Parquet on disk after a
//!     write/close cycle — entropy ≥ `ENTROPY_FLOOR_BITS_PER_BYTE` (for
//!     files ≥ `ENTROPY_MIN_FILE_SIZE`) and `b"PAR1"` absent from every
//!     byte window. See the two const doc-comments for the empirical
//!     calibration source.
//! (b) Round-trip identity — `encrypt → decrypt == original` across the
//!     CI matrix (`[ubuntu-latest, windows-latest, macos-latest]`).
//! (c) Decryption with wrong key fails closed — no silent-empty,
//!     no information leakage in the surfaced error.
//! (d) All four ADR-010 compensating controls removed from the codebase
//!     — Block A grep-test (four verbatim control strings absent) +
//!     Block B grep-test (five plaintext-API symbols absent). Bundled
//!     into one test per the T0.2.0 close-out iteration 4 §9 (f) lock.
//! (e) Tampered ciphertext is detected — bit-flip a sealed byte mid-
//!     ciphertext; assert `VaultError::Storage(_)` with substring
//!     `"AEAD authentication failed"` (per the γ tightening of the
//!     unseal-site wrapping in `sealed_object_store.rs`).
//!
//! These tests live at integration tier (not module tests) because the
//! BRD §6 acceptance contract is the build-level invariant that gates
//! T0.2.16 alpha-cohort distribution — module-level Phase 0d tests
//! remain in `vector_store.rs` as defence-in-depth tightening pins.

use std::path::{Path, PathBuf};

use tempfile::TempDir;
use vault_core::{Boundary, MemoryId, VaultError};
use vault_storage::{LanceVectorStore, VectorStore};
use walkdir::WalkDir;

// ============================================================
//   Test helpers
// ============================================================

/// K3 KDF context-string. Pinned production value per ADR-008 amendment
/// (Phase 0e). The canonical production derivation site is
/// `vault_app::keychain::derive_at_rest_key`; we replicate the call shape
/// here to model the production caller flow without depending on
/// vault-app from a vault-storage integration test.
const K3_CONTEXT: &str = "vault memory at-rest sealing v1";

/// Sealing envelope framing length: `VERSION_BYTE (1) ||
/// GRANULARITY_PER_FILE (1) || dryoc_header (24) = 26 bytes`. Mirrors
/// the `pub(crate) TOTAL_FRAMING_LEN` constant in `sealed_object_store.rs`;
/// duplicated here because the const is not part of the public API and
/// integration tests cannot reach `pub(crate)` items.
const SEAL_FRAMING_LEN: usize = 26;

/// Entropy floor for criterion (a). Calibrated against an empirical
/// 952-byte Lance fragment file (sealed AEAD ciphertext) scoring 7.7709
/// bits/byte under the plug-in Shannon-entropy estimator. The plug-in
/// estimator is biased downward on small samples by approximately
/// `E[H] ≈ 8.0 - 184/N` bits/byte for N truly-random samples. 7.5
/// sits cleanly between gzip-compressed worst-case (~7.3) and the
/// small-AEAD-with-finite-sample-bias floor (~7.6-7.8), preserving the
/// privacy property "no plaintext on disk" across the full file-size
/// distribution Lance produces. See (f) commit message for the
/// runtime-confirmation rationale.
const ENTROPY_FLOOR_BITS_PER_BYTE: f64 = 7.5;

/// Minimum file size to apply the entropy floor. Below this, the
/// plug-in estimator's finite-sample bias dominates and the measurement
/// stops being meaningful — so we fall back to PAR1-magic-absence as
/// the sole on-disk plaintext indicator for very small files.
const ENTROPY_MIN_FILE_SIZE: usize = 512;

/// Derive the at-rest key the way production does: K3 BLAKE3 over the
/// master_key with the locked context-string.
fn derive_at_rest_key_test(master_key: &[u8; 32]) -> [u8; 32] {
    blake3::derive_key(K3_CONTEXT, master_key)
}

/// Returns the unit vector along axis `i % 4` in 4-D space — orthogonal,
/// unambiguous exact-match top-hit under cosine distance. See Phase 0d's
/// `sealed_open_round_trip_returns_inserted_rows` rationale.
fn orthogonal_unit_4d(i: usize) -> [f32; 4] {
    let axis = i % 4;
    let mut v = [0.0f32; 4];
    v[axis] = 1.0;
    v
}

/// Recursive walk: returns every regular file under `dir`.
fn walk_every_file(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect()
}

/// Shannon entropy in bits per byte. Result is in `[0.0, 8.0]`; uniform
/// random data approaches 8.0 — the AEAD ciphertext the sealed path
/// produces sits at or above `ENTROPY_FLOOR_BITS_PER_BYTE` for files of
/// at least `ENTROPY_MIN_FILE_SIZE` bytes. The plug-in estimator implemented
/// here is biased downward on small samples per the const doc-comments.
fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let len = bytes.len() as f64;
    let mut entropy = 0.0;
    for &c in &counts {
        if c == 0 {
            continue;
        }
        let p = c as f64 / len;
        entropy -= p * p.log2();
    }
    entropy
}

/// Locate one `.lance` fragment data file under `<tmp>/<table>.lance/data/`.
/// Returns the path to the largest such file (deterministic when only one
/// fragment was written; documented in the empirical-check note on the
/// iteration). Used by criterion (e) for the byte-flip target.
fn find_largest_lance_fragment(tmp_dir: &Path) -> Option<PathBuf> {
    walk_every_file(tmp_dir)
        .into_iter()
        .filter(|p| {
            // path components include a "data" segment AND the file
            // extension is ".lance"
            let has_data = p
                .components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case("data"));
            let is_lance = p.extension().and_then(|e| e.to_str()) == Some("lance");
            has_data && is_lance
        })
        .max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
}

/// Locate the workspace root by walking up from `CARGO_MANIFEST_DIR`.
/// `crates/vault-storage` → `crates` → workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .expect("workspace root: CARGO_MANIFEST_DIR has fewer than 2 parents")
}

// ============================================================
//   Criterion (a) — no plaintext on disk
// ============================================================

#[tokio::test]
async fn acceptance_a_no_plaintext_on_disk_after_write_close() {
    let tmp = TempDir::new().unwrap();
    let master: [u8; 32] = *b"acceptance-a-master-key-32-bytes";
    let at_rest = derive_at_rest_key_test(&master);

    let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest)
        .await
        .unwrap();
    let boundary = Boundary::new("acceptance_a").unwrap();
    for i in 0..10 {
        store
            .upsert(&MemoryId::new(), &orthogonal_unit_4d(i), &boundary)
            .await
            .unwrap();
    }
    // Drop store: release any internal buffering before walking disk.
    // Lance flushes on table drop.
    drop(store);

    let files = walk_every_file(tmp.path());
    assert!(
        !files.is_empty(),
        "no files written under {} — Lance wrote nothing or temp tree shape changed",
        tmp.path().display()
    );

    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        if bytes.is_empty() {
            continue;
        }

        // PAR1 magic absent in every byte window. The strongest single
        // signal that no plaintext parquet was written through any
        // sealing-bypass path.
        assert!(
            !bytes.windows(4).any(|w| w == b"PAR1"),
            "file {} contains plaintext PAR1 magic — sealing was bypassed \
             mid-write or a code path is emitting plaintext through the \
             sealed connection",
            path.display()
        );

        // Entropy floor: only meaningful for files large enough to give
        // a stable distribution. See ENTROPY_FLOOR_BITS_PER_BYTE +
        // ENTROPY_MIN_FILE_SIZE doc-comments for the empirical calibration
        // and finite-sample-bias rationale.
        if bytes.len() >= ENTROPY_MIN_FILE_SIZE {
            let h = shannon_entropy(&bytes);
            assert!(
                h >= ENTROPY_FLOOR_BITS_PER_BYTE,
                "file {} entropy {:.4} bits/byte < {:.1} — plaintext or \
                 highly-redundant payload detected through the sealed path \
                 (file size {} bytes)",
                path.display(),
                h,
                ENTROPY_FLOOR_BITS_PER_BYTE,
                bytes.len()
            );
        }
    }
}

// ============================================================
//   Criterion (b) — round-trip identity
// ============================================================

#[tokio::test]
async fn acceptance_b_round_trip_identity_encrypt_decrypt_equals_original() {
    let tmp = TempDir::new().unwrap();
    let master: [u8; 32] = *b"acceptance-b-master-key-32-bytes";
    let at_rest = derive_at_rest_key_test(&master);

    let boundary = Boundary::new("acceptance_b").unwrap();
    let mut ids: Vec<MemoryId> = Vec::new();
    let mut vectors: Vec<[f32; 4]> = Vec::new();

    // Write phase: open, upsert 4 orthogonal-unit vectors, drop.
    {
        let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest)
            .await
            .unwrap();
        for i in 0..4 {
            let id = MemoryId::new();
            let v = orthogonal_unit_4d(i);
            store.upsert(&id, &v, &boundary).await.unwrap();
            ids.push(id);
            vectors.push(v);
        }
        assert_eq!(
            store.count(None).await.unwrap(),
            4,
            "pre-drop count must equal upsert count"
        );
    } // store dropped — flushes pending writes through SealedObjectStore::put_opts

    // Read phase: reopen with same key, verify all rows round-trip.
    let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest)
        .await
        .unwrap();

    assert_eq!(
        store.count(None).await.unwrap(),
        4,
        "post-reopen count must equal pre-drop count — sealed→unsealed identity at row-count level"
    );

    for (id, v) in ids.iter().zip(vectors.iter()) {
        let hits = store
            .search(v, 4, std::slice::from_ref(&boundary))
            .await
            .unwrap();
        assert!(
            !hits.is_empty(),
            "exact-match search after reopen returned no hits for id {id}"
        );
        assert_eq!(
            hits[0].0, *id,
            "round-trip identity FAIL: top-hit for vector that exactly matches \
             id {id} returned a different id {}; encrypt→decrypt did not round-\
             trip the vector data",
            hits[0].0
        );
    }
}

// ============================================================
//   Criterion (c) — wrong key fails closed, no info leak
// ============================================================

#[tokio::test]
async fn acceptance_c_wrong_key_open_fails_closed_with_generic_message() {
    let tmp = TempDir::new().unwrap();
    let master_correct: [u8; 32] = *b"acceptance-c-CORRECT-master-32by";
    let master_wrong: [u8; 32] = *b"acceptance-c-WRONG-master-32-byt";
    let key_correct = derive_at_rest_key_test(&master_correct);
    let key_wrong = derive_at_rest_key_test(&master_wrong);

    let boundary = Boundary::new("acceptance_c_secret_boundary").unwrap();
    let secret_ids: Vec<MemoryId> = (0..3).map(|_| MemoryId::new()).collect();

    // Write with K1
    {
        let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &key_correct)
            .await
            .unwrap();
        for (i, id) in secret_ids.iter().enumerate() {
            store
                .upsert(id, &orthogonal_unit_4d(i), &boundary)
                .await
                .unwrap();
        }
    }

    // Reopen with K2: must fail at open OR at first read. Silent success
    // is a privacy-contract violation.
    let reopen = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &key_wrong).await;
    let outcome: Result<(), VaultError> = match reopen {
        Err(e) => Err(e),
        Ok(store) => match store.count(None).await {
            Err(e) => Err(e),
            Ok(_) => Ok(()),
        },
    };
    let err = outcome.expect_err(
        "wrong-key reopen MUST fail closed — silent success means the AEAD \
         authentication is not enforcing per-file binding",
    );

    let msg = err.to_string();

    // Information-leakage check: the error message must not contain any
    // memory id, boundary name, or other secret-bearing artefact from the
    // K1-protected data. The data_dir path is NOT a leak: it was supplied
    // by the caller, who already knows it.
    for id in &secret_ids {
        let id_str = id.to_string();
        assert!(
            !msg.contains(&id_str),
            "wrong-key error leaks memory id {id_str} into the surfaced \
             message — privacy contract violated. Full message: {msg}"
        );
    }
    assert!(
        !msg.contains(boundary.as_str()),
        "wrong-key error leaks boundary name {:?} into the surfaced \
         message — privacy contract violated. Full message: {msg}",
        boundary.as_str()
    );
}

// ============================================================
//   Criterion (d) — four ADR-010 controls + plaintext-API symbols absent
// ============================================================

#[test]
fn acceptance_d_four_adr_010_controls_and_plaintext_api_symbols_absent() {
    // Block A — four ADR-010 compensating-control verbatim strings recovered
    // from d556b97^ (sub-task (e)'s parent, the last commit that contained
    // them). Locked in T0.2.0 Phase 3 sub-task (f) plan iteration #1.
    //
    // Sources (pre-(e)):
    //   #1 modal banner h2:    crates/vault-tauri/dist/index.html:176
    //   #2 persistent strip:   crates/vault-tauri/dist/index.html:187
    //   #3 plaintext WARN log: crates/vault-storage/src/vector_store.rs:313-316
    //   #4 ALPHA filename:     crates/vault-storage/src/vector_store.rs:83 (const value)
    let block_a: &[(&str, &str)] = &[
        (
            "control_1_modal_banner_h2",
            "ALPHA BUILD \u{2014} Read before continuing",
        ),
        (
            "control_2_persistent_strip",
            "ALPHA \u{2014} vector store is unencrypted. V0.2 fixes this.",
        ),
        (
            "control_3_plaintext_warn_log",
            "LanceDB data dir is plaintext (V0.1 alpha \u{2014} see ADR-010). Encryption layer ships in T0.2.0.",
        ),
        (
            "control_4_alpha_filename_value",
            "ALPHA_DO_NOT_STORE_REAL_DATA.txt",
        ),
    ];

    // Block B — plaintext-API symbols that V0.1 exposed and (e) deleted.
    // Patterns include the literal open-paren where applicable to prevent
    // a future `::open_compat(` / `::open_legacy(` from silently passing
    // a prefix-only substring search. The const/fn-name patterns are
    // already unique enough not to need a paren.
    let block_b: &[(&str, &str)] = &[
        ("plaintext_LanceVectorStore_open", "LanceVectorStore::open("),
        ("plaintext_StorageBackend_open", "StorageBackend::open("),
        ("ALPHA_WARNING_FILENAME_const", "ALPHA_WARNING_FILENAME"),
        (
            "format_migration_error_dialog_fn",
            "format_migration_error_dialog",
        ),
        ("acknowledge_alpha_banner_cmd", "acknowledge_alpha_banner"),
    ];

    let crates_dir = workspace_root().join("crates");
    assert!(
        crates_dir.is_dir(),
        "expected crates/ directory at workspace root {}",
        crates_dir.display()
    );

    // Canonical path of this very test file — must be excluded from the
    // walk, otherwise the test would self-fail on its own fixture data.
    let self_path = workspace_root().join("crates/vault-storage/tests/t0_2_0_acceptance.rs");
    let canonical_self = self_path.canonicalize().ok();

    // Extension allowlist: BRD's "removed from the codebase" naturally
    // means production source. Walk `*.rs`, `*.toml`, `*.json`, `*.html`,
    // `*.css`, `*.js` — the file types that could contain either the
    // control strings or the symbol names.
    let walk_exts = ["rs", "toml", "json", "html", "css", "js"];

    let mut violations: Vec<String> = Vec::new();

    let walker = WalkDir::new(&crates_dir)
        .into_iter()
        // Defensive directory skip: target/ (build artefacts, may contain
        // codegen with old strings) and .git/ (not normally under crates/
        // but defensive-skip is the same shape as the test-file self-
        // exclusion).
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            if e.file_type().is_dir() && (name == "target" || name == ".git") {
                return false;
            }
            true
        });

    for entry in walker {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();

        // Self-exclusion.
        if let Some(canon) = &canonical_self {
            if path.canonicalize().ok().as_ref() == Some(canon) {
                continue;
            }
        }

        // HANDOFF*.md and Agent_Build_Specification.txt are audit-trail /
        // BRD documents, not production source. Locked per the iteration
        // exclusion-list amendment. (Note: these are under crates/ only
        // if symlinked, which the project does not do — but the file-name
        // check is the canonical exclusion regardless of location.)
        let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if fname.starts_with("HANDOFF") && fname.ends_with(".md") {
            continue;
        }
        if fname == "Agent_Build_Specification.txt" {
            continue;
        }

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !walk_exts.contains(&ext) {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(_) => continue, // non-UTF-8 or unreadable: skip
        };

        for (label, needle) in block_a {
            if content.contains(needle) {
                violations.push(format!("BLOCK A {label}: found in {}", path.display()));
            }
        }
        for (label, needle) in block_b {
            if content.contains(needle) {
                violations.push(format!("BLOCK B {label}: found in {}", path.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "ADR-010 controls and/or plaintext-API symbols present in codebase \
         (sub-task (f) acceptance criterion (d) FAIL):\n  {}",
        violations.join("\n  ")
    );
}

// ============================================================
//   Criterion (e) — tampered ciphertext returns AEAD-auth error
// ============================================================

#[tokio::test]
async fn acceptance_e_tampered_ciphertext_returns_err_with_aead_marker() {
    let tmp = TempDir::new().unwrap();
    let master: [u8; 32] = *b"acceptance-e-master-key-32-bytes";
    let at_rest = derive_at_rest_key_test(&master);

    // Write 5 rows; with a fresh lance table on small data this produces
    // exactly one fragment file under <table>.lance/data/<frag>.lance.
    let boundary = Boundary::new("acceptance_e").unwrap();
    {
        let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest)
            .await
            .unwrap();
        for i in 0..5 {
            store
                .upsert(&MemoryId::new(), &orthogonal_unit_4d(i), &boundary)
                .await
                .unwrap();
        }
    }

    // Find the fragment data file; pick the largest to maximise ciphertext
    // body length over the framing+header constant overhead.
    let fragment = find_largest_lance_fragment(tmp.path()).expect(
        "expected at least one lance fragment data file under \
         <tmp>/<table>.lance/data/ post-write",
    );

    // Flip a byte mid-ciphertext (post-26-byte framing, well inside the
    // payload body — not the 16-byte Poly1305 tag at the very end). XOR
    // 0xFF guarantees the byte changes value.
    const TAMPER_OFFSET: usize = SEAL_FRAMING_LEN + 5;
    let mut bytes = std::fs::read(&fragment).unwrap();
    assert!(
        bytes.len() > TAMPER_OFFSET + 32,
        "fragment {} is {} bytes — too small to safely flip at offset {} \
         (need at least {} for framing + ciphertext-body + tag headroom)",
        fragment.display(),
        bytes.len(),
        TAMPER_OFFSET,
        TAMPER_OFFSET + 32
    );
    bytes[TAMPER_OFFSET] ^= 0xFF;
    std::fs::write(&fragment, &bytes).unwrap();

    // Reopen + read. Open may succeed (only the manifest is read at open
    // time; the manifest lives in a different sealed file we didn't
    // touch). The data-fragment read happens during search/count/validate.
    let store = LanceVectorStore::open_with_at_rest_key(tmp.path(), 4, &at_rest)
        .await
        .unwrap();

    // Force a full-data read path. Search with limit > row count + a
    // query unit-vector forces lance to score every row in the table,
    // which requires fully decoding the tampered fragment.
    let result = store
        .search(&orthogonal_unit_4d(0), 32, std::slice::from_ref(&boundary))
        .await;

    let err = result.expect_err(
        "search over a sealed store whose ciphertext was bit-flipped MUST \
         return Err — AEAD authentication failure is the privacy-contract \
         floor",
    );
    let msg = err.to_string();
    assert!(
        matches!(err, VaultError::Storage(_)),
        "tampered-ciphertext error should surface as VaultError::Storage; \
         got: {err:?}"
    );
    assert!(
        msg.contains("AEAD authentication failed"),
        "tampered-ciphertext error MUST contain the substring \
         'AEAD authentication failed' (per the γ tightening of the unseal-\
         site wrapping in sealed_object_store.rs's unseal_file_bytes). \
         Got: {msg}"
    );
}
