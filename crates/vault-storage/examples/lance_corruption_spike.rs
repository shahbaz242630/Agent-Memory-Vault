//! Spike: does `LanceVectorStore::open` fail eagerly on a corrupted Lance
//! fragment file, or does the error surface lazily on the first read?
//!
//! Per Phase C plan Issue 2 — placement of the hard-corruption test in C1
//! vs C2 depends on the answer:
//!   - **Eager** → test belongs in C1 alongside `cascading.rs` (open is the
//!     gate; degraded-mode signal lives in `StorageBackend::open`).
//!   - **Lazy** → test belongs in C2 alongside `divergence.rs`, OR C1 adds
//!     explicit eager validation to `LanceVectorStore::open` and documents
//!     it as a deliberate decision (e.g., a "scan all fragments at open"
//!     pre-check).
//!
//! Run via:
//!   ```
//!   cargo run -p vault-storage --example lance_corruption_spike
//!   ```
//! (Requires PROTOC + Strawberry Perl on PATH per ADR-006 / ADR-011.)
//!
//! The spike creates a temp vault, inserts memories, closes the store,
//! corrupts a `.lance` fragment file's first 64 bytes (overwrites the
//! Parquet/IPC magic header), and observes:
//!   1. Does `LanceVectorStore::open` succeed?
//!   2. If yes, does `count()` succeed?
//!   3. If yes, does `search()` against the corrupted data succeed?

use std::path::Path;

use tempfile::TempDir;
use vault_core::{Boundary, MemoryId};
use vault_storage::{LanceVectorStore, VectorStore};

#[tokio::main]
async fn main() {
    eprintln!("===== LanceDB hard-corruption spike =====");
    eprintln!();

    let tmp = TempDir::new().expect("tempdir");
    let data_dir = tmp.path().join("vec_store");
    eprintln!("data dir: {}", data_dir.display());

    // 1. Open a fresh store, insert some entries, close.
    {
        let store = LanceVectorStore::open(&data_dir, 4)
            .await
            .expect("initial open");
        let work = Boundary::new("work").expect("boundary");
        for i in 0..5 {
            let id = MemoryId::new();
            let embedding = vec![0.1f32 * (i as f32 + 1.0); 4];
            store.upsert(&id, &embedding, &work).await.expect("upsert");
        }
        let count = store.count(None).await.expect("count");
        eprintln!("step 1 — clean store: {count} entries inserted");
        // Drop the store so files flush to disk.
    }

    // 2. Locate a `.lance` fragment file. LanceDB lays out:
    //   data_dir/
    //     memories.lance/
    //       data/<uuid>.lance        ← fragment data
    //       _versions/<n>.manifest
    //       _transactions/...
    let fragment = find_first_fragment(&data_dir).expect("find fragment file");
    eprintln!("step 2 — corrupting fragment: {}", fragment.display());
    let original = std::fs::read(&fragment).expect("read fragment");
    eprintln!("        original size: {} bytes", original.len());
    eprintln!(
        "        original first 16 bytes: {:02x?}",
        &original[..16.min(original.len())]
    );

    // Overwrite the first 64 bytes with garbage so the Parquet magic /
    // IPC stream header is destroyed. Keep the rest intact so reads further
    // into the file are still meaningful (in case LanceDB skips the header).
    let mut corrupted = original.clone();
    let n = 64.min(corrupted.len());
    for byte in corrupted.iter_mut().take(n) {
        *byte = 0xAB;
    }
    std::fs::write(&fragment, &corrupted).expect("overwrite fragment");
    eprintln!(
        "        first 16 bytes after corruption: {:02x?}",
        &corrupted[..16.min(corrupted.len())]
    );
    eprintln!();

    // 3. Re-open. Does `open()` fail eagerly?
    eprintln!("step 3 — re-opening corrupted store:");
    match LanceVectorStore::open(&data_dir, 4).await {
        Ok(store) => {
            eprintln!("        open(): Ok  ← LAZY corruption detection");
            // Probe further: does count() or search() fail?
            match store.count(None).await {
                Ok(n) => eprintln!("        count(): Ok({n})  ← still lazy, deeper probe needed"),
                Err(e) => eprintln!("        count(): Err({e})  ← surfaces here"),
            }
            let work = Boundary::new("work").expect("boundary");
            match store.search(&[0.1, 0.1, 0.1, 0.1], 5, &[work]).await {
                Ok(rows) => eprintln!(
                    "        search(): Ok({} rows)  ← still lazy or recovered",
                    rows.len()
                ),
                Err(e) => eprintln!("        search(): Err({e})  ← surfaces on first read"),
            }
        }
        Err(e) => {
            eprintln!("        open(): Err({e})  ← EAGER corruption detection");
        }
    }

    eprintln!();
    eprintln!("===== spike complete =====");
}

/// Walk `data_dir/*.lance/data/` recursively and return the first file whose
/// extension is `.lance` (these are the actual fragment data files in
/// LanceDB 0.8's on-disk layout).
fn find_first_fragment(data_dir: &Path) -> Option<std::path::PathBuf> {
    fn walk(dir: &Path, found: &mut Option<std::path::PathBuf>) {
        if found.is_some() {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
                if found.is_some() {
                    return;
                }
            } else if path.extension().and_then(|s| s.to_str()) == Some("lance")
                // Skip `*.lance` directories — we want fragment files inside `data/`.
                && path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == "data")
                    .unwrap_or(false)
            {
                *found = Some(path);
                return;
            }
        }
    }
    let mut found = None;
    walk(data_dir, &mut found);
    found
}
