# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)
**Last updated:** 2026-05-07 (T0.2.0 Phase 0a-fix COMPLETED — `concurrent_upserts_all_succeed` regression resolved via ADR-038 mutex + LANCE_MEM_POOL_SIZE shell-level ceiling; spike v1 FORMALLY FAILED; Phase 0b active)
**Updated by:** Claude (Opus 4.7)

> **📁 V0.1 historical record:** `HANDOFF_V0.1_ARCHIVE.md` — frozen as of 2026-05-06. Full T0.1.1 → T0.1.12 phase narratives, ADRs 001-036 full text, tech-debt closures, plan-iteration histories. Cross-link out to that file when V0.2 work needs V0.1 detail; do NOT paraphrase.

---

## Current Status

**Active task:** **T0.2.0 Phase 0c — at-rest encryption re-spike on upgraded LanceDB stack.** Phase 0a-fix landed ADR-038 (4 layers, commit `1e58d30`); 216/216 vault-storage tests pass (382/382 workspace-wide, 17 pre-existing ignored). Phase 0b API drift audit completed during Phase 0a-fix CI wait — clean apart from one elevated tech-debt entry (is_permanent classifier substring drift on lance 4.0 error wording). Phase 0c rewrites `at_rest_spike.rs` using lance-io 4.x's `ObjectStoreProvider` + `ObjectStoreRegistry` — the integration shape Phase 0a-research validated as the V0.2.0 path forward.

**Why we got here:** V0.2 first task per BRD §6 is T0.2.0 (LanceDB Encryption at Rest, HARD GATE per ADR-010). Spike v1 (lance 0.15 era, designed against `WrappingObjectStore`) FORMALLY FAILED 2026-05-07 — discovered lance-io 0.15's `LocalObjectReader` bypasses the `object_store::ObjectStore` trait for `file://` URIs in BOTH directions, defeating both the `WrappingObjectStore` wrapper AND direct injection via `ObjectStoreParams.object_store`. Web research found lance-io 4.x exposes a first-class `ObjectStoreProvider` + `ObjectStoreRegistry` API designed for this exact integration — but requires a **major lancedb upgrade** (0.8 → 0.27.2, 19 minor versions). Phase 0a executed the upgrade; Phase 0a-fix resolved a `merge_insert` memory regression surfaced by the upgrade (see ADR-038); Phase 0c re-spikes the at-rest extension on the upgraded stack.

---

## T0.2.0 Phase 0 plan (mid-flight)

