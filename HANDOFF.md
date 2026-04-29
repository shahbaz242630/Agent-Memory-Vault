# Memory Vault — Build Handoff

**Last updated:** 2026-04-29
**Updated by:** Claude (Opus 4.7)
**Current version:** V0.1 — Internal Alpha
**Current phase:** V0.1 in progress — `vault-storage::MetadataStore` (T0.1.3) implementation complete locally, all four DoD gates green, awaiting commit/push approval

---

## Current Status

**Active task:** T0.1.3 — vault-storage: SQLite + SQLCipher — **implementation complete, all four DoD gates green locally; awaiting commit/push approval**
**Started:** 2026-04-28
**Last test run:** 2026-04-29 — `cargo test -p vault-storage` 39/39 passing; `cargo build --workspace` zero warnings; `cargo clippy --workspace --all-targets -- -D warnings` zero warnings; `cargo fmt --all --check` clean

**Deliverables in this task:**
- `MetadataStore` async API: `open`, `create_memory`, `get_memory`, `update_memory`, `delete_memory`, `list_memories` (with `MemoryFilter`), `append_audit_event`, `list_audit_events`, `verify_audit_chain`. Every CRUD path is transactional and atomic with the corresponding audit-log append.
- Three tables created via the migration runner (`0001_initial.sql`):
  - `memories` — flat metadata (id, content, memory_type, source_agent, boundary, created_at, valid_from, valid_until, confidence, access_count, last_accessed, superseded_by, metadata_json) with indexes on (boundary), (memory_type), (created_at desc), (superseded_by IS NULL).
  - `audit_log` — tamper-evident BLAKE3 hash chain per BRD §11.9.2 (seq, event_id, timestamp, user_id, device_id, event_type, resource_type, resource_id, boundary, actor_kind, actor_name, result, details_json, prev_event_hash, event_hash).
  - `schema_migrations` — one-row-per-applied-migration ledger (version, applied_at, description). Forward-migration test confirmed: pre-applied versions are skipped, only new versions run; version-gap detection refuses to run if migrations are non-contiguous.
- SQLCipher key handling: `SqlCipherKey` newtype with `Zeroize`/`ZeroizeOnDrop`, no `Debug`/`Display`. Key set via `PRAGMA key` immediately after open; verified with a `SELECT count(*) FROM sqlite_master` round-trip; WAL + foreign_keys + synchronous=NORMAL pragmas applied. Wrong-key reopen test confirms decryption failure.
- Boundary filtering enforced at the SQL level (`WHERE boundary = ?` bound param, never spliced) per BRD §11.7.1 — boundary leak via list query is mechanically impossible.
- Audit chain integrity:
  - Genesis hash documented as `0000…0000` (64 zero hex chars).
  - `audit_chain_detects_tampering` direct test: tampering with `boundary` on seq=1 breaks `verify_audit_chain` with "tampering detected" error.
  - `tampering_breaks_chain_at_every_seq` proptest (8 cases × chains of 2–5 events): for **every** seq position, a single-byte boundary mutation breaks the chain; restoring the original byte heals it. Adversarial coverage of "the audit chain catches every tamper, every time."
  - `concurrent_writes_all_succeed_and_chain_stays_valid`: 20 concurrent `create_memory` tasks all succeed (Mutex<Connection> serialises chain-tip read + insert), all memories retrievable, chain validates after the fact, total event count matches expected.
- Performance honestly measured (`perf_budget_open_migrate_first_audit`):
  - `open + migrate` ≈ **120 ms** (BRD target was 50 ms — **missed by ~70 ms**)
  - `+ first audit insert` ≈ 120 ms (steady-state, dominated by open cost above)
  - steady-state audit insert ≈ **197 µs**
  - The first-open cost is dominated by SQLCipher's default PBKDF2 (256k SHA-512 iterations) — the 50 ms target is not achievable with secure default KDF settings on this CPU class. **Decision needed (see Blockers below):** accept the cost and revise the target, or tune `kdf_iter` (security trade-off — would need ADR + threat-model review).
- 39 tests across `audit`, `key`, `metadata_store`, `migrations` modules. Property tests use `tokio_test::block_on` (proptest is sync-only; we wrap each case in a block-on so async API can be exercised).

---

## Recently Completed

