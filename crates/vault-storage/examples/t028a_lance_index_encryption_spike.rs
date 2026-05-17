//! T0.2.7 Phase 0 — t028a security spike: Lance HNSW index encryption
//! envelope verification.
//!
//! **Question this spike answers.** When [`LanceVectorStore`] creates an
//! `IvfHnswSq` vector index via the `vault-sealed://` ObjectStoreProvider
//! path (T0.2.0 Phase 0e ADR-008 amendment), do ALL on-disk index artifacts
//! route through the sealed envelope, or does Lance's index-storage path
//! bypass the provider for some file class (e.g., raw HNSW graph dumps,
//! IVF centroid arrays, index manifests)?
//!
//! **Pass condition.** Every file in the data dir starts with the locked
//! VAULT_SEALED prefix bytes (`0x01 || 0x00 || dryoc_header (24 bytes) ||
//! AEAD ciphertext`). The first two bytes (`VERSION_BYTE = 0x01`,
//! `GRANULARITY_PER_FILE = 0x00`) are the structural sentinel; if a file's
//! first two bytes match, we know the seal_file_bytes path was invoked
//! (per crates/vault-storage/src/sealed_object_store.rs:90-94).
//!
//! **Fail mode = hard blocker.** If any file fails the prefix check, T0.2.7
//! cannot ship HNSW index integration without either:
//! (a) a Lance contribution that wires the index path through
//!     ObjectStoreProvider, OR
//! (b) a vault-storage shim that intercepts index-file writes at a
//!     different boundary, OR
//! (c) a BRD §11.5.1 amendment carving an exception (UNACCEPTABLE — leaks
//!     vector relationships which encode memory clustering structure).
//!
//! Per T0.2.7 plan iteration 2 (locked 2026-05-15): t028a is strict-serial
//! before t028b. Spike methodology = compile-and-run, NOT web research.
//!
//! ## Fixture choice rationale
//!
//! Security spike uses **random synthetic vectors**, not realistic content.
//! Per iteration-2 amendment A, the t028b PERFORMANCE benchmark MUST use
//! the t026 realism-rewritten content shape. t028a does NOT — the security
//! property "are all on-disk files sealed?" is content-agnostic; testing
//! it on random vectors is sufficient and avoids spinning up the BGE
//! embedder for a check that doesn't care about embedding quality.
//!
//! Run with (PowerShell on Windows, per standing rules):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-storage --release --example t028a_lance_index_encryption_spike
//! ```

#![allow(clippy::too_many_lines)]

use std::error::Error;
use std::path::{Path, PathBuf};

use vault_core::{Boundary, MemoryId};
use vault_storage::{LanceVectorStore, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];
const EMBEDDING_DIM: usize = 384;

/// 1024 random vectors comfortably exceeds the IVF training threshold
/// (lancedb 0.27.2 IvfHnswSqIndexBuilder default num_partitions is
/// `sqrt(num_rows)`; 1024 rows → 32 partitions, well-trained).
const N_MEMORIES: usize = 1024;

/// VAULT_SEALED envelope prefix — must match
/// `crates/vault-storage/src/sealed_object_store.rs:90-94`.
const VERSION_BYTE: u8 = 0x01;
const GRANULARITY_PER_FILE: u8 = 0x00;

