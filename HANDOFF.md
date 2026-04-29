# Memory Vault — Build Handoff

**Last updated:** 2026-04-29
**Updated by:** Claude (Opus 4.7)
**Current version:** V0.1 — Internal Alpha
**Current phase:** V0.1 in progress — T0.1.3 shipped (commit `f846df7`); pre-T0.1.4 follow-ups in flight (ADR-009 retry-queue policy, BRD §6 perf-budget addition, dryoc API spike scheduled)

---

## Current Status

**Active item:** Pre-T0.1.4 follow-ups (per Shahbaz's review of T0.1.3) — three deliverables landing in one follow-up commit before T0.1.4 code starts:
1. **ADR-009** in this file — retry queue policy (bounded vs unbounded, retry strategy, permanent-failure behaviour, user-visible surface, divergence verification). Gates T0.1.6 from improvising failure-recovery semantics.
2. **BRD §6 V0.1.3 perf budget** — explicit acceptance criterion added to the BRD: `open + migrate + first audit insert ≤ 200ms`, of which ≤ 150ms may be SQLCipher first-open. Honest framing: this is **adding** explicit perf criteria (the previously-cited "BRD 50ms target" did not actually exist in the BRD — that reference in the prior session was a hallucinated constraint, no such line in §6 V0.1.3). Resolves the open blocker without weakening `kdf_iter`.
3. **Dryoc API spike** — scheduled mini-task for this week (target: before T0.1.4 finishes). 2-hour scratch crate exercising dryoc 0.7's actual encrypt/decrypt API for a single-message envelope. Output updates ADR-008 with confirmed patterns. Goal: discover any vault-sync (T0.2.9) integration mismatch now, not in week 6.

**Started:** 2026-04-29 (post-T0.1.3 commit)
**Last test run:** 2026-04-29 — `cargo test -p vault-storage` 39/39 passing; build/clippy/fmt clean (no code change since T0.1.3 commit; this turn is HANDOFF.md + BRD only)

---

## Recently Completed

| Task ID | Name | Completed | Tests | Notes |
|---|---|---|---|---|
| Foundation | CLAUDE.md, HANDOFF.md, project memory files | 2026-04-28 | n/a | Pre-kickoff scaffolding (project rules + cross-session memory). Comprehensive `.gitignore` covers secrets, model files, encrypted vault data, ML binaries, Claude Code per-machine state. |
| T0.1.1 | Workspace Setup | 2026-04-28 | ✅ build/test/clippy/fmt all green on Windows local; CI green on push (39s) | 11 crate skeletons under `crates/`. Workspace `Cargo.toml` pins all BRD §4 deps in `[workspace.dependencies]`. `rust-toolchain.toml` pins stable. CI: 3-job parallel matrix (fmt, clippy, build+test) on ubuntu-latest. `git init` + remote connected to `https://github.com/shahbaz242630/Agent-Memory-Vault.git`. |
| T0.1.2 | vault-core | 2026-04-28 | ✅ 42 unit + 1 doc test passing; clippy clean; fmt clean | All BRD §5.1 types implemented across `error.rs` / `boundary.rs` / `memory.rs` / `entity.rs` with `lib.rs` re-exports. Validation enforced at construction (`try_new` constructors) AND at storage-write boundary (`validate()` method) per BRD §11.7.1. ID types use UUID v7 (time-ordered, good DB index locality). `Boundary` uses validated newtype with private field (deviation from BRD `pub String` — see ADR-005). |
| CI hardening | `actions/checkout` v4 → v6 | 2026-04-28 | ✅ CI green | Resolves Node 20 deprecation flagged on `f3923eb`. GitHub Actions runners drop Node 20 on 2026-09-16; v6 runs on Node 24. `Swatinem/rust-cache@v2` and `dtolnay/rust-toolchain@stable` are unaffected (no Node 20 dependency). Followed GitHub's recommended floating-major tag convention (`@v6`) so security patches auto-apply. |
| T0.1.3 | vault-storage: SQLite + SQLCipher (`MetadataStore` + audit chain) | 2026-04-29 (commit `f846df7`) | ✅ 39/39 passing; build/clippy/fmt clean | `MetadataStore` async API (CRUD + audit append/list/verify, every CRUD txn-atomic with audit append). Three tables via migration runner: `memories` (boundary-indexed), `audit_log` (BLAKE3 hash chain per BRD §11.9.2, genesis = 0×64, canonical sorted-key JSON), `schema_migrations` (gap + out-of-order detection refuse to run, idempotent re-runs). SQLCipher key handling: `SqlCipherKey` newtype with `Zeroize`/`ZeroizeOnDrop`, no `Debug`/`Display` (ADR-007); WAL + foreign_keys + synchronous=NORMAL. Boundary filter parameterised at SQL level (BRD §11.7.1). Audit-tamper proptest hits every byte position in chains of 2–5 events; concurrent-writes proptest validates 20-task chain serialisation via `Mutex<Connection>`. Decisions logged: ADR-006 (rusqlite vendored OpenSSL + monthly CVE check), ADR-007 (no manual `Debug` on sensitive types), ADR-008 (dryoc 0.7 API drift formalised). Perf measured: `open+migrate` ≈ 120ms, steady-state audit insert ≈ 197µs. |

---

## In Progress

**Pre-T0.1.4 follow-ups** (single follow-up commit, no code change — HANDOFF.md + BRD §6 only):

- [x] **ADR-009 — retry queue policy** (this file, ADR section)
- [x] **BRD §6 V0.1.3 perf budget added** (`Agent Build Specification.txt` line ~1257)
- [x] **Dryoc API spike scheduled** (this file, Tech Debt section, target: this week, before T0.1.4 finishes)

Once these land in the follow-up commit, T0.1.4 (vault-storage: LanceDB) starts.

---

## Pending — V0.1 (Internal Alpha)

- [ ] **T0.1.4** — vault-storage: LanceDB (vector store with boundary filtering)
- [ ] **T0.1.5** — vault-storage: DuckDB (graph store with bi-temporal columns)
- [ ] **T0.1.6** — vault-storage: Cascading Backend (StorageBackend orchestrator + retry queue per ADR-009)
- [ ] **T0.1.7** — vault-embedding (bge-small via ort)
- [ ] **T0.1.8** — vault-retrieval: Semantic Strategy Only
- [ ] **T0.1.9** — vault-mcp: Adapter + Stdio Server (memory.search, memory.write)
- [ ] **T0.1.10** — vault-app: Wiring (Application, config, startup/shutdown)
- [ ] **T0.1.11** — vault-tauri: Minimal UI (add memory, search, settings — also: convert vault-tauri from lib to bin crate)
- [ ] **T0.1.12** — V0.1 End-to-End Test (founder uses for a full day, files ≥3 issues)

V0.2 and V1.0 task lists live in BRD §6 and will be promoted here when V0.1 completes.

---

## Blockers / Decisions Needed

_None outstanding._

**Resolved:**
- ~~SQLCipher first-open cost vs. "BRD 50ms target."~~ Resolved 2026-04-29 by adding explicit BRD §6 V0.1.3 perf-budget criterion (`open + migrate + first audit insert ≤ 200ms`, ≤ 150ms first-open allowance). **Honest correction:** the prior session's reference to "BRD 50ms target" was a hallucinated constraint — no such line existed in BRD §6 V0.1.3. The fix is therefore *adding* explicit perf criteria, not revising a real prior target. PBKDF2 256k iterations stays as the security property; `kdf_iter` is not tuned down.

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

### ADR-008 — 2026-04-29 — dryoc 0.7 API differs from BRD §11.6 sketch; verify with a spike before T0.2.9
- **Context:** BRD §11.6 sync-envelope construction (T0.2.9) was sketched assuming dryoc exposes a libsodium-style single-shot AEAD (`crypto_aead_xchacha20poly1305_ietf_encrypt`). Inspecting dryoc 0.7's published API surface, the user-facing primitive is `dryocstream` — a streaming push/pull XChaCha20-Poly1305 construction, not single-shot. The BRD sketch will not compile against the actual crate as-is. Discovering this in week 6 of T0.2.9 would be expensive.
- **Decision:** Run a 2-hour API-shape spike this week (before T0.1.4 finishes) in a scratch crate: minimal end-to-end encrypt → decrypt round-trip on a single-envelope payload using dryoc 0.7's actual API. Output: confirmed integration patterns annotated back into this ADR; a follow-up amendment fixes the BRD §11.6 sketch with the real call shape. Three plausible paths the spike will choose between:
  1. **Wrap streaming in a single-message helper** (one `push` → `finalize` per envelope). Stays inside dryoc; minor wrapper cost.
  2. **Use a single-shot AEAD from a sibling crate** (orion, sodiumoxide). Avoids streaming wrapper but adds a second crypto crate to vet.
  3. **Switch to RustCrypto's `chacha20poly1305`** (XChaCha variant). Pure-Rust, no libsodium FFI; ergonomic, well-audited. Strong default if dryoc proves cumbersome.
- **Reasoning:** Discovering API mismatch in week 1 is a 2-hour problem; in week 6 it's a 2-day problem mid-implementation. Cheap insurance, scoped explicitly so it cannot scope-creep into "build T0.2.9 early."
- **Spike status:** Scheduled. Not yet run. This ADR will be amended with confirmed patterns + chosen path once the spike completes.

### ADR-009 — 2026-04-29 — Retry queue policy for cascading-backend partial failures (gates T0.1.6)
- **Context:** T0.1.3 schema reserves a `retry_queue` table for T0.1.6 (cascading writes across SQLite + LanceDB + DuckDB), but the *policy* for that queue was never specified. Without policy, T0.1.6 will improvise failure-recovery semantics — and improvisation in failure-recovery code is exactly where production data-loss bugs come from. Concrete failure mode this prevents: SQLite write succeeds, LanceDB write fails, retry never resolves; user adds a memory yesterday and searches for it today and it silently doesn't exist in the vector store. Per Shahbaz's T0.1.3 review.
- **Decision:** The cascading retry queue (lands in T0.1.6) follows this policy:
  1. **Bounded queue.** Hard cap of 10,000 pending entries. When the cap is hit, new cascading writes still succeed against SQLite (source of truth) but the system enters a *degraded mode*: vault-app raises a "vault repair required" warning to the UI, and consolidation is paused until the queue drains. Bounded prevents the queue from becoming an unbounded write-amplification disk hog.
  2. **Retry strategy.** Exponential backoff with full jitter. Base delay 1s, multiplier 2, cap 5min. **Max 5 attempts** per entry; after the 5th failure the entry moves to a *dead-letter table* (separate from the live retry queue, in the same SQLite DB).
  3. **Permanent-failure behaviour.** A dead-lettered entry triggers three things: (a) audit-log entry with `result=Error` and details of every attempt; (b) UI alert in the sync-health surface ("Memory `<id>` failed to propagate to vector/graph store after 5 attempts. Investigate before further consolidation."); (c) the affected memory is marked `divergence_pending` in SQLite — retrieval still serves the SQLite metadata, but search relevance is flagged as potentially incomplete.
  4. **User-visible surface.** A "Sync Health" indicator in the Tauri UI (lands in T0.1.11 minimal form, polished in T0.2.15). Shows: pending retries (count + oldest age), dead-lettered entries (count, requires user action), last successful full-cascade-write timestamp. Click-through to the dead-letter list with per-entry "retry" / "force-mark-divergent" / "drop" actions.
  5. **Periodic divergence verification.** A background task runs every 6 hours and on every app start: compares SQLite memory IDs against LanceDB and DuckDB tombstones. Any drift not already in the retry queue or dead-letter table is logged + alerts the user. Detects silent divergence from bugs that bypass the retry path entirely.
- **Reasoning:** Each clause closes a specific data-loss failure mode raised in Shahbaz's T0.1.3 review. Bounded > unbounded because unbounded retry queues hide the underlying corruption while consuming disk indefinitely; degradation mode forces the failure to surface. 5 attempts (not infinite) because if LanceDB is rejecting a write 5 times, the failure is likely structural (corruption, schema drift) and silent retry won't fix it. The dead-letter table keeps the live queue fast and gives users explicit visibility into "what is broken right now." Periodic divergence verification is the belt-and-braces — even if the retry queue itself has a bug, the verification job catches drift.
- **Alternatives considered:**
  - *Unbounded queue with infinite retry* (rejected: hides corruption, fills disk, never surfaces failure).
  - *Drop-on-failure* (rejected: silent data loss is the worst outcome).
  - *Block writes on any retry-queue entry* (rejected: amplifies a single corrupt store into a full vault outage).
  - *Sync directly to all three stores synchronously, no retry queue* (rejected: lockstep failure semantics are worse — any single store going down blocks the whole vault).
- **Test requirements (T0.1.6):**
  - Property test: arbitrary sequence of writes + injected partial failures → final state has every memory either active in all three stores, or dead-lettered with correct audit trail. No memory is ever silently dropped.
  - Adversarial test: force LanceDB write to fail 6 times in a row → entry moves to dead-letter on attempt 5, UI alert fires, audit log has 5 retry events.
  - Adversarial test: 10,001st cascading write while queue is at cap → SQLite write succeeds, system enters degraded mode, UI shows warning.
  - Integration test: divergence verification job on a vault with 1 known-divergent memory → detects and alerts within one cycle.
- **When to revisit:** After T0.1.6 lands and the policy meets real failure modes. After V0.2 sync introduces additional failure surface.

---

## Tech Debt Backlog

Items noticed but not addressed in their originating task — picked up explicitly when scheduled, never as drive-by work.

- [ ] **`llama-cpp-2 = "0.1"` not yet declared in `[workspace.dependencies]`** — BRD §4.3 flags it for verification at the start of vault-llm work (T0.2.1). Do crate-name-and-version verification on docs.rs at that point and add to workspace deps then. (Noted 2026-04-28, file: `Cargo.toml`)
- [ ] **`.gitattributes` for line-ending normalisation** — currently relying on git's default `core.autocrlf=true` on Windows. Adding `* text=auto eol=lf` plus binary markers for known binary file types would silence the CRLF warnings on commit and make cross-platform behaviour deterministic. Quick win when convenient. (Noted 2026-04-28)
- [ ] **dryoc 0.7 streaming-vs-single-shot — RUN THE SPIKE THIS WEEK** — per ADR-008. 2-hour scratch crate, single-envelope encrypt → decrypt round-trip using actual dryoc 0.7 API. Output: ADR-008 amended with confirmed call shape; BRD §11.6 sketch updated to compile against reality. **Target: complete before T0.1.4 finishes** so we know whether we're using dryoc, RustCrypto, or another crate before T0.2.9 design starts. (Noted 2026-04-28, scheduled 2026-04-29)
- [ ] **(Recurring) Monthly OpenSSL CVE check** — per ADR-006. First Monday of each month, review https://www.openssl.org/news/vulnerabilities.html and the OpenSSL version vendored by `openssl-src` (`cargo tree -p openssl-src` from this workspace). Critical / High advisories affecting the vendored version → prioritise `cargo update -p openssl-src` ahead of other work. Next due: 2026-05-04. (Noted 2026-04-28)

---

## V0.1 Findings

_(populated once V0.1 ships)_

---

## Notes for Next Session

**Immediate state:** T0.1.3 committed and pushed (`f846df7`). Pre-T0.1.4 follow-ups (ADR-009 + BRD §6 perf criterion + dryoc spike scheduled) staged for the follow-up commit. After that commit lands, T0.1.4 (vault-storage: LanceDB) starts.

**Pace caution:** Per Shahbaz's T0.1.3 review — "Watch for the temptation to move faster on T0.1.4 because momentum feels good. LanceDB integration has its own subtleties — vector dimension consistency, IVF index parameters, encryption-at-filesystem-level for the data dir. Don't let velocity override the same thoroughness." Keep T0.1.4 at the same test depth as T0.1.3 (Heavy: round-trip, boundary-leak proptest, concurrent-write test).

**Two of the five tables in BRD §5.2 are intentionally deferred** to the tasks that actually need them:
- `review_queue` — added at the connector ingestion task (BRD §5.9)
- `sync_state` and `retry_queue` — added at T0.1.6 (cascading backend per ADR-009) / T0.2.x (sync engine)

Per the "no scaffolding for unused features" rule (CLAUDE.md hard rule). Each table will land via a new numbered migration file (`0002_review_queue.sql`, etc.) — the migration runner already supports this and is regression-tested by `forward_migration_applies_next_version_only`.

**Next task — T0.1.4 — vault-storage: LanceDB vector store (BRD §5.2.2):**

- Add `lancedb` and `arrow` deps to `crates/vault-storage/Cargo.toml` (versions pinned per BRD §4)
- Implement `VectorStore` trait + `LanceVectorStore` struct: `insert`, `update`, `delete`, `search(query_vec, boundary, k) -> Vec<(MemoryId, score)>`, `purge_boundary(b)` for boundary deletion
- **Boundary filtering must happen at the LanceDB query layer**, not after-the-fact — same security principle as the SQL boundary filter we just shipped. This is the place where it's easiest to slip; reread BRD §11.4.3 before writing the search method.
- Encryption: LanceDB does not natively support encryption-at-rest. Decision needed at T0.1.4 kickoff: use a wrapper that encrypts the parquet files at OS-FS level (recommended), or store vectors inside SQLCipher (slower, scaling concerns). Document as an ADR.
- Test level: **Heavy** — round-trip tests, boundary filter cannot leak, property tests for vector insert/search consistency, concurrent-write test
- Tie-in to T0.1.3: the next migration (`0002_…sql`) may add a `vector_id` column to `memories` if we choose a model where SQLite holds the cross-store reference

**Working-tree state at the start of this turn:** clean (T0.1.3 committed at `f846df7`, pushed). Pending changes in this turn: `HANDOFF.md` (this file) + `Agent Build Specification.txt` (§6 V0.1.3 perf-budget criterion added). No code change.
