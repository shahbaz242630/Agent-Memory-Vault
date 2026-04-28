# Memory Vault — Build Handoff

**Last updated:** 2026-04-28
**Updated by:** Claude (Opus 4.7)
**Current version:** V0.1 — Internal Alpha
**Current phase:** V0.1 in progress — workspace bootstrapped, ready for vault-core

---

## Current Status

**Active task:** T0.1.2 — vault-core (not started)
**Started:** —
**Last test run:** 2026-04-28 — `cargo build/test/clippy/fmt --workspace` all green on the empty skeleton

The Cargo workspace is bootstrapped with all 11 crate skeletons. CI workflow is in place. The four-gate Definition of Done check (build / test / clippy / fmt) passes locally on the no-op skeleton. Awaiting first push to verify CI runs green on GitHub.

---

## Recently Completed

| Task ID | Name | Completed | Tests | Notes |
|---|---|---|---|---|
| Foundation | CLAUDE.md, HANDOFF.md, project memory files | 2026-04-28 | n/a | Pre-kickoff scaffolding (project rules + cross-session memory). Comprehensive `.gitignore` covers secrets, model files, encrypted vault data, ML binaries, Claude Code per-machine state. |
| T0.1.1 | Workspace Setup | 2026-04-28 | ✅ build/test/clippy/fmt all green on Windows local | 11 crate skeletons under `crates/`. Workspace `Cargo.toml` pins all BRD §4 deps in `[workspace.dependencies]` (lazy — no transitive fetches yet). `rust-toolchain.toml` pins stable. CI: 3-job parallel matrix (fmt, clippy, build+test) on ubuntu-latest. `git init` + remote connected to `https://github.com/shahbaz242630/Agent-Memory-Vault.git`. README.md from remote preserved as base. |

---

## In Progress

_(none — paused before T0.1.2 kickoff for partner review)_

---

## Pending — V0.1 (Internal Alpha)

- [ ] **T0.1.2** — vault-core (Memory, Entity, Relationship, Boundary, error types — BRD §5.1)
- [ ] **T0.1.3** — vault-storage: SQLite + SQLCipher (encrypted metadata store, migrations)
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

---

## Tech Debt Backlog

Items noticed but not addressed in their originating task — picked up explicitly when scheduled, never as drive-by work.

- [ ] **`llama-cpp-2 = "0.1"` is not yet declared in `[workspace.dependencies]`** — BRD §4.3 flags it for verification at the start of vault-llm work (T0.2.1). Do crate-name-and-version verification on docs.rs at that point and add to workspace deps then. (Noted 2026-04-28, file: `Cargo.toml`)

---

## V0.1 Findings

_(populated once V0.1 ships)_

---

## Notes for Next Session

**T0.1.2 — vault-core (BRD §5.1) is up next.** Implement these public types in this crate, with serde derives, doc comments on every public item, and round-trip serde tests:

- `Memory`, `MemoryId`, `MemoryType` (Episodic / Semantic / Procedural)
- `Entity`, `EntityId`, `EntityType`
- `Relationship`, `RelationshipId`
- `Boundary`
- `VaultError` (one variant per failure category) + `VaultResult<T>` alias

Files (per BRD §5.1):
- `src/lib.rs` — re-exports
- `src/memory.rs`
- `src/entity.rs`
- `src/boundary.rs`
- `src/error.rs`

Test level for vault-core is **Medium** (BRD §7.1): unit tests for type construction + serialization, round-trip serde tests, property tests for `MemoryId` / `EntityId` uniqueness.

Workspace deps to wire into `crates/vault-core/Cargo.toml`:
```toml
[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
thiserror = { workspace = true }

[dev-dependencies]
proptest = { workspace = true }
serde_json = { workspace = true }
```

Acceptance: All types compile, all unit tests pass, doc comments on every public item, `cargo build/test/clippy/fmt --workspace` all green.