// Each on-disk file falls into one of three verdict classes:
//   - Sealed              : first two bytes are (VERSION_BYTE, GRANULARITY_PER_FILE).
//   - EmptyOrTooShort     : fewer than two bytes — no payload to leak; logged + counted.
//   - PlaintextPrefix     : two or more bytes but not the sealed sentinel — a HARD FAIL.
// Counts are tracked in parallel scalars/vecs below; no enum is needed since
// each class is handled at a distinct branch in the inspection loop.

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let sep = "=".repeat(100);
    println!("{sep}");
    println!("T0.2.7 Phase 0 — t028a Lance HNSW index encryption envelope spike");
    println!(
        "Started: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("{sep}");

    // Use a non-temp dir so the operator can inspect post-spike if needed.
    // Sibling to the existing at_rest_spike for consistency.
    let data_dir = std::env::temp_dir().join("memory-vault-t028a-spike");
    if data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)?;
    }
    std::fs::create_dir_all(&data_dir)?;
    println!("Data dir: {}", data_dir.display());

    println!("\nOpening LanceVectorStore via sealed ObjectStoreProvider...");
    let store =
        LanceVectorStore::open_with_at_rest_key(&data_dir, EMBEDDING_DIM, &TEST_AT_REST_KEY)
            .await?;
    println!("Sealed store ready.");

    let boundary = Boundary::new("t028a-spike")?;

    println!("\nUpserting {N_MEMORIES} synthetic random vectors (dim={EMBEDDING_DIM})...");
    for i in 0..N_MEMORIES {
        let id = MemoryId::new();
        // Deterministic-but-varying vectors: each dim is a sine of a
        // distinct integer combination — produces well-spread cosines
        // sufficient for IVF training without pulling in `rand` as a dep.
        let vec: Vec<f32> = (0..EMBEDDING_DIM)
            .map(|j| {
                let theta = ((i.wrapping_mul(17) + j.wrapping_mul(31)) % 9973) as f32;
                (theta * 0.001).sin()
            })
            .collect();
        store.upsert(&id, &vec, &boundary).await?;
        if (i + 1) % 256 == 0 {
            println!("  upserted {}/{N_MEMORIES}", i + 1);
        }
    }

    println!("\nCreating HNSW (IvfHnswSq) vector index...");
    let index_start = std::time::Instant::now();
    store.create_vector_index_hnsw_sq().await?;
    println!(
        "Index created in {:.2}s.",
        index_start.elapsed().as_secs_f64()
    );

    println!("\nWalking data dir + inspecting all on-disk files for VAULT_SEALED envelope...");
    let mut all_paths = Vec::<PathBuf>::new();
    walk(&data_dir, &mut all_paths)?;
    println!("Found {} files. Inspecting prefixes...\n", all_paths.len());

    let mut sealed = 0usize;
    let mut empty_or_short = Vec::<PathBuf>::new();
    let mut plaintext = Vec::<PathBuf>::new();

    for path in &all_paths {
        let bytes = std::fs::read(path)?;
        let size_bytes = bytes.len() as u64;
        let rel = path.strip_prefix(&data_dir).unwrap_or(path);

        if bytes.len() < 2 {
            println!("  [SHORT  ] {} ({size_bytes} bytes)", rel.display());
            empty_or_short.push(path.clone());
            continue;
        }

        if bytes[0] == VERSION_BYTE && bytes[1] == GRANULARITY_PER_FILE {
            println!("  [SEALED ] {} ({size_bytes} bytes)", rel.display());
            sealed += 1;
        } else {
            let first_eight: Vec<u8> = bytes.iter().take(8).copied().collect();
            println!(
                "  [PLAIN? ] {} ({size_bytes} bytes, first 8: {:02x?})",
                rel.display(),
                first_eight
            );
            plaintext.push(path.clone());
        }
    }

    println!("\n{sep}");
    println!("VERDICT");
    println!("{sep}");
    println!("Total files inspected: {}", all_paths.len());
    println!("  Sealed envelope:   {sealed}");
    println!("  Empty / too short: {}", empty_or_short.len());
    println!("  Plaintext prefix:  {}", plaintext.len());

    if !plaintext.is_empty() {
        println!(
            "\n** FAIL — {} files do NOT match the VAULT_SEALED envelope.",
            plaintext.len()
        );
        println!("   Lance is writing index artifacts via a path that bypasses the");
        println!("   sealed ObjectStoreProvider. This is a BRD §11.5.1 violation.");
        println!("   T0.2.7 HNSW index integration is HARD-BLOCKED pending escalation.");
        println!();
        for path in &plaintext {
            println!("   - {}", path.display());
        }
        std::process::exit(1);
    }

    if !empty_or_short.is_empty() {
        println!(
            "\n** WARNING — {} files are empty or <2 bytes.",
            empty_or_short.len()
        );
        println!("   Likely Lance manifest / index-metadata placeholders that don't");
        println!("   round-trip through the sealing layer because they have no");
        println!("   plaintext payload. Worth surfacing to verify they're expected:");
        for path in &empty_or_short {
            let rel = path.strip_prefix(&data_dir).unwrap_or(path);
            println!("   - {}", rel.display());
        }
        println!("   This is NOT necessarily a fail — short/empty files cannot leak");
        println!("   semantic vector content. But document them in the t028a report.");
    }

    println!(
        "\n** PASS — all {} non-trivial files match VAULT_SEALED envelope ({} sealed, {} empty/short).",
        all_paths.len(),
        sealed,
        empty_or_short.len()
    );
    println!("   T0.2.7 HNSW index integration is GREEN for envelope compliance.");
    println!("   Proceed to t028b (HNSW vs IVF benchmark on realism-rewritten fixture).");
    Ok(())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
