# Memory Vault — Build Handoff

**Last updated:** 2026-04-30 (T0.1.6 phase A shipped, phase B next session)
**Updated by:** Claude (Opus 4.7)
**Current version:** V0.1 — Internal Alpha
**Current phase:** V0.1 in progress — **T0.1.6 shipping in three phases per the agreed plan.** Phase A (commit `34b7715`) shipped: SQL migration 0002 (retry_queue + dead_letter + pending_sync) + the planning doc `T0.1.6_PLAN.md`. Phase B (data-layer modules: retry_queue.rs + dead_letter.rs + pending_sync.rs + tests) is next session's work. Phase C (cascading orchestrator + divergence + vault-cli + ADRs) closes the task.

---

## Current Status

**Active item:** **T0.1.6 phase B — data-layer modules.** Per `T0.1.6_PLAN.md` and the three-phase ship plan agreed on 2026-04-30. Phase A is done; phase B starts next session with **`retry_queue.rs` test-first per BRD §0.3**, then `dead_letter.rs` + `pending_sync.rs`. Estimated ~1100 LoC across the three modules + their unit tests. Phase C (cascading orchestrator + adversarial tests + divergence + vault-cli + ADR amendments) ships separately as the third commit.

**Phase A's deliverables (already on `origin/main`):**
1. `crates/vault-storage/src/migrations/0002_cascade_infra.sql` — three new tables + indexes per `T0.1.6_PLAN.md`. Strict FIFO via `(memory_id, sequence_id)` UNIQUE on retry_queue, partial index on unresolved dead-letter, UPSERT-semantics PK on pending_sync.
2. `T0.1.6_PLAN.md` — full design, status `Approved` with all seven Shahbaz refinements applied.
3. `forward_migration_applies_next_version_only` test refactored to use synthetic migration lists (robust against future migration count changes).

**Last test run:** 2026-04-30 (commit `34b7715`) — `cargo test --workspace` **140 passing** (vault-core 45, vault-storage 94 [+2 new for 0002], doc 1); build/clippy/fmt clean.

---

## Recently Completed