| Task ID | Name | Completed | Tests | Notes |
|---|---|---|---|---|
| Foundation | CLAUDE.md, HANDOFF.md, project memory files | 2026-04-28 | n/a | Pre-kickoff scaffolding (project rules + cross-session memory). Comprehensive `.gitignore` covers secrets, model files, encrypted vault data, ML binaries, Claude Code per-machine state. |
| T0.1.1 | Workspace Setup | 2026-04-28 | ✅ build/test/clippy/fmt all green on Windows local; CI green on push (39s) | 11 crate skeletons under `crates/`. Workspace `Cargo.toml` pins all BRD §4 deps in `[workspace.dependencies]`. `rust-toolchain.toml` pins stable. CI: 3-job parallel matrix (fmt, clippy, build+test) on ubuntu-latest. `git init` + remote connected to `https://github.com/shahbaz242630/Agent-Memory-Vault.git`. |
| T0.1.2 | vault-core | 2026-04-28 | ✅ 42 unit + 1 doc test passing; clippy clean; fmt clean | All BRD §5.1 types implemented across `error.rs` / `boundary.rs` / `memory.rs` / `entity.rs` with `lib.rs` re-exports. Validation enforced at construction (`try_new` constructors) AND at storage-write boundary (`validate()` method) per BRD §11.7.1. ID types use UUID v7 (time-ordered, good DB index locality). `Boundary` uses validated newtype with private field (deviation from BRD `pub String` — see ADR-005). |
| CI hardening | `actions/checkout` v4 → v6 | 2026-04-28 | ✅ CI green | Resolves Node 20 deprecation flagged on f3923eb. GitHub Actions runners drop Node 20 on 2026-09-16; v6 runs on Node 24. `Swatinem/rust-cache@v2` and `dtolnay/rust-toolchain@stable` are unaffected (no Node 20 dependency). Followed GitHub's recommended floating-major tag convention (`@v6`) so security patches auto-apply. |

---

## In Progress

**T0.1.3 — vault-storage: SQLite + SQLCipher** — code complete, all four DoD gates green locally, awaiting commit/push approval. Working-tree changes:
- `Cargo.toml` (workspace) — `rusqlite` feature flipped to `bundled-sqlcipher-vendored-openssl`; `blake3 = "1.8"` added to `[workspace.dependencies]`
- `crates/vault-storage/Cargo.toml` — deps wired (vault-core, rusqlite, tokio, async-trait, thiserror, tracing, serde, serde_json, chrono, uuid, blake3, zeroize) + dev-deps (proptest, tempfile, tokio-test)
- `crates/vault-storage/src/lib.rs` — re-exports
- `crates/vault-storage/src/key.rs` — `SqlCipherKey`
- `crates/vault-storage/src/audit.rs` — audit types, BLAKE3 sealing, chain verification
- `crates/vault-storage/src/metadata_store.rs` — `MetadataStore` async API + 39-test suite
- `crates/vault-storage/src/migrations/{mod.rs, 0001_initial.sql}` — migration runner + initial schema
- `crates/vault-storage/proptest-regressions/` — proptest seed files (recommended to commit per proptest docs)

---

## Pending — V0.1 (Internal Alpha)

- [ ] **T0.1.4** — vault-storage: LanceDB (vector store with boundary filtering)
- [ ] **T0.1.5** — vault-storage: DuckDB (graph store with bi-temporal columns)
- [ ] **T0.1.6** — vault-storage: Cascading Backend (StorageBackend orchestrator + retry queue)
- [ ] **T0.1.7** — vault-embedding (bge-small via ort)
- [ ] **T0.1.8** — vault-retrieval: Semantic Strategy Only
- [ ] **T0.1.9** — vault-mcp: Adapter + Stdio Server (memory.search, memory.write)
- [ ] **T0.1.10** — vault-app: Wiring (Application, config, startup/shutdown)
- [ ] **T0.1.11** — vault-tauri: Minimal UI (add memory, search, settings — also: convert vault-tauri from lib to bin crate)
- [ ] **T0.1.12** — V0.1 End-to-End Test (founder uses for a full day, files ≥3 issues)

V0.2 and V1.0 task lists live in BRD §6 and will be promoted here when V0.1 completes.

---

## Blockers / Decisions Needed

