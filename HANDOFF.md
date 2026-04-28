# Memory Vault — Build Handoff

**Last updated:** 2026-04-28
**Updated by:** Claude (Opus 4.7)
**Current version:** V0.1 — Internal Alpha
**Current phase:** V0.1 in progress — vault-core complete, vault-storage up next

---

## Current Status

**Active task:** T0.1.3 — vault-storage: SQLite + SQLCipher (not started)
**Started:** —
**Last test run:** 2026-04-28 — `cargo test -p vault-core` 42 passing + 1 doc test, all four gates green workspace-wide

T0.1.2 — vault-core is complete. All domain types (`Memory`, `MemoryId`, `MemoryType`, `Entity`, `EntityId`, `EntityType`, `Relationship`, `RelationshipId`, `Boundary`, `NewMemory`) and the workspace-wide error catalogue (`VaultError`, `VaultResult`) are implemented with validation, serde round-trip support, and property tests. Awaiting commit/push approval.

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

_(none — T0.1.2 ready for commit, awaiting partner approval)_

---

## Pending — V0.1 (Internal Alpha)

- [ ] **T0.1.3** — vault-storage: SQLite + SQLCipher (encrypted metadata store, migrations) — BRD §5.2
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

_(none)_

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

---

## Tech Debt Backlog

Items noticed but not addressed in their originating task — picked up explicitly when scheduled, never as drive-by work.

- [ ] **`llama-cpp-2 = "0.1"` not yet declared in `[workspace.dependencies]`** — BRD §4.3 flags it for verification at the start of vault-llm work (T0.2.1). Do crate-name-and-version verification on docs.rs at that point and add to workspace deps then. (Noted 2026-04-28, file: `Cargo.toml`)
- [ ] **`.gitattributes` for line-ending normalisation** — currently relying on git's default `core.autocrlf=true` on Windows. Adding `* text=auto eol=lf` plus binary markers for known binary file types would silence the CRLF warnings on commit and make cross-platform behaviour deterministic. Quick win when convenient. (Noted 2026-04-28)

---

## V0.1 Findings

_(populated once V0.1 ships)_

---

## Notes for Next Session

**T0.1.3 — vault-storage: SQLite + SQLCipher (BRD §5.2) is up next.** Implementation plan:

- Add SQLite/SQLCipher deps to `crates/vault-storage/Cargo.toml`:
  ```toml
  [dependencies]
  vault-core = { path = "../vault-core" }
  rusqlite = { workspace = true }   # already pinned with bundled-sqlcipher feature
  tokio = { workspace = true }
  async-trait = { workspace = true }
  thiserror = { workspace = true }
  tracing = { workspace = true }
  serde_json = { workspace = true }
  chrono = { workspace = true }
  ```
- Implement `MetadataStore` (the SQLite-backed store) with these tables:
  - `memories` — flat metadata per memory (id, content, type, boundary, timestamps, confidence, access_count, superseded_by, metadata_json)
  - `audit_log` — tamper-evident hash chain (BRD §11.9.2)
  - `review_queue` — proposed memories from connectors (BRD §5.9)
  - `sync_state` — pointers for the sync engine
  - `retry_queue` — partial-failure recovery for cascading writes (BRD §5.2)
- Schema migrations via a simple incremental migration runner (track `schema_version` in a one-row table)
- SQLCipher: open with `PRAGMA key = '<derived-from-master-key>'` immediately after open. For T0.1.3, accept the key as a parameter; key derivation lives in vault-sync.

Test level **Heavy** (BRD §7.1): every CRUD path needs a happy-path + error-path test, encrypted DB cannot open with wrong key, schema migrations are idempotent, audit-log hash chain is verifiable, property tests for round-trip integrity.

**Acceptance** (BRD §6 V0.1.3): Encrypted SQLite database is created, all CRUD tests pass, can be opened only with correct passphrase. CI green.

**Working-tree state at end of session 2026-04-28:** Two logical concerns ready for commit, batched into one because they share a HANDOFF.md update:
1. Housekeeping — untrack CLAUDE.md, .gitignore update, ADR-004
2. T0.1.2 implementation — vault-core source + tests + Cargo.toml deps + ADR-005 + recently-completed entry
