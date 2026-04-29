# Memory Vault — Build Handoff

**Last updated:** 2026-04-29 (post-T0.1.4 commit `710576c`)
**Updated by:** Claude (Opus 4.7)
**Current version:** V0.1 — Internal Alpha
**Current phase:** V0.1 in progress — T0.1.4 (vault-storage LanceDB) shipped (`710576c`). Next: dryoc API spike (ADR-008, ~2h), then T0.1.5 (vault-storage DuckDB graph store).

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
| T0.1.4 | vault-storage: LanceDB (`VectorStore` trait + `LanceVectorStore`) | 2026-04-29 (commit `710576c`) | ✅ vault-storage 59/59 + vault-core 44/44 + doc 1/1 = **104/104** passing; build/clippy/fmt clean | Domain-only `VectorStore` trait (BRD §2.2): `MemoryId` / `Boundary` / `&[f32]` only — Arrow types stay inside the impl. Mandatory access control on `search` via non-Optional `&[Boundary]` (BRD §11.4.3); empty slice returns empty result, not error — compile-time impossible to "forget to filter." `LanceVectorStore` on lancedb 0.8 + arrow 52.1: `merge_insert(&["id"])` with id-only match column (boundary-change updates in place, no duplicates), `DistanceType::Cosine` calibrated for L2-normalised bge-small embeddings, `only_if` filter via `build_boundary_filter` + `quote_sql_string` (defense-in-depth atop Boundary's tightened charset). 20 new tests including boundary-leak proptest and 20-task concurrent-upserts test. ADRs landed: 010 (V0.1 plaintext-on-disk HARD GATE before T0.2.16, four loud compensating controls), 011 (protoc + monthly CVE check), 012 (lance feature-minimisation investigated, no flags available, accept), 013 (chrono `=0.4.38` pin policy with revisit triggers + monthly CVE check). Boundary tightened to `[a-zA-Z0-9_-]{1,64}` in vault-core (ADR-005 amended). BRD §6 V0.1.3 perf budget added; BRD §6 V0.2 T0.2.0 (LanceDB encryption-at-rest) added as new first task with HARD GATE for T0.2.16. |

---

## In Progress

**Pre-T0.1.5: dryoc API spike** (per ADR-008 + Shahbaz's direction "run after T0.1.4 commits, before T0.1.5 starts"). ~2-hour scratch crate exercising dryoc 0.7's actual encrypt/decrypt API on a single-message envelope. Output: ADR-008 amended with the confirmed call shape and chosen path (streaming wrapper / sibling crate / RustCrypto `chacha20poly1305`); BRD §11.6 sketch reconciled with reality. Then T0.1.5 (vault-storage DuckDB graph store) starts.

**Two T0.1.4 follow-ups noted by Shahbaz (next-session, low priority):**
- **Search-side dimension check is already done** (post-Shahbaz-review verification: `search_rejects_dimension_mismatch` test passes; check at top of `search` after the empty-`authorized_boundaries` early-return). Flagged here so future-me doesn't accidentally re-add it.
- **ADR-014 (TODO): ALPHA file write failure policy.** Currently `LanceVectorStore::open` returns `Err` if the data dir is read-only / quota-exceeded. Per Shahbaz: downgrade to "log WARN + proceed" — the startup WARN log is the primary safety control, the file is secondary. Implementation + ADR + test land in next session before T0.1.5 (or folded into the dryoc-spike turn).

---

## Pending — V0.1 (Internal Alpha)

- [ ] **T0.1.5** — vault-storage: DuckDB (graph store with bi-temporal columns)
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
- **When to revisit:** T0.2.9 implementation kickoff (re-run spike to verify, then write the production envelope code). On any dryoc minor-version bump (re-run spike to confirm API still matches). On any path-decision concern (e.g., perf measurements showing the per-envelope wrapper is too costly — unlikely at our throughput).

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

---

## Tech Debt Backlog

Items noticed but not addressed in their originating task — picked up explicitly when scheduled, never as drive-by work.

- [ ] **`llama-cpp-2 = "0.1"` not yet declared in `[workspace.dependencies]`** — BRD §4.3 flags it for verification at the start of vault-llm work (T0.2.1). Do crate-name-and-version verification on docs.rs at that point and add to workspace deps then. (Noted 2026-04-28, file: `Cargo.toml`)
- [ ] **`.gitattributes` for line-ending normalisation** — currently relying on git's default `core.autocrlf=true` on Windows. Adding `* text=auto eol=lf` plus binary markers for known binary file types would silence the CRLF warnings on commit and make cross-platform behaviour deterministic. Quick win when convenient. (Noted 2026-04-28)
- [ ] **dryoc 0.7 streaming-vs-single-shot — RUN THE SPIKE THIS WEEK** — per ADR-008. 2-hour scratch crate, single-envelope encrypt → decrypt round-trip using actual dryoc 0.7 API. Output: ADR-008 amended with confirmed call shape; BRD §11.6 sketch updated to compile against reality. **Target: complete before T0.1.4 finishes** so we know whether we're using dryoc, RustCrypto, or another crate before T0.2.9 design starts. (Noted 2026-04-28, scheduled 2026-04-29)
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

**Working-tree state at the start of T0.1.4 code work:** clean after the ADR-010 + T0.2.0 commit lands. The next code commit will introduce `crates/vault-storage/src/lance_vector_store.rs` (or similar — final naming follows the API verification step) plus deps in `crates/vault-storage/Cargo.toml`.