- **SQLCipher first-open cost vs. BRD 50 ms target.** Measured `open + migrate` ≈ 120 ms; the 50 ms target in BRD §6 V0.1.3 is not achievable with SQLCipher's default `kdf_iter = 256000` (PBKDF2-SHA512) on this CPU class. Two paths:
  1. **Accept the cost, revise the target.** First-open is a once-per-session event (vault-tauri startup); 120 ms is imperceptible. Document in BRD addenda.
  2. **Tune `kdf_iter` down.** Each halving of iterations roughly halves brute-force resistance. Would need an ADR + threat-model review, and would deviate from SQLCipher defaults that other security audits assume.
  Recommendation: take path (1). Awaiting Shahbaz's call.

---

## Architecture Decisions Log

### ADR-001 — 2026-04-28 — CI runs on ubuntu-latest only for V0.1
- **Context:** BRD §11.7.5 requires release-time signing on macOS + Windows, but acceptance for T0.1.1 is just "CI passes on a no-op commit."
- **Decision:** V0.1 CI uses ubuntu-latest only for fmt / clippy / build+test (3 parallel jobs).
- **Reasoning:** Cheapest, fastest. We have no platform-specific code yet. macOS + Windows job matrices add cost and complexity that buys us nothing until we ship native binaries (V0.2 onward).
- **Alternatives considered:** 3x platform matrix from day one (rejected: premature). macOS-only (rejected: ubuntu is cheaper for CI minutes).
- **When to revisit:** Add macOS to the matrix in T0.1.7 (vault-embedding loads ONNX Runtime, which is platform-specific). Add Windows in T0.1.9 (MCP stdio has Windows-specific quirks per BRD §5.7 implementation notes).

### ADR-002 — 2026-04-28 — `#![forbid(unsafe_code)]` on all skeleton crates
- **Context:** BRD §11.7.4 mandates safety-by-default. FFI-heavy crates (`vault-embedding` for ort, `vault-llm` for llama.cpp) will need `unsafe` for FFI but can isolate it.
- **Decision:** All 11 crate skeletons start with `#![forbid(unsafe_code)]`. Crates that need FFI later relax to `#![deny(unsafe_code)]` at crate root and use `#[allow(unsafe_code)]` on the single FFI module that wraps the C library.
- **Reasoning:** Safety is the default, exceptions are documented. `forbid` is stricter than `deny` (cannot be overridden by inner attributes), so accidental unsafe in any other module fails the build.
- **Alternatives considered:** `deny(unsafe_code)` everywhere from day one (rejected: weaker default). No annotation (rejected: BRD §11.7.4 requirement).

### ADR-003 — 2026-04-28 — `vault-tauri` ships as library at T0.1.1, converts to binary at T0.1.11
- **Context:** BRD §5.11 says `vault-tauri` has `src/main.rs` as a Tauri entry point, but T0.1.1 says "all crate skeletons (empty `lib.rs` files)."
- **Decision:** `vault-tauri` is a library skeleton (`src/lib.rs`) until T0.1.11. T0.1.11 swaps the crate to a binary with `src/main.rs` and a `tauri.conf.json`.
- **Reasoning:** Skeleton uniformity simplifies T0.1.1. Binary conversion is mechanical and belongs to the task that actually uses it.

### ADR-004 — 2026-04-28 — `CLAUDE.md` is gitignored and never committed
- **Context:** The first T0.1.1 commit (`d105f68`) included `CLAUDE.md`, the project-scoped Claude Code rules file. After review, Shahbaz directed: "claude.md shouldnt be committed .. never commit." He treats project rules / agent instructions as per-machine configuration, not shared repo content.
- **Decision:** `CLAUDE.md` is added to `.gitignore` and untracked from the repo via `git rm --cached CLAUDE.md`. The working-tree file is preserved on Shahbaz's machine and continues to auto-load each Claude Code session. Future edits remain local-only.
- **Reasoning:** Honour Shahbaz's explicit preference. The previous commit's tree contains `CLAUDE.md` but the file has no secrets, so history rewrite (force-push, `git filter-repo`) is unnecessary and would be destructive.
- **Alternatives considered:** History rewrite to scrub the file from `d105f68` (rejected: destructive, no security need). Move CLAUDE.md content into a tracked `docs/agent-rules.md` (rejected: defeats the purpose of being per-machine).
- **Mirrored in cross-session memory:** `feedback_never_commit_claude_md.md`.