| Task ID | Name | Completed | Tests | Notes |
|---|---|---|---|---|
| Foundation | CLAUDE.md, HANDOFF.md, project memory files | 2026-04-28 | n/a | Pre-kickoff scaffolding (project rules + cross-session memory). Comprehensive `.gitignore` covers secrets, model files, encrypted vault data, ML binaries, Claude Code per-machine state. |
| T0.1.1 | Workspace Setup | 2026-04-28 | ✅ build/test/clippy/fmt all green on Windows local; CI green on push (39s) | 11 crate skeletons under `crates/`. Workspace `Cargo.toml` pins all BRD §4 deps in `[workspace.dependencies]`. `rust-toolchain.toml` pins stable. CI: 3-job parallel matrix (fmt, clippy, build+test) on ubuntu-latest. `git init` + remote connected to `https://github.com/shahbaz242630/Agent-Memory-Vault.git`. |
| T0.1.2 | vault-core | 2026-04-28 | ✅ 42 unit + 1 doc test passing; clippy clean; fmt clean | All BRD §5.1 types implemented across `error.rs` / `boundary.rs` / `memory.rs` / `entity.rs` with `lib.rs` re-exports. Validation enforced at construction (`try_new` constructors) AND at storage-write boundary (`validate()` method) per BRD §11.7.1. ID types use UUID v7 (time-ordered, good DB index locality). `Boundary` uses validated newtype with private field (deviation from BRD `pub String` — see ADR-005). |
| CI hardening | `actions/checkout` v4 → v6 | 2026-04-28 | ✅ CI green | Resolves Node 20 deprecation flagged on `f3923eb`. GitHub Actions runners drop Node 20 on 2026-09-16; v6 runs on Node 24. `Swatinem/rust-cache@v2` and `dtolnay/rust-toolchain@stable` are unaffected (no Node 20 dependency). Followed GitHub's recommended floating-major tag convention (`@v6`) so security patches auto-apply. |
| T0.1.3 | vault-storage: SQLite + SQLCipher (`MetadataStore` + audit chain) | 2026-04-29 (commit `f846df7`) | ✅ 39/39 passing; build/clippy/fmt clean | `MetadataStore` async API (CRUD + audit append/list/verify, every CRUD txn-atomic with audit append). Three tables via migration runner: `memories` (boundary-indexed), `audit_log` (BLAKE3 hash chain per BRD §11.9.2, genesis = 0×64, canonical sorted-key JSON), `schema_migrations` (gap + out-of-order detection refuse to run, idempotent re-runs). SQLCipher key handling: `SqlCipherKey` newtype with `Zeroize`/`ZeroizeOnDrop`, no `Debug`/`Display` (ADR-007); WAL + foreign_keys + synchronous=NORMAL. Boundary filter parameterised at SQL level (BRD §11.7.1). Audit-tamper proptest hits every byte position in chains of 2–5 events; concurrent-writes proptest validates 20-task chain serialisation via `Mutex<Connection>`. Decisions logged: ADR-006 (rusqlite vendored OpenSSL + monthly CVE check), ADR-007 (no manual `Debug` on sensitive types), ADR-008 (dryoc 0.7 API drift formalised). Perf measured: `open+migrate` ≈ 120ms, steady-state audit insert ≈ 197µs. |
| T0.1.4 | vault-storage: LanceDB (`VectorStore` trait + `LanceVectorStore`) | 2026-04-29 (commit `710576c`) | ✅ vault-storage 59/59 + vault-core 44/44 + doc 1/1 = **104/104** passing; build/clippy/fmt clean | Domain-only `VectorStore` trait (BRD §2.2): `MemoryId` / `Boundary` / `&[f32]` only — Arrow types stay inside the impl. Mandatory access control on `search` via non-Optional `&[Boundary]` (BRD §11.4.3); empty slice returns empty result, not error — compile-time impossible to "forget to filter." `LanceVectorStore` on lancedb 0.8 + arrow 52.1: `merge_insert(&["id"])` with id-only match column (boundary-change updates in place, no duplicates), `DistanceType::Cosine` calibrated for L2-normalised bge-small embeddings, `only_if` filter via `build_boundary_filter` + `quote_sql_string` (defense-in-depth atop Boundary's tightened charset). 20 new tests including boundary-leak proptest and 20-task concurrent-upserts test. ADRs landed: 010 (V0.1 plaintext-on-disk HARD GATE before T0.2.16, four loud compensating controls), 011 (protoc + monthly CVE check), 012 (lance feature-minimisation investigated, no flags available, accept), 013 (chrono `=0.4.38` pin policy with revisit triggers + monthly CVE check). Boundary tightened to `[a-zA-Z0-9_-]{1,64}` in vault-core (ADR-005 amended). BRD §6 V0.1.3 perf budget added; BRD §6 V0.2 T0.2.0 (LanceDB encryption-at-rest) added as new first task with HARD GATE for T0.2.16. |
| T0.1.4 follow-ups | ADR-014 (ALPHA-file fail-soft) + ADR-008 dryoc spike PASSED | 2026-04-29 (commit `9effa1a`) | ✅ vault-storage **60/60** + vault-core 44/44 + doc 1/1 = **105/105** passing; build/clippy/fmt clean; `cargo run -p vault-sync --example dryoc_spike` exit 0 | **ADR-014:** `LanceVectorStore::open` no longer fails on alpha-warning-file write errors (read-only data dir, quota, FS error). Startup WARN log (ADR-010 control #3) is the **primary** safety control; the file (#4) is **secondary**. Test `open_succeeds_when_alpha_file_write_fails_per_adr_014` pre-creates the alpha path as a directory so `fs::write` fails on every platform; asserts `open()` succeeds and the store remains functional. ADR-010 compensating-control #4 amended to mark the file as secondary + reference ADR-014. **ADR-008 dryoc spike:** `crates/vault-sync/examples/dryoc_spike.rs` (~180 lines) — round-trip + adversarial (wrong-key, single-bit ciphertext flip) all pass. Findings annotated into ADR-008: required imports (`Bytes` + `NewByteArray` from `dryoc::types` — BRD §11.6 sketch missed these), `Sized` quirk (pass `&Vec<u8>` not `&[u8]`), 24-byte opaque header, 17-byte AEAD overhead per envelope, `Tag::FINAL = PUSH \| REKEY` libsodium bit layout. Path #1 (streaming-as-single-message) chosen over path #2 (sibling crate) and path #3 (RustCrypto). dryoc declared as `[dev-dependencies]` in vault-sync (not promoted to `[dependencies]` until T0.2.9). |
| ADR-008 lock-down | AAD scheme + chunk-size policy | 2026-04-29 (commit `5fdf0d8`) | ✅ no code change; ADR-only commit | **AAD scheme** locked: `AAD = BLAKE3("vault-aad-v1" \|\| memory_id_bytes \|\| boundary_bytes)`. Domain-separator prefix prevents collision with the audit-log BLAKE3 chain in vault-storage; `v1` suffix lets us rotate later without ambiguity; fixed 32-byte AAD regardless of boundary length. T0.2.9 passes this as `aad` to `push_to_vec` / `pull_to_vec`; wrong metadata in transit → AEAD auth fails → decryption fails closed. **Chunk-size policy:** ≤ 1MB plaintext = single-shot (one `push_to_vec(..., FINAL)`); > 1MB = chunked streaming. V0.1/V0.2 single-shot only — BRD §11.7.1 caps memories at 100KB, far below the threshold. Triggers to re-evaluate listed in ADR-008. ADR-008 is now fully retired; T0.2.9 can proceed without re-investigation. |
| T0.1.5 | vault-storage: DuckDB (`GraphStore` trait + `DuckDbGraphStore` + ADR-015 boundary scoping) | 2026-04-30 (commit `062e17f`) | ✅ vault-core **45/45** + vault-storage **92/92** + doc 1/1 = **138/138** passing; build/clippy/fmt clean | Domain-only `GraphStore` trait (BRD §2.2): `Entity` / `Relationship` / `EntityId` / `RelationshipId` / `Boundary` only — DuckDB types stay inside `DuckDbGraphStore`. Mandatory access control on `traverse` via non-Optional `&[Boundary]` (BRD §11.4.3); empty slice returns empty result, not error — same compile-time discipline as `LanceVectorStore::search`. **`TraversalOptions` struct** (Shahbaz refinement during T0.1.5 review) groups `max_hops` + `relation_filter` + `follow_aliases` so the trait signature stays stable as V0.2 adds knobs (`include_archived`, time-range filters) without breaking callers; mandatory params (`from`, `authorized_boundaries`) stay positional. **`DuckDbGraphStore`** on duckdb 1.2 (workspace pin "1.0" = ^1.0; T0.1.5b will exact-pin per BRD §2.9): `Connection: Send + !Sync` wrapped in `Mutex<Connection>` + `spawn_blocking` mirror of `MetadataStore`. Schema in `migrations_graph/0001_initial.sql`: `entities` with composite `UNIQUE (name, entity_type, boundary)` (ADR-015 watch #3), `relationships` with bi-temporal `valid_from` / `valid_until` / `confidence` from day one (watch #2) + denormalised `boundary` for fast traversal-time SQL filtering. **ADR-015 enforcement mechanism locked: app-layer in `create_relationship`** because DuckDB 1.x supports neither subquery-CHECK nor triggers; property test fuzzes the API as the SQL-layer backstop's substitute. **Recursive CTE traversal** with `list_append`-tracked relationship-id path (BLOB[]) for full-chain reconstruction, `list_position` cycle break, depth-bounded with strict respect for `max_hops` (Shahbaz-added test verifies a 5-hop graph queried with `max_hops=2` returns nothing past hop 2). Boundary filter applies at every recursion step + at the final entity join (defense in depth, watch #1). 28 graph_store tests + 4 migrations_graph tests including: cross-boundary rejection (`AccessDenied`), `same_as` / `alias_for` schema permissiveness (forward-compat), `follow_aliases=true` traverses alias edges but still respects `authorized_boundaries` (not a privilege escalation), bi-temporal supersede atomicity, 20-task concurrent-creates, and a property test fuzzing arbitrary small graphs to confirm zero boundary leaks. **vault-core amendment:** `Entity` gained `boundary: Boundary` field + `NewEntity` builder mirroring `NewMemory`. ADR-015 logged with three Shahbaz-supplied additions (schema-level enforcement specifics, `same_as` forward-compat pattern, consolidator policy contract for T0.2.2). T0.1.5b cleanup task logged in tech debt: workspace caret-pin → exact-pin sweep per BRD §2.9. **Tech-debt note:** discovered DuckDB 1.2.2 wedges on autocommit INSERT UNIQUE-violation; worked around with pre-flight `SELECT COUNT(*)` inside explicit tx in `create_entity` (logged for revisit on duckdb upgrade). |
| T0.1.5b | Workspace caret pins → exact pins per BRD §2.9 | 2026-04-30 (commit `bc1ffca`) | ✅ Cargo.lock byte-identical post-edit (success criterion); 138/138 passing; build/clippy/fmt clean | Mechanical edit scoped to workspace `Cargo.toml` + HANDOFF.md only. Replaced caret syntax (`"X.Y"`) with exact pins (`"=X.Y.Z"`) for every dep that has a workspace-member consumer, using the version currently resolved in `Cargo.lock`: tokio `=1.52.1`, async-trait `=0.1.89`, futures `=0.3.32`, thiserror `=2.0.18`, anyhow `=1.0.102`, serde `=1.0.228`, serde_json `=1.0.149`, tracing `=0.1.44`, tracing-subscriber `=0.3.23`, uuid `=1.23.1`, lancedb `=0.8.0`, arrow-array `=52.2.0`, arrow-schema `=52.2.0`, duckdb `=1.2.2`, rusqlite `=0.32.1`, dryoc `=0.7.2`, reqwest `=0.12.28`, zeroize `=1.8.2`, blake3 `=1.8.5`, proptest `=1.11.0`, tokio-test `=0.4.5`, tempfile `=3.27.0`. `chrono =0.4.38` left as-is (already exact per ADR-013). **Deps NOT in `Cargo.lock` yet** (no workspace member consumes them): `ort`, `tokenizers`, `yrs`, `rmcp`, `tauri`, `tauri-build`, `mockall`. These stay caret-pinned with an inline comment explaining they get exact-pinned at the task that first wires them into a member crate (T0.1.7 / T0.1.9 / T0.1.11 / T0.2.1 / T0.2.9). The lazy `workspace.dependencies` semantics means caret-pinned-but-unused entries cannot drift in the lockfile until first use — safe by construction. **Verification:** `git diff Cargo.lock` returns zero lines after the edit + `cargo build --workspace` succeeds; all four DoD gates re-pass; tests unchanged at 138/138. No code changed. |
| T0.1.6a | T0.1.6 phase A: SQL schema + planning doc | 2026-04-30 (commit `34b7715`) | ✅ vault-core 45/45 + vault-storage **94/94** (+2 new migration tests) + doc 1/1 = **140/140** passing; build/clippy/fmt clean | First of three commits shipping T0.1.6. Migration `0002_cascade_infra.sql` adds three tables to the SQLite metadata DB: `retry_queue` (UUID v7 PK + `(memory_id, sequence_id)` UNIQUE for FIFO-per-memory ordering anchored to audit chain + `(next_attempt_at)` index for worker polling), `dead_letter` (resolution enum: pending / retried_succeeded / retried_failed / acknowledged / auto_recovered, partial index on unresolved for CLI list query), `pending_sync` (UPSERT semantics on `memory_id` for cap-overflow catch-up). `T0.1.6_PLAN.md` lands at repo root as the historical reference for the design — Status: Approved with all seven Shahbaz refinements applied (corruption soft-fail test, FIFO ordering invariant, is_permanent classifier, pending_sync overflow path, deterministic stratified divergence sampling, vault-cli passphrase auth, background task lifecycle contract). `forward_migration_applies_next_version_only` test refactored to use synthetic migration lists so it stays robust against future migration count changes (T0.1.6b will add 0003 for sync_state in T0.2.x — this test won't break). Phase B (retry_queue.rs + dead_letter.rs + pending_sync.rs + module tests, ~1100 LoC) is next-session's work. |

---

## In Progress

**T0.1.6 phase B — data-layer modules.** Next session starts here. Order:

1. **`retry_queue.rs`** — test-first (BRD §0.3). Tests: insert + dequeue ordering (strict FIFO per `memory_id` by `sequence_id` ASC), backoff scheduling (8 attempts: 1/2/4/8/16/30/60/120s with ±25% jitter; verify with mocked clock so tests stay deterministic), `is_permanent` classifier (DimensionMismatch / AccessDenied / schema-mismatch dead-letter on attempt 1), payload round-trip (JSON + format version). Implementation: `Mutex<Connection>` + `spawn_blocking` mirror of MetadataStore.
2. **`dead_letter.rs`** — test-first. Tests: insert, list-unresolved, resolution-state transitions (pending → retried_succeeded / retried_failed / acknowledged / auto_recovered, with `resolved_at` set), idempotent re-resolution rejected, payload round-trip.
3. **`pending_sync.rs`** — test-first. Tests: UPSERT semantics (latest operation supersedes), oldest-first dequeue, FK-orphan handling.

Phase C ships separately: `cascading.rs` orchestrator + `divergence.rs` + `FaultInjector` + 5 adversarial tests + new `crates/vault-cli/` crate with passphrase auth + ADR-009 amendment + ADR-016 + final HANDOFF entry.

---

## Pending — V0.1 (Internal Alpha)

- [ ] **T0.1.6** — vault-storage: Cascading Backend (StorageBackend orchestrator + retry queue per ADR-009)
- [ ] **T0.1.7** — vault-embedding (bge-small via ort)
- [ ] **T0.1.8** — vault-retrieval: Semantic Strategy Only
- [ ] **T0.1.9** — vault-mcp: Adapter + Stdio Server (memory.search, memory.write)
- [ ] **T0.1.10** — vault-app: Wiring (Application, config, startup/shutdown)
- [ ] **T0.1.11** — vault-tauri: Minimal UI (add memory, search, settings — also: convert vault-tauri from lib to bin crate). **MUST also implement two ADR-010 compensating controls:** (a) modal first-run banner — non-dismissible until acknowledged, click recorded in metadata_store; (b) persistent UI banner at the top of every launch ("ALPHA — vector store is unencrypted. V0.2 fixes this."). Both removed by T0.2.0. Easy to forget when UI work starts months from now — both items are pre-committed here.
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

### ADR-008 — 2026-04-29 — dryoc 0.7 API differs from BRD §11.6 sketch; verified by spike, path #1 chosen
- **Context:** BRD §11.6 sync-envelope construction (T0.2.9) was sketched assuming dryoc exposes a libsodium-style single-shot AEAD (`crypto_aead_xchacha20poly1305_ietf_encrypt`). Inspection of dryoc 0.7's API confirmed: the user-facing XChaCha20-Poly1305 primitive is `DryocStream` (the streaming construction `crypto_secretstream_xchacha20poly1305`); `dryoc::classic` lists `crypto_secretbox` (XSalsa20-Poly1305 — different cipher), `crypto_box`, `crypto_secretstream_xchacha20poly1305`, but no `crypto_aead_xchacha20poly1305_ietf` single-shot module. The BRD sketch as-written does not compile against dryoc 0.7. Three candidate paths from earlier draft:
  1. **Wrap streaming as single-message** — one `init_push` → `push_to_vec(plaintext, AAD, Tag::FINAL)` → `init_pull` → `pull_to_vec` cycle per envelope. Stays inside dryoc.
  2. **Sibling crate** (`orion`, `sodiumoxide`) — single-shot AEAD with XChaCha20. Adds a second crypto crate to vet.
  3. **RustCrypto `chacha20poly1305`** — pure-Rust, single-shot XChaCha20-Poly1305 AEAD. Drop dryoc.
- **Decision:** Take **path #1 — wrap streaming as single-message.** Verified by `crates/vault-sync/examples/dryoc_spike.rs` (run 2026-04-29, exit 0, all assertions held). Spike covered the full round-trip + adversarial tampering; the streaming construction with a single `Tag::FINAL` push is materially equivalent to a single-shot AEAD for our envelope use case.
- **Spike findings (run 2026-04-29):**
  - **API path:** `use dryoc::dryocstream::{DryocStream, Header, Key, Push, Pull, Tag}` + `use dryoc::types::{Bytes, NewByteArray}`. Both `Bytes` (for `header.as_slice()`) and `NewByteArray` (for `Key::gen()`) are trait imports the BRD sketch missed.
  - **Sized-input quirk:** `push_to_vec` and `pull_to_vec` are generic over `Input: Bytes`, but `Bytes` requires `Sized`. Passing a `&[u8]` slice fails the bound (the slice ref is sized but Input is inferred as the unsized `[u8]`). Materialise plaintext / ciphertext as `Vec<u8>` and pass `&Vec<u8>` — Input is then inferred as `Vec<u8>`, sized. T0.2.9 envelope code must do the same; document at the call site.
  - **Header is 24 bytes**, opaque, must travel alongside the ciphertext (the spike concatenates: `envelope = header || ciphertext`). On the receive side: `let header: Header = bytes.try_into()?;` recovers the typed header from the leading 24 bytes.
  - **AEAD overhead is 17 bytes per envelope** (16 Poly1305 tag + 1 message tag byte). Plaintext 86 → ciphertext 103. Total wire size: 127 bytes for an 86-byte plaintext.
  - **`Tag::FINAL` is `PUSH | REKEY`** in libsodium's bit layout (FINAL = 0x03 = 0x01 | 0x02). `matches!(tag, Tag::FINAL)` correctly identifies it; the `Debug` output renders the constituent bits (`Tag(PUSH | REKEY)`) which is initially confusing but harmless. Document at call sites that recover the tag.
  - **AAD parameter:** `push_to_vec(&plaintext, aad: Option<&[u8]>, Tag::FINAL)`. The spike passes `None`; T0.2.9 binds `aad = Some(&memory_id_bytes_concat_boundary_bytes)` per BRD §11.3.2 ("AAD includes memory ID and boundary").
  - **Adversarial assertions held:** wrong-key decryption returns `Err`; single-bit ciphertext flip returns `Err`. AEAD authenticity check works as expected.
- **BRD §11.6 reconciliation:** The BRD sketch's call signature (`crypto_aead_xchacha20poly1305_ietf_encrypt(...)`) is not what we'll write. The actual T0.2.9 envelope code uses the streaming-as-single-message wrapper documented above. BRD §11.6 will be amended at T0.2.9 kickoff to match the real shape.
- **Reasoning for path #1 over alternatives:**
  - **Path #1 (chosen):** Already integrated; spike confirms the cipher and AEAD properties we want; only ergonomic cost is the per-envelope `Vec<u8>` materialisation and a thin wrapper that bundles `header || ciphertext`. Manageable.
  - **Path #2 (sibling crate):** Would mean vetting a second cryptography crate (license, audit history, maintenance, dep tree) for marginal API ergonomics. Crypto crates are not a place to multiply-source.
  - **Path #3 (RustCrypto):** Pure-Rust + single-shot is appealing, but means dropping dryoc entirely (we're already pinned via BRD §4.4) and re-evaluating the decision behind dryoc's selection. Defer until either dryoc proves problematic in production or a major version bump invalidates this spike.
- **Spike artifact:** `crates/vault-sync/examples/dryoc_spike.rs` (~140 lines) — kept long-term as executable documentation of the working pattern. Re-runnable via `cargo run -p vault-sync --example dryoc_spike` to verify dryoc's behaviour after future bumps. dryoc declared as `[dev-dependencies]` in `vault-sync` so the spike compiles without making vault-sync formally depend on dryoc until T0.2.9.
- **Spike status:** **Run 2026-04-29 — PASSED.** Path #1 viable. T0.2.9 will use the documented call shape.

- **AAD scheme (locked here so T0.2.9 doesn't redesign):** Per BRD §11.3.2, AAD must bind both `memory_id` and `boundary` to the ciphertext so an attacker cannot swap a ciphertext from one (memory, boundary) context into another and have it still validate. The construction:
  ```text
  AAD = BLAKE3("vault-aad-v1" || memory_id_bytes || boundary_bytes)
  ```
  - **Domain-separator prefix `"vault-aad-v1"`** — prevents AAD collision with any other BLAKE3 use in the workspace (the audit chain already uses BLAKE3 in `vault-storage`; we don't want algebraic crossover). The `v1` suffix lets us rotate the AAD scheme later (e.g., adding a device-id binding) without ambiguity — older envelopes carry `vault-aad-v1`, newer ones carry `vault-aad-v2`, and a receiver can dispatch on the version to pick the right reconstruction.
  - **Inputs:** `memory_id_bytes` is the 16-byte UUID v7 from `MemoryId.0.as_bytes()`; `boundary_bytes` is `boundary.as_str().as_bytes()` (≤ 64 bytes per the tightened `Boundary` charset, ADR-005 amendment). Concatenation is unambiguous because `memory_id_bytes` has fixed length 16 — no length-prefix needed.
  - **Output:** 32-byte BLAKE3 hash. Fixed-size regardless of boundary length, which simplifies envelope framing.
  - **Why hash and not just concat:** (a) fixed AAD size, (b) the BLAKE3 prefix-free property gives a clean domain separation, (c) boundary names — even validated — don't need to appear in the AAD verbatim. Defense-in-depth without performance cost (BLAKE3 of 80 bytes is sub-microsecond).
  - **T0.2.9 must:** compute the AAD via `blake3::hash` over the three concatenated parts and pass it as the `aad` argument to `push_to_vec` and `pull_to_vec`. Wrong AAD → AEAD authentication fails → decryption returns `Err`. The receiver re-derives the AAD from the envelope's `(memory_id, boundary)` metadata before decrypting; if the metadata was tampered with in transit, the AAD differs and decryption fails closed.

- **Chunk-size policy (when to graduate from single-shot to true streaming):** Path #1 (single `push_to_vec` per envelope) is the right shape for **V0.1, V0.2, and likely V1.x** because BRD §11.7.1 caps memory content at 100KB. Single-shot at 100KB is comfortable: ~120KB envelope after AEAD overhead, in-memory copy is sub-millisecond, no fragmentation pressure on the AEAD construction.
  - **Single-shot threshold:** apply path #1 unconditionally for memories ≤ **1 MB** of plaintext content. The 1MB ceiling is roughly where in-memory copy + a single AEAD call become noticeable on cold paths (memory-mapped files become preferable, sync-payload chunks start mattering).
  - **Above 1MB:** switch to chunked streaming using the same `DryocStream` API but with multiple `push_to_vec(..., Tag::MESSAGE)` calls followed by a final `Tag::FINAL`. The `init_push` / `init_pull` setup is identical; only the inner loop changes. The header still travels once at envelope start.
  - **Revisit triggers:** (a) BRD §11.7.1 raises the per-memory content cap above 1MB; (b) connectors (V1.0+) start ingesting attachments / transcripts that produce >1MB payloads; (c) the consolidator's checkpoint snapshots (BRD §5.6) exceed 1MB after compression. Any of these → re-evaluate the single-shot vs chunked split, write the chunked variant, add a routing helper that picks based on size.
  - **For V0.1 (founder-only alpha):** single-shot only. No chunked code path. Keep it simple.

- **When to revisit:** T0.2.9 implementation kickoff (re-run spike to verify, then write the production envelope code using the AAD scheme + single-shot policy above). On any dryoc minor-version bump (re-run spike to confirm API still matches). On the chunk-size triggers listed above. On any path-decision concern (e.g., perf measurements showing the per-envelope wrapper is too costly — unlikely at our throughput).

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

### ADR-010 — 2026-04-29 — LanceDB stores plaintext on disk for V0.1 only; T0.2.0 is a HARD GATE before any beta user
- **Status:** APPROVED 2026-04-29 by Shahbaz. Per BRD §11.15 escalation discipline.

- **Explicit deviation from BRD §11.5.1:** This ADR documents a *bounded, time-limited deviation* from BRD §11.5.1 ("All data on disk is encrypted. No exceptions"). The deviation applies **only to the V0.1 internal-alpha distribution on Shahbaz's own dev machine**. It does not apply to V0.2 or any external user. T0.2.0 (added to BRD §6 V0.2 by this commit) closes the deviation before any beta user installs the product.

- **V0.1 alpha scope:** The deviation is in force from this commit through end of V0.1 only. V0.1 is founder-only, manual entry, no cloud sync, no external distribution. The plaintext exposure surface is one machine — Shahbaz's. The exception expires at the moment T0.2.0 lands; no further authorisation extends it.

- **Context:** T0.1.4 introduces LanceDB as the vector store. LanceDB 0.8 has no native at-rest encryption — it writes plaintext Parquet files to a data directory. BRD §11.5.1 prescribes encrypted-data-dir-via-dryoc-into-tmpfs, but two obstacles block applying that prescription in T0.1.4: (1) the dryoc API is unresolved (ADR-008 spike has not run), so building on top of it now means rewriting after the spike; (2) the BRD's Windows half ("memory-only handle") is under-specified — Windows has no built-in tmpfs, and a proper sealed-`ObjectStore` adapter is its own architecture project. Three options were evaluated:
  - **A. Plaintext on disk for V0.1, encryption gates V0.2 via T0.2.0.** Approved.
  - B. Half-baked encryption now (sealed-tarball decrypted to a temp dir on open, re-encrypted on close). Process crash leaves plaintext temp dir; not actually "encrypted at rest" in any threat-model-meaningful sense.
  - C. Skip LanceDB for V0.1 entirely — store 384-dim embeddings as BLOBs in SQLCipher's `memories` table, brute-force cosine in-memory. Honors §11.5.1 literally but defers the LanceDB integration risk to V0.2.

- **Reasoning:**
  - **V0.1 distribution is founder-only.** BRD §6 V0.1: "Founder can install the app on their Mac." No external user has disk access. The §11.1 threat "compromised endpoint" still applies but at a different risk magnitude than production.
  - **Half-baked crypto is worse than no claim.** A sealed-tarball-on-close scheme that leaves plaintext temp dirs on crash violates CLAUDE.md ("no half-finished implementations") and would suggest a guarantee we don't deliver.
  - **The dryoc question must be answered first.** Building cryptographic layers on an unverified API is exactly the rework the ADR-008 spike is meant to prevent.
  - **Skipping LanceDB defers a different risk.** The BRD chose LanceDB after vector-DB evaluation; testing that integration in V0.1 surfaces real issues (vector dim consistency, IVF parameters, Arrow schema, query layer boundary filtering). Option C means V0.2 carries two large unknowns instead of one.
  - **The deviation is bounded and reversible.** Plaintext window scoped to V0.1 founder-only. Adding the encryption layer in T0.2.0 is additive — no LanceDB code from T0.1.4 needs to change beyond the data-dir wrapper.

- **HARD GATE — T0.2.0 before T0.2.16:** T0.2.0 (LanceDB Encryption at Rest) is added to BRD §6 V0.2 as a hard dependency for T0.2.16 (Beta Onboarding). **If T0.2.0 slips, V0.2 ship date slips.** No external user receives a build that contains the V0.1 plaintext-LanceDB code path. T0.2.0's Definition of Done includes a test that asserts the data dir contains no plaintext Parquet files after a write/close cycle, and that all four V0.1 compensating controls (below) are removed from the codebase.

- **Compensating controls — loud, not buried:** Every one of these is mandatory for the V0.1 build that lands at T0.1.4. All four are removed automatically by T0.2.0:
  1. **Modal first-run banner — not dismissible until acknowledged.** Tauri webview shows a modal on first launch: "ALPHA BUILD — your vector data is stored UNENCRYPTED on disk. Do NOT put real personal data, credentials, or sensitive information into this vault. Encryption ships in V0.2 before any beta user receives the product." User must click an "I understand" button to proceed; the click is recorded in `metadata_store` (so we can verify acknowledgement during alpha review). Lands at T0.1.11.
  2. **Persistent banner at top of UI on every launch.** A small but always-visible warning strip at the top of the app window: "ALPHA — vector store is unencrypted. V0.2 fixes this." Persists across sessions until T0.2.0 ships. Lands at T0.1.11.
  3. **WARN-level log on every startup** if the data dir is unencrypted. Emitted by `vault-storage` at LanceDB open: `tracing::warn!("LanceDB data dir is plaintext (V0.1 alpha — see ADR-010). Encryption layer ships in T0.2.0.")`. Lands at T0.1.4. Visible in any tracing subscriber, including the dev console and any future log forwarder.
  4. **`ALPHA_DO_NOT_STORE_REAL_DATA.txt` file in the data dir.** Auto-written on first LanceDB open if absent. Content: explicit warning + ADR-010 reference + T0.2.0 issue tracker pointer + creation timestamp. Read-only on Mac/Linux (chmod 0444), not deletable from the UI. Lands at T0.1.4.
  - **Removal trigger:** T0.2.0's DoD includes deleting all four. The modal/banner code paths are removed from the Tauri commands; the `warn!` log emits an `info!` "encryption active" message instead; the `ALPHA_DO_NOT_STORE_REAL_DATA.txt` file is deleted on T0.2.0 first-run with a one-time `info!` log noting the upgrade.

- **Other (always-on, not gated to T0.2.0):**
  - **Boundary filtering at the LanceDB query layer is non-negotiable** (BRD §11.4.3, BRD §11.7.1). Encryption deferred ≠ access control deferred. Filter implemented at `lance` query construction, not in application code post-fetch.
  - **Memory inserts still go through boundary-validated `vault-core` types.** Plaintext-on-disk does not relax input validation.
  - **The audit log (in SQLCipher) records every LanceDB write/search.** Cascading backend (T0.1.6) wires this through.

- **Alternatives considered:**
  - *Option B (half-baked encryption):* rejected per "Reasoning" above.
  - *Option C (vectors-in-SQLCipher):* rejected — defers LanceDB integration risk to V0.2; nontrivial in-memory cosine code we'd throw away.
  - *Whole-disk encryption (FileVault/BitLocker):* rejected — out of scope; doesn't satisfy §11.2 SP-1 zero-knowledge guarantee.

- **Test requirements at T0.1.4:** round-trip integrity, boundary-leak proptest at the LanceDB query layer, concurrent-write test, vector dimension consistency. Plus: assert `ALPHA_DO_NOT_STORE_REAL_DATA.txt` is created on first open with the expected content + read-only perms; assert the WARN log fires on every open.

- **Test requirements at T0.2.0 (must pass before T0.2.16 unblocks):** all four V0.1 compensating controls fully removed from the build; vector data dir contains no plaintext Parquet on disk after a write/close cycle (verified by reading raw bytes and checking entropy + magic-bytes absence); decryption with wrong key fails closed; round-trip identity (`encrypt → decrypt == original`) on the full vector store across all supported platforms (Mac, Windows, Linux).

- **When to revisit:** Beginning of V0.2 — T0.2.0 is the first V0.2 task and gates all subsequent V0.2 work that touches user data.

- **Amendment 2026-04-29 (ADR-014):** Compensating control #4 (the `ALPHA_DO_NOT_STORE_REAL_DATA.txt` file) is downgraded from "non-negotiable" to "secondary safety control." If the data directory is read-only / quota-exceeded / otherwise unwriteable, `LanceVectorStore::open` now logs a WARN with the underlying error and proceeds rather than failing. Compensating control #3 (the startup WARN log) remains the **primary** control and fires unconditionally on every open. Rationale + test details in ADR-014.

### ADR-011 — 2026-04-29 — `protoc` (Google Protobuf compiler) is a per-machine build-time dependency for lancedb
- **Context:** During T0.1.4 kickoff, `cargo check -p vault-storage` failed because `lancedb` 0.8 transitively pulls `lance-encoding` and `lance-file`, both of which use `prost-build` to generate Rust from `.proto` schema files at build time. `prost-build` invokes the `protoc` binary; if it's not on PATH (or pointed at via `PROTOC` env var), the build fails immediately with `Could not find protoc`. Analogous to ADR-006's Strawberry Perl requirement for SQLCipher's OpenSSL build.
- **Decision:** Each developer machine installs `protoc` system-wide. CI installs it per-job via `arduino/setup-protoc` (or platform equivalent). The `PROTOC` env var is preferred over PATH lookup for build determinism — set in `.cargo/config.toml` (tracked tech-debt) or per-shell.
  - **Shahbaz's machine (done 2026-04-29):** `winget install Google.Protobuf` → installs to `C:\Users\<user>\AppData\Local\Microsoft\WinGet\Packages\Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe\bin\protoc.exe`. winget adds the package's bin dir to PATH for new shells.
  - **Mac/Linux (when added):** `brew install protobuf` / `apt install protobuf-compiler` / equivalent.
  - **CI:** add `arduino/setup-protoc@v3` (or equivalent) before any storage-test job in `.github/workflows/ci.yml`. Pin major version per the same convention as ADR-001 / `actions/checkout@v6`.
- **Reasoning:** lancedb is a BRD-pinned dep (§4.2). Replacing it to avoid protoc is a much bigger architecture decision; building with `protoc-bin-vendored` adds workspace-wide build-script weirdness for a problem the system install solves cleanly. Mirrors the established Strawberry Perl pattern.
- **Alternatives considered:**
  - *`protoc-bin-vendored` crate via cargo build-script PROTOC env-var indirection:* rejected — adds workspace-wide build-script complexity for a problem the system install solves.
  - *Skip lancedb / use a different vector store:* rejected — out of scope; would require BRD §4.2 amendment.
  - *Pin lancedb to a version that doesn't transitively need protoc:* rejected — lancedb has used prost since 0.x; no version dodges the requirement.
- **Operational follow-up (required, recurring):** **Monthly protobuf CVE check.** First Monday of each month, review https://github.com/protocolbuffers/protobuf/security/advisories and the installed `protoc --version`. Critical / High advisories affecting the installed version → upgrade ahead of other work. Tracked in tech-debt below.
- **Build environment that worked for T0.1.4 cargo check + tests on 2026-04-29 (git-bash on Windows):**
  ```
  PATH="/c/Strawberry/perl/bin:$PATH" \
  PROTOC="/c/Users/shahb/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe" \
  cargo check -p vault-storage   # 44.57s clean
  cargo test  -p vault-storage   # 39/39 passing in 18.15s
  cargo test  -p vault-core      # 44/44 passing in 0.03s
  ```
  - `PATH=/c/Strawberry/perl/bin:$PATH` is needed in git-bash because MSYS2's `/usr/bin/perl` lacks `Locale/Maketext/Simple.pm`, which openssl-src's build script requires (transitively pulled by lancedb's aws/azure object_store features). Strawberry Perl has the full standard library. PowerShell shells don't need this — they don't have `/usr/bin/perl` in PATH at all.
  - `PROTOC=...` is preferred over PATH lookup so the build doesn't depend on shell PATH semantics.
- **When to revisit:** When `.cargo/config.toml` lands with `[env]` block making the env vars persistent (tracked as tech debt). When CI adds vault-storage to its build matrix (need `setup-protoc` + Strawberry-equivalent there).

### ADR-012 — 2026-04-29 — LanceDB feature minimization investigated; no flags available; AWS SDK + dual-arrow accepted as V0.1 cost
- **Context:** Per Shahbaz's Phase-1 review, the lancedb 0.8 + lance 0.15 transitive tree pulled `aws-config`, `aws-sdk-*`, `datafusion 40`, plus arrow v51 AND arrow v52 simultaneously (Cargo.lock grew by ~5,900 lines). For an embedded vector store on user devices that never talks to LanceDB Cloud or S3, these are dead weight: bigger binary, larger supply-chain attack surface, more `cargo audit` noise, slower compiles. Investigated whether LanceDB exposes feature flags to disable these.
- **Findings (verified 2026-04-29 via docs.rs + GitHub):**
  - `lancedb` 0.8 features: `default = []` (empty), `remote = ["dep:reqwest"]`, `fp16kernels`, `s3-test`, `openai`, `polars`, `sentence-transformers`. **No feature gates the AWS / GCP / Azure cloud backends.** We never enabled `remote` — it's not the source of the AWS SDK pull.
  - `lance` 0.15 features: `fp16kernels`, `cli`, `tensorflow`, `dynamodb` (gates aws-sdk-dynamodb), `dynamodb_tests`, `substrait`. **No feature gates the core S3/GCS/Azure backends in `object_store`** — those are non-optional in `lance-io` (`object_store = "0.10"` with default features in lance-io's Cargo.toml, which transitively includes AWS/GCP/Azure).
  - `cargo tree -p vault-storage -i aws-config` confirms `aws-config` enters the tree exclusively through `lance-io`, not through any feature we toggled.
  - `cargo tree -p vault-storage --duplicates` shows the unavoidable arrow 51/52 split: `fsst` (Lance's string compression) pins arrow 51 internally; the rest of the tree uses arrow 52. Plus typical churn on rand 0.8/0.9, hyper 0.14/1.x, rustls 0.21/0.23, http 0.2/1.x.
- **Decision:** **No feature minimization available at the lance/lancedb 0.8/0.15 layer.** Accept the AWS SDK + dual-arrow + datafusion footprint for V0.1. Do NOT fork or `[patch]` lance / object_store for V0.1 — vendor maintenance is a much bigger cost than the binary footprint we save, and a fork delays T0.1.4 indefinitely. Document the constraint and revisit on a clear trigger.
- **Reasoning:**
  - **V0.1 internal alpha is not the time to fight the vendor's feature surface.** Forking lance to remove cloud backends would mean owning a parallel branch, tracking upstream security fixes, and re-applying patches at every lance release. The cost dwarfs V0.1's binary-size or CVE-noise benefit.
  - **The supply-chain risk is bounded.** `cargo audit` runs in CI on every commit (BRD §11.7.5); CVEs in transitive AWS SDK or arrow crates surface immediately. We don't hide the risk by shipping it; we monitor.
  - **`object_store`'s cloud backends are dormant code paths we never invoke.** The crates ship in the binary but no code path reaches them — we only call lance's local-filesystem read/write surface. Dormant code is still attack surface, but the surface is far smaller than active integration.
- **Mandatory monitoring (ongoing):**
  - `cargo audit` already runs in CI per BRD §11.7.5. Any High/Critical CVE in `aws-config`, `aws-sdk-*`, `arrow`, `arrow-*`, `object_store`, `datafusion*` triggers immediate triage.
  - Monthly review of binary size at `cargo build --release -p vault-tauri` (when that crate becomes a binary at T0.1.11). Baseline measurement: capture at T0.1.11; investigate if it grows >10% month-over-month without an obvious feature reason.
- **When to revisit (any of these triggers):**
  - **lance gains feature flags for cloud backends** (track lance releases; check `[features]` at every minor bump).
  - **A High/Critical CVE in the dormant cloud-backend tree** that we cannot patch without forking. At that point, fork pressure exceeds maintenance pressure.
  - **V1.0 release prep.** Binary distribution to paying customers raises the bar — bloat we tolerate in alpha is harder to justify in production. Re-evaluate fork-vs-accept then.
  - **lance/lancedb major-version bump** that restructures the dep graph. Re-audit at every major bump.
- **Alternatives considered:**
  - *Fork `lance` / `lance-io` to remove `object_store` cloud features* — rejected for V0.1; revisit at V1.0 prep if still relevant.
  - *Use `[patch.crates-io]` in workspace `Cargo.toml`* — same problem as forking, plus brittle.
  - *Pin to a different vector-store crate (e.g., `qdrant-client`, `instant-distance`)* — rejected; out of scope; would require BRD §4.2 amendment.

### ADR-013 — 2026-04-29 — chrono `=0.4.38` pin: tactical, with explicit revisit triggers and monthly CVE monitoring
- **Context:** chrono 0.4.44 added `Datelike::quarter()` which conflicts with arrow-arith 52.x's `ChronoDateExt::quarter()` (same method name on the same receiver via two traits — ambiguous at the call site in arrow-arith). T0.1.4 build broke until chrono was pinned to `=0.4.38`. The pin is a tactical fix, not a strategic one — pinning chrono to an old version forever is exactly the kind of tech debt that festers and slowly opens us to chrono CVEs we cannot patch.
- **Decision:** chrono is pinned at `=0.4.38` until any one of these triggers fires; on trigger, evaluate the pin and update or remove. Monthly chrono security advisory check is added to the recurring task in tech debt.
- **Revisit triggers (any):**
  1. **arrow upgrade past the conflict.** When arrow-array / arrow-schema move past 52.x to a release that fixed `ChronoDateExt::quarter` (renamed it, removed the trait, or qualified the call site), bump chrono and remove the pin.
  2. **High or Critical chrono CVE on a 0.4.39+ version that cannot be backported to 0.4.38.** At that point, the security exposure outweighs the build-break risk. Two paths in priority order: (a) check whether the CVE is patchable via a forward bump combined with arrow-arith fixes (PR upstream or local `[patch]`); (b) if neither path works, fork arrow-arith to qualify the `quarter()` call site; (c) absolute last resort, accept the CVE risk for the remaining V0.1 alpha window and document explicitly in HANDOFF.md as a security blocker.
  3. **lancedb / arrow-arith publish a release where the conflict is resolved** — e.g., `arrow-arith` calls `ChronoDateExt::quarter(&d)` explicitly. Bump and unpin.
  4. **chrono itself publishes a 0.4.4x release that removes the `Datelike::quarter` method**, or `chrono = "0.5"` ships and we evaluate the new major.
- **Operational follow-up (recurring, monthly):** **chrono security-advisory check.** First Monday of each month — same recurring schedule as the OpenSSL CVE check (ADR-006) and the protobuf CVE check (ADR-011) — review https://rustsec.org/advisories/?keyword=chrono and the chrono crate's GitHub security advisories. High/Critical advisory affecting 0.4.38 → run the trigger 2 evaluation path above. Tracked in the Tech Debt Backlog below.
- **Reasoning:** Without explicit triggers and a monitoring cadence, "tech debt logged" means "indefinite drift." Naming the triggers + putting it on the same monthly cadence as our other security pins (OpenSSL, protobuf) makes the pin a maintained artifact, not a forgotten one. The chrono pin becomes part of the same monthly security-hygiene rhythm rather than a separate concern.
- **When to revisit:** Each monthly check, plus immediately on any of the explicit triggers above.

### ADR-014 — 2026-04-29 — ALPHA file write failure: WARN + proceed (file is secondary, log is primary)
- **Context:** Phase 3 review (Shahbaz, 2026-04-29). The original ADR-010 implementation failed `LanceVectorStore::open` if `write_alpha_warning` returned an error — i.e., a read-only / quota-exceeded / network-share-restricted data directory would prevent the vault from opening at all. That's a denial-of-service against legitimate use: the user's vault simply doesn't load, and the diagnostic is buried in the inner io::Error rather than surfaced as a clean operational signal.
- **Decision:** ALPHA file write is a **secondary** safety control. On failure: log a `tracing::warn!` event with the underlying io error + the data dir path, then proceed with `open()` — the LanceDB connection, table open, and PRIMARY startup WARN log (ADR-010 compensating control #3) all continue normally. The user's vault is operational; the alpha-warning signal is degraded but the primary safety mechanism (the WARN log on every open) still fires.
- **Reasoning:**
  - **Two-tier safety: primary vs secondary.** ADR-010's compensating controls fall into two tiers. The startup WARN log (#3) is **primary** — it fires every open via `tracing::warn!`, is captured by any tracing subscriber (dev console, log forwarder, future audit pipeline), and is impossible to silence without code changes. The ALPHA file (#4) is **secondary** — a passive on-disk artefact that's useful if a curious user browses the data directory but doesn't actively gate or signal during normal operation. Tying primary safety to a secondary control's success was the design error this ADR fixes.
  - **The failure mode is realistic.** Read-only data dirs happen: network shares with restricted writes, full disks (the file is small but disk-full triggers `ENOSPC` regardless), Windows directories with inherited deny-write ACLs, sandboxed environments. Failing closed on any of these is a real UX defect.
  - **The primary control still fires.** The WARN log is emitted unconditionally by `open()` regardless of whether the file write succeeded. The user's tracing subscriber (dev or production) sees the alpha warning every startup. We don't lose the safety signal; we just don't gate the entire vault on a secondary artefact.
  - **The diagnostic is louder, not quieter.** Failing `open()` produces an opaque "io error: ..." that the user has to debug. Logging WARN + proceeding produces a *named* alarm: `"ALPHA warning file write failed (data dir may be read-only or out of space)"` with the underlying error attached. Operators can act on it without diving into source code.
- **Implementation:** `LanceVectorStore::open` now wraps `write_alpha_warning(data_dir)` in `if let Err(e) = ... { warn!(error = %e, ..., "ALPHA warning file write failed ... see ADR-014"); }` instead of `?`. The startup WARN log (ADR-010 control #3) immediately follows and fires unconditionally.
- **Test:** `open_succeeds_when_alpha_file_write_fails_per_adr_014` — pre-creates the alpha path as a *directory* (so `fs::write` fails on every platform we support), then asserts `open()` succeeds, the path is still a directory (not clobbered), and the store is otherwise functional (`dimension()`, `count()` work).
- **Related amendments:** ADR-010 compensating-control #4 description amended (above) to mark the file as secondary and reference ADR-014.
- **Alternatives considered:**
  - *Fail closed (the original behaviour):* rejected — see "Reasoning" above.
  - *Retry-with-backoff on the file write:* rejected — adds complexity for a secondary control. If the dir is read-only, retry won't help.
  - *Move the alpha warning into a tracing-subscriber sink instead of a file:* rejected — that's just more elaborate primary control, not a replacement for the file's "passive on-disk hint when a user browses the data dir" purpose.
- **When to revisit:** When T0.2.0 ships (encryption-at-rest), the ALPHA file is removed entirely along with the WARN log; ADR-014 becomes archival.

### ADR-015 — 2026-04-30 — `Entity` and `Relationship` are boundary-scoped at the schema layer (deviation from BRD §5.1)
- **Status:** APPROVED 2026-04-30 by Shahbaz. Per BRD §11.15 escalation discipline.

- **Context:** BRD §5.1 sketches `Entity` and `Relationship` without a `boundary` field. BRD §11.4.3 mandates boundaries as access control on every retrieval surface. T0.1.5 must reconcile the two before schema lands. Two paths considered:
  - **(a) Boundary on `Entity`** — every entity tagged at creation; relationships restricted to within-boundary endpoints (one explicit exception, see *same_as* below); traversal queries take `authorized_boundaries: &[Boundary]` and filter at the SQL `WHERE` clause. Strongest enforcement, biggest deviation from §5.1.
  - **(b) Unbounded entities, derive boundary at retrieval** — entities/relationships are global; access control happens later when retrieval joins entities back to memories that carry boundary. Matches §5.1 literally.

- **Decision: take path (a).** `Entity` carries a `boundary: Boundary` field (validated newtype per ADR-005). Relationships are within-boundary by default. The DuckDB schema enforces this, the `GraphStore` API surfaces require the caller to declare authorized boundaries, and SQL filters apply at every traversal hop.

- **Reasoning (continuity with prior decisions):**
  - **Same pattern as ADR-005, ADR-010, and ADR-007.** Each of those chose "enforce at the schema/storage/type layer" over "enforce at the caller" for the same reason: caller discipline fails silently, schema enforcement fails loudly. ADR-005 made `Boundary` a validated newtype. ADR-010 made boundary filtering at the LanceDB query layer non-negotiable. ADR-015 extends the same posture to the graph store.
  - **Path (b)'s specific failure mode is invisible by construction.** Any direct graph-traversal call that bypasses the memory join becomes a boundary leak. We don't have such a call today, but the consolidator (T0.2.2–T0.2.4) and retrieval entity-expansion (V1.0+) will both need direct entity traversal. If a future caller forgets the join, no test catches it — the traversal returns syntactically correct entities, just from the wrong boundary. Path (a) makes that failure mode unrepresentable: the SQL `WHERE` clause is the gate.
  - **Cross-boundary entity duplication is a feature, not a bug.** Auto-fusing "Sarah at work" with "Sarah my friend" means an agent in the work boundary could pull personal context (birthday, family) it should never see. The user explicitly opting in via *same_as* (below) is the right interaction. The duplication matches how humans actually compartmentalise — recall context for "Sarah at work" differs from "Sarah my friend" even in human cognition. Bounded duplication, not broken model.

- **Three additions Shahbaz called out for the ADR (locked here so T0.1.5 implementation and T0.2.2–T0.2.4 consolidator have a clear contract):**

  1. **Schema-level enforcement specifics (DuckDB).**
     - `entities` table: `boundary TEXT NOT NULL` column. Validated as `Boundary` (newtype, charset `[a-zA-Z0-9_-]{1,64}` per amended ADR-005) at the `create_entity` API boundary before insert.
     - `relationships` table: `from_entity_id` and `to_entity_id` both reference `entities` via `FOREIGN KEY` (DuckDB enforces FK on insert). Plus a denormalised `boundary TEXT NOT NULL` column on the relationship row itself for fast traversal-time filtering (avoids a JOIN back to `entities` on every hop of every traversal — meaningful at 1–3 hop depths over a multi-thousand-entity graph).
     - **Enforcement mechanism (LOCKED 2026-04-30 after duckdb 1.0 API + DuckDB engine 1.x SQL feature verification):** the within-boundary invariant is **app-layer-enforced inside `create_relationship`**, transactionally:
       1. `BEGIN`.
       2. `SELECT boundary FROM entities WHERE id IN (?, ?)` — both endpoints.
       3. If both endpoints share the same boundary → set `relationships.boundary` to that value, proceed to insert.
       4. If endpoints differ AND `relation_type IN ('same_as', 'alias_for')` → permitted; set `relationships.boundary` to the from-endpoint's boundary (asymmetric, but consistent — traversal-time filtering can still use this column as a hint, and `follow_aliases = true` widens the filter to the caller's full `authorized_boundaries` slice).
       5. If endpoints differ AND `relation_type` is anything else → return a named `VaultError::CrossBoundaryRelationshipForbidden` (or equivalent variant). `ROLLBACK`.
       6. `COMMIT`.
       **Why app-layer not SQL-layer:** DuckDB 1.x does not support triggers, and CHECK constraints in DuckDB 1.x cannot reference other rows / other tables via subquery (column-level CHECK only takes per-row boolean expressions). Both SQL-layer mechanisms that *would* have enforced this invariant declaratively are unavailable. The next-best alternative — declarative SQL-layer enforcement — would require materialising endpoint boundaries onto the relationship row and a per-row CHECK that the inserted row's `boundary` matches the materialised endpoint boundaries, which is exactly what the app-layer transactional path already does. App-layer + property test is the same enforcement strength with simpler schema.
       **Property test (mandatory, locked):** fuzz `create_relationship` with arbitrary endpoint pairs (same boundary, different boundary, mix of `same_as` / `alias_for` / other relation types) and assert the API returns `Ok` exactly when the invariant holds and `Err(CrossBoundaryRelationshipForbidden)` otherwise. The property test IS the SQL-layer backstop's substitute — without it the invariant becomes "trust the impl," which fails the schema-vs-caller-discipline test in ADR-015's reasoning.
     - Every traversal query takes `authorized_boundaries: &[Boundary]` (non-`Optional`, mirroring `LanceVectorStore::search`'s mandatory access-control parameter from T0.1.4). Empty slice returns empty result, not an error — compile-time impossible to "forget to filter."
     - Defense in depth atop `Boundary`'s tightened charset: SQL parameter binding (parameterised query, never interpolation) + `quote_sql_string` helper (carry-over from `LanceVectorStore`) for any unavoidable literal embedding in CTE construction.

  2. **`same_as` / `alias_for` relationship — documented escape valve for cross-boundary linking (forward-compat note, NOT implemented in T0.1.5).**
     - `relation_type = "same_as"` (and reserved sibling `"alias_for"`) are the **only** relation types allowed to span boundaries. Schema enforcement: the within-boundary CHECK on relationships exempts rows where `relation_type IN ('same_as', 'alias_for')`.
     - `same_as` creation is **never auto-generated by the consolidator, extractors, or any ingestion path**. Always requires explicit user action via the UI / a deliberate API call, never implicit.
     - Traversal queries that follow `same_as` edges must be opt-in: `traverse(..., follow_aliases: bool)`, default `false`. With `follow_aliases = true`, the recursive CTE may cross to entities in any of the `authorized_boundaries` (still bounded by the caller's authorization set — `same_as` is not a privilege escalation).
     - Every `same_as` / `alias_for` create / supersede / delete is a **privacy-significant audit event**: `audit_log.action = 'same_as_link'` or `'same_as_unlink'`, full from/to entity IDs, both boundaries, user-confirmation token. Audit chain (BLAKE3 from T0.1.3) covers it like any other event.
     - **T0.1.5 scope:** the schema MUST permit `same_as` rows (the within-boundary CHECK exempts them), the trait API SHOULD reserve a `follow_aliases` parameter on `traverse` (default false, no-op for V0.1 because no `same_as` rows exist yet), no UI / consolidator / audit-event work in V0.1. Forward-compatibility groundwork only.

  3. **Consolidator policy (specs the contract for T0.2.2–T0.2.4 now, before they're implemented).**
     - When the consolidator processes memories from multiple boundaries and finds entities with the same `(name, entity_type)` across boundaries, it **never auto-merges them**.
     - It **may** flag `(boundary_a:entity_x, boundary_b:entity_y)` as a candidate for user review in a `cross_boundary_link_candidates` queue (separate table, lands at T0.2.2 alongside the consolidator). The user reviews and either creates a `same_as` link explicitly or dismisses the candidate.
     - The consolidator **never** writes to the `same_as` / `alias_for` graph itself; only the user-driven UI path does.
     - This is a contract spec, not V0.1 implementation. T0.1.5 doesn't build the candidates queue; T0.2.2 does. Documenting now means T0.2.2 starts from "implement this contract" instead of "improvise this contract."

- **Three implementation watch-items for T0.1.5** (called out by Shahbaz; tests must cover):

  1. **Recursive CTE traversal must apply boundary filter at every hop, not just the start.** A 2-hop traversal from entity A in boundary `work` could otherwise traverse a relationship into boundary `personal` and return personal entities (in path (a) this should already be impossible because cross-boundary edges don't exist outside `same_as`, but the CTE must still filter defensively). **Property test (Heavy): for any 1–3 hop traversal with `authorized_boundaries = [b]`, no returned entity has `boundary != b`.** Run the proptest with arbitrary graphs including injected `same_as` edges with `follow_aliases = false` to verify the alias bypass guard.
  2. **Bi-temporal columns on `relationships` from day one.** Per BRD §5.1, `Relationship` has `valid_from`, `valid_until`, `confidence`. Schema includes all three from the first migration. Adding bi-temporal columns to a graph table after rows exist is migration-painful. **Test: queries-as-of-past-time return historical state.** When the consolidator (T0.2.2–T0.2.4) supersedes a relationship by setting `valid_until = now` and creating a successor, a traversal at time `t < now` returns the old relationship; at time `t >= now` returns the new one.
  3. **Entity name uniqueness scoped to `(name, entity_type, boundary)`, not `(name, entity_type)`.** Two entities both named "Sarah" can coexist if they're in different boundaries — that's the cross-boundary feature, not a bug. UNIQUE constraint on the composite key. **Test: insert "Sarah" / Person / work succeeds; insert "Sarah" / Person / personal succeeds; insert "Sarah" / Person / work again fails with a duplicate-key error mapped to a clean `VaultError` variant.**

- **Alternatives considered:**
  - *Path (b) with retrieval-layer enforcement:* rejected — see "Reasoning" above. Invisible failure mode in any future direct-traversal caller.
  - *Boundary on `Relationship` only, not `Entity`:* rejected — leaves entity-only traversals (graph operations that walk entities without going through edges) unguarded.
  - *Tag-based boundary (multi-boundary entities by default, intersect with authorized at query):* rejected — collapses the privacy-decision boundary back into the data model, undermining the user-opt-in property of `same_as`.
  - *Defer the decision to T0.2.x:* rejected — schema decisions are expensive to retrofit (Shahbaz's framing, applied here: five minutes of policy now saves a week of migration later, especially once millions of rows exist).

- **Test requirements at T0.1.5 (Heavy):**
  - Round-trip identity for `Entity` / `Relationship` (write → read returns identical struct).
  - Boundary-leak property test (item 1 above) — every 1–3 hop traversal respects `authorized_boundaries`.
  - Bi-temporal correctness (item 2 above) — historical-time queries return the correct historical state.
  - Composite-uniqueness test (item 3 above) — `(name, entity_type, boundary)` is unique, not `(name, entity_type)`.
  - Concurrent-write safety: 20 tasks creating / superseding entities and relationships in parallel, final state coherent.
  - Cross-boundary edge rejection: `create_relationship` with endpoints in different boundaries and `relation_type != 'same_as'` returns `VaultError::BoundaryViolation` (or equivalent named variant).
  - `same_as` schema permissiveness (forward-compat): the schema *accepts* a `same_as` row spanning two boundaries (test inserts one directly via SQL and asserts the row persists). The trait API does not yet expose the creation path — that's T0.2.2+.

- **Test requirements at T0.2.2 (consolidator, when it lands):** consolidator never produces `same_as` rows; the cross-boundary candidate queue receives flagged pairs; an integration test confirms the user-review path is the only way `same_as` rows can be created.

- **When to revisit:**
  - When T0.2.2 implements the consolidator, verify the policy contract above is what the consolidator code actually does. Update ADR-015 if any clause needs sharpening.
  - If user research surfaces a workflow where auto-fusion is the preferred default for some entity classes, re-evaluate — but the bar is high (the privacy default is the right default).
  - If DuckDB's constraint mechanics force an awkward encoding (e.g., the within-boundary CHECK has to live entirely in app-layer code with no SQL backstop), revisit and decide whether to harden via a different mechanism (trigger with PL/SQL emulation, or a tighter app-layer assertion + property-test discipline).

---

## Tech Debt Backlog

Items noticed but not addressed in their originating task — picked up explicitly when scheduled, never as drive-by work.

- [ ] **`llama-cpp-2 = "0.1"` not yet declared in `[workspace.dependencies]`** — BRD §4.3 flags it for verification at the start of vault-llm work (T0.2.1). Do crate-name-and-version verification on docs.rs at that point and add to workspace deps then. (Noted 2026-04-28, file: `Cargo.toml`)
- [x] ~~**T0.1.5b — `[workspace.dependencies]` caret pins → exact pins per BRD §2.9**~~ — DONE 2026-04-30 (commit pending approval). All deps with a workspace-member consumer exact-pinned to `Cargo.lock` values; deps not yet consumed (ort, tokenizers, yrs, rmcp, tauri, tauri-build, mockall) stay caret-pinned with an inline comment naming the task that first wires them in. Cargo.lock byte-identical post-edit; all four DoD gates re-pass. Logged in Recently Completed.
- [ ] **DuckDB 1.2.2 autocommit-INSERT wedge on UNIQUE constraint violation** — Discovered during T0.1.5: `Connection::execute("INSERT INTO entities ...", ...)` against a row that violates the composite UNIQUE `(name, entity_type, boundary)` does NOT return an error promptly — it wedges the connection indefinitely (test hung >5min, killed). Worked around in `DuckDbGraphStore::create_entity` by pre-checking with `SELECT COUNT(*)` inside an explicit transaction before the INSERT (the explicit-tx + pre-flight pattern avoids the wedge). The pre-check + insert is atomic because we hold the connection mutex for the whole tx. **Operational follow-up:** revisit on every duckdb-rs minor bump — if a fix lands upstream, drop the pre-flight check and rely on a single INSERT with native UNIQUE-violation error mapping (cleaner code + one less round-trip). Tracked here so future-us doesn't quietly inherit the workaround as canon. (Noted 2026-04-30, file: `crates/vault-storage/src/graph_store.rs` `create_entity`)
- [ ] **`.gitattributes` for line-ending normalisation** — currently relying on git's default `core.autocrlf=true` on Windows. Adding `* text=auto eol=lf` plus binary markers for known binary file types would silence the CRLF warnings on commit and make cross-platform behaviour deterministic. Quick win when convenient. (Noted 2026-04-28)
- [x] ~~**dryoc 0.7 streaming-vs-single-shot — RUN THE SPIKE THIS WEEK**~~ — DONE 2026-04-29 (commit `9effa1a`). Path #1 (DryocStream-as-single-message) confirmed; spike artifact `crates/vault-sync/examples/dryoc_spike.rs` kept long-term. ADR-008 fully retired in commit `5fdf0d8` with AAD scheme + chunk-size policy locked.
- [ ] **chrono pinned to `=0.4.38` (POLICY: ADR-013, revisit triggers explicit)** — Tactical pin to dodge arrow-arith / chrono `quarter()` conflict; ADR-013 documents the four explicit revisit triggers (arrow upgrade past the conflict, High/Critical chrono CVE on 0.4.39+, lancedb/arrow-arith resolves the conflict, chrono major bump). Monthly chrono security-advisory check is on the recurring schedule below alongside OpenSSL and protobuf. (Noted 2026-04-29, file: `Cargo.toml`)
- [ ] **Build env vars need a persistent home** — T0.1.4 build requires `PROTOC` set to the winget protoc path AND Strawberry Perl in front of `/usr/bin/perl` on PATH (so openssl-src's build script finds a Perl with the full standard library, not MSYS2's minimal one). Currently passed inline on every cargo invocation. Should land as either `.cargo/config.toml` `[env]` block (machine-portable via env-var lookup) or a `scripts/dev-build.sh` helper. CI needs equivalent: `arduino/setup-protoc` action + `shogo82148/actions-setup-perl` (or use Strawberry on Windows runners). (Noted 2026-04-29, ADR-011)
- [ ] **(Recurring) Monthly OpenSSL CVE check** — per ADR-006. First Monday of each month, review https://www.openssl.org/news/vulnerabilities.html and the OpenSSL version vendored by `openssl-src` (`cargo tree -p openssl-src` from this workspace). Critical / High advisories affecting the vendored version → prioritise `cargo update -p openssl-src` ahead of other work. Next due: 2026-05-04. (Noted 2026-04-28)
- [ ] **(Recurring) Monthly protobuf CVE check** — per ADR-011. First Monday of each month, review https://github.com/protocolbuffers/protobuf/security/advisories and the installed protoc version (`protoc --version`). Critical / High advisories affecting the installed version → bump via `winget upgrade Google.Protobuf` (Mac/Linux equivalent) ahead of other work. Next due: 2026-05-04. Noted 2026-04-29.
- [ ] **(Recurring) Monthly chrono CVE check** — per ADR-013. First Monday of each month, review https://rustsec.org/advisories/?keyword=chrono and chrono's GitHub security advisories. High/Critical advisory affecting 0.4.38 → evaluate the ADR-013 trigger-2 paths in order (forward-bump + arrow-arith fix, fork arrow-arith, accept-and-document-as-blocker). Next due: 2026-05-04. Noted 2026-04-29.
- [x] ~~**ADR-014: ALPHA file write failure policy**~~ — landed 2026-04-29 in this session. `LanceVectorStore::open` now logs WARN + proceeds on alpha-file write failure; ADR-014 written, ADR-010 compensating-control #4 amended, test `open_succeeds_when_alpha_file_write_fails_per_adr_014` pins the behaviour.

---

## V0.1 Findings

_(populated once V0.1 ships)_

---

## Notes for Next Session

**Immediate state:** Working tree clean. `origin/main` at `34b7715` (T0.1.6a). Workspace 140/140 tests, build/clippy/fmt clean. **`T0.1.6_PLAN.md` is the design document — read it before writing any phase B code.** Status in plan: Approved with all seven Shahbaz refinements applied.

**T0.1.6 ship plan (three-phase, agreed 2026-04-30):**
- ✅ **Phase A — `34b7715`** — SQL migration 0002 + plan doc.
- 🔜 **Phase B — next session** — Data-layer modules: `retry_queue.rs`, `dead_letter.rs`, `pending_sync.rs` + their unit tests. Test-first per BRD §0.3. Each module: tests file → impl → tests pass → next module. ~1100 LoC total.
- ⏳ **Phase C — after phase B** — `cascading.rs` orchestrator + `divergence.rs` + `FaultInjector` + 5 adversarial tests + new `crates/vault-cli/` crate with passphrase auth + ADR-009 amendment + ADR-016 (FullySynced deferred + cascade-ordering invariant locked) + tech-debt note on `VaultError::Storage(String)` grab-bag refactor + final HANDOFF entry.

**How phase B starts (concrete first steps):**
1. Read `T0.1.6_PLAN.md` Q2 (retry queue policy: 8 attempts, ±25% jitter, `is_permanent` classifier with DimensionMismatch / AccessDenied / schema-mismatch on attempt 1).
2. Create `crates/vault-storage/src/retry_queue.rs` — start with the `#[cfg(test)] mod tests` block. Tests cover: enqueue + dequeue FIFO ordering by `sequence_id`, schedule_next_attempt with mocked clock to verify the 1/2/4/8/16/30/60/120s schedule, `is_permanent` returns `true` for permanent error variants and `false` for transient ones, dead-letter transition fires after 8th attempt OR on attempt 1 if permanent, payload round-trip via JSON.
3. Implement until tests pass. `Mutex<Connection>` + `spawn_blocking` against the existing `MetadataStore` pattern (don't introduce a parallel connection — share `MetadataStore`'s connection or accept it as a constructor parameter; phase C orchestrator decides which pattern).
4. Then `dead_letter.rs` (resolution-state transitions), then `pending_sync.rs` (UPSERT semantics).

**Key invariants to preserve from phase A:**
- Migration 0002 schema is canonical. Don't redefine column types or indexes in code — query against the schema as written.
- `(memory_id, sequence_id)` UNIQUE on retry_queue is the FIFO anchor. The `sequence_id` value comes from the audit chain's monotonic index (T0.1.3) — phase B's enqueue API needs to accept a sequence_id parameter (caller — the orchestrator in phase C — supplies it from the audit append).
- `dead_letter.resolution` enum strings are: `'retried_succeeded'`, `'retried_failed'`, `'acknowledged'`, `'auto_recovered'`. Match exactly.
- `pending_sync.memory_id` is the PK with UPSERT (`ON CONFLICT(memory_id) DO UPDATE SET operation = excluded.operation, queued_at = excluded.queued_at`) — verify SQLite supports this syntax (it does, since 3.24).

**Pace caution carried forward:** T0.1.6 is the highest-stakes integration in V0.1. Don't merge phase B until phase B's tests are independently green — phase C will integration-test against them.

**Tables still deferred to later tasks** (no scaffolding rule):
- `review_queue` — added at connector ingestion task (BRD §5.9)
- `sync_state` and `retry_queue` — added at T0.1.6 (cascading backend per ADR-009) / T0.2.x (sync engine)
Each lands via a new numbered SQLite migration file (`0002_…sql`, etc.). The migration runner already supports this and is regression-tested by `forward_migration_applies_next_version_only`.