| Phase | Status | Work |
|---|---|---|
| **0a** | ✅ DONE | Bump lancedb 0.8 → 0.27.2 + transitive (lance-io 4.0, arrow 57, datafusion 52, object_store 0.12). Inventory breaking changes. **Compile: PASS.** Tests: 38 PASS / 1 FAIL — `concurrent_upserts_all_succeed` 8 GB allocation. Root-caused: lance 4.0 routes `merge_insert` through datafusion JOIN with no RAM ceiling (lance-format/lance#1983, #3601). |
| **0a-fix** | ✅ DONE | ADR-038 four layers all landed: (1) `Arc<tokio::sync::Mutex<()>>` in `LanceVectorStore` (Rust-side runtime serialisation), (2) `LANCE_MEM_POOL_SIZE=268435456` (256 MiB shell-side ceiling) at `.cargo/config.toml` `[env]` + ci.yml + vault-tauri main.rs doc + T0.2.14 forward-pointer, (3) `[build] jobs = 4` + `RUST_TEST_THREADS=4` in `.cargo/config.toml` (dev memory caps for build + test parallelism), (4) test-data updates for lance 4.0 behavior changes (Cosine NaN-zero-vector, footer-magic file format). Tests: **216/216 vault-storage lib pass; 382/382 workspace-wide pass** (17 ignored, all pre-existing markers). +1 new `lance_mem_pool_size_env_var_ceiling_reaches_test_process` regression pin for layer 2; 2 existing tests modified for layer 4 lance 4.0 finding (concurrent_upserts non-zero embeddings, corruption tests use footer instead of header). ADR-002 unsafe-collision rationale documented (rustc 1.92 + `std::env::set_var` unsafe → cannot be done from Rust). |
| **0b** | ✅ DONE (audit + ADR-039 production fix + 4 regression tests + classifier widen) | Brief vault-storage API drift audit, run during Phase 0a-fix CI wait. Initial inventory: 17 lancedb API surfaces compile + test clean against 0.27.2; connection lifecycle invariant preserved. Shahbaz pushed back THREE times on "low-risk" / "defer-to-V0.2" framing — first round added 3 memory-system verifications + classifier widen, second round added 2 deeper verifications (compaction effectiveness + sidecar surface), THIRD round escalated tombstoning from "ADR-territory deferred to Phase 0e" to "fix in this commit or product development ends here." Results: (1) `is_permanent` widened to recognise `schema`/`CastError`/`dimension`/`No vector column found` lance 4.0 wording variants. (2) `merge_insert_last_write_wins_for_embedding_column` PASSES — lance 4.0 preserves last-write-wins semantics; no data-corruption regression. (3) `read_during_write_returns_monotonic_consistent_snapshots` PASSES — V2 MVCC preserves snapshot reads. (4) **ADR-039 implemented in production**: `LanceVectorStore::delete()` now calls `OptimizeAction::Prune { older_than: TimeDelta::zero(), delete_unverified: true, error_if_tagged_old_versions: false }` immediately after `table.delete()`, holding the ADR-038 upsert mutex throughout. Verified empirically (Phase 0b session): `OptimizeAction::All` was INSUFFICIENT (5 files still contained probe-string post-cleanup, default 7-day retention); only zero-retention `Prune` achieves full physical removal (0 files post-prune, `prune.bytes_removed: 12162`). Trade-off: lose lance time-travel undo capability — correct for a privacy-property memory vault. (5) `delete_physically_removes_content_per_adr_039` regression pin: post-delete, scans every file under data dir; assertion fails loudly if the probe string is ever found post-delete (catches accidental Prune-call removal or lance retention-semantic regression). Code changes: `is_permanent` widened (`retry_queue.rs`) + production `delete()` modified to prune (`vector_store.rs`) + 1 new classifier test + 3 new regression tests. **Test count: 220** (was 216 pre-audit; +4 net from Phase 0b: classifier-variants + 3 regression probes; floor breach surfaced + approved before commit per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`). All 220 vault-storage tests pass. Bundles with next code commit per `feedback_admin_changes_ride_with_code.md`. |
| **0c** | PENDING | Rewrite `at_rest_spike.rs` using lance-io 4.x `ObjectStoreProvider` + `ObjectStoreRegistry`. Implement `SealedFileStoreProvider` for `vault-sealed://` scheme. |
| **0d** | PENDING | Re-spike. PASS = full at-rest round-trip through Lance + on-disk sealing + adversarial fail-closed. No partial pass per Shahbaz's discipline directive. |
| **0e** | PENDING | ADR-037 (lancedb upgrade rationale) + ADR-008 amendment (at-rest extension locking K3 KDF + Finding 2(c) AAD + ObjectStoreProvider integration) + T0.2.0 plan paragraph iteration 1. ADR-038 already drafted at 0a-fix, will cross-link from ADR-037. |

---

## Session-end working-tree state (2026-05-07, single bundled commit pending Shahbaz approval)

**Phase 0a (dep bump) — files modified:**
- `Cargo.toml` (workspace): `lancedb = "=0.27.2"` (was 0.8.0), `chrono = "=0.4.44"` (was 0.4.39, ADR-013 supersedes — note in ADR-037 at Phase 0e), `arrow-array = "=57.2.0"` and `arrow-schema = "=57.2.0"` (were 52.2.0). Removed spike-specific direct deps for `lance` / `lance-table` / `object_store` / `bytes` / `url` (Phase 0c will re-add at lancedb 0.27 transitives).
- `crates/vault-storage/Cargo.toml`: removed dev-deps added during V1 spike (`lance`, `lance-table`, `object_store`, `dryoc`, `bytes`, `url`). Phase 0c re-adds.

**Phase 0a-fix (ADR-038 implementation) — files modified:**
- `crates/vault-storage/src/vector_store.rs`: added `upsert_lock: Arc<tokio::sync::Mutex<()>>` field to `LanceVectorStore` + acquired at `upsert()` entry. Tokio mutex held across the `merge_insert` await; bounded peak memory restored.
- `.cargo/config.toml` (NEW): `[env]` block with `LANCE_MEM_POOL_SIZE = { value = "268435456", force = true }`. `force = true` so config wins over any pre-existing shell var. Dev-side enforcement layer (closes the open tech-debt entry "Build env vars need persistent home").
- `.github/workflows/ci.yml`: added `LANCE_MEM_POOL_SIZE: "268435456"` to top-level `env:` block. CI-side enforcement layer.
- `crates/vault-tauri/src/main.rs`: added ADR-038 cross-reference paragraph in module-level doc, naming the env-var requirement for V0.2 platform launchers (T0.2.14).
- `HANDOFF.md`: this file — Phase 0a-fix status, ADR-038 full text, T0.2.14 forward-pointer in the V0.2 hard-gate table, removal of the closed `.cargo/config.toml` tech-debt entry.

**Files renamed (preserved as failure documentation):**
- `crates/vault-storage/examples/at_rest_spike.rs.v1_fail_disabled` (was `at_rest_spike.rs`). 870-line V1 spike artifact documenting Stage A PASS + Stage B PASS + Stage C/D FORMAL FAIL (LocalObjectReader bypass empirically confirmed). Cargo does not pick it up due to extension. KEEP AS-IS — executable failure documentation referenced by upcoming ADR-008 amendment.

---

## Phase 0a regression — root cause + resolution (2026-05-07)

**Compile:** `cargo check -p vault-storage` → PASS in 2m 45s. vault-storage's library code uses a stable subset of lancedb's API across the 0.8 → 0.27.2 range (Connection, Table, merge_insert, query, count_rows, delete — all unchanged shape).

**Initial test result (pre-fix):** `cargo test -p vault-storage --lib` → 38 PASS, 1 FAIL (catastrophic).

**The single FAIL — `concurrent_upserts_all_succeed`:**
- 20 concurrent `merge_insert` calls × 4-dim embedding dataset
- Triggered `memory allocation of 8589934592 bytes failed` (8 GB request)
- Process aborted with `STATUS_STACK_BUFFER_OVERRUN (0xc0000409)`
- Pre-upgrade (lancedb 0.8): same test PASSED reliably (V0.1 test record)

**Investigation (per `feedback_source_read_call_graph_upstream_of_empirical.md`):**
1. **Source-read first** — read the failing test (`vector_store.rs:1051`), the `LanceVectorStore::upsert` impl, the `merge_insert` builder usage, the Arrow `RecordBatch` shape. Captured the call graph before web-searching.
2. **Web research** — searched lance/lancedb GitHub for `merge_insert` + memory issues. Found:
   - **[lance#1983](https://github.com/lance-format/lance/issues/1983)** (foundational, opened by westonpace, lance maintainer): *"merge_insert uses a datafusion join internally. Since we do not provide a RAM limit to datafusion it uses a lot of RAM by default to run the JOIN."*
   - **[lance#3601](https://github.com/lance-format/lance/issues/3601)**: *"merge_insert spill configuration easily too small if server has many cores"* — datafusion `ExternalSorter` + `RepartitionExec` allocate per-partition memory; partition count scales with CPU.
   - **[lance#6151](https://github.com/lance-format/lance/issues/6151)** ("Migrate away from Merge"): structural V2 path migration in flight.
3. **Source confirmation** — fetched `rust/lance-datafusion/src/exec.rs` to verify env-var path: `LANCE_MEM_POOL_SIZE` is read inside `mem_pool_size()` and converted to a datafusion `FairSpillPool`. Programmatic alternative exists (`LanceExecutionOptions::mem_pool_size`) but lancedb's `Table::merge_insert` doesn't expose it; env-var is the only practical knob.

**Root cause:** lancedb 0.27.2 (lance 4.0-rc.3) routes `merge_insert` through datafusion's full JOIN planner (`HashJoinExec` + `ExternalSorter` + `RepartitionExec`). Each call independently spawns a physical plan with `target_partitions = get_num_compute_intensive_cpus().min(8)` and **no RAM ceiling by default**. lancedb 0.8's older path had implicit per-table serialisation that masked unbounded allocation; lance 4.0's V2 path does not. 20 concurrent calls × 8 partitions × greedy datafusion allocation = 8 GB peak request.

**Resolution (Phase 0a-fix, ADR-038):** two-layer defense — Rust-side mutex serialisation in `LanceVectorStore::upsert_lock` + shell-level `LANCE_MEM_POOL_SIZE=268435456` (256 MiB) ceiling at every process-launch site. Tests: 40/40 PASS post-fix (39 pre-existing restored + 1 new ADR-038 layer-2 regression pin).

---

## Locked decisions from V1 spike (verified empirically; survive into Phase 0c re-spike)

These hold regardless of integration path. Phase 0c spike rewrite consumes them as-is.

- **AAD scheme** (Finding 2 candidate (c)): `AAD = BLAKE3("vault-at-rest-v1" || file_path_bytes)`. No `version_id` binding in V0.2; replay-attack residual documented. Verified by V1 spike Stage B wrong-AAD adversarial.
- **KDF** (K3): `at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key)`. Single-source-crypto preserves ADR-008 line 693 principle. Verified by V1 spike Stage B round-trip.
- **Sealing shape** (iter-3 §3.4): `version_byte (0x01) || granularity_marker (0x00 = per-file) || dryoc_header (24 bytes) || ciphertext`. 26-byte framing + 17-byte AEAD = 43-byte total per-file overhead. Verified by V1 spike Stage C on-disk inspection.
- **Granularity** (iter-3 §3.1): per-file, V0.2 unconditional. Revisit at V1.0 if column-projection latency surfaces.
- **Path #1 cipher** (ADR-008): DryocStream-as-single-message wrapped per envelope, `Tag::FINAL` on push. Same as T0.2.9 sync envelope. Same `dryoc 0.7.2` dep.
- **AAD-parameter sized-input quirk** (extends ADR-008 line 684): `dryoc::DryocStream::push_to_vec` / `pull_to_vec` require `Option<&Vec<u8>>` for AAD too — not just plaintext. Document at call sites.
- **Integration path** (W'' validated by web research, not yet runtime-confirmed): `ObjectStoreProvider` + `ObjectStoreRegistry` via custom `vault-sealed://` URI scheme. Bypasses `LocalObjectReader` because there's no fast-path for unknown schemes.

---

## Doc-only edits queued (ride with next code commit per `feedback_admin_changes_ride_with_code.md`)

- **HANDOFF.md plan amendment** (lines 15 + 40 of pre-2026-05-07 state): "alpha-distribution as natural first task" framing is wrong — BRD §6 numbers T0.2.0 (LanceDB Encryption at Rest) first. Already partially superseded by this update; full sweep needed before T0.2.0 commit.
- **BRD §6.2 line 1412 amendment**: stale reference to "ADR-008 dryoc/RustCrypto/sibling-crate spike" — that spike retired 2026-04-29 commit `5fdf0d8`. Update to reflect closure + name at-rest-extension as actual pre-T0.2.0 question.
- **BRD §11.5.1 amendment**: tmpfs/memory-handle prescription is overridden by ObjectStoreProvider integration path (assuming Phase 0d PASS). Conditional on spike re-run outcome.
- **ADR-013 supersession note**: chrono 0.4.39 pin lifted by ADR-037 (Phase 0e); document in archive cross-link.

---

## V0.1 retrospective (one-paragraph summary)

**V0.1 internal alpha SHIPPED 2026-05-06** across **12 sequential tasks** (T0.1.1 → T0.1.12) and **36 ADRs**. CI matrix sustained green across `[ubuntu-latest, windows-latest, macos-latest]` from T0.1.11 Phase 1 onwards. Founder dogfood (T0.1.12) ran 6+ hours cumulative on Windows 11 dev box per ADR-029 branch (2): 11 memories saved, 14+ search queries, full-laptop reboot persistence test ✅, Start Menu launch path test ✅, persistent VAULT_KEY user env-var test ✅. **2 findings filed, both Phase 3 lib→bin Tauri-template-default-drop class** — Finding #1 (search returns max_results regardless of relevance, mitigated by lowering UI default 10→3 per Phase 5e), Finding #2 (stray Windows console window alongside Tauri UI from missing `windows_subsystem = "windows"` attribute, fixed in Phase 5e). Finding #3 honest closure note per ADR-036 BRD §6 line 1393 amendment (≥3 issues → ≥2 issues OR honest closure). **Saved-memory `feedback_runtime_confirmation_after_web_spike.md` triple-validated** during T0.1.11 (Phase 1 ort/ORT version coupling + Phase 5b rmcp stdin-hang per ADR-034 + Phase 5c Tauri 2 `window.__TAURI__` default per ADR-035). Founder-distribution MSI artefact: SHA-256 `03d127371f6a881366e2f048d81f2785de97f68236c5d52747bf0100284d0a06` (106.02 MB / 111,173,632 bytes; Phase 5e build). **Full V0.1 historical record:** see `HANDOFF_V0.1_ARCHIVE.md`.

---

## V0.2 scope (BRD §6.2)

Per Agent_Build_Specification.txt §6.2:

- **Sleep consolidator** — background entity/relationship extraction from raw memories during idle periods. Builds the knowledge graph that semantic-only retrieval can't directly access.
- **Boundaries hardening** — multi-boundary UI + provenance + access-control surface beyond V0.1's hardcoded "default" boundary.
- **Cross-device sync** — encrypted Yjs CRDT-based sync across the user's devices via end-to-end encrypted relay. Server cryptographically cannot read.
- **30-user beta cohort** — alpha-distribution task: signed installers + onboarding flow + telemetry + feedback loop.

**Polished onboarding** per BRD §5.11 V0.2 — V0.2 likely introduces a frontend framework (React/Vue/Svelte) + bundler (Vite). When that lands, ADR-035's `withGlobalTauri:true` flips back to `false` (default) and dist/index.html is replaced.

---

## V0.2 task decomposition

**TBD at next session's plan-paragraph drafting.** Forward-pointers from V0.1 ADRs suggest the natural first task is **alpha-distribution** consolidating multiple V0.2 hard-gates (see "V0.2 hard-gate ADR forward-pointers" section below). T0.2.x task numbering will land at first V0.2 plan iteration.

---

## ADR-038 — Concurrent-upsert serialisation + LANCE_MEM_POOL_SIZE shell ceiling (drafted 2026-05-07, T0.2.0 Phase 0a-fix)

**Status:** ACCEPTED, in force from this commit.

**Context.** T0.2.0 Phase 0a bumped `lancedb` from `=0.8.0` to `=0.27.2` (transitively: `lance 4.0-rc.3`, `lance-io 4.0`, `arrow 57`, `datafusion 52`, `object_store 0.12`). The bump unblocked the V0.2.0 at-rest-encryption integration path (lance-io 4.x's `ObjectStoreProvider` API — see Phase 0c). After the bump, `cargo test -p vault-storage --lib` returned 38 PASS / 1 FAIL: `concurrent_upserts_all_succeed` allocated 8 GB and aborted the process with `STATUS_STACK_BUFFER_OVERRUN (0xc0000409)` on Windows. Pre-upgrade (lancedb 0.8) the same test passed reliably; V0.1 CI was sustained-green across this exact test for the entire T0.1.x sequence.

**Root cause.** lancedb 0.27.2 (lance 4.0) routes `Table::merge_insert` through datafusion's full JOIN planner — `HashJoinExec` + `ExternalSorter` + `RepartitionExec`. Each call independently spawns a physical plan with `target_partitions = get_num_compute_intensive_cpus().min(8)` and, by default, **no RAM ceiling**. lancedb 0.8's older `merge_insert` path had implicit per-table serialisation that masked unbounded allocation; lance 4.0's V2 path does not. With 20 concurrent calls (the test's load), 20 × 8 partitions × greedy datafusion allocation peaked at ~8 GB. Verified by:
- Empirical reproduction (Phase 0a session, 2026-05-07).
- Source-read of `lance-datafusion/src/exec.rs` `mem_pool_size()` and `merge_insert.rs::execute_uncommitted_v2`.
- Maintainer-authored upstream issue [lance-format/lance#1983](https://github.com/lance-format/lance/issues/1983): *"merge_insert uses a datafusion join internally. Since we do not provide a RAM limit to datafusion it uses a lot of RAM by default to run the JOIN."*
- Companion upstream issue [lance-format/lance#3601](https://github.com/lance-format/lance/issues/3601): *"merge_insert spill configuration easily too small if server has many cores"*.
- Structural fix [lance-format/lance#6151](https://github.com/lance-format/lance/issues/6151) ("Migrate away from Merge") still open at V0.2 plan time — we cannot wait on upstream.

**Decision.** Three-layer defense-in-depth resolution. Layers 1+2 handle the **runtime** memory regression; Layer 3 handles a related **build-time** memory exhaustion that surfaced when running the workspace DoD gate.

- **Layer 1 — Rust-side serialisation.** `LanceVectorStore` carries a private `upsert_lock: Arc<tokio::sync::Mutex<()>>`. Every `VectorStore::upsert` impl call acquires the lock at function entry and holds it across the `merge_insert` await. Effect: only one datafusion plan runs per store at a time, restoring the implicit serialisation lancedb 0.8 provided. Implemented at `crates/vault-storage/src/vector_store.rs` (struct field + `upsert()` body); doc comment on the field cross-links this ADR + lance#1983 + lance#3601.
- **Layer 2 — Shell-level memory pool ceiling.** Every process that loads `lancedb` MUST have `LANCE_MEM_POOL_SIZE=268435456` (256 MiB) set in its environment **before** the binary launches. Lance reads this env var lazily on first datafusion-plan construction and converts it to a `FairSpillPool` shared across the process. Effect: even if a future caller bypasses the mutex (e.g. a different `VectorStore` impl or a refactor that drops the lock), datafusion spills to disk under pressure rather than OOM-aborting. Landing sites:
  1. `.cargo/config.toml` `[env]` block with `force = true` (dev — every `cargo {build,test,run}` invocation).
  2. `.github/workflows/ci.yml` top-level `env:` block (CI — all jobs, all matrix OSes).
  3. `crates/vault-tauri/src/main.rs` doc comment — names the requirement for any non-cargo launcher of the Tauri binary.
  4. **Forward-pointer to T0.2.14 (Stub Installer):** every V0.2 platform launcher MUST set the env var before binary invocation — WiX MSI pre-args (Windows), Info.plist `LSEnvironment` (macOS .app), `.desktop` `Exec` wrapper (Linux). Captured in the V0.2 hard-gate forward-pointers table below as ADR-038.
- **Layer 3 — Build-time link-job parallelism cap (`.cargo/config.toml` `[build] jobs = 4`).** Surfaced empirically during the Phase 0a-fix DoD-gate run on Shahbaz's 16-logical-CPU Windows dev box with 16 GB RAM + 12.8 GB pagefile = 28 GB total virtual memory budget. Two distinct failure modes observed during iteration:
  - **At default `jobs = 16`** (cargo's `num_cpus()`): `cargo build --workspace --all-targets` aborted during the compile phase with `memory allocation of 17301520 bytes failed` + `error[E0786]: failed to mmap 'libtokio-...rlib': The paging file is too small for this operation to complete (os error 1455)`. `os error 1455` = Windows `ERROR_COMMITMENT_LIMIT` (committed virtual memory exhausted). Peak demand was ~48 GB (16 jobs × ~3 GB peak per rustc/linker) — well over the 28 GB budget.
  - **At `jobs = 8`** (first cap attempt): compile succeeded but the linker failed on `vault-tauri`'s binary (which pulls in Tauri + WebView2 + `vault-app` + the entire transitive graph) with `error: linking with link.exe failed: LNK1102: out of memory`. Peak demand was ~24 GB — still margin-fragile. Cascade: dependent crates (vault-storage lib test, vault-app lib test, vault-mcp tests) hit `E0463 can't find crate` and `E0282 type annotations needed` because they couldn't resolve types from the failed-to-link rlibs.
  - **At `jobs = 4`** (current setting): both compile and link fit in the virtual memory budget with ~12 GB headroom for OS + editor + browser. Build wall-clock is ~50% slower than 16-job over-subscription would have been in optimal RAM conditions — the trade-off chosen for build-correctness.

  Root cause: lance 4.0's much heavier dep tree (`lance-{io,table,datafusion,file,encoding,index,namespace}`, `datafusion 52`, `arrow 57`, `datafusion-{datasource,catalog,optimizer,...}`) roughly tripled per-rustc link-time peak memory vs the lancedb 0.8 era — `vault-tauri`'s link step alone hits ~3-4 GB on lance 4.0.

  CI is unaffected: GitHub Actions Linux + Windows runners have 4 logical CPUs each (as of 2026-05), so `jobs = 4` is the default there and the cap is a no-op on CI. Lives in the same `.cargo/config.toml` as Layer 2 — cohesive single dev-config file for the entire Phase 0a-fix shell-side configuration of the Lance/datafusion stack. Saved-memory `feedback_surgical_cargo_clean_first.md` partially anticipated this class ("stale-cache symptoms (E0463, E0786, spurious type-inference)") — Phase 0a-fix narrows the diagnosis to Windows pagefile/linker memory exhaustion specifically, not stale cache. Future revisits: drop this entirely when (a) the lance dep tree shrinks (LanceDB 0.30+ exploring this per upstream chatter), (b) Shahbaz's dev box gets more RAM, (c) we switch to the lower-memory `lld` linker, or (d) cargo gains a memory-aware scheduler.

  **Layer 3 sibling at test-runtime — `RUST_TEST_THREADS = 4`** (`.cargo/config.toml` `[env]` block): caps libtest's intra-test-binary parallelism the same way `jobs = 4` caps cargo's build-time parallelism. Surfaced empirically AFTER Layers 1-3 + Layer 4 fixes: `cargo test --workspace` still aborted on `vault-storage`'s lib test with `memory allocation of 17179869184 bytes failed` because 16 parallel test threads each opened their own `LanceVectorStore` and lance 4.0's per-store memory footprint (~700 MB-1 GB) × 16 = ~16 GB peak, exhausting the same 28 GB virtual memory budget. Distinct failure mode from build-time: stderr output interleaves crashes from multiple test threads simultaneously ("memory allocation of memory allocation of 17179869184 bytes failed" — the "memory allocation of" prefix appears twice because two threads logged at the same time). The mutex (Layer 1) bounds INTRA-test concurrency for `concurrent_upserts_all_succeed`; INTER-test parallelism is uncoordinated and needs its own cap. CI no-op for the same reason as `jobs = 4` (4-CPU runners default to 4 test threads).

- **Layer 4 — lance 4.0 Cosine-NaN-vector regression (test-only finding, no production impact).** Surfaced empirically AFTER Layers 1-3 fixed the memory regression: with the mutex serialising upserts and `LANCE_MEM_POOL_SIZE` bounding the memory pool, `concurrent_upserts_all_succeed` no longer OOM'd — but it still failed at the post-upsert search assertion with `concurrent insert lost id <uuid>`. The original test inserted 20 vectors including a zero vector at i=0 (`[0,0,0,0]`) and searched with a zero query (also `[0,0,0,0]`). lancedb 0.8 returned all 20 ids; lancedb 0.27.2 / lance 4.0 lost at least one. Three-way controlled diagnostic (Phase 0a-fix session, 2026-05-07):

  | Test | Embeddings | Query | Metric | Result |
  |---|---|---|---|---|
  | `concurrent_upserts_all_succeed` (original) | incl. zero | zero | **Cosine** | **FAILED** — id lost |
  | `..._non_zero_vectors` | no zero | non-zero | Cosine | PASSED |
  | `..._with_zero_vector_searchable_under_l2` | incl. zero | zero | **L2** | **PASSED** |

  Tests 2 and 3 differ from Test 1 in only one variable each. Conclusion: lance 4.0's Cosine search filters out NaN-distance rows (cosine of `[0,0,0,0]` vs any vector is `0 / (0 * ||v||)` = NaN), where lancedb 0.8 included them. **Production impact: zero.** BGE-small-en-v1.5 (production embedding model per ADR-019/020) produces L2-normalised vectors with magnitude ≈ 1.0 and never zero. `LanceVectorStore::search` uses Cosine (production path) but never receives zero-vector queries from `vault-retrieval`. The Phase 0a-fix landed adjustment is test-only: `concurrent_upserts_all_succeed` was modified to use embeddings 1.0..=20.0 (no zero) and a non-zero query — its INTENT (concurrent visibility under our production Cosine path) is preserved. Both diagnostic tests were deleted after serving their investigative purpose; the finding lives here in ADR-038 + as a tech-debt entry for filing the upstream lance issue.

  **Upstream filing tech-debt:** see HANDOFF.md "Open tech-debt" entry "Phase 0a-fix Cosine NaN-vector upstream issue" — should be filed against `lance-format/lance` once we have a minimal-repro Python or Rust example demonstrating the lancedb 0.8 → 0.27.2 regression. Defer to V0.2 alpha-distribution timing window when other compatibility checks happen.

**ADR-002 unsafe-collision rationale (why not set the env var from Rust).** `std::env::set_var` was marked `unsafe` in rustc 1.80 (POSIX `getenv` race semantics). Memory Vault is on rustc 1.92 with `#![forbid(unsafe_code)]` workspace-wide per ADR-002 — a foundational V0.1 invariant earned by every crate, not a coding-style preference. Amending ADR-002 to permit a single `unsafe { set_var }` block here would establish exactly the wrong precedent for future "small unsafe shortcuts." The shell-level launcher layer is also the **correct semantic home**: lance reads the env var lazily on first datafusion plan, so it must already be in the environment when the binary starts; setting it from Rust would be a race against lance's own initialisation in any threaded launcher path. Layer 2 lives outside Rust because it must.

**Why `force = true` in `.cargo/config.toml`.** Cargo's `[env]` keys default to `force = false`, which makes a pre-existing shell env var win over the config value. We want the opposite: a developer accidentally exporting an absurdly high `LANCE_MEM_POOL_SIZE` (e.g. for a benchmark spike that they forgot to undo) must not silently override the ceiling. `force = true` makes the config the floor for safety; deliberate overrides require editing the config or using `cargo --config 'env.LANCE_MEM_POOL_SIZE.value="..."'` explicitly.

**Why setup-dev-env scripts are NOT a landing site.** The scripts are *invoked* (e.g. `bash scripts/setup-dev-env.sh`), not *sourced*. An `export LANCE_MEM_POOL_SIZE=...` inside an invoked script lives only for the script's subshell — it does not propagate back to the parent shell that runs subsequent `cargo test`. Putting an `export` line there would be theatre-enforcement: it looks load-bearing but isn't. `.cargo/config.toml` is the real dev-side site because the mechanism (cargo reads the config and injects into every spawned child) matches the semantic ("set before any cargo-spawned binary starts").

**Consequences.**
- `LanceVectorStore::upsert` is now serialised. Throughput cap: one upsert at a time per store. V0.1's `RetryWorker` is already single-threaded per ADR-017 (strict-FIFO-per-`memory_id` audit-`seq` ordering) so this is not a real regression. V0.2's sleep-consolidator (T0.2.x) runs idle-time background and does not rely on parallel upserts. No production throughput affected.
- All processes that load `lancedb` now require `LANCE_MEM_POOL_SIZE` in their environment. Cargo and CI are covered automatically; **T0.2.14 alpha-distribution launchers must add this** (forward-pointer below).
- The `.cargo/config.toml` tech-debt entry ("Build env vars need persistent home") is closed in this commit — this fix naturally discharges that debt by adding a real persistent home.
- ADR-037 (lancedb upgrade rationale, drafted at Phase 0e) will cross-link this ADR. ADR-038 lands earlier than ADR-037 numerically because it was forced by the regression that 0a surfaced; the chronology reflects how the work unfolded.

**Verification.**
- 216/216 vault-storage tests pass (382/382 workspace-wide, 17 pre-existing ignored) post-fix: 39 pre-existing tests (including the previously-failing `concurrent_upserts_all_succeed`, now restored) + 1 new ADR-038 layer-2 regression pin (`lance_mem_pool_size_env_var_ceiling_reaches_test_process`) asserting `LANCE_MEM_POOL_SIZE` reaches the `cargo test` process env. The +1 floor breach was surfaced + approved by Shahbaz before commit per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`.
- CI matrix green across `[ubuntu-latest, windows-latest, macos-latest]` (verified post-push).
- Peak memory during the 20-concurrent-upsert test sits well under 256 MiB on Windows (mutex makes it effectively sequential — one plan at a time × 8 partitions × ~4 MB).

**Upstream tracking.** Re-evaluate at any of these signals: (a) lance#1983 closes with a configurable RAM limit shipped through lancedb's surface API; (b) lance#6151 lands the V2 merge path with bounded memory by construction; (c) lancedb exposes `LanceExecutionOptions::mem_pool_size` through `Table::merge_insert`. When any fires, drop Layer 2 (env var) first, then Layer 1 (mutex) only after a runtime confirmation that concurrent upserts no longer balloon memory.

---

## Locked-into-code invariants (forward-carried verbatim from V0.1)

These invariants are pinned by V0.1 code or tests — V0.2 work that touches the relevant subsystems MUST preserve them or explicitly amend with ADR + reasoning. **Verbatim per `feedback_quote_locked_artefacts_dont_paraphrase.md` discipline.**

### Storage / Cascading
- **`MAX_RETRY_QUEUE_DEPTH = 10_000`** (`crates/vault-storage/src/cascading.rs:73`)
- **`MAX_ATTEMPTS = 8`** + schedule **1, 2, 4, 8, 16, 30, 60, 120 seconds** + **±25% jitter** (`crates/vault-storage/src/retry_queue.rs:45`, schedule per ADR-009 amendment)
- **`LAST_ERROR_MAX_BYTES = FAILURE_REASON_MAX_BYTES = 4096`** (`crates/vault-storage/src/retry_queue.rs:54` + `crates/vault-storage/src/dead_letter.rs:33`)
- **`RetryQueue::poll_due` ordering: `(sequence_id ASC, next_attempt_at ASC)`** — strict FIFO per memory_id anchored to audit `seq` per ADR-017
- **`dead_letter.resolution` enum strings:** `'retried_succeeded'` | `'retried_failed'` | `'acknowledged'` | `'auto_recovered'`
- **`JitterSource: Send + Sync`** supertrait (extended at T0.1.6 C1b for tokio::spawn `Send` constraint at T0.1.10)
- **`is_permanent` classifier** covers `DimensionMismatch` / `AccessDenied` / `Storage(msg).contains("schema")` per ADR-009 amendment
- **`StorageBackend::open` returns `Ok` with `DegradedMode` flag on `validate_readable` failure** — never errors out so vault-cli triage stays available per ADR-018
- **`VectorStore::validate_readable` + `GraphStore::validate_readable` MUST exercise data-decode** (read row + parse column values), NOT metadata-only — per ADR-018 ("any manifest/fragment store recreates the blind spot if the check is metadata-only")
- **`VectorStore::contains(id) -> bool` is O(1) per id** (LanceDB impl uses `count_rows(Some("id = '<uuid>'"))`)

### Application / Lifecycle
- **Cascading retry worker spawned by `Application::start()`** — `tokio::sync::watch::channel(false)` + `worker.run(rx)` spawn + `Sender` returned for shutdown signaling. Per ADR-034 V0.1 fix-forward (was `start_with_mcp` pre-Phase-5b)
- **`AppConfig` field names CI-enforced** (rename-prohibition discipline pin from T0.1.10 Phase 2b)
- **`AppConfig::Debug` impl redacts `SqlCipherKey` field** as `<redacted>` (regression check from T0.1.10 Phase 2b)

### Retrieval / Embedding
- **`EMBEDDING_DIM = 384`** (`crates/vault-embedding/src/provider.rs:29`) — bge-small-en-v1.5
- **`MAX_QUERY_BYTES = 2_048`** (`crates/vault-retrieval/src/retriever.rs:51`) — Q3 query-length cap
- **`MAX_RESULTS_CAP = 100`** (`crates/vault-retrieval/src/retriever.rs:46`) — Q3 max_results cap
- **`SemanticRetriever::retrieve` score formula:** `score = 1.0 - cosine_distance` (per Q7 contract), exactly one site
- **Score sort order:** score-DESC then `created_at`-DESC (per Q9 contract), then `take(max_results)`

### Frontend / Tauri
- **`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`** at top of `crates/vault-tauri/src/main.rs` — kills stray console window on Windows release builds (per Phase 5e Finding #2 fix)
- **`app.withGlobalTauri = true`** in `tauri.conf.json` — exposes `window.__TAURI__` to bundled HTML+JS (per ADR-035; flips back to `false` at V0.2 ES module migration)
- **`bundle.resources` map in tauri.conf.json:** `onnxruntime.dll` + `model.onnx` + `tokenizer.json` paths (per ADR-019 Phase 5a; V0.2 alpha-distribution adds per-platform `bundle.windows.resources` / `bundle.macOS.resources` / `bundle.linux.resources` syntax to drop the cross-platform placeholder hack)

---

## V0.2 hard-gate ADR forward-pointers

ADRs that pin V0.2 work — each MUST be addressed before V0.2 alpha-cohort distribution opens. Full ADR text in `HANDOFF_V0.1_ARCHIVE.md`.

| ADR | One-line summary | V0.2 trigger |
|---|---|---|
| **ADR-010** | LanceDB stores plaintext on disk for V0.1 only | **HARD GATE before T0.2.0** — encryption-at-rest must ship before any V0.2 beta user receives the product. ADR-010 banners (modal + persistent strip) removed at this gate. |
| **ADR-031** | V0.1 unsigned Windows MSI deviation | **HARD GATE before V0.2 alpha-cohort opens** — Windows code-signing cert procurement (~$200-500/yr) + WiX signing pipeline + GitHub Actions secret + DoD test asserting signed MSI. Removes 3 V0.1 compensating controls (founder-only / SHA-256 record / SmartScreen Run-anyway). |
| **ADR-029 amendment** | macOS signing pipeline (Apple Dev ID already enrolled) | **HARD GATE before V0.2 alpha-cohort opens (macOS portion)** — App Store Connect API key + GitHub Actions secret + notarization workflow (~1 day setup; cert procurement already done). |
| **ADR-032** | V0.1 SQLCipher passphrase from `VAULT_KEY` env var | **V0.2 alpha-distribution must migrate to OS keychain** — keyring-core ecosystem mid-migration at V0.1; revisit at V0.2 plan time + spike picks branch (D) keychain. Plaintext-in-user-env-registry compensating control retired. |
| **ADR-033** | macOS ORT-loading test cfg-skip (upstream microsoft/onnxruntime#24579 OrtEnv mutex race) | **Revisit triggers (any):** (a) upstream PR fixing 1.22.x (not Node-only); (b) Mac procured for founder dogfood pre-V0.2; (c) we drop ort for a different ONNX runtime. Re-enable cfg-skip when trigger fires. |
| **ADR-034** | V0.1 vault-tauri is UI-only; MCP integration deferred | **V0.2 alpha-distribution adds `vault-tauri.exe --mcp-stdio-server` subcommand-split** — clap dep + headless mode + concurrent-data-access mutex design (UI-mode and MCP-mode mutually exclusive OR daemon architecture). Claude Desktop / Cursor / etc. integration via this path. |
| **ADR-035** | V0.1 `withGlobalTauri:true` for minimal HTML+JS frontend | **V0.2 alpha-distribution likely introduces frontend framework + bundler (Vite)** — `withGlobalTauri` flips to `false` (default), dist/index.html replaced with bundled ES module imports from `@tauri-apps/api/core`. Same task adds explicit `script-src 'self'` (or hash-pinned) CSP directive to close the defense-in-depth gap named in ADR-035 compensating control #1. |
| **ADR-036** | BRD §6 V0.1 acceptance bar amendment (≥3 → ≥2 OR honest closure) | **No V0.2 trigger — permanent V0.1-only amendment.** V0.2 / V1.0 acceptance bars are separate text. |
| **ADR-038** | Concurrent-upsert serialisation + LANCE_MEM_POOL_SIZE shell-level ceiling (Phase 0a-fix, drafted 2026-05-07) | **HARD GATE inside T0.2.14 (Stub Installer)** — every V0.2 platform launcher MUST set `LANCE_MEM_POOL_SIZE=268435456` (256 MiB) before invoking the binary, since the env var must already be in the environment when lance does its lazy first-call datafusion-plan init. **Windows MSI**: WiX `<Environment>` table or wrapper `.bat` pre-args that `set LANCE_MEM_POOL_SIZE=268435456` before launching `vault-tauri.exe`. **macOS .app**: `Info.plist` `LSEnvironment` dict entry. **Linux .desktop**: `Exec=env LANCE_MEM_POOL_SIZE=268435456 /usr/bin/vault-tauri %u` wrapper. DoD test: launcher-level integration test asserting the env var reaches the spawned process — fails fast if any platform launcher drops it. |
| **ADR-039** | Hard-delete API for lance 4.0 tombstoning (Phase 0b production fix, drafted 2026-05-07) | **HARD GATE CLEARED in Phase 0b commit** (was originally targeted as gate before T0.2.16 Beta Onboarding; resolved earlier per Shahbaz's "fix in this commit or product development ends here" escalation). Implementation: `LanceVectorStore::delete()` now calls `Table::optimize(OptimizeAction::Prune { older_than: Some(TimeDelta::zero()), delete_unverified: Some(true), error_if_tagged_old_versions: Some(false) })` immediately after `table.delete()`, holding the ADR-038 upsert mutex throughout. Trade-off (lose time-travel undo) accepted for privacy-property memory vault. Regression pin (`delete_physically_removes_content_per_adr_039`) asserts no probe string remains in any file post-delete. Full ADR text drafted at Phase 0e alongside ADR-037 + ADR-008 amendment; the production code + regression test land NOW so V0.2 work can proceed without the privacy-contract gap. |

---

## Active ADRs with V0.2+ implications (one-line summaries)

Beyond hard-gates above, the following V0.1-era ADRs remain active and shape V0.2 work. Full text in archive.

- **ADR-001** — CI runs on `[ubuntu-latest, windows-latest, macos-latest]` 3-platform matrix
- **ADR-002** — `#![forbid(unsafe_code)]` on all crates
- **ADR-003** — vault-tauri ships as binary at T0.1.11; library kept alongside for testable utilities
- **ADR-004** — `CLAUDE.md` is gitignored and never committed (local-only)
- **ADR-005** — `Boundary` validated newtype `[a-zA-Z0-9_-]{1,64}`
- **ADR-006** — `rusqlite` `bundled-sqlcipher-vendored-openssl` + monthly OpenSSL CVE check
- **ADR-007** — No manual `Debug` impls on types holding sensitive runtime state
- **ADR-008** — dryoc 0.7 path #1 (DryocStream-as-single-message) — locked at T0.1.4 follow-up; consumed at T0.2.9 sync
- **ADR-009** — Retry queue policy gates T0.1.6 (amended C1b)
- **ADR-011** — `protoc` per-machine build-time dep + monthly CVE check
- **ADR-012** — LanceDB feature minimization investigated; AWS SDK + dual-arrow accepted as V0.1 cost
- **ADR-013** — `chrono` `=0.4.39` pin (advanced from 0.4.38 at T0.1.9 Phase 1)
- **ADR-014** — ALPHA file write failure: WARN + proceed (file is secondary, log is primary)
- **ADR-015** — `Entity` and `Relationship` boundary-scoped at schema layer (BRD §5.1 deviation)
- **ADR-016** — Connection-ownership: orchestrator routes through `MetadataStore::with_transaction`
- **ADR-017** — Cascade-ordering invariant: strict FIFO per `memory_id` by audit `seq`
- **ADR-018** — `FullySynced` deferral + eager corruption validation in `StorageBackend::open`
- **ADR-019** — ort native-lib distribution: `load-dynamic` + bundled dylib + path-resolution layering
- **ADR-020** — Model + tokenizer integrity: paired-files SHA-256, fail-fast at startup
- **ADR-022** — `tokenizers` feature-trim (drop `esaxx_fast` + `progressbar`) for MSVC C runtime
- **ADR-023** — MCP wire format: no capability token field in V0.1; deferred to V0.2 sync/multi-agent
- **ADR-024** — vault-mcp JSON-RPC error mapping + locked `mcp.tool_invoke` details_json schema
- **ADR-025** — MCP trust-boundary contract: tool args UNTRUSTED, app-supplied `authorized_boundaries` TRUSTED
- **ADR-026** — rmcp pinned `=1.5.0` + April 15 RCE class scope analysis (server-not-host)
- **ADR-027** — Pre-dispatch validation: tracing-only, no audit-chain append
- **ADR-028** — Memory update semantics: preserve provenance, overwrite content + classification
- **ADR-029** — V0.1 founder dogfood platform: Windows accepted, BRD amended (Mac → Mac or Windows)
- **ADR-030** — vault-tauri MCP role: server-only, no host functionality in V0.1

---

## Open tech-debt (V0.2 deadlines)

V0.2-deadline items forward-carried from V0.1. Full original entries with cross-links + reasoning in archive's Tech Debt Backlog section. Recurring monthly checks (cargo audit / OpenSSL CVE / protoc / chrono / rustc / ort / tokenizers) continue under `.github/workflows/monthly-tech-debt.yml` automation.

- **BRD v1.3 T0.2.7: vector index intervention (HNSW or IVF)** — empirical 382ms median retrieval at 1K memories vs 200ms BRD §5.5 ceiling; intervention codified in BRD as explicit T0.2.7 deliverable + acceptance bar (vault-retrieval perf gate stays `#[ignore]`-d until T0.2.7 ships)
- **Phase 5c: `#[tracing::instrument]` on 5 Tauri command handlers** — observability gap surfaced at Phase 5c diagnosis; V0.2 alpha-distribution lands instrumentation across `crates/vault-tauri/src/commands.rs`'s 5 `*_inner` async fns
- **Phase 5b: WiX UpgradeCode + version-bump for auto-upgrade install** — V0.2 alpha-cohort needs auto-upgrade ergonomics; custom WiX template with stable UpgradeCode GUID
- **Phase 5a: Cross-platform Mac/Linux dylib bundling deferred** per ADR-029 branch (2) Windows-dogfood lock — V0.2 alpha-distribution adds `libonnxruntime.dylib` + `libonnxruntime.so` bundle.resources entries
- **Phase 5a: V0.1 MSI installer fatness ~106 MB** (model.onnx is 133 MB before MSI compression) — V1.0 mitigation candidates: smaller model swap (Matryoshka truncation), post-install download, shared system path
- **Phase 5e: Tauri 2 starter-template diff audit** — Phase 3 lib→bin conversion missed at least 3 Tauri-template defaults (`withGlobalTauri` per ADR-035 + `windows_subsystem` per Phase 5e Finding #2 + possible-fourth TBD); V0.2 alpha-distribution audits before introducing bundler + framework changes
- **`VaultError::WorkerSpawnFailed` variant re-evaluation at V0.2 alpha cut** — currently unreachable; remove if no concrete consumer surfaces
- **MCP server graceful-shutdown** — V0.1 known limitation; V0.2/V1.0 work when rmcp adds transport-level close API OR supervisor-pattern lifecycle lands
- **Disk-full handling on user device** — V0.2 hardening; SQLITE_FULL detection + `VaultError::DiskFull` variant + Tauri error toast
- **`VaultError::Storage(String)` grab-bag → structured variants** — `is_permanent` substring-matching cleanup; T0.2.x stand-alone refactor task. **Priority elevated by Phase 0b audit (2026-05-07):** lance 4.0 error wording is inconsistent ("schema mismatch" / "CastError" / "No vector column found to match" coexist for related schema-shape faults); current `Storage(msg).contains("schema")` classifier misses non-"schema"-worded permanent-class errors, retrying 8 times before dead-lettering. Production risk LOW (orchestrator's `eager_validate` catches dim/schema before merge_insert), but landing the structured-variant refactor early-V0.2 is now warranted rather than deferring deep into V0.2.x.
- **`pending_sync` sweep — extend schema migration 0003 with cascade payload** (`embedding BLOB` + `boundary TEXT`) at T0.2.x
- **macOS CI matrix `macos-latest` re-verification** — 1-2 month label migration cadence per actions/runner-images README; re-verify at each V0.2 task plan time
- **512-token context ceiling vs T0.2.x connector ingestion** — bge-small-en-v1.5 max sequence; nomic-embed-text-v1.5 candidate at T0.2.x connector kickoff
- **Annual model-currency re-check** — re-run T0.1.9 model spike at V0.2 release + before V1.0 GA
- **Future-swap evaluation criteria locked** — license / first-party ONNX / prefix-injection compat / MTEB-R parity / ≤512 MB
- **`.gitattributes` for line-ending normalisation** — quick win when convenient
- **Phase 0a-fix Cosine NaN-vector upstream issue** — file against `lance-format/lance` once we have a minimal-repro example. Regression: lancedb 0.8 returned NaN-distance rows in Cosine search; lancedb 0.27.2 / lance 4.0 filters them out. Affects zero-magnitude vectors only (cosine `0 / (0 * ||v||)` = NaN). Memory Vault production unaffected (BGE embeddings are L2-normalised non-zero) — purely an upstream-behaviour-change finding for the wider community. Defer to V0.2 alpha-distribution window when other compatibility checks happen. See ADR-038 Layer 4.
- ~~**Phase 0b finding: lance 4.0 delete-tombstoning vs Memory Vault privacy property**~~ — **CLOSED 2026-05-07 by ADR-039 production fix in this commit.** Lance 4.0 tombstones on delete by default; verified empirically that `OptimizeAction::All` is insufficient (default 7-day retention preserves deleted bytes; 5 files still contained probe-string post-cleanup) and only `OptimizeAction::Prune { older_than: TimeDelta::zero(), delete_unverified: true, error_if_tagged_old_versions: false }` achieves physical removal (`prune.bytes_removed: 12162`, 0 files post-prune). **Resolution shipped:** `LanceVectorStore::delete()` modified to call zero-retention `Prune` immediately after `table.delete()`, holding the ADR-038 upsert mutex throughout. Trade-off: lose lance time-travel undo — correct for a privacy-property memory vault. Regression pin (`delete_physically_removes_content_per_adr_039`) asserts no probe string anywhere on disk post-delete; fails loudly if the prune call is ever removed or lance changes retention semantics. **Beta cohort can now receive builds claiming "permanent deletion" — privacy contract preserved.**

**Closed in this commit (T0.2.0 Phase 0a-fix):**
- ~~**Build env vars need persistent home** — `.cargo/config.toml` `[env]` block or `scripts/dev-build.sh` helper~~ — Closed 2026-05-07: `.cargo/config.toml` landed with `[env]` block setting `LANCE_MEM_POOL_SIZE = { value = "268435456", force = true }` (per ADR-038). The persistent-home pattern is now available for any future Lance/datafusion env-knob; further entries can be added as needed.

---

## Cross-platform CI state

**Sustained green across `[ubuntu-latest, windows-latest, macos-latest]` from T0.1.11 Phase 1 onwards** through V0.1 alpha-cut (Phase 5e). Workflow file: `.github/workflows/ci.yml`.

- **fmt** — single ubuntu-latest job (rustfmt OS-agnostic)
- **clippy** — 3-platform matrix; `cargo clippy --workspace --all-targets -- -D warnings`
- **build-and-test** — 3-platform matrix; `cargo build --workspace --all-targets` + `cargo test --workspace`
- **Linux Tauri 2 native deps** preinstalled via apt step gated on `runner.os == 'Linux'`
- **macOS provisioning** invokes existing `setup-dev-env.sh` Darwin branch
- **vault-embedding test fixtures** cached via `actions/cache@v4` keyed on `setup-dev-env.{sh,ps1}` + `integrity.rs` hash
- **Tauri-build bundle.resources cross-platform existence-check workaround** — `touch` placeholder steps for clippy + non-Windows build-and-test (Phase 5d); V0.2 alpha-distribution removes via per-platform bundle-config syntax
- **Monthly tech-debt sweep** (`.github/workflows/monthly-tech-debt.yml`) — first Monday each month at 09:00 UTC; auto-opens GitHub issue with cargo audit + version-pin verification report

---

## Standing rules (CLAUDE.md-promoted defaults)

Per saved memory + CLAUDE.md (gitignored, local-only). One line each — full reasoning in saved-memory files.

- **Per-step CI green check before staging next push** — `gh run list --workflow=ci.yml -L 1` showing `success` on previous push before staging next; promoted to documented default 2026-05-04 (6/6 vault-code data points across T0.1.10).
- **Admin changes ride with next code commit, no standalone CI cycle** — HANDOFF.md edits, BRD amendments, ADR-only updates, tech-debt notes, doc-only changes do NOT get their own commit. Saves a 45-min CI cycle per admin commit.
- **(α) relaxation clause** — explicit user direction can relax strict gating for batched commits when time-cost of strict gating exceeds inherited-failure risk; relaxation acknowledged in commit message body.
- **Spike-before-lock** — uncertain API surfaces get a minimum-viable scratch spike before production code; methodology declared upfront (web research / compile-and-run / hybrid); findings captured in ADR.
- **Source-read the call graph BEFORE designing empirical investigations** — when triggers fire and a hypothesis ranking exists, source-read the failing operation's call graph FIRST (T0.1.10 Phase 1b lesson).
- **Quote locked artefacts, don't paraphrase** — operational doc text saying "per ADR-X" or "per §Y" must match the cited artefact verbatim or quote it.
- **Floor forecasts are pre-declarations, not estimates** — when a plan paragraph forecasts +N tests, breaching it (even by 1) requires plan-amendment surface BEFORE commit, not silent slip.
- **Trust `gh run list` / `gh run view` actual status, not `gh run watch` exit code** — watch-tool failures are network/rate-limit/session-drop transients, NOT CI failures.
- **Stop-and-escalate when scope drift or contract divergence surfaces** — don't paper over with `prop_assume!` or silent skips.
- **Plan-iteration depth scales with design-surface size** — contract-establishing tasks warrant 2-3 iterations; consume-existing-contracts tasks warrant 1 iteration + optional spike.
- **Web-research spike requires runtime confirmation in next phase** — TRIPLE-VALIDATED across T0.1.11 (Phase 1 ort/ORT + Phase 5b rmcp stdin-hang + Phase 5c Tauri 2 default). Pattern-recognition signal: V0.2 plan-time candidate discipline upgrade is "scheduled-runtime-confirmation in execution phases" — don't promote prematurely.
- **Confirm before every commit and every push** — combined approval per saved-memory `feedback_confirm_before_commit_push.md`. On approval, commit AND push without re-asking.
- **Run cargo from PowerShell on Windows, not Bash** — ADR-006's bundled-sqlcipher-vendored-openssl chain needs Strawberry Perl modules; PowerShell PATH resolves Strawberry first.
- **Surgical `cargo clean -p <crate>` first; full `cargo clean` is escalation** — stale-cache symptoms get per-crate clean first.
- **Never run parallel cargo invocations on the same workspace** — package-cache file lock; gates run strictly serial fmt → clippy → build → test.
- **Co-Authored-By tag uses bare "Claude"** — no model-version qualifier.
- **Broken CI is a regression, fix in same session** — never defer to "CI follow-up" tech debt.

---

## V0.1 archive cross-link

Full V0.1 historical record — every ADR's full text, every phase narrative, every plan-iteration history, every closed tech-debt entry, every retrospective — lives at:

**`HANDOFF_V0.1_ARCHIVE.md`** (frozen 2026-05-06; do not edit)

Convention: when V0.2 work needs V0.1 detail, **cross-link to the archive section, do NOT paraphrase** (per saved-memory `feedback_quote_locked_artefacts_dont_paraphrase.md`).