### ADR-005 — 2026-04-28 — `Boundary` uses a validated newtype with a private field
- **Context:** BRD §5.1 sketches `pub struct Boundary(pub String)`, but BRD §11.7.1 requires that boundary names be validated (≤ 64 bytes, no control characters) at every public API boundary.
- **Decision:** `Boundary` wraps a private `String` and exposes `Boundary::new(...)` / `TryFrom<String>` / `FromStr` constructors that validate, plus `as_str()` / `into_inner()` / `Display` accessors. Serde uses `try_from = "String"` so deserialisation also runs validation.
- **Reasoning:** The §11.7.1 invariant is security-critical (boundary names feed into mandatory access control). A `pub String` field would let any caller bypass validation, making invariants depend on caller discipline rather than the type system. The BRD §5.1 sketch is illustrative; matching the spirit (a boundary type that storage and retrieval can trust) is more important than matching the literal `pub` field.
- **Alternatives considered:** `pub String` field with a separate `validate()` method (rejected: footgun — easy to forget). `Boundary(String)` private without constructors (rejected: no clean way to construct from caller code).

### ADR-006 — 2026-04-28 — `rusqlite` feature `bundled-sqlcipher-vendored-openssl` (vendored OpenSSL) + monthly OpenSSL CVE monitoring
- **Context:** Building `rusqlite` with SQLCipher requires linking SQLCipher (which depends on OpenSSL). Three feature options exist in `rusqlite`: `bundled-sqlcipher` (link to system OpenSSL), `bundled-sqlcipher-vendored-openssl` (vendor + statically link OpenSSL), and BYO system SQLCipher. The first two require a Perl interpreter at build time to drive the OpenSSL build script.
- **Decision:** Use `bundled-sqlcipher-vendored-openssl`. Install Strawberry Perl on each developer machine (Shahbaz's machine: done 2026-04-28 via `winget install StrawberryPerl.StrawberryPerl`). CI installs Perl via `actions/setup-perl` when the storage tests run.
- **Reasoning:** Vendoring eliminates "works on my machine, breaks on yours" entirely — the vault binary contains its OpenSSL inside, and there is no system-OpenSSL ABI surface for users to drift on. Cost: we own the responsibility to track OpenSSL CVEs ourselves. Worth it for a security-critical, end-user-distributed binary where we cannot assume up-to-date system libraries.
- **Alternatives considered:** System OpenSSL (rejected: cross-platform install pain for end users; can't guarantee user has a non-vulnerable version). System SQLCipher (rejected: same reason, plus SQLCipher is rare on consumer machines).
- **Operational follow-up (required, recurring):** **Monthly OpenSSL CVE check.** First Monday of each month, review https://www.openssl.org/news/vulnerabilities.html and the OpenSSL version vendored by the current `openssl-src` crate. If a Critical or High-severity advisory affects the bundled version, prioritise a `cargo update -p openssl-src` + rebuild ahead of any other work. Tracked as a recurring tech-debt item below.

### ADR-007 — 2026-04-29 — No manual `Debug` impls on types that hold sensitive runtime state
- **Context:** A test in `vault-storage` (`opening_with_wrong_key_fails`) used `format!("...{result:?}")` in its panic message, which required `MetadataStore: Debug`. `rusqlite::Connection` does not derive `Debug` (intentional — the type owns a live encrypted DB handle and key-derived state). I attempted to fix the test by stubbing a manual `Debug` impl on `MetadataStore` that returned `"MetadataStore { .. }"`. Shahbaz rejected this.
- **Decision:** Do not add manual `Debug` impls to `MetadataStore`, `SqlCipherKey`, future SQLCipher-backed types, or any type that holds key material, raw connections, decrypted secrets, or other sensitive runtime state. Fix the consumer (test, error message, log statement) to not require `Debug` — use static description strings in panic / error / log messages instead of `{:?}`.
- **Reasoning:** Extends the spirit of BRD §11.5.3 ("no `Debug`/`Display` impl on key types"). Even a stub `Debug` impl creates a habit of glossing over which types contain sensitive state, and one day someone derives `Debug` on a struct with a non-private `key: SqlCipherKey` field and the key gets logged. Refusing to provide `Debug` at all forces the conversation every time.
- **Alternatives considered:** Stub `Debug` returning a fixed string like `"MetadataStore { .. }"` (rejected: sets the precedent above). Wrap the connection in a `Debug`-able newtype (rejected: same precedent, with extra ceremony).
- **Test-side pattern:** Replace `assert!(matches!(x, Pattern(_)), "got {x:?}")` with an explicit `match x { Pattern(_) => {}, _ => panic!("static description here") }` when `x` doesn't impl `Debug`.

---

## Tech Debt Backlog

Items noticed but not addressed in their originating task — picked up explicitly when scheduled, never as drive-by work.

- [ ] **`llama-cpp-2 = "0.1"` not yet declared in `[workspace.dependencies]`** — BRD §4.3 flags it for verification at the start of vault-llm work (T0.2.1). Do crate-name-and-version verification on docs.rs at that point and add to workspace deps then. (Noted 2026-04-28, file: `Cargo.toml`)
- [ ] **`.gitattributes` for line-ending normalisation** — currently relying on git's default `core.autocrlf=true` on Windows. Adding `* text=auto eol=lf` plus binary markers for known binary file types would silence the CRLF warnings on commit and make cross-platform behaviour deterministic. Quick win when convenient. (Noted 2026-04-28)
- [ ] **dryoc 0.7 only exposes streaming XChaCha20-Poly1305, not single-shot AEAD** — BRD §11.6 sync envelope construction (T0.2.9) was sketched assuming `crypto_aead_xchacha20poly1305_ietf_encrypt` (single-shot). dryoc 0.7's surface is `dryocstream` (streaming push/pull). Two options when T0.2.9 lands: (a) wrap streaming in a single-message helper (one-`push`-then-`finalize` per envelope), (b) switch to a different crate that exposes the libsodium-style single-shot API (orion / sodiumoxide). Re-verify dryoc's API at T0.2.9 kickoff before deciding. (Noted 2026-04-28, file: `Agent Build Specification.txt` §11.6)
- [ ] **(Recurring) Monthly OpenSSL CVE check** — per ADR-006. First Monday of each month, review https://www.openssl.org/news/vulnerabilities.html and the OpenSSL version vendored by `openssl-src` (`cargo tree -p openssl-src` from this workspace). Critical / High advisories affecting the vendored version → prioritise `cargo update -p openssl-src` ahead of other work. Next due: 2026-05-04. (Noted 2026-04-28)

---

## V0.1 Findings

_(populated once V0.1 ships)_

---

## Notes for Next Session

**Immediate:** T0.1.3 working tree is staged-ready but **not committed** — every git history change requires per-action approval per CLAUDE.md. Once Shahbaz approves, commit + push, then move on.

**Two of the five tables in BRD §5.2 are intentionally deferred** to the tasks that actually need them:
- `review_queue` — added at T0.1.X around connector ingestion (BRD §5.9)
- `sync_state` and `retry_queue` — added at T0.1.6 (cascading backend) / T0.2.x (sync engine)

This is per the "no scaffolding for unused features" rule (CLAUDE.md hard rule). Each table will land via a new numbered migration file (`0002_review_queue.sql`, etc.) — the migration runner already supports this and is regression-tested by `forward_migration_applies_next_version_only`.

**Next task — T0.1.4 — vault-storage: LanceDB vector store (BRD §5.2.2):**

- Add `lancedb` and `arrow` deps to `crates/vault-storage/Cargo.toml` (versions pinned per BRD §4)
- Implement `VectorStore` trait + `LanceVectorStore` struct: `insert`, `update`, `delete`, `search(query_vec, boundary, k) -> Vec<(MemoryId, score)>`, `purge_boundary(b)` for boundary deletion
- **Boundary filtering must happen at the LanceDB query layer**, not after-the-fact — same security principle as the SQL boundary filter we just shipped
- Encryption: LanceDB does not natively support encryption-at-rest. Decision needed at T0.1.4 kickoff: use a wrapper that encrypts the parquet files at OS-FS level (recommended), or store vectors inside SQLCipher (slower, scaling concerns). Document as an ADR.
- Test level: **Heavy** — round-trip tests, boundary filter cannot leak, property tests for vector insert/search consistency
- Tie-in to T0.1.3: the next migration (`0002_…sql`) may add a `vector_id` column to `memories` if we choose a model where SQLite holds the cross-store reference

**Working-tree state at end of session 2026-04-29:** T0.1.3 implementation complete, four DoD gates green, `HANDOFF.md` updated. Awaiting commit/push approval. Files in working tree:
- `Cargo.toml`, `Cargo.lock` — workspace dep updates (rusqlite feature, blake3)
- `crates/vault-storage/Cargo.toml` — full dep wiring
- `crates/vault-storage/src/{lib.rs, key.rs, audit.rs, metadata_store.rs, migrations/}` — implementation + tests
- `crates/vault-storage/proptest-regressions/` — proptest seed corpus (recommended-to-commit per proptest docs)
- `HANDOFF.md` — this file
