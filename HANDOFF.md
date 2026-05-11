# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)
**Last updated:** 2026-05-11 session-end-2 checkpoint (CI fix-forward for `ac577f4`'s windows-latest fail + Tier 3 founder-smoke SKIPPED on dev machine).

**Session-end-1 (prior, still accurate as a milestone record):** Phase 2 + ADR-041 both SHIPPED. ADR-041 cfg-fix `ac577f4` (push 12:41Z, run `25670758274`). ADR-041 implementation landed at `6f2af9d` (push 12:21Z, run `25669781494`) BUT failed non-Windows clippy/build on unused-imports + dead-code; cfg-fix immediately followed in same session per `feedback_broken_ci_is_regression_not_techdebt.md`. Phase 2 fmt-fix `02799b5` (run `25660977905` GREEN matrix-wide 1h4m55s) preceded. All workspace DoD gates green pre-push: fmt clean, clippy `-D warnings` clean, build zero warns 41m38s, test 0 failures (vault-app: 27 passed +8 bridge tests; vault-storage lib: 232 stable; migration_v0_1_to_sealed: 16 stable; 17 pre-existing ignored markers preserved).

**Session-end-2 events (this checkpoint):** `ac577f4` CI run `25670758274` completed with **windows-latest build+test FAILED** — `tier_2_real_v0_1_vault_db_bridges_and_preserves_5_rows` (added in `6f2af9d` per ADR-041 §5) panicked because the captured V0.1 fixture binary `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/vault.db` was silently gitignored by `.gitignore`'s `*.db` rule (line 90) and never committed. Ubuntu + macOS all green (cfg-fix DID resolve the original `6f2af9d` non-Windows regression — that part worked). Locally the test passed because the fixture was on disk; CI runner doesn't get gitignored files — same class of "local-DoD-green ≠ CI-green" failure that originally drove the per-step-CI standing rule. **Fix-forward commit this session** (the commit this HANDOFF rides with): `.gitignore` negation rule for the fixture path + `git add -f` of `vault.db` (98 KB) + `vault.db-wal` (650 KB) so CI gets the real V0.1-binary-emitted byte shape for the realism-gate test. Per `feedback_broken_ci_is_regression_not_techdebt.md` resolved same session, not deferred to tech debt. **Tier 3 founder smoke SKIPPED on dev machine:** full user-profile scan found NO V0.1 production vault on this Windows box. Actual Tauri bundle identifier is `com.shahbaz242630.memory-vault` (HANDOFF Tier 3 procedure at lines 78-110 has stale `com.memoryvault.dev` path — corrects with Phase 3 ride-along); even at the correct identifier path `%APPDATA%\com.shahbaz242630.memory-vault\` doesn't exist and `%LOCALAPPDATA%\com.shahbaz242630.memory-vault\` only contains WebView2 runtime cache (no `lance/`, no `vault.db`). Either ADR-029 V0.1 dogfood happened on a different machine, was cleaned up, or never persisted in Tauri-app form on this dev box. Tier 2 fixture-replay (the test we're fixing in this commit) IS the realism gate per ADR-041 §5 — exercises the real V0.1-binary-emitted byte shape with 5 rows + the captured `fixture-capture-key-do-not-use-in-prod` passphrase. Tier 3 was an additional realism layer against a specific founder vault; deferred to first-alpha-cohort-member with real V0.1 data. **Next coding work:** T0.2.0 Phase 3 (controls removal + acceptance suite) per T0.2.0 close-out plan iteration 1.

**Sub-task (a) post-push state (2026-05-11, post-`e27e6dc`):** T0.2.0 Phase 3 sub-task (a) shipped at `e27e6dc8894ec2cf62ccec9b7eef3cc38b48112e` (push `ac577f4..e27e6dc main -> main`, CI run `25678902497` in flight). vault-cli sealed migration via keychain + `test_helpers` module promotion + iteration 4 §3/§7/§9 amendments + session-end-2 CI fix-forward bundle (HANDOFF session-end-2 paragraph + `.gitignore` negation + Tier 2 fixture binaries `vault.db` + `vault.db-wal` force-added). All 9 files in `e27e6dc`. Local DoD all green (cargo check workspace / vault-app 27 / vault-cli 20 incl. 2 NEW sub-task (a) / vault-tauri 7 / clippy workspace clean / fmt --all --check clean). **Working-tree drift surfaced post-push:** `Cargo.lock` modified — `rpassword` (`7.5.1`) + transitive `rtoolbox` (`0.0.5`) package entries removed (consequence of vault-cli/Cargo.toml dropping `rpassword`; cargo re-resolved during the gate runs after staging completed). Drift NOT in `e27e6dc`; bundles with sub-task (b)'s commit per `feedback_admin_changes_ride_with_code.md` (rationale: sub-task (b)'s raw-Parquet read spike will also touch `crates/vault-storage/Cargo.toml` adding arrow/parquet dev-dep — Cargo.lock drifts further; bundling captures both deltas in one resolution pass rather than two consecutive lockfile-churn commits). CI doesn't pass `--locked` to cargo build/test in `.github/workflows/ci.yml`, so CI on `e27e6dc` re-resolves silently and passes; the drift is cosmetic-only at the CI layer. **Discipline-fix saved-memory:** `feedback_git_status_check_between_fmt_and_add.md` — insert `git status --short` between final `cargo fmt --all --check` and `git add` to catch this class of post-gate drift consistently. Mirrors the pre-write check pattern (e.g., tokio-features check pre-scaffold-authoring). **Next coding work:** Phase 3 sub-task (b) raw-Parquet read spike per iteration 4 §4 — `crates/vault-storage/examples/v0_1_raw_parquet_read_spike.rs` Stages A–D. Kicks off after CI green confirms on `e27e6dc`.

**Updated by:** Claude (Opus 4.7)

> **📁 V0.1 historical record:** `HANDOFF_V0.1_ARCHIVE.md` — frozen as of 2026-05-06. Full T0.1.1 → T0.1.12 phase narratives, ADRs 001-036 full text, tech-debt closures, plan-iteration histories. Cross-link out to that file when V0.2 work needs V0.1 detail; do NOT paraphrase.

---

## Current Status

**Active task:** **T0.2.0 Phase 2 — implementation milestone HOLDS (2026-05-11 session); ready for commit pending Tier 3 founder smoke.** Detector + scaffolding milestone SHIPPED at `a4f293c`, CI run `25635424433` GREEN. This session's deliverables:

1. **Migration loop impl** — `migrate_v0_1_to_sealed_if_needed` (`crates/vault-storage/src/migration.rs`) end-to-end: cookie-presence check → 6-state detector → V0_1ShapeMigrate path runs full read-V0.1 → write-sealed → atomic dir-swap (Windows-correct ordering per iteration 2 §2 calibration C) → cookie/marker/backup cleanup → INFO log. PostSwapMarkerCleanup deletes ALPHA marker, returns NoMigrationNeeded. V0_2CleanNoOp / FirstRunInstallNoOp return NoMigrationNeeded. HalfStateCorruption / ThirdPartyData fail closed with named diagnostic substring per `feedback_quote_locked_artefacts_dont_paraphrase.md`. ~470 LOC including cookie-recovery state machine.
2. **Cookie-recovery state machine** — handles 3 named visible-states from iteration 2 §2 calibration B: temp_dir+!vector_dir → resume from step 8b; backup_dir+!vector_dir+!temp_dir → restore-then-restart; vector_dir-with-V0.1-shape → cookie-was-stale, restart. State 4 (any other) fails closed for manual triage.
3. **StorageBackend::open_with_at_rest_key** — sealed companion to `open()` per iteration 1 §3. Refactored shared assembly path (`Self::assemble`) to avoid duplicate validate_readable + audit + struct construction. `Application::new` flipped to call sealed companion with `&config.at_rest_key`. Plaintext `open()` retained for migration source path; Phase 3 deletes both.
4. **vault-tauri main.rs step 5b** — inserted between AppConfig construction (step 5) and Application::new (step 6). Calls migration before sealed StorageBackend opens. Fail-closed Err → `format_migration_error_dialog` + fatal startup dialog + non-zero exit per ADR-040 discipline.
5. **`format_migration_error_dialog`** + spec-pin test — parallels `format_keychain_error_dialog`'s pattern. Special-cases half-state + third-party variants with tailored recovery options + HANDOFF cross-link; falls through to `format_startup_failure_dialog` for non-migration variants.
6. **Two design-question catches in same session — both produced ADR amendments riding with this commit:**
   - **Signature fix:** `LanceVectorStore::open_with_at_rest_key(master_key)` was misleadingly named — it took master_key and derived K3 internally, while `AppConfig.at_rest_key` already holds the K3-derived key per ADR-040. First production caller (migration) would have hit `K3(K3(master_key))` silent-mismatch. Fixed: parameter rename master_key→at_rest_key, internal `derive_at_rest_key` removed, vault-storage's duplicate K3 site (`sealed_object_store::derive_at_rest_key`) deleted entirely. Canonical K3 site is now sole vault-app::keychain::derive_at_rest_key per ADR-040 amendment + this session's amendment.
   - **AAD path semantics fix (ADR-008 amendment v2):** First migration test `cookie_recovery_resumes_step_b_when_temp_dir_exists_and_vector_dir_missing` failed AEAD authentication after atomic-rename. Root cause: `compute_aad(file_path)` used the absolute filesystem path, breaking when migration's `temp_dir → vector_dir` rename changed every file's absolute path. Fixed: `SealedObjectStore` now tracks `base_path` from `new_store`'s URL; AAD computed over `relative_path = location.strip_prefix(base_path)`. Rename-invariant for the data dir; preserves within-vault position binding.
7. **Floor amendments surfaced + pre-approved 2026-05-11 in-session** per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`:
   - +5 pre-approved this session (3 cookie-recovery integration tests + 1 rename-positive integration `sealed_open_at_path_a_rename_to_b_open_at_b_succeeds` + 1 right-key-wrong-relative-path-negative unit `aad_differs_for_distinct_relative_paths_within_same_store`).
   - +4 surplus pre-approved this session: 1 unit-level rename-invariance positive + 3 defensive pins on `relative_path_str` helper. All cheap unit tests on a security-critical AAD function; defend rename-invariance against silent refactor regressions.
   - +1 vault-tauri test pre-approved per iteration 1 §5 (`format_migration_error_dialog_includes_recovery_options`).
   - **Plan amendment (cookie path naming):** `vector_dir.with_extension("vault_migration_in_progress")` per-vault sibling instead of iteration 2 §2 literal `vector_dir.parent().join(".vault_migration_in_progress")` — production semantics unchanged (one cookie per vault, in parent dir, JSON-encoded paths), but per-vault sibling form needed for parallel-test isolation under `RUST_TEST_THREADS=4` (every `tempfile::tempdir()` shares `/tmp/` as parent → literal name collides). Pre-approved in-session.
   - Net Phase 2 vault-storage test count: 232 lib + 16 migration = 248. Was 226 lib + 13 migration (7 detector + 6 ignored scaffolding) at scaffolding milestone `a4f293c`. Net +9 vault-storage tests this commit. Plus +1 vault-tauri.
8. **All workspace DoD gates green:** `cargo fmt --all --check` (after auto-apply); `cargo clippy --workspace --all-targets -- -D warnings`; `cargo build --workspace --all-targets` (zero warnings, 23m20s); `cargo test --workspace --no-fail-fast` (0 failures, 17 pre-existing ignored markers preserved).
9. **Tier 3 founder smoke status: BLOCKED on discovered Phase 1 follow-on.** During Tier 3 prep (2026-05-11 in-session) the integration gap surfaced: Phase 1's `read_or_init_master_key` generates a new random master_key on first launch when no keychain entry exists; it does NOT bridge from V0.1's `VAULT_KEY` env-var-derived SQLCipher passphrase. Phase 2 LanceDB migration would succeed (plaintext doesn't need a key) but `Application::new`'s `MetadataStore::open` would then fail at SQLCipher decryption with the wrong (new keychain-derived) passphrase. Phase 1's integration smoke at line 146 explicitly punted: "Phase 2/3 wire actual consumption into LanceVectorStore::open_with_at_rest_key. Tests here pre-date that path." So no integration test caught this. **Phase 2 ships as-is** (LanceDB migration is correct in isolation); the V0.1→V0.2 SQLCipher passphrase bridge is queued as a discovered Phase 1 follow-on with **HARD GATE before V0.2 alpha cohort distribution** (T0.2.14/T0.2.16) — see "Open tech-debt" entry below.

**Working tree at session pause** (all bundle with Phase 2 commit per `feedback_admin_changes_ride_with_code.md`): HANDOFF.md M, AppConfig and Cargo.toml workspace deps unchanged, `crates/vault-storage/{src/{cascading,migration,sealed_object_store,vector_store,lib}.rs, tests/migration_v0_1_to_sealed.rs, Cargo.toml}` M, `crates/vault-app/src/{application,lib}.rs` M, `crates/vault-tauri/src/{lib,main}.rs` M.

**Why we got here:** V0.2 first task per BRD §6 is T0.2.0 (LanceDB Encryption at Rest, HARD GATE per ADR-010). Spike v1 (lance 0.15 era, designed against `WrappingObjectStore`) FORMALLY FAILED 2026-05-07 — discovered lance-io 0.15's `LocalObjectReader` bypasses the `object_store::ObjectStore` trait for `file://` URIs in BOTH directions, defeating both the `WrappingObjectStore` wrapper AND direct injection via `ObjectStoreParams.object_store`. Web research found lance-io 4.x exposes a first-class `ObjectStoreProvider` + `ObjectStoreRegistry` API designed for this exact integration — but requires a **major lancedb upgrade** (0.8 → 0.27.2, 19 minor versions). Phase 0a executed the upgrade; Phase 0a-fix resolved a `merge_insert` memory regression surfaced by the upgrade (see ADR-038); Phase 0b audit + ADR-039 production fix; Phase 0c re-spiked the at-rest extension on the upgraded stack with the spike-discipline runtime-confirmation (per `feedback_runtime_confirmation_after_web_spike.md`) — and caught a real privacy bug in the Phase 0b ADR-039 implementation, fixed before V0.2 beta cohort exposure.

---

## T0.2.0 next-session opener (2026-05-11 session-end-2 checkpoint)

**On session open, do these two in order:**

### 1. Verify CI green on the latest fix-forward commit

The fix-forward commit this HANDOFF rides with addresses `ac577f4`'s windows-latest test failure by un-gitignoring the Tier 2 V0.1 fixture binary (`.gitignore` negation rule + `git add -f` on `vault.db` + `vault.db-wal` under `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/`). Look up the actual commit + run ID with:

```powershell
gh run list --workflow=ci.yml -L 3 --json databaseId,headSha,status,conclusion,displayTitle
```

Then verify the topmost run (most recent push):

```powershell
gh run view <RUN_ID> --json status,conclusion,jobs -q '"status=" + .status + " conclusion=" + (.conclusion // "(empty)") + "\n" + (.jobs | map("  " + .name + ": " + .status + (if .conclusion != "" then " (" + .conclusion + ")" else "" end)) | join("\n"))'
```

Trust `gh run view` actual status, NOT `gh run watch` exit code (per `feedback_gh_run_watch_exit_not_equal_run_status.md`).

- **If `conclusion=success`:** CI green, proceed to step 2.
- **If `conclusion=failure`:** broken CI is a regression — fix in this session per `feedback_broken_ci_is_regression_not_techdebt.md`. Diagnose via `gh run view <RUN_ID> --log-failed` and per-job failures in the JSON output.
- **If still `in_progress`:** historically the matrix takes 40-67 min; rerun the command after a few minutes.

### 2. Phase 3 — controls removal + acceptance suite (NEXT CODING WORK)

Per T0.2.0 close-out plan iteration 1 Phase 3:

- Remove ADR-010 banners (modal + persistent strip) from `crates/vault-tauri/`.
- Delete plaintext `LanceVectorStore::open` (retained through Phase 2 for migration source path) — sealed `open_with_at_rest_key` becomes the only constructor.
- Delete plaintext `StorageBackend::open` (parallels the LanceVectorStore deletion).
- Write T0.2.0 acceptance suite (DoD tests proving sealing is the only path — no plaintext escape hatches survive Phase 3).

Mechanical work; no major design questions expected. Tier 3 is NOT a prerequisite (deferred to first-alpha-cohort-member with real V0.1 data per session-end-2 reasoning above).

**Ride-along admin edits to bundle with Phase 3 commit** (per `feedback_admin_changes_ride_with_code.md`):
- Close the ADR-041 SQLCipher bridge tech-debt entry at line ~1779 — shipped this T0.2.0 closeout window across `6f2af9d` + `ac577f4` + the session-end-2 fix-forward.
- Correct or strike-through the stale Tier 3 procedure at this section's predecessor's lines 78-110 in the prior checkpoint (had `com.memoryvault.dev` instead of actual `com.shahbaz242630.memory-vault`; now superseded entirely by the Tier 3 skip decision in session-end-2 — easiest move is to delete the stale procedure rather than fix the path).

### 3. Reference — open work units (per T0.2.0 close-out plan iteration 1)

| Phase | Status | What |
|---|---|---|
| ADR-041 SQLCipher bridge | ✅ SHIPPED (`6f2af9d` + `ac577f4` + session-end-2 fix-forward) | Closes the Phase 2 follow-on gap; Tier 2 fixture-replay is the realism gate |
| Phase 2 Tier 3 founder smoke | SKIPPED on dev machine (deferred to first alpha-cohort member with real V0.1 data) | Tier 2 fixture-replay covers the realism layer; Tier 3 was an additional layer against a specific founder vault that doesn't exist on this dev box |
| Phase 3 — controls removal + acceptance suite | NEXT CODING WORK | Remove ADR-010 banners (modal + persistent strip), delete plaintext `LanceVectorStore::open` + plaintext `StorageBackend::open`, T0.2.0 acceptance suite (DoD tests proving sealing is the only path). Mechanical work; no major design questions expected. |
| Phase 4 — founder dogfood on sealed (Windows) | Blocked on Phase 3 | Re-run V0.1's 6-hour ADR-029 dogfood pattern against the sealed build |
| Phase 5 — T0.2.0 hard-gate clearance | Blocked on Phase 4 | Banners removed, sealing-only invariant locked, founder dogfood passed, all BRD §6.2 acceptance criteria green |

---

## T0.2.0 Phase 0 plan (mid-flight)

| Phase | Status | Work |
|---|---|---|
| **0a** | ✅ DONE | Bump lancedb 0.8 → 0.27.2 + transitive (lance-io 4.0, arrow 57, datafusion 52, object_store 0.12). Inventory breaking changes. **Compile: PASS.** Tests: 38 PASS / 1 FAIL — `concurrent_upserts_all_succeed` 8 GB allocation. Root-caused: lance 4.0 routes `merge_insert` through datafusion JOIN with no RAM ceiling (lance-format/lance#1983, #3601). |
| **0a-fix** | ✅ DONE | ADR-038 four layers all landed: (1) `Arc<tokio::sync::Mutex<()>>` in `LanceVectorStore` (Rust-side runtime serialisation), (2) `LANCE_MEM_POOL_SIZE=268435456` (256 MiB shell-side ceiling) at `.cargo/config.toml` `[env]` + ci.yml + vault-tauri main.rs doc + T0.2.14 forward-pointer, (3) `[build] jobs = 4` + `RUST_TEST_THREADS=4` in `.cargo/config.toml` (dev memory caps for build + test parallelism), (4) test-data updates for lance 4.0 behavior changes (Cosine NaN-zero-vector, footer-magic file format). Tests: **216/216 vault-storage lib pass; 382/382 workspace-wide pass** (17 ignored, all pre-existing markers). +1 new `lance_mem_pool_size_env_var_ceiling_reaches_test_process` regression pin for layer 2; 2 existing tests modified for layer 4 lance 4.0 finding (concurrent_upserts non-zero embeddings, corruption tests use footer instead of header). ADR-002 unsafe-collision rationale documented (rustc 1.92 + `std::env::set_var` unsafe → cannot be done from Rust). |
| **0b** | ✅ DONE (audit + ADR-039 production fix + 4 regression tests + classifier widen) | Brief vault-storage API drift audit, run during Phase 0a-fix CI wait. Initial inventory: 17 lancedb API surfaces compile + test clean against 0.27.2; connection lifecycle invariant preserved. Shahbaz pushed back THREE times on "low-risk" / "defer-to-V0.2" framing — first round added 3 memory-system verifications + classifier widen, second round added 2 deeper verifications (compaction effectiveness + sidecar surface), THIRD round escalated tombstoning from "ADR-territory deferred to Phase 0e" to "fix in this commit or product development ends here." Results: (1) `is_permanent` widened to recognise `schema`/`CastError`/`dimension`/`No vector column found` lance 4.0 wording variants. (2) `merge_insert_last_write_wins_for_embedding_column` PASSES — lance 4.0 preserves last-write-wins semantics; no data-corruption regression. (3) `read_during_write_returns_monotonic_consistent_snapshots` PASSES — V2 MVCC preserves snapshot reads. (4) **ADR-039 implemented in production**: `LanceVectorStore::delete()` now calls `OptimizeAction::Prune { older_than: TimeDelta::zero(), delete_unverified: true, error_if_tagged_old_versions: false }` immediately after `table.delete()`, holding the ADR-038 upsert mutex throughout. Verified empirically (Phase 0b session): `OptimizeAction::All` was INSUFFICIENT (5 files still contained probe-string post-cleanup, default 7-day retention); only zero-retention `Prune` achieves full physical removal (0 files post-prune, `prune.bytes_removed: 12162`). Trade-off: lose lance time-travel undo capability — correct for a privacy-property memory vault. (5) `delete_physically_removes_content_per_adr_039` regression pin: post-delete, scans every file under data dir; assertion fails loudly if the probe string is ever found post-delete (catches accidental Prune-call removal or lance retention-semantic regression). Code changes: `is_permanent` widened (`retry_queue.rs`) + production `delete()` modified to prune (`vector_store.rs`) + 1 new classifier test + 3 new regression tests. **Test count: 220** (was 216 pre-audit; +4 net from Phase 0b: classifier-variants + 3 regression probes; floor breach surfaced + approved before commit per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`). All 220 vault-storage tests pass. Bundles with next code commit per `feedback_admin_changes_ride_with_code.md`. |
| **0c** | ✅ DONE (spike v2 ALL STAGES PASS + ADR-039 production fix in same commit) | Rewrote `at_rest_spike.rs` using lance-io 4.x `ObjectStoreProvider` + `ObjectStoreRegistry`. Stages A/B/C runtime-confirmed `vault-sealed://` scheme intercepts both write+read flows (refutes v1 LocalObjectReader bypass). Stage D adversarial sweep + ADR-039-through-sealing PASS via Compact+Prune. Stage E 2×2 diagnostic ({sealed, plain file://} × {Prune-alone, Compact+Prune}) attributed ADR-039 issue to Lance Prune semantics, NOT sealing-wrapper interference (sealed and plain identical OptimizeStats). Discovered Phase 0b ADR-039 production code was insufficient for partial-fragment deletes (Memory Vault's actual single-id delete pattern); amended `vector_store.rs` `delete()` to `Compact + Prune`; added `delete_partial_fragment_physically_removes_content_per_adr_039` regression pin (test count 220→221, floor amendment surfaced + approved before commit per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`). All DoD gates green: build / 221 tests / clippy `-D warnings` / fmt --check. Sealing wrapper proven NON-interfering — Phase 0d wiring is straight production-mirror. |
| **0d** | ✅ DONE (production wiring + 5 sealed-path tests + workspace dep promotion) | New `crates/vault-storage/src/sealed_object_store.rs` (~395 LOC): `SealedObjectStore` (object_store::ObjectStore impl, AEAD seal/unseal on put_opts/get_opts, head/list size adjustments), `SealedFileStoreProvider` (lance-io ObjectStoreProvider impl for `vault-sealed://` scheme, overrides `extract_path` + `calculate_object_store_prefix` per Finding 2 to handle the unknown scheme), `derive_at_rest_key` (K3 BLAKE3-derived key wrapped in `Zeroizing` for Drop-zeroization), URL helpers `make_vault_sealed_uri` + `vault_sealed_to_local_path`. ADR-007 manual Debug redaction on both Sealed types — never leaks key bytes. New `LanceVectorStore::open_with_at_rest_key(path, dim, &master_key)` constructor in `vector_store.rs` (existing plaintext `open()` retained for V0.1 alpha backwards-compat — removed at formal at-rest gate close in T0.2.0 proper). New `_session: Option<Arc<Session>>` field on `LanceVectorStore` to keep the registered provider alive for the store's lifetime. Workspace deps promoted: `lance-io`, `lance-core`, `object_store`, `bytes`, `url`, `dryoc` → `[workspace.dependencies]` (production code now consumes them). `walkdir` stays spike-local. **5 new tests:** `sealed_open_round_trip_returns_inserted_rows` (orthogonal unit vectors → exact-match top-hit), `sealed_open_with_wrong_key_fails_closed` (AEAD authentication mismatch surfaces), `sealed_open_writes_framing_bytes_to_disk` (every on-disk file starts `0x01 0x00`, no PAR1 magic anywhere — the strongest single signal that v1 LocalObjectReader-bypass class is gone in production), `sealed_delete_partial_fragment_physically_removes_content` (ADR-039 partial-fragment invariant survives sealed wrapper, BLAKE3 content-hash-set-difference), `sealed_open_emits_distinguishing_info_log` (positive INFO assertion, bleed-resistant under tracing-test parallel-test sharing). **Test count: 221 → 226** (matches floor forecast exactly). All DoD gates green: build (workspace+all-targets) / 226 tests / clippy `-D warnings` / fmt --check. Phase 0c CI green confirmed before staging per CLAUDE.md per-step CI standing rule. |
| **0e** | ✅ DONE (doc-only, working tree only — rides with next code commit per `feedback_admin_changes_ride_with_code.md`) | ADR-037 drafted (lancedb 0.8 → 0.27.2 upgrade rationale, in force from Phase 0a). ADR-008 amendment drafted (V0.2 at-rest extension lock-in: K3 KDF, Finding 2(c) AAD, per-file granularity, ObjectStoreProvider integration, ADR-007 redaction, in force from Phase 0d). T0.2.0 close-out plan iteration 1 drafted (5 phases — Phase 1 keychain → Phase 2 V0.1 migration → Phase 3 controls-removal + acceptance suite → Phase 4 founder-dogfood-on-sealed Windows-only → Phase 5 hard-gate clearance; 2 open questions for iteration 2). Queued sweep edits applied: BRD §6.2 line 1411-1413 (stale ADR-008 spike reference + tmpfs prescription replaced by ObjectStoreProvider integration), BRD §11.5.1 (tmpfs/memory-handle prescription superseded by sealed-ObjectStore-adapter approach), ADR-013 supersession note (chrono pin advanced 0.4.39 → 0.4.44 by ADR-037 trigger 1), V0.2-task-decomposition framing fix (alpha-distribution-as-first-task superseded by BRD §6 numerical ordering). ADR-038 already drafted at 0a-fix; ADR-039 amendment text already in this file. |

---

## Session-end working-tree state (2026-05-09, Phase 0e doc-drafting block)

**Phase 0d (production-wire SealedFileStoreProvider) was committed at `dcefd9b` and CI-green at run `25565477569`.** The files listed below are the historical Phase 0d record; they are no longer in the working tree as uncommitted changes. Phase 0e doc-drafting added the following uncommitted edits (all doc-only, ride with next code commit per `feedback_admin_changes_ride_with_code.md`):

- **`HANDOFF.md`** — header `Last updated` flipped to 2026-05-09; Active Task block flipped to Phase 1 keychain spike; Phase 0e row in Phase 0 plan table flipped to DONE; V0.2 task decomposition framing fix; Doc-only-edits-queued items struck-through with "APPLIED Phase 0e sweep"; ADR-013 entry annotated with chrono 0.4.39 → 0.4.44 advance per ADR-037 trigger 1; new sections inserted: ADR-037 (lancedb upgrade rationale), ADR-008 amendment (V0.2 at-rest extension lock-in), T0.2.0 close-out plan iteration 1.
- **`Agent_Build_Specification.txt`** — §6.2 T0.2.0 lines 1411-1415 amended to mark the ADR-008 spike + Windows-path TODO + implementation as resolved/done with cross-references to ADR-037 + ADR-008 amendment + Phase 0d commit; §11.5.1 LanceDB tmpfs prescription marked superseded for the LanceDB path with explicit pointer to the `vault-sealed://`-via-ObjectStoreProvider integration; DuckDB path noted as still-open-question.

**Phase 0d (production-wire SealedFileStoreProvider) — historical record of files modified at commit `dcefd9b`:**
- `Cargo.toml` (workspace, NEW dep entries): added `lance-io = "=4.0.0"`, `lance-core = "=4.0.0"`, `object_store = "=0.12.5"`, `bytes = "=1.11.1"`, `url = "=2.5.8"` to `[workspace.dependencies]`. Versions match Cargo.lock pre-flight snapshot from Phase 0c. `dryoc` was already workspace.
- `crates/vault-storage/src/sealed_object_store.rs` (NEW, ~395 LOC): production module ported from `examples/at_rest_spike.rs`. Strips spike instrumentation (fire-counters, eprintln traces, stage scaffolding, identity provider, test-only helpers). Keeps: `SealedObjectStore` (object_store::ObjectStore impl), `SealedFileStoreProvider` (lance_io::object_store::providers::ObjectStoreProvider impl), sealing primitives (`derive_at_rest_key` / `compute_aad` / `seal_file_bytes` / `unseal_file_bytes`), URL helpers, locked sealing-shape constants (`VERSION_BYTE` / `GRANULARITY_PER_FILE` / `TOTAL_FRAMING_LEN` / `SEAL_OVERHEAD`), `VAULT_SEALED_SCHEME` const. ADR-007 manual `Debug` redaction on both Sealed types — never leaks key bytes. Key wrapped in `Arc<Zeroizing<[u8; 32]>>` for cheap-clone + Drop-zeroize. Multipart writes return `NotSupported` (per-file granularity locked, V1.0 revisit if column-projection latency surfaces).
- `crates/vault-storage/src/lib.rs`: added `pub mod sealed_object_store;` + `pub use sealed_object_store::{derive_at_rest_key, make_vault_sealed_uri, SealedFileStoreProvider, VAULT_SEALED_SCHEME};` re-export.
- `crates/vault-storage/src/vector_store.rs`:
  - imports: `lancedb::{ObjectStoreRegistry, Session}`, `zeroize::Zeroizing`, `crate::sealed_object_store::*`.
  - new field on `LanceVectorStore`: `_session: Option<Arc<Session>>` (held for provider lifetime guarantee; never read after construction; leading underscore intentional).
  - existing `open()` updated to set `_session: None` (V0.1 plaintext path unchanged in behavior).
  - new constructor `open_with_at_rest_key(data_dir: &Path, dimension: usize, master_key: &[u8; 32]) -> VaultResult<Self>`: builds `ObjectStoreRegistry::default()` + inserts `SealedFileStoreProvider` for `vault-sealed://` scheme, wraps in `Session::new(0, 0, Arc::new(registry))`, opens via `lancedb::connect(uri).session(session.clone()).execute()`. URI built via `make_vault_sealed_uri(canonical_abs_path)`. Does NOT emit V0.1 plaintext WARN; does emit distinguishing INFO `"LanceVectorStore opened (at-rest sealed path)"`.
  - +5 tests at end of `mod tests`: `sealed_open_round_trip_returns_inserted_rows`, `sealed_open_with_wrong_key_fails_closed`, `sealed_open_writes_framing_bytes_to_disk`, `sealed_delete_partial_fragment_physically_removes_content`, `sealed_open_emits_distinguishing_info_log`. Helper `walk_every_file` shared across the new tests.
- `crates/vault-storage/Cargo.toml`: workspace-dep entries for production deps moved into `[dependencies]` block (lance-io / lance-core / object_store / bytes / url / dryoc all `{ workspace = true }`); `[dev-dependencies]` retains `walkdir = "=2.5.0"` only (production code uses `std::fs::read_dir` recursion).

**Phase 0d — files NOT modified (intentional):**
- `crates/vault-storage/examples/at_rest_spike.rs`: untouched. Stays as runtime-confirmation evidence + ADR-008 amendment cross-reference at Phase 0e.
- `crates/vault-storage/examples/at_rest_spike.rs.v1_fail_disabled`: untouched. ADR-008 amendment cross-references the v1 FAIL evidence at Phase 0e.
- ADR-010 banners (modal + persistent strip in vault-tauri): NOT removed in Phase 0d — banner removal is gated on the formal at-rest gate close in T0.2.0 proper, not the wiring.
- `LanceVectorStore::open()` (plaintext): retained unchanged for V0.1 backwards-compat. Removed in T0.2.0 proper.

**HANDOFF.md updates (this file):**
- Header `Last updated` flipped to Phase 0d completion.
- Active Task block rewritten — Phase 0e is now the active task.
- Phase table: 0d row PENDING → DONE with full deliverables; 0e expanded.
- Working-tree state section (this section) replaced with Phase 0d files.

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

**All four queued items applied to working tree at Phase 0e sweep, 2026-05-09. Listed here for traceability; the next code commit (Phase 1 keychain spike) bundles the diff.**

- ~~**HANDOFF.md plan amendment** (lines 15 + 40 of pre-2026-05-07 state)~~ — APPLIED Phase 0e sweep: "V0.2 task decomposition" section rewritten to reflect BRD §6.2 numerical ordering (T0.2.0 first per ADR-010 hard-gate, not alpha-distribution).
- ~~**BRD §6.2 line 1412 amendment**~~ — APPLIED Phase 0e sweep: stale "ADR-008 dryoc/RustCrypto/sibling-crate spike" reference replaced with ADR-008 closure + ADR-037 (lancedb upgrade) + ADR-008 amendment (V0.2 at-rest extension lock-in) cross-references.
- ~~**BRD §11.5.1 amendment**~~ — APPLIED Phase 0e sweep: tmpfs/memory-handle prescription replaced with ObjectStoreProvider-integration via `vault-sealed://` URI scheme per ADR-008 amendment locks; spike v2 runtime evidence + Phase 0d production wiring satisfies the integration condition.
- ~~**ADR-013 supersession note**~~ — APPLIED Phase 0e sweep: chrono pin advance 0.4.39 → 0.4.44 (ADR-037 trigger 1: arrow upgrade past the conflict via arrow-arith 57.2.0) noted in HANDOFF.md "Active ADRs with V0.2+ implications" entry; archive entry remains unchanged for historical record per archive-frozen convention.

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

**Per BRD §6.2 numerical ordering**, T0.2.0 (LanceDB Encryption at Rest, HARD GATE per ADR-010) is the first V0.2 task — NOT alpha-distribution. The alpha-cohort-distribution work (T0.2.14 Stub Installer + T0.2.16 Beta Onboarding) is hard-blocked on T0.2.0 acceptance per ADR-010 contract; it cannot land first. T0.2.0 close-out subtasks are enumerated in the "T0.2.0 close-out plan iteration 1" section below; remaining T0.2.x tasks (T0.2.1 vault-llm Phi-4-mini, T0.2.2-T0.2.6 vault-consolidator phases, T0.2.7 retrieval multi-strategy + vector index, T0.2.8 boundary enforcement, T0.2.9-T0.2.13 vault-sync, T0.2.14-T0.2.16 distribution + onboarding) follow per BRD §6.2 ordering. (Earlier framing here as "alpha-distribution as natural first task" was wrong — superseded 2026-05-09 at Phase 0e sweep edits.)

---

## ADR-037 — lancedb 0.8.0 → 0.27.2 upgrade (drafted 2026-05-09, T0.2.0 Phase 0e)

**Status:** ACCEPTED, in force from T0.2.0 Phase 0a (commit `1e58d30` Phase 0a-fix; the upgrade itself landed in the Phase 0a working tree consumed by 0a-fix).

**Context.** V0.1 was on `lancedb = "=0.8.0"` (lance 0.15, lance-io 0.15, arrow 51/52 split, datafusion 40, object_store 0.10) per ADR-012 ("AWS SDK + dual-arrow accepted as V0.1 cost"). T0.2.0 (LanceDB Encryption at Rest, HARD GATE per ADR-010) requires intercepting Lance's I/O so a sealing layer can wrap every read and write. The V1 spike (run 2026-05-07, archived as `at_rest_spike.rs.v1_fail_disabled`) FORMALLY FAILED at lance-io 0.15: both `WrappingObjectStore` and direct `ObjectStoreParams.object_store` injection were bypassed by lance-io's `LocalObjectReader` fast-path for `file://` URIs in BOTH directions. Web research (Phase 0a, verification report 2026-05-08) found lance-io 4.x exposes a first-class `ObjectStoreProvider` + `ObjectStoreRegistry` API. Registering a custom provider for an UNKNOWN scheme (`vault-sealed://`) bypasses any built-in fast-paths because there's no fast-path implementation for unknown schemes. Reaching that API requires moving from lancedb 0.8 to **0.27.2** — 19 minor versions, transitively `lance 4.0-rc.3`, `lance-io 4.0`, `arrow 57`, `datafusion 52`, `object_store 0.12`.

**Decision.** Adopt `lancedb = "=0.27.2"` workspace-wide. The bump is the only known integration path for the at-rest extension; staying on 0.8 forces either `WrappingObjectStore` (proven non-functional at v1 spike Stage A — fire-counters read 0 on the read flow) or a much heavier sealed-tarball-on-close architecture (rejected at ADR-010 option B as half-baked-crypto). The chosen path keeps Lance's local-filesystem semantics intact and routes every byte through our provider.

**Consequences (downstream ADRs forced by this upgrade).**

- **ADR-038 (Concurrent-upsert serialisation + LANCE_MEM_POOL_SIZE shell ceiling).** lance 4.0 routes `merge_insert` through datafusion's full JOIN planner with no implicit per-table serialisation and no RAM ceiling by default. The pre-existing `concurrent_upserts_all_succeed` test surfaced the regression as an 8 GB allocation aborting the process. Three-layer + sibling defense landed at Phase 0a-fix. **This ADR would not have been written without the upgrade.**

- **ADR-039 amendment (Compact+Prune for partial-fragment deletes).** lance 4.0's `OptimizeAction::Prune` does not compact partial fragments — `data_files_removed: 0` empirically on the single-id-delete pattern that is Memory Vault's actual production API. The Phase 0c spike Stage E 2×2 diagnostic discovered this and the production fix landed in the same commit. **This finding would not have surfaced without the upgrade — V0.1's lancedb 0.8 path didn't have the same tombstone semantics.**

- **ADR-013 supersession (chrono pin advanced 0.4.39 → 0.4.44).** ADR-013's trigger 1 ("arrow upgrade past the conflict") fired: `arrow-arith 57.2.0` resolved the `ChronoDateExt::quarter()` collision that forced the original 0.4.38 pin. The pin form (`=0.4.44`) is retained per ADR-013 monthly-CVE-check discipline; the version inside the pin advanced. The pre-existing dual-bound band `[0.4.39, 0.4.44)` is now `[0.4.44, 0.4.45)` — single-version pin. ADR-013 amendment text in `HANDOFF_V0.1_ARCHIVE.md` will cross-link this ADR.

- **lance 4.0 Cosine NaN-vector behaviour change (test-only finding, ADR-038 Layer 4).** lance 4.0 filters NaN-distance rows from Cosine search where lance 0.15 included them. Production unaffected — BGE-small-en-v1.5 produces L2-normalised non-zero embeddings. Test-only adjustment landed at Phase 0a-fix. Upstream issue filing tracked as tech-debt.

- **ADR-012 dormant-cloud-backend footprint unchanged.** lance-io 4.0 still pulls `aws-config`, `aws-sdk-*`, etc. through the same `object_store` cloud-backend default-features path. ADR-012's monitoring + revisit triggers continue to apply.

- **Build-time + test-runtime memory regression (ADR-038 Layer 3 + sibling).** lance 4.0's heavier dep tree roughly tripled per-rustc link-time peak memory; surfaced as `LNK1102: out of memory` on Shahbaz's 16-CPU / 16 GB Windows dev box. Resolution: `[build] jobs = 4` + `RUST_TEST_THREADS = 4` in `.cargo/config.toml`.

**Verification.**

- `cargo build --workspace --all-targets`: clean, zero warnings.
- `cargo test -p vault-storage`: 226/226 passing post-Phase-0d (was 39/39 pre-upgrade).
- `cargo test --workspace`: workspace-wide passing on `[ubuntu-latest, windows-latest, macos-latest]` per Phase 0d push CI run `25565477569` (success in 47m53s).
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --all --check`: clean.
- Spike v2 runtime confirmation (Phase 0c Stages A-E PASS) proved the at-rest integration path the upgrade enabled is functional.
- Phase 0d 5 sealed-path regression tests confirm the production-wired path matches the spike's runtime evidence.

**When to revisit.**

- **Lance V2 merge path lands with bounded memory by construction** ([lance#6151](https://github.com/lance-format/lance/issues/6151)) → drop ADR-038 Layer 2 (env var) first, then Layer 1 (mutex) only after concurrent-upsert runtime confirmation.
- **lancedb gains feature flags for cloud backends** → ADR-012 binary-size revisit fires.
- **lance/lancedb major-version bump** → re-audit dep graph; re-run at-rest spike to confirm `ObjectStoreProvider` + `ObjectStoreRegistry` API still works.

**Cross-links.** ADR-008 (V0.2 at-rest extension amendment, Phase 0e). ADR-010 (V0.1 plaintext-LanceDB compensating-controls; this upgrade enables T0.2.0 acceptance). ADR-012 (dormant-cloud-backend acceptance, unchanged). ADR-013 (chrono pin advanced — see above). ADR-038 (concurrent-upsert + LANCE_MEM_POOL_SIZE, forced by this upgrade). ADR-039 (Compact+Prune amendment, forced by this upgrade). Spike v1 evidence: `crates/vault-storage/examples/at_rest_spike.rs.v1_fail_disabled`. Spike v2 evidence: `crates/vault-storage/examples/at_rest_spike.rs`.

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

## ADR-039 amendment — Compact-then-Prune required for partial-fragment deletes (Phase 0c spike Stage E discovery, 2026-05-08)

**Status:** ACCEPTED, in force from this commit. Supersedes the Phase 0b ADR-039 implementation (Prune-alone) which was insufficient for Memory Vault's actual delete API pattern.

**Context.** ADR-039 (Phase 0b, 2026-05-07, commit `2d3c57a`) shipped `LanceVectorStore::delete()` calling `Table::optimize(OptimizeAction::Prune { older_than: TimeDelta::zero(), delete_unverified: true, error_if_tagged_old_versions: false })` immediately after `table.delete()`, with the Phase 0b regression test `delete_physically_removes_content_per_adr_039` confirming "0 probe-files post-prune" and `prune.bytes_removed: 12162`. The privacy contract claim was: "deleted memory bytes are physically removed from disk."

**Discovery.** Phase 0c spike v2 ran a 2×2 diagnostic matrix (Stage E) that crossed `{plain file://, vault-sealed://}` × `{Prune-alone, Compact+Prune}` to attribute potentially-anomalous behavior between Lance Prune semantics and the sealing wrapper. Lance's own `OptimizeStats` made the answer unambiguous:

| Scenario | Result | OptimizeStats |
|---|---|---|
| plain file:// + Prune-alone | **FAIL** (data file bit-for-bit identical post-Prune) | `prune.data_files_removed: 0` |
| plain file:// + Compact+Prune | PASS | `compaction.fragments_removed: 1, fragments_added: 1`; `prune.data_files_removed: 1, bytes_removed: 159379` |
| vault-sealed:// + Prune-alone | **FAIL** (identical) | `prune.data_files_removed: 0` |
| vault-sealed:// + Compact+Prune | PASS | `compaction.fragments_removed: 1, fragments_added: 1`; `prune.data_files_removed: 1, bytes_removed: 159379` |

Sealed and plain paths are byte-identical in Lance's reported stats — sealing wrapper is **not** the cause; this is plain Lance 4.0 Prune semantics.

**Why Phase 0b's regression test passed despite the bug.** The Phase 0b test (`delete_physically_removes_content_per_adr_039`) writes 5 rows to one boundary, then deletes ALL 5 rows. With every row in the fragment marked for deletion, the fragment becomes empty and Lance can drop it via Prune alone (no compaction needed — a special case). The test's probe-string scan correctly returns 0 hits because the entire fragment file is gone. **But Memory Vault's actual delete API is `LanceVectorStore::delete(memory_id)` — single-row delete-per-call.** Each call leaves siblings in the same fragment, producing partial-fragment tombstones — the case Prune-alone does not handle. The Phase 0b test exercised the special case, not the production case.

**Decision.** `LanceVectorStore::delete()` now calls Compact-then-Prune in sequence. Compact rewrites the partial fragment with surviving rows only (dropping tombstoned bytes from the data file); Prune-with-zero-retention then removes the orphaned original. Both operations hold the ADR-038 upsert mutex throughout — preserves Phase 0b's defense-in-depth against in-flight upsert races. Same trade-off (lose lance time-travel undo) accepted as correct for a privacy-property memory vault.

**Code change.** `crates/vault-storage/src/vector_store.rs`:
- Import: add `CompactionOptions` to the `lancedb::table::{...}` import.
- `delete()` body lines 350-395: add `OptimizeAction::Compact { options: CompactionOptions::default(), remap_options: None }` BEFORE the existing `OptimizeAction::Prune { ... }`. Comment block expanded with the Stage E findings + measured stats.

**New regression pin.** `delete_partial_fragment_physically_removes_content_per_adr_039` (companion to the existing full-fragment pin):
- Writes 10 rows to one boundary (single fragment).
- Deletes the FIRST 5 rows by single-id call (partial-fragment delete — the actual production pattern).
- Snapshots pre-delete data file content-hashes (BLAKE3).
- Asserts EVERY pre-delete data file's content-hash is absent from post-delete file content-hashes (set difference must be empty).
- Under Prune-alone the data file is bit-for-bit identical → set-difference contains the original hash → assertion fails loudly. Under Compact+Prune the data file is rewritten with 5 surviving rows only → original hash gone → set-difference empty → test passes.

**Latency.** Each delete now pays Compact + Prune. Estimated ~0.5-2s on test fixture (scales with fragment size, not old-version count). Acceptable for the privacy property; Memory Vault deletes are rare events.

**Verification.**
- 221/221 vault-storage tests pass post-amendment (was 220 pre-Phase-0c; +1 floor breach for the new partial-fragment pin, surfaced + approved by Shahbaz before commit per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`).
- Spike Stage D end-to-end PASS using Compact+Prune sequence (matches production exactly).
- Spike Stage E 2×2 matrix shows Compact+Prune passes on both plain file:// and vault-sealed:// paths.
- DoD gates green: build / test 221/221 / clippy `-D warnings` / fmt --check.

**Upstream tracking.** Re-evaluate when any of: (a) Lance 4.0+ surfaces an `OptimizeAction` variant that internally does Compact+Prune as one atomic operation; (b) Lance changes Prune semantics to compact partial fragments by default; (c) lance-format/lance issue tracker discusses unifying compaction with prune. Until then, Compact+Prune is the locked sequence.

**Bounded blast radius of the Phase 0b bug.** V0.1 is alpha-only (founder dogfood per ADR-029). No external user has ever held a build claiming "permanent deletion" — the privacy contract was only ever exposed internally. T0.2.0's V0.2 beta cohort distribution would have shipped the Phase 0b bug to 30 external users; the Phase 0c spike caught it 2 phases ahead of that exposure. **No remediation owed.**

---

## ADR-008 amendment — V0.2 at-rest extension lock-in (drafted 2026-05-09, T0.2.0 Phase 0e)

**Status:** ACCEPTED, in force from T0.2.0 Phase 0d (commit `dcefd9b`; production module landed at `crates/vault-storage/src/sealed_object_store.rs` + the `LanceVectorStore::open_with_at_rest_key` constructor).

**Context.** ADR-008 (V0.1, 2026-04-29) locked Path #1 (DryocStream-as-single-message) for the T0.2.9 sync-envelope. The same primitive applies to the V0.2 at-rest extension on top of LanceDB, but the at-rest call site differs in three ways from sync envelopes:
1. **Per-file invocation, not per-memory.** The at-rest layer wraps Lance's data files (Parquet shards, manifest fragments, index files), not application-level memory records. AAD must bind the file's identity, not a memory's.
2. **Integration via lance-io's ObjectStoreProvider.** lancedb 0.27.2 (per ADR-037) exposes a registry-based mechanism that intercepts I/O for a custom URI scheme (`vault-sealed://`). The seal/unseal routines are called from inside an `object_store::ObjectStore` impl, not from application code.
3. **No version_id in V0.2.** The sync-envelope path will (T0.2.9+) bind a sync-state version into AAD; the at-rest path cannot, because Lance writes files with manifest sequencing that is itself the version-coordinate. Adding version-id binding here would either circular-depend on Lance's manifest or duplicate it. V0.2 documents the replay-attack residual (sealed-file replaced with older sealed copy of itself at the same path) and revisits at V1.0.

The Phase 0c spike v2 runtime-confirmed all locks below; the Phase 0d production module ports the spike verbatim minus instrumentation.

**Decision (locks added on top of ADR-008's Path #1).**

- **K3 KDF.** `at_rest_key = blake3::derive_key("vault memory at-rest sealing v1", &master_key)`. Single-source-crypto preserves ADR-008 line 693 principle ("crypto crates are not a place to multiply-source"). The domain-separator string `"vault memory at-rest sealing v1"` is distinct from any other BLAKE3 use in the workspace — the audit chain uses BLAKE3 directly (no `derive_key`), the sync-envelope AAD uses `blake3::hash` with prefix `"vault-aad-v1"`, and the at-rest AAD (below) uses `blake3::hash` with prefix `"vault-at-rest-v1"`. The `"v1"` suffix lets us rotate the KDF later without ambiguity.

- **AAD scheme (Finding 2 candidate (c)).** `AAD = BLAKE3("vault-at-rest-v1" || file_path_bytes)` — the file path inside the `vault-sealed://` URI bound into AEAD authentication. Wrong-path-decryption fails closed (verified by spike Stage B + Phase 0d `sealed_open_with_wrong_key_fails_closed` regression pin). Distinct domain separator from ADR-008's sync-envelope AAD (`"vault-aad-v1"` per line 700). Inputs are unambiguous because `file_path_bytes` is the post-`extract_path` normalised path; no length-prefix needed. **No version_id binding in V0.2** — replay residual documented at top of this section; revisit V1.0.

- **Sealing shape (locked, per-file granularity).** `sealed_bytes = version_byte (0x01) || granularity_marker (0x00 = per-file) || dryoc_header (24 bytes) || ciphertext`. 26-byte framing prefix + 17-byte AEAD overhead per ADR-008 line 686 = **43-byte total per-file overhead**. Materialised in `crates/vault-storage/src/sealed_object_store.rs` as `VERSION_BYTE` / `GRANULARITY_PER_FILE` / `TOTAL_FRAMING_LEN` / `SEAL_OVERHEAD` constants. Phase 0d regression pin `sealed_open_writes_framing_bytes_to_disk` asserts every on-disk file under the sealed data dir starts with `0x01 0x00` and contains zero PAR1 magic — the strongest single signal that the v1 LocalObjectReader-bypass class is gone in production.

- **Per-file granularity (V0.2 unconditional).** Every file Lance writes through the provider seals as one envelope. No chunking. Per ADR-008 line 707 ("Path #1 ... is the right shape for V0.1, V0.2, and likely V1.x"); BRD §11.7.1 100KB per-memory cap means the largest sealed Parquet shard (the embedding column) is comfortably bounded. Multipart writes return `NotSupported` from `SealedObjectStore::put_multipart` — per-file granularity is locked at the trait boundary. **Revisit triggers (any):** column-projection latency benchmark shows per-file granularity insufficient (V1.0 retrieval-perf review); BRD §11.7.1 raises per-memory cap above 1MB; a Lance compaction operation produces files larger than the in-memory copy budget.

- **ObjectStoreProvider integration (`vault-sealed://` URI scheme).** `SealedFileStoreProvider` registered in a per-store `lance::ObjectStoreRegistry` for the `vault-sealed://` scheme; `LanceVectorStore::open_with_at_rest_key` builds the registry, wraps it in a `Session`, and connects via `lancedb::connect(uri).session(session.clone()).execute()`. The provider overrides `extract_path` and `calculate_object_store_prefix` per spike Finding 2 to handle the unknown scheme — Lance's defaults assume scheme-known-paths and would mis-route `vault-sealed://` URIs. The session is held on `LanceVectorStore` via `_session: Option<Arc<Session>>` to keep the registered provider alive for the store's lifetime.

- **AAD-parameter sized-input quirk (extending ADR-008 line 684).** `dryoc::DryocStream::push_to_vec` and `pull_to_vec` require `Option<&Vec<u8>>` for AAD — not `Option<&[u8]>` — for the same `Sized` trait bound that ADR-008 documented for plaintext. The at-rest seal/unseal routines materialise the BLAKE3 AAD digest as a `Vec<u8>` before passing. T0.2.9 sync envelopes have the same constraint per ADR-008 line 684; this amendment confirms it applies symmetrically to the at-rest call site.

- **dryoc 0.7.2 unchanged.** Same dep version as ADR-008 lock. Workspace-promoted to `[workspace.dependencies]` at Phase 0d (was vault-sync `[dev-dependencies]` pre-V0.2).

- **ADR-007 manual `Debug` redaction.** Both `SealedObjectStore` and `SealedFileStoreProvider` carry the at-rest key; both have manual `Debug` impls that redact the key field as `<redacted>`. The key itself is wrapped in `Arc<Zeroizing<[u8; 32]>>` for cheap-clone + Drop-zeroization per BRD §11.5.3. Verified at Phase 0d code-review reading.

**Consequences.**

- **43-byte per-file overhead** is bounded and small. For BRD §11.7.1's 100KB-per-memory ceiling, the worst-case sealed-file ratio is ≤0.05%; for small index files (<1KB) the ratio is higher but the absolute cost is sub-millisecond. No production benchmark warranted in V0.2.

- **Replay-attack residual (no version_id binding).** A sealed file at path P can be replaced with an older sealed copy of itself at path P and validate. Memory Vault's threat model (BRD §11.1) flags this as low-likelihood: the attacker needs filesystem write access to the data dir, at which point they already have the SQLCipher metadata + audit log access path open — that is the honest context, not a mitigant; the threat model already treats this as an attacker-already-inside scenario. **V0.2 has no detection mechanism for the replay residual. The risk is documented, not mitigated.** V1.0 threat-model review designs real detection — likely candidates: version_id binding into AAD, or audit-chain row-count checkpoint at Lance manifest sequence boundaries.

- **`vault-sealed://` URI scheme is reserved across the workspace.** No other crate may register a provider for this scheme. Documented at the `VAULT_SEALED_SCHEME` constant in `sealed_object_store.rs` and exposed via `pub use` from `vault-storage::lib.rs`.

- **Multipart writes are unsupported.** Lance's V2 path mostly does not multipart-upload to local files, but if a future Lance feature (e.g. a column-projection compaction) calls `put_multipart`, the provider will return `NotSupported` and the operation will fail loudly. **Revisit trigger:** Lance changelog announces multipart-local-write usage.

**Verification.**

- Phase 0c spike v2 (`crates/vault-storage/examples/at_rest_spike.rs`) Stages A-E PASS. Stage A fire-counters confirm UNKNOWN-scheme bypass works; Stage B AAD round-trip + wrong-key adversarial; Stage C end-to-end sealed write+read; Stage D PAR1-magic absence + entropy ≥ 7.9; Stage E 2×2 plain×sealed × Prune-alone × Compact+Prune diagnostic (drove the ADR-039 amendment).
- Phase 0d production-side regression pins: `sealed_open_round_trip_returns_inserted_rows`, `sealed_open_with_wrong_key_fails_closed`, `sealed_open_writes_framing_bytes_to_disk`, `sealed_delete_partial_fragment_physically_removes_content`, `sealed_open_emits_distinguishing_info_log`. 226/226 vault-storage tests pass post-Phase-0d.
- CI matrix sustained green across `[ubuntu-latest, windows-latest, macos-latest]` per Phase 0d push CI run `25565477569`.

**When to revisit.**

- **Column-projection latency surfaces** at V1.0 retrieval review → switch to per-column or per-row-group granularity. Granularity-marker byte (`0x00 = per-file`) is reserved for forward-compat: `0x01` could mean per-column, `0x02` per-row-group, etc. — distinguishable on read.
- **BRD §11.7.1 per-memory content cap raised above 1MB** → re-evaluate per-file granularity vs chunked streaming (same triggers as ADR-008 line 711).
- **V1.0 threat-model review elevates replay-attack risk** → add version_id binding to AAD; bump domain-separator string to `"vault-at-rest-v2"`.
- **dryoc minor-version bump** → re-run spike v2 to confirm DryocStream API still matches.
- **lance-io API change** affecting `ObjectStoreProvider` / `ObjectStoreRegistry` / `Session` shape → re-run spike v2; potentially bump domain-separator if any sealed-on-disk format changes.
- **Lance announces multipart-local-write usage** → spike `put_multipart` integration; lift the `NotSupported` return.

**Cross-links.** ADR-008 V0.1 lock (`HANDOFF_V0.1_ARCHIVE.md`, 2026-04-29). ADR-007 (manual `Debug` redaction). ADR-010 (V0.1 plaintext-LanceDB compensating controls; this amendment is the integration that closes them at T0.2.0 acceptance). ADR-037 (lancedb upgrade rationale; this amendment is the at-rest extension built on the upgraded stack). ADR-039 amendment (Compact+Prune; the seal/unseal layer is non-interfering per spike Stage E 2×2 diagnostic, AND the privacy property survives composition through the sealed wrapper per Phase 0d regression pin `sealed_delete_partial_fragment_physically_removes_content`). Spike v2 evidence: `crates/vault-storage/examples/at_rest_spike.rs`. Production module: `crates/vault-storage/src/sealed_object_store.rs`.

---

## ADR-008 amendment v2 — AAD path semantics: absolute → relative-to-store-root (T0.2.0 Phase 2 implementation, 2026-05-11)

**Status:** ACCEPTED, in force from this commit (T0.2.0 Phase 2 implementation milestone). Supersedes the AAD path semantics in the original ADR-008 amendment (2026-05-09) for the on-disk computation; the K3 KDF, sealing shape, granularity, ObjectStoreProvider integration, and ADR-007 redaction locks all stand unchanged.

**Context.** ADR-008 amendment (2026-05-09) locked AAD as `BLAKE3("vault-at-rest-v1" || file_path_bytes)`. The Phase 0d spike + production wiring computed `file_path_bytes` from `location.as_ref()` inside `SealedObjectStore::aad_for_path`, where `location: &ObjectPath` is the path Lance hands the wrapper at I/O time. For `LocalFileSystem::new()` (the inner store the SealedFileStoreProvider wraps), this stringifies to the **absolute filesystem path** that `SealedFileStoreProvider::extract_path` produced via `ObjectPath::from_absolute_path`.

Phase 0d round-trip + wrong-key tests passed because they always opened, sealed, dropped, and re-opened at the SAME path — AAD was identical on both sides. Migration's atomic dir-swap pattern is the first caller that needs **file mobility**: Phase 2 step 8a renames `vector_dir → backup_dir`, step 8b renames `temp_dir → vector_dir`, and the next-launch open of the new sealed `vector_dir` recomputes AAD from the post-rename absolute path — which doesn't match the AAD bound at seal time (over the pre-rename `temp_dir` path). Result: AEAD authentication fails with "Message authentication mismatch", sealed data unreadable.

**Discovery.** Surfaced 2026-05-11 by `cookie_recovery_resumes_step_b_when_temp_dir_exists_and_vector_dir_missing` — the first test that exercised seal-at-A → rename-A→B → open-at-B. Phase 0d's test surface couldn't see this class because round-trip tests are path-stable by construction. The catch landed before any sealed user data exists in production (V0.2 hasn't shipped); had it surfaced post-beta-cohort, the fix would have required a key-rotation migration or a sealed-format version-byte bump.

**Decision.** AAD is computed over the **path relative to the vault data dir root**, not the absolute filesystem path. `SealedObjectStore` stores `base_path: ObjectPath` (computed once in `SealedFileStoreProvider::new_store` from the `vault-sealed://<abs>` URL Lance hands in). `aad_for_path(location)` strips the `base_path` prefix from `location` and computes `BLAKE3("vault-at-rest-v1" || relative_path)`. Domain-separator string `"vault-at-rest-v1"` UNCHANGED — relative-vs-absolute is an input-canonicalisation change, not a domain-separator change.

**Properties preserved (relative to V1 amendment).**

- **Within-vault position binding.** Two files at distinct relative paths under the same store yield distinct AADs. Prevents within-vault file substitution attacks (e.g., swapping `_versions/<old>.manifest` over `_versions/<current>.manifest`). Pinned by `aad_differs_for_distinct_relative_paths_within_same_store`.
- **Per-file granularity** (still 26-byte framing prefix + 17-byte AEAD overhead = 43-byte total).
- **K3 KDF** unchanged.
- **Wrong-key fail-closed** unchanged (per the same Phase 0d test, now with pre-derived K3 setup matching the parameter signature fix below).

**New properties this amendment adds.**

- **Rename-invariance for the data dir.** Same physical file at the SAME relative position under two different absolute base paths yields equal AAD. Pinned by `aad_is_equal_for_same_relative_path_under_different_bases` (unit-level, fast-fail) + `sealed_open_at_path_a_rename_to_b_open_at_b_succeeds` (Phase 0d-style integration). Migration's atomic dir-swap pattern works without re-sealing.
- **Defensive fallback for paths outside `base_path`.** `relative_path_str` returns the full location string (NOT panic, NOT empty). The fallback isn't rename-invariant for those paths, but Lance shouldn't request them. Pinned by `relative_path_str_falls_back_to_full_when_location_outside_base`.
- **Empty-base fallback.** If `vault_sealed_to_local_path` fails at `new_store` (defensive), `base_path` is empty and AAD computes over the full location. Pinned by `relative_path_str_with_empty_base_returns_full_location`.

**Cross-vault substitution defense — unchanged + reasoning documented.**

The relative-path AAD does not by itself prevent substituting a sealed file from vault A into vault B. That defense lives at the **per-vault key separation** layer: each Memory Vault install has its own keychain entry → distinct master_key → distinct K3 derived at-rest key → AEAD authentication fails when a file sealed under vault A's key is unsealed under vault B's key. This is a stronger layer than per-vault-AAD-binding could provide; per-vault key separation is enforced by the V0.2 keychain layer (ADR-040) and applies regardless of AAD content.

**Open question deferred to V0.3 sync amendment (NOT decided here).** If V0.3 cross-device sync ever introduces a shared at-rest key across devices (which would itself be a security regression vs the current K3-derived-from-local-master_key design), a per-vault UUID baked into AAD would become load-bearing for cross-device-substitution defense. Today it is redundant. The V0.3 sync ADR must explicitly re-evaluate this if the threat model changes; flagged here so future-Claude-or-Shahbaz cannot miss it.

**Verification.**

- 232/232 vault-storage lib tests pass (was 226 pre-amendment; +6 = 5 sealed_object_store unit tests + 1 vector_store rename-invariance integration).
- 16/16 migration tests pass (was 7 detector + 6 ignored scaffolding at scaffolding milestone; ignore→live transitions + 3 cookie-recovery tests added).
- `cargo build --workspace --all-targets` zero warnings.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all --check` clean.

**No backwards compatibility break.** No sealed user data exists outside the spike + Phase 0d test fixtures (V0.2 not shipped). All sealed data created from this commit forward uses relative-path AAD. Phase 0d test fixtures are recreated each test run; no on-disk continuity to preserve.

**Cross-links.** ADR-008 amendment v1 (above) — V1 path semantics superseded by this amendment for AAD computation only; all other locks stand. ADR-040 amendment (signature fix below; K3 derivation site canonicalised at vault-app::keychain). Phase 2 plan iteration 2 §2 calibration C (locked rename ordering — works because relative paths inside the dir are stable across rename of the dir itself). `feedback_runtime_confirmation_after_web_spike.md` (the migration test that surfaced this drift was a runtime confirmation of the seal+rename property, not a fresh spike — the spike-discipline catch worked one layer down). `feedback_source_read_call_graph_upstream_of_empirical.md` (root cause was source-read of `extract_path` + `aad_for_path` to confirm absolute-path drift before designing the fix; not empirical-only). Production module: `crates/vault-storage/src/sealed_object_store.rs`.

---

## ADR-040 amendment — Signature fix: open_with_at_rest_key takes already-derived at_rest_key (T0.2.0 Phase 2, 2026-05-11)

**Status:** ACCEPTED, in force from this commit. Strengthens ADR-040 amendment ("at_rest_key flows from keychain through AppConfig to migration consumer") with an explicit single-canonical-K3-site invariant.

**Context.** Phase 0d (2026-05-08) wired `LanceVectorStore::open_with_at_rest_key(data_dir, dim, master_key)` — the parameter was named `master_key` and the function derived K3 internally via vault-storage's local `derive_at_rest_key`. Phase 1 (2026-05-10, ADR-040) added the keychain layer that performs K3 derivation in vault-app::keychain and stores the already-derived `at_rest_key` in `AppConfig.at_rest_key`. The two layers were latently inconsistent: Phase 1 expected to forward `&config.at_rest_key` to a constructor that takes the at-rest key directly, but Phase 0d's signature would re-derive yielding `K3(K3(master_key))` — silently different keying material than anything else in the system, sealed data unreadable by any clean code path.

**Discovery.** Surfaced 2026-05-11 at the start of Phase 2 implementation when the migration loop wiring would have been the first production caller. Caught BEFORE wiring, not after — the spike-before-lock instinct working without a formal spike scheduled. Source-read-the-call-graph before empirical investigation surfaced the silent-bug class at exactly the right moment.

**Decision.**

- **`LanceVectorStore::open_with_at_rest_key`** parameter rename `master_key: &[u8; 32]` → `at_rest_key: &[u8; 32]`. Internal `derive_at_rest_key(master_key)` call removed. Function consumes the already-derived 32-byte at-rest key directly. Doc comment names the K3-once invariant explicitly.
- **`vault_storage::sealed_object_store::derive_at_rest_key`** deleted (was the duplicate K3 site that the internal derivation called). The `pub use` re-export at `vault-storage/src/lib.rs` removed. Module-doc updated to clarify K3 derivation lives at `vault_app::keychain::derive_at_rest_key` per ADR-040.
- **Single canonical K3 site invariant:** `vault_app::keychain::derive_at_rest_key` is the lone production K3 call site in the workspace after this commit. Verified by grep at fix time — no other production caller exists. Tests may inline `blake3::derive_key("vault memory at-rest sealing v1", ...)` for fixture setup matching the production caller flow; the canonical-site discipline applies to production code only.
- **`StorageBackend::open_with_at_rest_key`** sealed companion to `StorageBackend::open` mirrors the new signature (`at_rest_key: &[u8; 32]`).

**Phase 0d test setup updates.** All 5 Phase 0d tests pre-derive K3 at fixture setup before passing into the sealed constructor — modeling the production caller flow. Mechanical change; no test count delta. Workspace pin of the K3 context string lives in `vault_app::keychain::tests::derive_at_rest_key_is_deterministic_and_uses_k3_kdf_context`; vault-storage tests inline the magic string with comment cross-link.

**Verification.**

- 5 Phase 0d sealed_open_* tests pass against the new signature (pre-derive K3 at fixture setup).
- All workspace DoD gates green (see ADR-008 amendment v2 verification).

**Cross-links.** ADR-040 (V0.2 Phase 1 keychain layer; `at_rest_key` flows from keychain through AppConfig). ADR-008 amendment v1 (K3 KDF lock; this amendment doesn't change the K3 contract, only its caller responsibility). ADR-008 amendment v2 (above; AAD path semantics — same Phase 2 commit, same canonical-derivation-then-sealing flow). `feedback_source_read_call_graph_upstream_of_empirical.md` (catch was source-read-first, not empirical-only). Production sites: `crates/vault-storage/src/vector_store.rs::open_with_at_rest_key`, `crates/vault-storage/src/cascading.rs::open_with_at_rest_key`, `crates/vault-app/src/keychain.rs::derive_at_rest_key`.

---

## ADR-041 — V0.1 VAULT_KEY → V0.2 keychain SQLCipher passphrase bridge — plan iteration 2 LOCKED (2026-05-11)

**Status:** PROPOSED, plan iteration 2 LOCKED post-Shahbaz-review (2026-05-11). Implementation gates on Phase 2 fmt-fix CI green (run `25660977905`) → spike Stages A + B (compile-and-run) → if pass, ADR-041 implementation milestone (+8 tests + ADR-041 final text + commit + push + CI watch). Iteration 3 RESERVED for spike findings only.

### 1. Iteration 1 answers (LOCKED)

- **Spike methodology:** compile-and-run, **Stages A + B only**. Stage C (kill-mid-rekey atomicity probe) DROPPED — not deterministic enough to be load-bearing; assume non-atomic and design around with snapshot (§3 below).
- **Failure mode #4 → cross-store snapshot-commit invariant:** keychain write FIRST, then pre-rekey snapshot, then PRAGMA rekey, then verify, then snapshot cleanup. Detailed sequence in §3.
- **Bridge location:** option (c) sibling fn `bridge_or_init_master_key(data_dir, namespace, vault_id)` in `vault-app/src/keychain.rs`. Composes existing `read_or_init_master_key`. vault-tauri main.rs changes one call site.

### 2. Cross-store commit-or-rollback without native 2PC — LOCKED workspace invariant

Future cross-store work cross-links this section rather than re-deriving.

**Core property — system state at every step.** The system is in exactly one of:
- **(a)** Original state intact + co-store writes rolled back (or never started).
- **(b)** Snapshot or backup exists + state can be rolled forward (commit) or restored (abort).
- **(c)** Commit complete + original state retired.

The system is never simultaneously in `¬a ∧ ¬b ∧ ¬c` (no half-committed unrecoverable window).

**Two implementation patterns derived from store capabilities:**

**Pattern A — atomic-rename with implicit backup.** Used when the store supports atomic file/dir-level moves (POSIX/NTFS rename). The destructive op IS an atomic swap; old state becomes implicit backup as a side effect of the rename pair. **Canonical instance:** Phase 2's `vector_dir → backup_dir` followed by `temp_dir → vector_dir` (per "T0.2.0 Phase 2 — plan iteration 2" §2 calibration C). The cookie file (`vector_dir.with_extension("vault_migration_in_progress")`) is a SEPARATE role — an in-flight marker that tracks which crash-recovery state-machine branch applies on next launch — NOT a data snapshot. Don't conflate the marker file with the implicit backup; they serve different purposes.

**Pattern B — explicit pre-copy snapshot.** Used when the destructive op modifies state in place (no atomic file-level swap available at the destructive layer). Pre-copy snapshot taken before destructive op; cleanup only on full success; rollback restores from snapshot. **Canonical instance:** ADR-041's `vault.db.pre_v0_2_bridge` snapshot before `PRAGMA rekey`. SQLCipher rekey rewrites in place — no atomic-swap primitive available at the SQLite layer, so explicit snapshot is required to satisfy the invariant.

**Shared discipline (both patterns):**
1. Cheapest-to-rollback co-store write goes FIRST. (Pattern A / Phase 2: cookie file write before rename pair. Pattern B / ADR-041: keychain entry write before snapshot+rekey.)
2. Destructive op runs only after the co-store write succeeds.
3. Verification of post-destructive state gates cleanup.
4. Cleanup only on full success; failure rolls back via the pattern's backup mechanism.

**Forward-reference discipline:** future cross-store work picks Pattern A when atomic-swap is available at the destructive layer, Pattern B otherwise. Confusing the two propagates wrong mental model across consumers (V0.2.x sync touching SQLCipher + LanceDB; V0.3 consolidator touching DuckDB + SQLCipher; T0.2.7+ retrieval-index work spanning stores).

### 3. Bridge sequence v2 — LOCKED

```text
Inputs: data_dir (Path), namespace (str), vault_id (str)
Triggers when ALL three: keychain entry absent, vault.db exists, VAULT_KEY env set

1. Open V0.1 vault.db with VAULT_KEY as passphrase. Schema-query verify
   (SELECT name FROM sqlite_master LIMIT 1).
   FAIL: dialog "VAULT_KEY env var does not unlock the V0.1 vault."
2. Generate new master_key via getrandom.
3. Derive new SQLCipher passphrase = hex(BLAKE3("vault sqlcipher passphrase v1", new_master_key)).
4. WRITE NEW MASTER_KEY TO KEYCHAIN (cheapest-to-rollback co-store write per §2 invariant).
   FAIL: dialog "Keychain write failed: <err>. No vault state modified; retry."
         Bridge exits cleanly; user retries after fixing keychain.
5. SNAPSHOT vault.db (Pattern B explicit pre-copy per §2). Snapshot at
   vault.db.pre_v0_2_bridge alongside vault.db.
   FAIL: dialog "Snapshot write failed: <err>." Bridge MUST also delete
         keychain entry before exiting (rollback step 4).
6. PRAGMA rekey '<new_passphrase>' on vault.db (destructive op per §2).
   FAIL: restore from snapshot, delete keychain entry, dialog with diagnostic.
7. Close + reopen vault.db with new passphrase. Schema-query verify
   (verification gate per §2).
   FAIL: restore from snapshot, delete keychain entry, dialog with diagnostic.
8. Delete snapshot file (success-only cleanup per §2).
9. Emit one-time INFO: "V0.1 → V0.2 SQLCipher passphrase bridge complete;
   VAULT_KEY env var no longer required."
10. Return new master_key.
```

**Property pinned:** at every step the system satisfies §2's core property — never `¬a ∧ ¬b ∧ ¬c`. Steps 1-4: state (a). Step 5 onward: state (b) until step 8; state (c) after.

### 4. Schema migration concern — PINNED no-op for V0.2.0

`MIGRATIONS` slice in `crates/vault-storage/src/migrations/mod.rs:28-39` contains exactly 2 entries (`0001_initial` T0.1.3-era + `0002_cascade_infra` T0.1.6-era), both shared between V0.1 + V0.2. Phase 2 (V0.2.0) did NOT add new SQLCipher schema migrations. `MetadataStore::open` runs `migrations::run` on every open with idempotent `CREATE TABLE IF NOT EXISTS` discipline — any future V0.2.x migration (e.g., the tech-debt `pending_sync` 0003 extension) rides through transparently AFTER the bridge has rekeyed the file. No bridge-side schema work required in V0.2.0.

**Forward-pointer pin:** when V0.2.x first adds a `0003_*.sql`, cross-link this section to confirm bridge + migrations composition still holds (it should — migrations run AFTER bridge completes, on the rekeyed file, no interaction).

### 5. Tier 2 fixture VERIFIED + WAL-replay sub-question pinned

Per `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md`:
- `vault.db` (98 KB) — present
- `vault.db-wal` (650 KB) — present (SQLite replays on open; bridge handles transparently via rusqlite + SQLCipher)
- Capture key: `VAULT_KEY = fixture-capture-key-do-not-use-in-prod`
- Capture commit: `1d72aac`, MSI SHA-256 `03d127371f6a881366e2f048d81f2785de97f68236c5d52747bf0100284d0a06`

**WAL-replay sub-question pinned for Tier 2 test:** the WAL is from a live V0.1 binary that didn't checkpoint before snapshot capture. Bridge rides through WAL-replay-on-open transparently (rusqlite + SQLCipher handles this); Tier 2 test asserts on POST-bridge content equality, so any WAL-replay quirk surfaces as test failure rather than silent data loss. **Optional Tier 1 +1 floor amendment** (only if the implementation surfaces WAL-replay sensitivity): create a synthetic fixture with uncommitted WAL on purpose; assert bridge handles it. Surface as floor breach if it materializes.

### 6. Test count floor pre-declaration — +8 vault-app tests

Per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`. Distribution + named tests:

**Tier 1 (vault-app/src/keychain.rs `mod tests` — synthetic SQLCipher fixtures, 7 tests):**
1. `bridge_rekeys_fresh_sqlcipher_file_and_preserves_rows` — happy path
2. `bridge_fails_closed_when_vault_key_env_var_is_wrong` — wrong passphrase
3. `bridge_fails_closed_when_vault_key_env_var_is_unset` — env var missing dialog
4. `bridge_no_op_when_keychain_entry_already_exists` — V0.2 second-launch path
5. `bridge_no_op_when_no_v0_1_sqlcipher_file_present` — fresh V0.2 install
6. `bridge_writes_keychain_before_rekey_ordering_invariant` — confirms keychain-write-first per §3 step 4-before-5 ordering (pinned by step ordering observation; spy/log/timestamp instrumentation TBD at impl time)
7. `bridge_restores_from_snapshot_on_rekey_failure` — fault-injection via methodology (c) below; restores snapshot + rolls back keychain on rekey failure

**Tier 2 (vault-app/tests/integration_smoke.rs OR new dedicated test file — captured V0.1 fixture, 1 test):**
8. `tier_2_real_v0_1_vault_db_bridges_and_preserves_5_rows` — uses fixture vault.db, asserts 5 known memories survive bridge

**Tier 3: manual founder smoke** (zero CI test count). Snapshot-then-launch-then-verify, same procedure as Phase 2's Tier 3 (now unblocked).

**Floor breach surface discipline:** any deviation from +8 surfaces in commit message per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`.

**Methodology declaration for test 7** (`bridge_restores_from_snapshot_on_rekey_failure`) — pre-declared per spike-discipline reflex applied to test infrastructure:

**Methodology choice: (c) filesystem permission fault-injection** (preferred), with (a) corrupted-fixture as fallback.

- **(c) chosen because:** no production seam grown for testing (option b's cost), no fixture-manufacturing-fragility (option a's risk), real rekey call exercised end-to-end. Implementation: open vault.db, run PRAGMA key + verify-readable, set file read-only via `Permissions::set_readonly(true)` (cross-platform: Unix clears write bits, Windows sets `FILE_ATTRIBUTE_READONLY`), call bridge → expect rekey failure → assert snapshot restored + keychain entry deleted.
- **Spike acceptance gains a probe** to confirm (c) viability: Stage A or B's setup includes "PRAGMA rekey against a read-only file fails cleanly" check. If probe shows silent-success-in-memory (rekey returns Ok but doesn't write), fall back to (a).
- **(a) corrupted-fixture fallback** (only if (c) probe fails): manufacture vault.db with truncated header or page-checksum corruption that PRAGMA rekey reliably rejects. Fixture path TBD if used.
- **(b) mock seam REJECTED** because adding a production seam that exists only for testing violates "no over-engineering" + "test the real thing" disciplines.

### 7. Iteration 3 RESERVED — fires only on spike findings

Iteration 3 fires only if:
- Stage A fails → bridge mechanism changes from PRAGMA rekey to manual `ATTACH DATABASE 'new.db' KEY 'K2'; INSERT INTO new SELECT * FROM main` (slower, more code, different fault modes).
- Stage B fails → wrong-key-after-rekey leaks → ADR-041 stops; SQLCipher passphrase bridging is not viable on this dep chain.
- Spike's read-only probe shows silent-in-memory rekey → §6 test 7 methodology flips from (c) to (a); minor scope shift, not iteration-3-worthy unless additional fault-injection design questions surface.

### 8. Cross-references

- HANDOFF.md "Open tech-debt" entry (V0.1 VAULT_KEY → V0.2 keychain bridge) — discovery context
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2" §2 calibration C (Phase 2's atomic dir-swap = canonical Pattern A instance per §2 above)
- `crates/vault-storage/src/migrations/mod.rs:28-39` — schema migrations slice (no-op evidence for §4)
- `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md` — Tier 2 fixture provenance + vault.db inventory + WAL note
- ADR-008 + amendments v1 + v2 (sealing layer; orthogonal to this bridge but established the snapshot-commit pattern)
- ADR-040 + amendments (keychain layer composition; this bridge extends `read_or_init_master_key`)
- `feedback_spike_methodology_explicit.md` — spike methodology declared (compile-and-run, Stages A+B + read-only probe)
- `feedback_floor_forecast_is_pre_declaration_not_estimate.md` — +8 test floor pre-declaration
- `feedback_runtime_confirmation_after_web_spike.md` — N/A (no web-research methodology here; all empirical)

### 9. Spike findings (2026-05-11) — iteration 2.1 amendment

Spike `crates/vault-storage/examples/sqlcipher_rekey_spike.rs` ran 2026-05-11 post-iteration-2-lock. Results:

| Stage | Outcome |
|---|---|
| Stage A — basic rekey + reopen with new key | ✅ PASS |
| Stage B — reopen with WRONG key fails closed | ✅ PASS |
| Read-only-file probe (§6 methodology check) | ❌ FAIL — Outcome 3 |

**Stages A + B PASS:** PRAGMA rekey is viable on the rusqlite + bundled-sqlcipher-vendored-openssl chain (ADR-006). Bridge mechanism in §3 stands as drafted. ADR-041 implementation proceeds.

**Read-only-file probe FAIL — Outcome 3 (silent-in-memory rekey):** SQLCipher's `PRAGMA rekey` on a read-only file returns Ok but the changes only live in the connection's memory; on close, the disk file remains unchanged (subsequent reopen with the new key fails because the file is still encrypted with the old key). This is documented SQLite/SQLCipher behavior — `PRAGMA rekey` doesn't perform an explicit write-permission check up front; it streams pages through the new cipher state and writes them lazily, so a write rejection happens silently mid-stream rather than at the PRAGMA call.

**Methodology flip per iteration 2 §7 pre-declaration:** test 7 (`bridge_restores_from_snapshot_on_rekey_failure`) methodology changes from **(c) filesystem permission fault-injection** to **(a) corrupted-fixture**. This was pre-anticipated and pre-approved at iteration 2 lock; no iteration 3 needed. Test 7 implementation (when ADR-041 impl lands) will:

1. Manufacture a `vault.db` fixture with a corruption shape that PRAGMA rekey reliably rejects (candidates: truncated header below the 16-byte SQLite header, page-checksum mismatch on a non-header page). Exact shape to be determined empirically at impl time — adopt whichever shape produces clean Err return from `PRAGMA rekey` on the rusqlite + bundled-sqlcipher chain.
2. Fixture lives at `crates/vault-app/tests/fixtures/sqlcipher_corrupted_for_rekey_failure.db` (path locked here for grep-discoverability).
3. Test asserts: bridge calls PRAGMA rekey → Err → snapshot restored + keychain entry deleted (rollback per §3 step 6 fail path).

**Spike does NOT ride into production.** The example file remains as runtime-confirmation evidence per `feedback_spike_playbook_for_unknowns.md` ("keep spike as executable documentation"). No spike-specific dep promotions needed (rusqlite + tempfile already in vault-storage main + dev deps).

**No iteration 3 needed** — spike findings match the iteration 2 §7 pre-declared "minor scope shift" branch exactly. Implementation kickoff proceeds.

### 10. Pin 1 (post-spike) — Post-write verification invariant (LOCKED, lands in ADR-041 final text)

The Read-only-file probe finding generalizes beyond test methodology to a **production-relevant property**. SQLCipher's `PRAGMA rekey` can return Ok with the changes living only in the connection's memory, not persisted to disk — and the same shape applies to any underlying-primitive write fault the OS surfaces as a no-op-success: read-only file (probed empirically), parent dir read-only / disk-full / antivirus-locked / ACL-denied / network-filesystem-flush quirks (class generalization). This validates that §3 step 7 (close + reopen + schema-query verify) is **load-bearing, not belt-and-suspenders**. Without it, a silent-failure-mode rekey would leave the keychain entry referencing a K2-encrypted file that's actually still K1 on disk, with no clean recovery on next launch.

**To pin in ADR-041 final text under named invariant:**

> **Post-write verification invariant.** Destructive cross-store operations MUST verify persistence via close + reopen + read-back rather than trusting the primitive's success return value. SQLCipher's `PRAGMA rekey` is the load-bearing example (spike Read-only probe 2026-05-11: rekey returns Ok on read-only files with changes in-memory only). The pattern generalizes — any underlying primitive may report success for an operation that didn't persist (disk-full, antivirus locks, ACL changes, network filesystem flush quirks). The verify step is what makes the snapshot-commit invariant in §2 actually safe; remove it and the whole invariant collapses to "we hope the primitive told us the truth."

**Cross-link from §2 Pattern A** (Phase 2): `validate_readable()` post-rename is the analogous verification step for the LanceDB store. Same invariant, two stores. Phase 2 commit `739e8da` got this right (see `crates/vault-storage/src/cascading.rs::assemble`); the rationale was "ADR-018 corruption-detection" but post-spike the deeper rationale is now named: post-write verification against silent-primitive-success is the load-bearing safety property, NOT just corruption-detection-on-open. Pattern A's verification step is identical in role to Pattern B's; future cross-store work consumes both as one invariant.

### 11. Pin 2 (post-spike) — Test 7 corruption-mode selection criteria + iteration-3 watch-trigger

Methodology flip (c) → (a) corrupted-fixture is locked per §9. The Read-only finding sharpens the prior caveat ("rekey might silently succeed on partially-corrupted files") from theoretical to empirically-evidenced concern. Some corruption classes will fail at initial open (truncated file, wrong magic bytes) BEFORE rekey runs; others might silent-succeed at rekey just like the read-only case did. Test 7 needs to find a corruption mode satisfying ALL THREE criteria:

1. **MUST allow vault.db to open successfully with K1** — bridge needs to get past §3 step 1 (`PRAGMA key` + schema-query verify) before reaching step 6 (rekey).
2. **MUST cause rekey to fail observably (Err return)** — not silent-success of any flavor (in-memory-only, disk-write-rejected-but-Ok-returned, etc.).
3. **MUST be cross-platform reproducible** — test runs on `[ubuntu-latest, windows-latest, macos-latest]` matrix.

Likely candidates worth probing during test 7 implementation:

- **Candidate (i) — parent directory read-only mid-bridge.** Make `<data_dir>/` read-only between bridge steps 1 and 6. SQLCipher rekey creates temp/journal files in the parent dir during page-by-page rewrite; read-only parent might surface as Err where vault-db-itself-read-only didn't. Cross-platform: `Permissions::set_readonly(true)` on the parent dir works on Unix (clears write bits) and Windows (sets `FILE_ATTRIBUTE_READONLY`).
- **Candidate (ii) — malformed page injection.** Modify a non-header page byte (e.g., page 4 byte offset 100) to trigger HMAC failure during rekey's page-by-page rewrite. Schema query (page 1, sqlite_master root) succeeds; page-N rewrite fails. Requires manufacturing the fixture once and checking it in at `crates/vault-app/tests/fixtures/sqlcipher_corrupted_for_rekey_failure.db` (path locked per §9).

**Watch-trigger for iteration 3:** if test 7 implementation cannot find a corruption mode satisfying all three criteria within ~1 hour of probing, **STOP and surface** — fault-injection design becomes a real ADR-041 scope question and iteration 3 fires for methodology re-evaluation. Don't grind on it for half a day silently. Probe candidate (i) first (cheaper — no fixture manufacturing); fall to (ii) if (i) doesn't satisfy criterion 2; surface if neither works.

---

## ADR-041 — V0.1 VAULT_KEY → V0.2 keychain SQLCipher passphrase bridge — final ADR (LOCKED, 2026-05-11)

**Status:** ACCEPTED, in force from this commit. Implementation lands at `crates/vault-app/src/keychain.rs::bridge_or_init_master_key` (plus the rekey + verify primitives at `crates/vault-storage/src/metadata_store.rs::{rekey_in_place, verify_sqlcipher_passphrase}`). Replaces ADR-032 (V0.1 VAULT_KEY-as-SQLCipher-passphrase) for V0.1 → V0.2 upgrade paths; ADR-040 keychain-as-master_key-source unchanged.

### Context

V0.1 sourced the SQLCipher passphrase directly from the `VAULT_KEY` environment variable (ADR-032). V0.2 Phase 1 (ADR-040) replaced that with an OS-keychain-derived passphrase: `hex(BLAKE3("vault sqlcipher passphrase v1", &master_key))`, where `master_key` is read from the OS keychain on each launch. Phase 1's `read_or_init_master_key` generated a NEW random master_key on first launch when no keychain entry existed; it did NOT bridge from V0.1's VAULT_KEY-derived passphrase. Discovered during Phase 2 Tier 3 founder-smoke preparation (2026-05-11): real V0.1 vaults would succeed at the Phase 2 LanceDB migration (plaintext doesn't need a key) but fail at `Application::new`'s `MetadataStore::open` because the new keychain-derived SQLCipher passphrase doesn't match V0.1's VAULT_KEY-derived passphrase.

### Decision

Add a `bridge_or_init_master_key(data_dir, namespace, vault_id, v0_1_vault_key)` composition fn alongside `read_or_init_master_key`. Three branches based on detected state:

1. **Keychain entry present** → return existing master_key (V0.2 second-launch path; identical to `read_or_init`).
2. **Keychain absent + no V0.1 vault.db** → delegate to `read_or_init_master_key` first-run path (fresh V0.2 install).
3. **Keychain absent + V0.1 vault.db present** → V0.1 bridge sequence (per "Bridge sequence" below).

Branch 3 fail-closes if `v0_1_vault_key` is `None` or empty (VAULT_KEY env var unset) — vault-tauri main.rs sources VAULT_KEY env once and passes it through (avoids unsafe `std::env::set_var` test contamination per ADR-002 `#![forbid(unsafe_code)]`).

### Bridge sequence — locked

Sequence + numbering verbatim from iteration 2 §3, applied in `run_v0_1_bridge` private fn:

1. Verify V0.1 passphrase unlocks vault.db (`vault_storage::verify_sqlcipher_passphrase`). Fail-fast at step 1 prevents writing an orphan keychain entry pointing to a master_key for a vault that can't be rekeyed.
2. Generate new master_key (`getrandom`).
3. Derive new SQLCipher passphrase = `hex(BLAKE3("vault sqlcipher passphrase v1", &new_master_key))`.
4. Write new master_key to OS keychain — cheapest-to-rollback co-store write FIRST per the cross-store snapshot-commit invariant (Pattern B).
5. Snapshot vault.db at `vault.db.pre_v0_2_bridge` (explicit pre-copy per Pattern B).
6. PRAGMA rekey to new passphrase (`vault_storage::rekey_in_place`).
7. Post-write verification — close + reopen + schema-query verify with new passphrase (embedded in `rekey_in_place`'s step 4 per the post-write verification invariant below).
8. Cleanup snapshot file (success-only).
9. Emit one-time INFO log: `"V0.1 → V0.2 SQLCipher passphrase bridge complete; VAULT_KEY env var no longer required."`
10. Return new master_key.

Failure at any step rolls back via the snapshot-commit invariant: step 5+ failures restore vault.db from snapshot AND delete the keychain entry; steps 1-4 failures don't need rollback because no destructive op has run yet.

### Post-write verification invariant — LOCKED, named, load-bearing

**Destructive cross-store operations MUST verify persistence via close + reopen + read-back rather than trusting the primitive's success return value.** SQLCipher's `PRAGMA rekey` is the load-bearing example (spike Read-only probe 2026-05-11: rekey returns Ok on read-only files with changes in-memory only). The pattern generalizes — any underlying primitive may report success for an operation that didn't persist (disk-full, antivirus locks, ACL changes, network filesystem flush quirks). The verify step is what makes the cross-store snapshot-commit invariant in iteration 2 §2 actually safe; remove it and the whole invariant collapses to "we hope the primitive told us the truth."

Implementation: `vault_storage::rekey_in_place` embeds the verify step (close + reopen + schema-query) internally; callers (the bridge) don't need to remember to add it. Phase 2's `validate_readable()` post-rename in `crates/vault-storage/src/cascading.rs::assemble` is the analogous verification step at the LanceDB store layer — same invariant, two stores, different primitives.

### Cross-store snapshot-commit invariant — Pattern B canonical instance

Per iteration 2 §2 (LOCKED workspace invariant). ADR-041 is the canonical Pattern B implementation (explicit pre-copy snapshot before destructive op). Phase 2's atomic dir-swap is the canonical Pattern A (atomic-rename with implicit backup). Both share: cheapest-to-rollback co-store write FIRST → destructive op → verification → cleanup-or-rollback. Future cross-store work cross-links iteration 2 §2 + this section to consume the canonical patterns rather than re-deriving.

### Per-vault UUID in AAD — deferred to V0.3 sync ADR

Documented in ADR-008 amendment v2 (Phase 2). Today: per-vault key separation (each Memory Vault install has its own keychain entry → distinct master_key → distinct K3 derived at-rest key → AEAD authentication fails on cross-vault file substitution regardless of AAD content). Cross-vault UUID binding becomes load-bearing if V0.3 sync ever introduces a shared at-rest key across devices. The V0.3 sync ADR MUST re-evaluate; flagged in ADR-008 amendment v2 §"Open question deferred to V0.3 sync amendment."

### Verification

- 8 tests pass on Windows runner: 7 Tier 1 (`crates/vault-app/src/keychain.rs` mod tests) + 1 Tier 2 (real V0.1 fixture from commit `1d72aac`, also in `crates/vault-app/src/keychain.rs` mod tests).
- Spike `crates/vault-storage/examples/sqlcipher_rekey_spike.rs` Stages A + B PASS (compile-and-run runtime evidence the primitive works on this dep chain).
- Test 7 corruption-mode methodology: **(a) corrupted-fixture, candidate (ii) malformed-page injection at byte offset 6000** — locks per the pin 2 watch-trigger discipline. The implementation uses an in-test corruption (mutate one byte at offset 6000 of a freshly-created multi-page V0.1 SQLCipher fixture) which reliably triggers PRAGMA rekey Err on the rusqlite + bundled-sqlcipher chain. No checked-in static corrupted-fixture file required.
- Test concurrency mutex: `KEYCHAIN_TEST_MUTEX` static in `mod tests` serializes the bridge tests' use of `keyring_core::set_default_store` / `unset_default_store` global state. Discovered during initial test run: parallel tests race on the global default-store slot, producing "No default store has been set" rollback failures. Production has no contention (single startup call); tests need the mutex.

### When to revisit

- **macOS / Linux keychain support lands at T0.2.0.x sub-task or T0.2.14** — bridge `#[cfg(windows)]` opens up; macOS-Keychain + Linux-Secret-Service backends consume the same bridge logic.
- **V0.2.x first SQLCipher schema migration** (`0003_*.sql`) — confirm bridge + migrations composition still holds (it should — migrations run AFTER bridge on the rekeyed file; idempotent CREATE TABLE IF NOT EXISTS discipline applies). Cross-link iteration 2 §4 forward-pointer.
- **VAULT_KEY env-var support removal** — once founder dogfood vault is bridged + alpha cohort has only-ever-used V0.2, can retire VAULT_KEY support entirely (delete branch 3 + the VAULT_KEY env-var read at vault-tauri main.rs). Defer to V1.0.

### Cross-references

- HANDOFF.md "ADR-041 plan iteration 2 LOCKED" (above) — full plan iteration history + design questions resolved
- HANDOFF.md "ADR-008 amendment v2" (Phase 2) — AAD path semantics; cross-store key separation defense for cross-vault substitution
- HANDOFF.md "ADR-040 amendment — Signature fix" (Phase 2) — single-canonical-K3-derivation site invariant; bridge consumes this discipline
- ADR-032 (V0.1 VAULT_KEY env-var path) — bridge retires this for V0.1 → V0.2 upgrade paths
- ADR-040 (Phase 1 keychain layer) — bridge composes `read_or_init_master_key`
- `crates/vault-storage/examples/sqlcipher_rekey_spike.rs` — compile-and-run runtime evidence
- `crates/vault-storage/src/metadata_store.rs::{verify_sqlcipher_passphrase, rekey_in_place}` — primitives
- `crates/vault-app/src/keychain.rs::bridge_or_init_master_key` — composition fn
- `crates/vault-tauri/src/main.rs` step 4 — production call site
- `feedback_runtime_confirmation_after_web_spike.md` — spike methodology discipline (compile-and-run)
- `feedback_floor_forecast_is_pre_declaration_not_estimate.md` — +8 test floor pre-declared, hit exactly (7 Tier 1 + 1 Tier 2)

---

## T0.2.0 close-out plan iteration 1 (drafted 2026-05-09, Phase 0e)

Phase 0 (a / a-fix / b / c / d / e) was the spike-then-foundation block: lancedb upgrade, ADR-038/-039 fixes, sealing primitive runtime-confirmed in spike v2, production-wired in Phase 0d, contract-locked in Phase 0e ADRs (037 + 008 amendment). T0.2.0 proper now consumes those contracts to reach **BRD §6 T0.2.0 acceptance** (5 criteria) and **ADR-010 hard-gate clearance**. Five close-out phases below; two open questions punted to iteration 2.

**Phase 1 — At-rest-key provenance: OS keychain (per ADR-032 amendment trigger).** ADR-032 line 289 ("V0.2 alpha-distribution must migrate to OS keychain — keyring-core ecosystem mid-migration at V0.1; revisit at V0.2 plan time + spike picks branch (D) keychain") is the trigger. **Methodology: spike-before-lock** per standing rule — 1-2 hour compile-and-run spike to pick the keychain crate (`keyring` 3.x vs `keyring-core`-derived; cross-platform-coverage check), runtime-confirm `set → get → wrong-account-fails-closed → process-restart-survives → Credential Manager UI shows entry` on Windows. Spike artifact kept under `crates/vault-app/examples/keychain_spike.rs` as executable documentation. **ADR-040 drafted at Phase 1 close** (post-spike, decision made from runtime evidence per spike-before-lock standing rule) with crate selection + key-storage policy (namespace, account, secret-shape). Production wiring at `Application::start_with_mcp`: read master_key from keychain on startup; on first run (no entry), generate new master_key + persist + use. **VAULT_KEY env var compensating control retired** — removed from `.env.example`, `setup-dev-env.{sh,ps1}`, CI workflow. Test: keychain round-trip Windows (≥1 regression pin); wrong-account-fails-closed pin.

**Phase 2 — V0.1 plaintext data migration (founder-only one-shot, per BRD §6 T0.2.0 line 1415).** One-time-on-first-launch detection: at `Application::start` startup, check if data dir has BOTH plaintext-shaped Parquet (PAR1 magic on root files) AND `ALPHA_DO_NOT_STORE_REAL_DATA.txt` present → V0.1 alpha shape detected. Migration path: open V0.1 dir via the still-present plaintext `LanceVectorStore::open` (not yet deleted — Phase 3 deletes it immediately after this lands), read all rows, re-write through `open_with_at_rest_key` with master_key from keychain (Phase 1 must land first), delete V0.1 plaintext data-dir contents post-migration, write distinguishing one-time INFO log. Test: synthetic V0.1-shape data dir → migration → assert post-state has framing-bytes `0x01 0x00` on every file + zero PAR1 magic + same row count + content-equality on every row. **Phase 2 is the LAST CALLER of plaintext `open()` in the codebase** — Phase 3 deletes the plaintext path immediately after.

**Phase 3 — ADR-010 compensating-controls removal + plaintext-`open()` deletion + BRD §6 acceptance-test suite.** Removes all four ADR-010 V0.1 compensating controls per ADR-010 removal-trigger clause: (1) modal first-run banner from vault-tauri, (2) persistent UI banner from vault-tauri, (3) WARN log at plaintext LanceDB open (the WARN site itself is removed alongside the plaintext path), (4) `ALPHA_DO_NOT_STORE_REAL_DATA.txt` file deleted on T0.2.0 first-run with one-time INFO log noting the upgrade. Plaintext `LanceVectorStore::open(path, dim)` deleted from `vector_store.rs` (Phase 2 was last caller). **BRD §6 T0.2.0 5-criterion acceptance suite** lands as named integration test in vault-app: (a) no-plaintext-on-disk after write/close (entropy ≥ 7.9 + zero PAR1 magic — extends Phase 0d's `sealed_open_writes_framing_bytes_to_disk` to top-level integration), (b) round-trip identity (encrypt → decrypt == original), (c) decryption with wrong key fails closed (extends Phase 0d's `sealed_open_with_wrong_key_fails_closed`), (d) all four ADR-010 controls absent from codebase (grep-test asserts the V0.1 strings are gone), (e) tampered-ciphertext detection — bit-flip a sealed byte, assert open returns Err with AEAD-auth message. **CI matrix runs the full suite on `[ubuntu-latest, windows-latest, macos-latest]`** per BRD §6 T0.2.0 acceptance #2 ("round-trip identity across Mac, Windows, Linux").

**Phase 4 — Founder-dogfood-on-sealed validation (Windows-only per ADR-029 V0.1 pattern).** **Pinned single-platform: Windows 11 dev box only.** ADR-029 V0.1 dogfood was Windows-only-with-amendment because that is the founder's actual hardware; T0.2.0 is the encryption-at-rest task, not the platform-expansion task. Mac CI coverage is already provided by Phase 3 acceptance suite running on `macos-latest` per BRD §6 T0.2.0 (b). Mac human-in-the-loop dogfood belongs at T0.2.16 (Beta Onboarding) or a dedicated Mac-procurement sub-task — not bundled into T0.2.0 close-out. **Test trajectory:** 6+ hours cumulative on Windows 11 dev box matching V0.1 dogfood pattern per HANDOFF.md retrospective line 111. ≥10 memories saved through the sealed path, ≥10 search queries return correct results, full-laptop-reboot persistence test, wrong-keychain-entry restart-fails-closed test (manual: temporarily corrupt the keychain entry, restart app, expect fail-closed startup; restore keychain entry afterward). Capture findings per **ADR-036 amended bar** (≥2 issues filed OR honest closure note). MSI artifact build at Phase 4 close — SHA-256 captured into HANDOFF.md V0.2 alpha-distribution forward-record.

**Phase 5 — T0.2.0 acceptance close + ADR-010 hard-gate clearance.** Final DoD pass (BRD §0.1 five conditions). HANDOFF.md updates: V0.2 hard-gate forward-pointer table flips ADR-010 from "HARD GATE before T0.2.0" → "HARD GATE CLEARED at T0.2.0 commit `<sha>`"; ADR-032 amendment trigger marked closed (keychain wired); active task flips to next T0.2.x per V0.2 task decomposition. **BRD §6.2 line 1411-1423 amendment** marks T0.2.0 DONE and lifts the T0.2.16 (Beta Onboarding) hard-block per ADR-010 contract.

**Acceptance-criteria mapping.** BRD §6 T0.2.0 (a) entropy + magic-bytes absence → Phase 3 acceptance suite (a) extending Phase 0d. (b) round-trip Mac/Windows/Linux → Phase 3 acceptance suite (b) on CI matrix. (c) wrong-key fails closed → Phase 3 acceptance suite (c) extending Phase 0d. (d) four V0.1 controls removed → Phase 3 acceptance suite (d) grep-test. (e) tampered ciphertext detected → Phase 3 acceptance suite (e) bit-flip test. ADR-010 hard-gate clearance → Phase 5.

**Open questions for iteration 2.**

1. **Cross-platform keychain coverage in chosen crate.** Phase 1 spike answers Windows. Mac (Keychain Services) and Linux (libsecret / DBus Secret Service) parity is open until the spike runs. If the chosen crate covers all three, no extra phases needed; if Windows-only, Mac/Linux keychain wiring becomes T0.2.0.x sub-task or T0.2.14 Stub-Installer-adjacent. Iteration 2 picks crate post-spike + decides scope.

2. **Migration-test fixture provenance.** Phase 2 test uses a "synthetic V0.1-shape data dir" — open question whether we manufacture it via test scaffolding (write Parquet + ALPHA file directly) or boot a full V0.1 binary one-shot to produce a real fixture. Scaffolding is cheaper but less realistic; binary-produced is the gold standard but adds a build step. Iteration 2 decides.

**Test-count floor forecast (per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`).** Phase 1: +3 (keychain round-trip, wrong-account, restart-survives). Phase 2: +2 (synthetic-V0.1-migration, alpha-marker-cleanup). Phase 3: +5 (BRD acceptance suite × 5 criteria). Phase 4: 0 (manual-test phase). Phase 5: 0 (admin close). **Aggregate floor: +10 tests across T0.2.0 close-out**, taking vault-storage from 226 → ≥236 (plus vault-app integration tests for Phases 1+2+3 acceptance suite at +5-7 in vault-app — total +15-17 across the workspace; vault-app figure firmer after iteration 2 spike informs Phase 1 shape).

**ADRs to draft during T0.2.0 close-out.** ADR-040 (Phase 1 close — keychain crate selection + key-storage policy). Possible amendment to ADR-032 depending on iteration-2 decision on question 1.

---

## T0.2.0 close-out plan iteration 2 (drafted 2026-05-09, Phase 1 kickoff)

**Iteration 1 stands.** Five-phase decomposition unchanged. Iteration 2 resolves one open question and explicitly defers the other to Phase 1 spike runtime evidence per spike-discipline standing rule.

**OQ #2 resolution — Migration-test fixture provenance (Phase 2 of close-out plan).**

Adopt **a three-tier fixture strategy**, not the single binary-vs-scaffolding choice iteration 1 framed:

1. **Tier 1 (CI regression, fast):** scaffolding-based migration test. Cargo-test setup writes a synthetic V0.1-shape data dir directly — Parquet files via `arrow_array::RecordBatch` + `parquet::arrow::ArrowWriter` exposed-as-fixture, plus the `ALPHA_DO_NOT_STORE_REAL_DATA.txt` marker file. Cheap, deterministic, runs on every commit on every CI matrix OS. Catches the regression class "migration logic mishandles a known V0.1 file shape." Lives in `crates/vault-storage/tests/migration_v0_1_to_sealed.rs`.

2. **Tier 2 (CI realism gate, checked-in):** snapshot a 5-memory synthetic V0.1 vault produced from a ONE-TIME run of the V0.1 binary at commit `1d72aac` (= `v0.1.0` tag, verified 2026-05-09; the V0.1 SHIPPED commit per HANDOFF.md V0.1 retrospective + the build whose Phase 5e MSI artifact SHA-256 was `03d12737...0a06`). Anonymized content (5 throwaway memories — no PII, no real data). Checked in under `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/` with a README capturing provenance + capture date + V0.1 commit SHA + V0.1 tag. Test runs migration on a copy of the fixture and asserts post-state (framing bytes / row count / content equality on every row). Catches "V0.1 actually wrote files we didn't anticipate in scaffolding" — the realism gap iteration 1 flagged. **Not a CI build step** (no V0.1 binary build in the matrix); just static data files.

3. **Tier 3 (one-shot manual smoke, before Phase 2 ships):** Shahbaz runs the migration on his actual V0.1 vault dir (the live dogfood vault from V0.1 alpha). Snapshot beforehand for safety. This is the gold-standard realism check on real-world data shape distribution. Not in CI; documented as a Phase 2 close-out checklist item; HANDOFF.md captures the smoke-test result + any findings.

**Why three tiers, not one:** iteration 1's binary-vs-scaffolding framing was a false binary. Tier 1 catches regression; Tier 2 catches V0.1-binary-output reality; Tier 3 catches Shahbaz's-actual-vault-distribution reality. Each tier has a distinct purpose and the cost of all three combined is small (Tier 1 is trivial Rust; Tier 2 is a 5-memory data-dir capture once; Tier 3 is Shahbaz manually running migration on his own vault). The scaffolding-only path leaves the realism gap; the binary-only path leaves the regression-on-every-commit gap; the three-tier composition closes both with bounded incremental cost.

**OQ #1 partially resolved by Phase 1 Leg 1 web research; final lock pending Leg 2 runtime evidence.**

Phase 1 Leg 1 web research (executed 2026-05-09) found the ecosystem migrated mid-V0.1-to-V0.2: `keyring` v3 era is end-of-line; v4 is sample-code-only with explicit "do not depend"; the active library is **`keyring-core` 1.0.0** (April 2026, `open-source-cooperative` org, maintained by Dan Brotsky), with credential stores split into per-platform crates. Cross-platform coverage IS available, but in a per-platform-crate pattern (not a single cross-platform crate). User code uses `#[cfg(target_os = "...")]` or a thin wrapper to pick the right store. Coverage map at May 2026:

| Platform | Crate | Version |
|---|---|---|
| Windows Credential Manager | `windows-native-keyring-store` | 1.0.0 (Apr 21 2026) |
| macOS Keychain Services | `apple-native-keyring-store` | 1.x line — has TWO modules: `Keychain` (unsigned-app compat, V0.1-era equivalent) + `Protected` (code-signed-only, biometric + iCloud-sync) |
| Linux Keyutils | `linux-keyutils-keyring-store` | 1.x line |
| Linux libsecret/DBus | `dbus-secret-service-keyring-store` | 1.0.0 (Apr 21 2026) — needs `crypto-rust` or `crypto-openssl` feature |

**Lock target for ADR-040 (pending Leg 2):** `keyring-core = "1"` + `windows-native-keyring-store = "1"` for V0.2 spike + Windows production wiring. macOS / Linux integration is per-platform-crate-add when those platforms enter founder-dogfood / CI matrix runtime confirmation. **Cross-platform path is unblocked**; OQ #1 closure is a per-platform-crate-add list, not an architecture redesign.

**Test-count floor forecast adjustment.** Iteration 1 forecast Phase 2 +2 tests. Three-tier fixture strategy adjusts to **Phase 2 +3 tests**: (a) Tier-1 scaffolding-migration round-trip, (b) Tier-2 fixture-replay round-trip, (c) Tier-2 ALPHA-marker-cleanup-after-migration. **Aggregate floor: +11 tests across T0.2.0 close-out** (was +10 at iteration 1), taking vault-storage from 226 → ≥237. vault-app integration tests for Phases 1+2+3 unchanged at +5-7. Total workspace +16-18 (was +15-17 at iteration 1; +1 from Phase 2 fixture tier addition).

---

## T0.2.0 close-out plan iteration 1.5 — Phase 1 scope correction (drafted 2026-05-09, post-source-read)

**Trigger.** Phase 1 production-wiring source-read of `crates/vault-app/src/application.rs` + `crates/vault-app/src/config.rs` + the V0.1 `vault-tauri` entry point surfaced three contract-class divergences from iteration 1's "Phase 1 — At-rest-key provenance: OS keychain" framing. Per `feedback_flag_review_as_plan_amendment.md`, these are plan amendments — flagged before any code landed rather than papered over with implementation choice. Iteration-1.5 captures the corrections; iteration 1's five-phase decomposition + iteration 2's three-tier fixture strategy remain intact above.

**Discovery 1 — `Application::start_with_mcp` is the WRONG location for keychain reading.**

`Application::new(config: &AppConfig)` consumes `config.key: SqlCipherKey` at construction time, cloning it into three `MetadataStore::open` / `StorageBackend::open` calls. By the time `start_with_mcp` runs as `&self`, the SqlCipherKey is already in flight. The keychain read MUST happen at **AppConfig construction site** (`vault-tauri/src/main.rs` for the V0.1 binary entry point), BEFORE `Application::new`. ADR-040's "Location: `Application::start_with_mcp`" line is imprecise — corrected here, ADR-040 amendment for the same correction lands in the same Phase 1 commit.

**Discovery 2 — Phase 1 must wire BOTH `SqlCipherKey` provenance AND at-rest-key sourcing.**

iteration 1 framed Phase 1 as at-rest-key only, with VAULT_KEY retirement as a side-deliverable. But VAULT_KEY currently feeds SqlCipherKey directly (V0.1 path); retiring VAULT_KEY in Phase 1 means SqlCipherKey provenance MUST flip to keychain-derived in Phase 1 — otherwise the build breaks. Plus: Phase 2 (V0.1 plaintext data migration) reads V0.1 plaintext data and re-writes through the sealed path; Phase 2 needs at-rest-key wired BEFORE it runs. So Phase 1 wires:
1. **Single master_key source** (32 bytes) read from keychain at vault-tauri main.rs.
2. **SqlCipherKey passphrase** derived from master_key (option β — see Discovery 3).
3. **at-rest-key derivation in vault-storage** keyed off master_key flowing through AppConfig (per ADR-008 amendment K3 KDF).

Phase 3 still owns plaintext-`open()` deletion + ADR-010 controls removal + acceptance suite. Phase 2 needs `LanceVectorStore::open()` (plaintext) alive to read V0.1 data; Phase 3's deletion timing remains contingent on Phase 2 being the LAST CALLER.

**Discovery 3 — master_key → SqlCipherKey derivation tree was unlocked.**

ADR-008 amendment locks master_key → at_rest_key via K3 KDF (`blake3::derive_key("vault memory at-rest sealing v1", &master_key)`). No equivalent lock existed for master_key → SqlCipherKey. Three options surfaced; **option β locked**:

```
sqlcipher_passphrase = hex(blake3::derive_key("vault sqlcipher passphrase v1", &master_key))
at_rest_key          = blake3::derive_key("vault memory at-rest sealing v1", &master_key)
```

Reasoning: matches the K3 KDF pattern; preserves single-source-crypto principle (BLAKE3 already in workspace, single primitive across SqlCipher + at-rest + sync envelope + audit chain — all domain-separated by distinct prefix strings); clean key-per-consumer isolation. Option α (hex-encode master_key directly as SqlCipher passphrase) works but exposes master_key to SQLCipher's PBKDF2 directly — breaks the subkey-per-consumer pattern. Option γ (raw key via `PRAGMA key = "x'<hex>'"`) requires SqlCipherKey API change — out of scope for Phase 1. **Lock captured in ADR-040 amendment** (extending production-wiring section's master_key flow with the derivation tree); cross-links ADR-008 amendment for the at-rest K3 KDF parallel.

**Discovery 4 — Phase 1 surface change is bounded by helper-module factoring.**

Source-read showed Phase 1 would touch `vault-tauri/src/main.rs` + `vault-tauri/src/lib.rs` + `vault-tauri/dist/index.html` (VAULT_KEY references). Without factoring, Phase 1 splits across vault-tauri (binary) + vault-app (composition root) + vault-storage (KDF consumer) + vault-core (error variant). **Decision: factor keychain-touching code into a `vault_app::keychain` module** with the public function:

```rust
pub fn read_or_init_master_key(vault_id: &str) -> VaultResult<Zeroizing<[u8; 32]>>;
```

vault-tauri's main.rs change becomes one function call (`let master_key = vault_app::keychain::read_or_init_master_key(&vault_id)?;`). Keychain-touching code lives in vault-app where it's unit-testable; `keyring-core` + `windows-native-keyring-store` deps live in vault-app's `[dependencies]`, not vault-tauri's. Surface change in vault-tauri stays minimal.

**Phase 1 close-out commit deliverables (corrected, supersedes iteration 1's Phase 1 paragraph for execution).**

1. `vault_app::keychain::read_or_init_master_key(vault_id) -> VaultResult<Zeroizing<[u8; 32]>>` helper module.
2. `vault_app::keychain::derive_sqlcipher_passphrase(&master_key) -> SqlCipherKey` derivation per option β. (Or co-located in vault-storage; decided at code-review time.)
3. `vault-tauri/src/main.rs` AppConfig construction site: read keychain → derive SqlCipherKey → build AppConfig → call `Application::new(&config)`. master_key flows through to `LanceVectorStore::open_with_at_rest_key` via `AppConfig` (new `at_rest_key` field added — name TBD at code review) once Phase 3 deletes plaintext `open()`. **In Phase 1**, master_key is read + SqlCipherKey derived; at_rest_key is staged for Phase 2/3 consumption (see below).
4. New `VaultError` variant (name TBD at code review) distinguishing keychain-provenance failure from generic `VaultError` variants.
5. VAULT_KEY env var retirement: removed from `.env.example`, `setup-dev-env.{sh,ps1}`, `.cargo/config.toml` `[env]`, `.github/workflows/ci.yml` `env:` block.
6. Workspace dep promotion: `keyring-core`, `windows-native-keyring-store`, `getrandom`, `hex` move from vault-app `[dev-dependencies]` to `[workspace.dependencies]` + vault-app `[dependencies]`.
7. ADR-040 amendment: production-wiring location correction (vault-tauri main.rs, not `Application::start_with_mcp`) + SqlCipher derivation tree lock (option β).

**Phase 2 dependency on Phase 1 (re-stated for clarity).** Phase 2 (V0.1 plaintext data migration) reads V0.1 plaintext via `LanceVectorStore::open()` and writes V0.2 sealed via `LanceVectorStore::open_with_at_rest_key(path, dim, &master_key)`. The master_key fed to the latter is the same master_key Phase 1 sourced from keychain. Phase 2 needs the AppConfig flow (or equivalent) to carry master_key from main.rs through to the migration call site. Concrete plumbing decision deferred to Phase 2 code review (could be a new AppConfig field `at_rest_key`, or could be a separate parameter to a Phase 2-specific migration function).

**Test-count floor adjustment (iteration 1.5).** iteration 1 forecast Phase 1 +3 tests (keychain round-trip / wrong-account / restart-survives — all pin-tests of the spike artefact's behaviour). iteration-1.5 adds **+1-2 vault-app unit tests** for `read_or_init_master_key`: (a) round-trip (write a master_key via this function path → read back → byte-equal); (b) first-run-generates-and-persists (no entry exists → function generates new key + persists + returns; second call reads the persisted entry). **Aggregate floor: +12-13 tests across T0.2.0 close-out** (was +11 at iteration 2; +1-2 from iteration-1.5 helper-module pins). Total workspace +17-19 (was +16-18 at iteration 2).

**Cross-references.** ADR-040 (Phase 1 close — keychain crate selection + key-storage policy). ADR-040 amendment (Phase 1 close — production-wiring location + SqlCipher derivation tree per option β). ADR-008 amendment (K3 KDF for at-rest sealing — parallel to the SqlCipher derivation locked here). ADR-032 (V0.1 VAULT_KEY env var; iteration-1.5 confirms its retirement timing as Phase 1, not deferred).

---

## Phase 1 keychain spike — methodology declaration (drafted 2026-05-09)

**Context.** Trigger: ADR-032 amendment ("V0.2 alpha-distribution must migrate to OS keychain — keyring-core ecosystem mid-migration at V0.1; revisit at V0.2 plan time + spike picks branch (D) keychain"). The crate-selection decision must be runtime-confirmed before production wiring, per spike-before-lock standing rule.

**Methodology: hybrid (web research + compile-and-run).** Per `feedback_runtime_confirmation_after_web_spike.md`, the compile-and-run leg is the **load-bearing** leg — web research informs but does not lock. Two legs run in this order, with a STOP-AND-REPORT checkpoint between them:

**Leg 1 — Web research — EXECUTED 2026-05-09, findings below.**

**Findings:** The `keyring` crate ecosystem migrated April 2026: v3.x is end-of-line, v4 is sample-code-only ("do not depend"), and the active library is `keyring-core` 1.0.0 with credential stores split into per-platform crates under the `open-source-cooperative` org (Dan Brotsky maintainer). Iteration 2's "keyring 3.x likely" prior was **invalidated** by Leg 1 — exactly the divergence the methodology gate was designed to catch per `feedback_runtime_confirmation_after_web_spike.md`. Crate-name / version map is captured in iteration 2's OQ #1 partial-resolution table above. No RustSec advisories surfaced for `keyring-core` or `windows-native-keyring-store` at web-research depth.

**API shape confirmed:**
```rust
keyring_core::set_default_store(store);
let entry = Entry::new("service", "user")?;
entry.set_secret(&bytes)?;     // binary, not just string — matches our 32-byte master_key
let bytes = entry.get_secret()?;
```

**Predicted lead candidate (Leg 1 web-research confirmed; runtime evidence pending Leg 2):** `keyring-core = "1"` + `windows-native-keyring-store = "1"`. Both are v1.0.0 stable, coordinated April 2026 release.

**Side-finding for ADR-040 forward-compat (concrete-not-hypothetical per `feedback_forward_compat_concrete_vs_hypothetical.md`).** `apple-native-keyring-store` has TWO modules: `Keychain` (unsigned-app compat — works for Memory Vault V0.1 unsigned per ADR-031) and `Protected` (code-signed-only, biometric + iCloud-sync). When V0.2 alpha-cohort macOS signing pipeline lands per ADR-029 amendment hard-gate, Mac users on signed builds gain biometric + iCloud-sync vs Mac users on the founder-unsigned path. ADR-040 forward-compat note will name this upgrade path explicitly — V0.2 alpha cohort signing is hard-gated, so this is not speculation.

**`cargo audit` against the actual resolved dep tree** is the load-bearing security check; runs in Leg 2 once Cargo.toml deps land.

**Sources:** [keyring on crates.io](https://crates.io/crates/keyring) · [keyring-core on crates.io](https://crates.io/crates/keyring-core) · [keyring-rs main repo](https://github.com/open-source-cooperative/keyring-rs) · [windows-native-keyring-store releases](https://github.com/open-source-cooperative/windows-native-keyring-store/releases) · [apple-native-keyring-store](https://github.com/open-source-cooperative/apple-native-keyring-store) · [dbus-secret-service-keyring-store](https://github.com/open-source-cooperative/dbus-secret-service-keyring-store) · [Keyring ecosystem wiki](https://github.com/open-source-cooperative/keyring-rs/wiki/Keyring) · [RustSec Advisory Database](https://rustsec.org/advisories/).

**Leg 2 — Compile-and-run on Windows (load-bearing, time budget ~60-90 min, GATED on Leg 1 go-ahead).**
- Spike artifact: `crates/vault-app/examples/keychain_spike.rs` — kept long-term as executable documentation per ADR-008 line 695 pattern.
- Spike-local dev-deps only (`[dev-dependencies]` of vault-app); no workspace-deps promotion until ADR-040 + production wiring at Phase 1 close.
- Spike implements four assertions, in order:
  1. **Round-trip byte-identity:** generate random 32-byte master_key, write to keychain (namespace = `com.memoryvault.spike.v0.2`, account = `spike-test-vault`), read back, assert byte-identical.
  2. **Wrong-account-fails-closed:** attempt read from keychain with wrong account string (`spike-test-vault-WRONG`), assert returns `Err` (not panic, not empty bytes, not silent success).
  3. **Process-restart-survives:** spike's main process writes the entry, spawns a child process (separate `std::process::Command` invocation of itself with a `--child-read` arg), child process reads + asserts identity, parent waits + checks child exit code.
  4. **Manual Credential Manager UI verification:** spike prints the namespace + account string to stdout; Shahbaz opens Windows Credential Manager (Control Panel → User Accounts → Credential Manager → Windows Credentials) and confirms the entry is visible at the printed namespace. Manual step — runtime evidence requires human confirmation that the entry persisted to the OS layer, not just the keyring crate's in-memory cache.
- **Cleanup:** spike deletes its own keychain entry on exit (success or failure path) so re-runs are deterministic. If cleanup fails, prints the namespace + account so Shahbaz can manually delete via Credential Manager.
- Spike runs from PowerShell per `feedback_cargo_on_windows_use_powershell.md`. Single command: `cargo run -p vault-app --example keychain_spike --release`.
- **Acceptance for Leg 2:** all four assertions PASS. Any failure → STOP and escalate to Shahbaz, do not improvise. Per `feedback_spike_methodology_explicit.md` discipline.

**ADR-040 keychain-namespace + account + payload locks (confirmed pre-spike, drafted from spike runtime evidence at Phase 1 close).**
- **Namespace.** Reverse-DNS convention matching macOS Keychain conventions.
  - Production: `com.memoryvault.v0.2`
  - Spike: `com.memoryvault.spike.v0.2`
  - Distinguishable from any other Memory Vault keychain entry; spike namespace prevents accidentally clobbering a real production entry during repeated spike runs.
- **Account string.** V0.2 ships single-vault per BRD §6.2; account string is the literal vault-id string (one keychain entry per vault). **Forward-compat for V1.0 multi-vault** (BRD §6 names multi-vault explicitly in V1.0 scope, so this is concrete-not-hypothetical per `feedback_forward_compat_concrete_vs_hypothetical.md`): the same account-string slot will hold the V1.0 vault-id; multiple entries under the same `com.memoryvault.v0.2` namespace gives multi-vault for free, no schema migration needed.
- **Secret payload.** Raw 32-byte `master_key` bytes — no encoding overhead, no parse-error class, matches the existing `[u8; 32]` Rust shape. Wrapped in `Zeroizing<[u8; 32]>` on read per BRD §11.5.3 / ADR-008 amendment K3-KDF wrapper pattern.

**Artifact retention.** Spike file kept long-term as executable documentation. ADR-040 cross-references the file path. If `keyring` minor-version bumps in the future, spike re-run is the verification mechanism — analogous to ADR-008 line 696 pattern for the dryoc spike.

**Failure conditions (any → stop and escalate, do not silently work around).**
- Web research finds no maintained crate with cross-platform claims that pass cargo-audit → escalate; ADR-032 may need re-architecting (e.g., DPAPI-direct on Windows, abandoning portable-keychain framing).
- Spike compile fails → diagnose root cause; do not patch around (per saved-memory `feedback_dont_propose_relaxation_for_speed.md`).
- Round-trip byte-identity fails → keyring crate is broken or misconfigured; escalate before any production wiring touches keychain.
- Wrong-account-fails-closed returns silent success or empty bytes → security failure mode; escalate immediately, do not proceed.
- Process-restart-survives fails → entry is in-memory-only; not actually persisted; ADR-032 amendment needs re-architecting.
- Credential Manager UI does not show the entry → entry is in-process-cache-only or in a wrong store; escalate.

**Out of scope for the spike (deferred to Phase 1 production wiring or later phases).**
- macOS Keychain Services runtime confirmation — covered by Leg 1 cross-platform-claim audit + CI matrix at Phase 3 acceptance suite, not by the manual-test leg of this spike (per ADR-029 V0.1 dogfood pattern: spike runtime confirmation happens on the founder's actual hardware, which is Windows).
- Linux libsecret runtime confirmation — same reason as macOS.
- Production wiring to `Application::start_with_mcp` — Phase 1 close, post-ADR-040.
- VAULT_KEY env var retirement — Phase 1 close, post-ADR-040.
- Migration of any existing V0.1 keys (none exist; V0.1 used VAULT_KEY env var, not keychain).

---

## ADR-040 — Keychain crate selection + key-storage policy (drafted 2026-05-09, T0.2.0 Phase 1)

**Status:** ACCEPTED, in force from T0.2.0 Phase 1 commit.

**Context.** ADR-032 amendment trigger ("V0.2 alpha-distribution must migrate to OS keychain"). T0.2.0 close-out plan iteration 1 Phase 1: at-rest-key provenance must move from VAULT_KEY env var (V0.1 compensating control) to OS keychain before V0.2 alpha-cohort distribution. Spike-before-lock standing rule: crate selection is a runtime-evidence decision. Phase 1 Leg 1 (web research, 2026-05-09) + Leg 2 (compile-and-run on Windows, 2026-05-09) ran the hybrid spike methodology declared in HANDOFF.md "Phase 1 keychain spike — methodology declaration."

**Decision (locks from combined Leg 1 + Leg 2 evidence).**

- **Crate stack (Windows production):**
  - `keyring-core = "=1.0.0"` — library API
  - `windows-native-keyring-store = "=1.0.0"` — Windows Credential Manager backend
  - Both v1.0.0 stable, coordinated April 2026 by `open-source-cooperative` org (Dan Brotsky maintainer)
  - Pinned per BRD §0.2 + ADR-002

- **Crate stack (cross-platform forward-compat, concrete-not-hypothetical per `feedback_forward_compat_concrete_vs_hypothetical.md`):** `apple-native-keyring-store` (macOS — `Keychain` module unsigned, `Protected` module signed-with-biometric+iCloud), `linux-keyutils-keyring-store`, `dbus-secret-service-keyring-store` (with `crypto-rust` feature). Per-platform crate-add at T0.2.0.x sub-task or T0.2.14 Stub-Installer-adjacent; not architecture-blocked.

- **API pattern:**
```rust
use keyring_core::Entry;
use windows_native_keyring_store::Store;

keyring_core::set_default_store(Store::new()?);
let entry = Entry::new(SERVICE, USER)?;
entry.set_secret(&master_key)?;          // 32-byte binary
let key = entry.get_secret()?;
keyring_core::unset_default_store();
```

- **Key-storage policy:**
  - **Namespace (service):** `com.memoryvault.v0.2` (production), `com.memoryvault.spike.v0.2` (spike-only). Reverse-DNS, distinguishable from any other Memory Vault keychain entry.
  - **Account (user):** literal vault-id string. V0.2 single-vault per BRD §6.2 → one entry. **V1.0 multi-vault forward-compat (concrete-not-hypothetical):** same account-string slot holds future V1.0 vault-id; multiple entries under the same namespace gives multi-vault for free, no schema migration.
  - **Secret payload:** raw 32-byte `master_key` bytes via `set_secret` / `get_secret`. No encoding overhead, no parse-error class, matches `[u8; 32]` Rust shape.
  - **In-memory protection:** wrap on read in `Zeroizing<[u8; 32]>` per BRD §11.5.3 / ADR-008 amendment K3-KDF pattern; never materialise as plaintext `Vec<u8>` outside the `get_secret` call site.
  - **Windows persistence:** `CRED_PERSIST_ENTERPRISE` (strongest of three Windows persistence types — survives reboots + logoffs; allows credential roaming on domain-joined machines with credential-roaming policy; functionally equivalent to Local Machine on personal-use). **Verified by Phase 1 Leg 2 UI screenshot.**
  - **Windows on-OS target-format:** `windows-native-keyring-store` renders `(service, user)` as `<user>.<service>` joined with literal dot. Phase 1 spike entry rendered as `spike-test-vault.com.memoryvault.spike.v0.2` in Credential Manager. Documented for user-facing diagnostics; user code is unaffected (keyring-core API abstracts over the per-platform format).

- **Production wiring (Phase 1 same-commit deliverable):**
  - Location: `Application::start_with_mcp` (V0.1 path established at T0.1.10 Phase 2; ADR-034 V0.1 fix-forward).
  - On startup: read master_key via `Entry::new(NAMESPACE, vault_id).get_secret()`. On `Err(NotFound)` → first-run: generate new 32-byte master_key via `getrandom`, persist via `set_secret`, proceed. On any other Err → fail-closed: ERROR log + new `VaultError` variant (name TBD at production-wiring code review) distinguishing keychain-provenance failure from generic `VaultError` variants → vault-app exits non-zero. **Never proceed with partially-recovered / empty / mismatched key.**
  - **VAULT_KEY env var retired:** removed from `.env.example`, `setup-dev-env.{sh,ps1}`, `.cargo/config.toml` `[env]`, CI workflow `env:` block.

**Spike runtime evidence (Phase 1 Leg 2, 2026-05-09).**

Spike artifact: `crates/vault-app/examples/keychain_spike.rs`. Run modes: interactive (full 4 assertions), `--no-interactive` (1-3 + leave for offline UI verify), `--cleanup` (idempotent delete-and-exit).

| # | Assertion | Result | Evidence |
|---|---|---|---|
| 1 | Round-trip byte-identity | **PASS** | 32 random bytes via `getrandom`; `set_secret` → `get_secret` byte-equal |
| 2 | Wrong-account-fails-closed | **PASS** | `Entry::new(NS, "spike-test-vault-WRONG").get_secret()` returned `Err` |
| 3 | Process-restart-survives | **PASS** | Parent spawned `current_exe()` with `--child-read <hex>`; child re-opened entry from fresh process; byte-identical; child exit 0 |
| 4 | Manual Credential Manager UI | **PASS** | Founder screenshot confirmed `spike-test-vault.com.memoryvault.spike.v0.2`; `User name: spike-test-vault`; `Persistence: Enterprise` |

Entry deleted via `--cleanup` post-verification. Re-runnable per ADR-008 line 696 pattern.

**Consequences.**

- **Cross-platform path unblocked, not landed.** OQ #1 partial resolution: per-platform-crate-add list = 3 crates; not architecture-redesign-blocked.
- **Mac signing forward-compat: free upgrade.** When ADR-029 amendment hard-gate lands (V0.2 alpha-cohort macOS signing pipeline), Mac users gain biometric + iCloud-sync via `apple-native-keyring-store::Protected` module without keyring-API change in user code — module selection happens at store-construction time.
- **Spike-local dev-deps promote to workspace at production wiring.** Phase 1 close commit moves `keyring-core`, `windows-native-keyring-store`, `getrandom`, `hex` from vault-app `[dev-dependencies]` to `[workspace.dependencies]` + vault-app `[dependencies]`. Pattern matches at-rest spike → ObjectStoreProvider production wiring.
- **Replay-attack residual on Windows: credentials accessible to any process running as the same Windows user.** Windows Credential Manager scopes to `(user, target)`; any same-user process can read the master_key via the same API. **No V0.2 mitigation** — same-user-process-isolation is the OS contract, not application contract. Higher-trust (DPAPI prompt-on-access, hardware-backed keystore) would be needed for hostile-same-user-process scenarios — out of scope V0.2, revisit V1.0 threat-model review. Same discipline as ADR-008 amendment replay residual.
- **VAULT_KEY migration is one-shot, founder-only.** V0.1 was Shahbaz-only; no external user has VAULT_KEY to migrate. First-run path generates new master_key under V0.2.

**Verification.**

- Phase 1 Leg 1 evidence: HANDOFF.md "Phase 1 keychain spike — methodology declaration" Leg 1 findings block.
- Phase 1 Leg 2: 4/4 assertions PASS (table above).
- Phase 1 close-out commit: production wiring + DoD gates green + CI matrix green (`[ubuntu-latest, windows-latest, macos-latest]`).
- `cargo audit` against the Phase 1 close commit dep tree.

**When to revisit.**

- `keyring-core` 1.x minor-version bump → re-run spike per ADR-008 line 696 pattern.
- macOS / Linux founder-dogfood platform expansion → add `apple-native-keyring-store` / `linux-keyutils-keyring-store` / `dbus-secret-service-keyring-store` per-platform crate; spike re-run on the new platform.
- Hardware-backed keystore option appears in keyring-core ecosystem → V1.0 threat-model review window; replay-attack residual mitigation.
- High/Critical RustSec advisory on `keyring-core` or any credential-store crate → standard monthly-tech-debt check picks up; immediate triage if blocking.
- `open-source-cooperative` maintainer / org change → re-evaluate maintenance health.

**Cross-links.** ADR-032 (V0.1 VAULT_KEY env var; this ADR closes the amendment trigger). ADR-008 amendment (K3 KDF + at-rest sealing — master_key sourced from keychain feeds at-rest sealing key derivation). ADR-029 amendment (V0.2 alpha-cohort macOS signing hard-gate; Mac path activates `Protected` module). ADR-031 (V0.1 unsigned MSI; no impact on Windows keychain — Credential Manager doesn't require code-signing). BRD §11.5.3 (in-memory key zeroize). Spike artifact: `crates/vault-app/examples/keychain_spike.rs`.

---

## ADR-040 amendment — Production-wiring location correction + master_key derivation tree (drafted 2026-05-09, T0.2.0 Phase 1 post-source-read)

**Status:** ACCEPTED, in force from T0.2.0 Phase 1 commit. Supersedes ADR-040's original "Location: `Application::start_with_mcp`" line; locks the master_key → SqlCipherKey derivation that ADR-040 left implicit.

**Trigger.** Phase 1 production-wiring source-read of `crates/vault-app/src/application.rs` + `crates/vault-app/src/config.rs` surfaced two contract divergences from the original ADR-040 text. Per `feedback_flag_review_as_plan_amendment.md`, the corrections land as an explicit ADR-040 amendment alongside the iteration-1.5 plan amendment in HANDOFF.md.

**Correction 1 — Production-wiring location.**

ADR-040 said: *"Location: `Application::start_with_mcp`."*

**Corrected:** Production wiring lives at the **AppConfig construction site**, which for the V0.1 binary entry point is `vault-tauri/src/main.rs`. `Application::new(config: &AppConfig)` consumes `config.key: SqlCipherKey` at construction time, cloning into three `MetadataStore::open` / `StorageBackend::open` calls. By the time `start_with_mcp` runs as `&self`, the SqlCipherKey is already in flight. The keychain read MUST happen BEFORE `Application::new`.

**Factoring:** Keychain-touching code lives in a new `vault_app::keychain` module (under `crates/vault-app/src/keychain.rs`) with the public surface:

```rust
pub fn read_or_init_master_key(vault_id: &str)
    -> VaultResult<Zeroizing<[u8; 32]>>;

pub fn derive_sqlcipher_passphrase(master_key: &[u8; 32])
    -> SqlCipherKey;

pub fn derive_at_rest_key(master_key: &[u8; 32])
    -> Zeroizing<[u8; 32]>;
```

vault-tauri's main.rs change becomes one helper-module call sequence: read keychain → derive SqlCipherKey → derive at_rest_key → build AppConfig → call `Application::new`. `keyring-core` + `windows-native-keyring-store` deps live in vault-app's `[dependencies]`, NOT vault-tauri's. Surface change in vault-tauri stays minimal; keychain module is unit-testable in vault-app.

**Correction 2 — master_key derivation tree.**

ADR-040 left master_key → SqlCipherKey derivation implicit. **Locked here as option β (domain-separated BLAKE3 subkeys):**

```
master_key             ← 32 bytes from keychain (Phase 1 Leg 2 spike-confirmed flow)

at_rest_key            = blake3::derive_key("vault memory at-rest sealing v1", &master_key)
                         ^ ADR-008 amendment K3 KDF — UNCHANGED, locked there

sqlcipher_passphrase   = hex(blake3::derive_key("vault sqlcipher passphrase v1", &master_key))
                         ^ NEW — locked here
```

**Reasoning for option β over alternatives.**
- **Option α (hex-encode master_key directly as SqlCipher passphrase):** smallest change, but exposes master_key bytes directly to SQLCipher's PBKDF2 — breaks the subkey-per-consumer pattern + couples the master_key value to two distinct cryptographic consumers.
- **Option β (chosen — domain-separated BLAKE3 subkeys):** matches ADR-008 amendment K3 KDF pattern; preserves single-source-crypto principle (BLAKE3 already in workspace, single primitive across SqlCipher + at-rest + sync envelope + audit chain — all domain-separated by distinct prefix strings); clean key-per-consumer isolation. Domain-separator string `"vault sqlcipher passphrase v1"` is distinct from the at-rest sealing prefix per the same domain-separator-distinct-prefix discipline ADR-008 amendment locked.
- **Option γ (raw key via `PRAGMA key = "x'<hex>'"`):** best perf (skips PBKDF2), but requires SqlCipherKey API change — out of scope for Phase 1.

**Hex encoding for the SqlCipher passphrase:** SqlCipherKey accepts a String passphrase per `vault-storage::SqlCipherKey::new(String)`. The 32-byte BLAKE3 derive_key output is hex-encoded to a 64-character ASCII string (`hex::encode`) before being passed in. SQLCipher then runs PBKDF2 over this string per its standard key-derivation flow. The hex-encoding is a serialization choice (32 raw bytes don't fit cleanly into a String passphrase), not a security choice — the derived 32 bytes are full-strength keying material; PBKDF2 over them is defense-in-depth.

**`vault_id` provenance for V0.2 single-vault.** Until BRD §6.2 multi-vault lands at V1.0, `vault_id` is a fixed string `"default"` for the keychain account field — matches the V0.1 hardcoded boundary `"default"` per BRD §5.1. V1.0 multi-vault forward-compat preserved per ADR-040: same account-string slot will hold real vault-ids; multiple entries under the same `com.memoryvault.v0.2` namespace gives multi-vault for free.

**Updated production-wiring contract (supersedes ADR-040's bullet of the same name).**

- Location: `vault-tauri/src/main.rs` AppConfig construction site (not `Application::start_with_mcp`).
- Helper: `vault_app::keychain::read_or_init_master_key(vault_id)`.
- On startup: `read_or_init_master_key("default")` returns `Zeroizing<[u8; 32]>`. On `Err(NotFound)` the helper internally generates new 32-byte master_key via `getrandom`, persists via `set_secret`, returns the newly-persisted key. On any other Err → fail-closed: ERROR log + new `VaultError` variant (name TBD at code review) → caller exits non-zero.
- Caller (vault-tauri main.rs) then calls `derive_sqlcipher_passphrase(&master_key)` + `derive_at_rest_key(&master_key)` to produce both consumers' keys.
- AppConfig grows a new `at_rest_key` field (name TBD at code review) carrying the derived at-rest key forward to the LanceDB layer; `AppConfig::key` continues to carry the derived SqlCipherKey. **Phase 1 lands the field with `#[allow(dead_code)]` + forward-pointer comment "Phase 2 migration consumer";** Phase 2/3 wire actual consumption.
- VAULT_KEY env var compensating control retired: removed from `.env.example`, `setup-dev-env.{sh,ps1}`, `.cargo/config.toml` `[env]`, CI workflow `env:` block.

**Verification.**

- `vault_app::keychain::read_or_init_master_key` regression pins (Phase 1 close, +2 vault-app tests per iteration-1.5 floor adjustment): (a) round-trip — write a master_key via this function path → read back → byte-equal; (b) first-run-generates-and-persists — no entry exists → function generates new key + persists + returns; second call reads the persisted entry.
- DoD gates green: build / test / clippy `-D warnings` / fmt --check.
- CI matrix green per `[ubuntu-latest, windows-latest, macos-latest]`. (macOS / Linux gate the keychain code as Windows-only at Phase 1; cross-platform keychain wiring is per-platform crate-add at T0.2.0.x sub-task per HANDOFF.md OQ #1 partial resolution.)

**Cross-links.** ADR-040 (original — superseded by this amendment for production-wiring location only; crate selection + key-storage policy + spike runtime evidence remain locked there). ADR-008 amendment (K3 KDF for at-rest sealing — parallel derivation pattern, this amendment matches its domain-separator-string discipline). ADR-032 (V0.1 VAULT_KEY env var; this amendment confirms retirement at Phase 1 close).

---

## T0.2.0 Phase 1 — close-out narrative (drafted 2026-05-10, post-DoD-gate close)

Phase 1 lands T0.2.0 close-out plan iteration-1.5's full deliverable list (lines 451-459) under green DoD gates, plus two test-driven fixes the gate cycle surfaced. Three things worth capturing for Phase 2 + future-session memory.

### 1. Implementation refinement vs plan — `read_or_init_master_key` signature

iteration-1.5 line 446 documented the helper signature as:

```rust
pub fn read_or_init_master_key(vault_id: &str) -> VaultResult<Zeroizing<[u8; 32]>>;
```

Production code landed with **two parameters**:

```rust
pub fn read_or_init_master_key(namespace: &str, vault_id: &str) -> VaultResult<Zeroizing<[u8; 32]>>;
```

**Why the deviation:** unit tests need to pass unique-per-test namespaces (`com.memoryvault.test.v0.2.<test_name>.<random_nonce>`) to prevent stale-entry collisions across test runs and parallel-test execution against the real Windows Credential Manager. Hardcoding the production namespace inside the helper would force tests to either share an entry (race-prone) or duplicate the helper logic.

**Production callers** in `vault-tauri/src/main.rs` pass `keychain::PRODUCTION_NAMESPACE` (= `"com.memoryvault.v0.2"`) — the production behaviour is identical to the iteration-1.5 contract.

**ADR-040 amendment lines 630-631 also paraphrase the single-param signature** — that's a documentation drift that the per-`feedback_quote_locked_artefacts_dont_paraphrase.md` discipline would catch on a careful re-read. Captured here as a Phase 1 refinement; ADR-040 amendment text is not re-edited (the amendment itself was a snapshot; the refinement amends the refinement). Future-session reading order: this section overrides the API-signature paraphrase in ADR-040 amendment.

Surfaced per `feedback_flag_review_as_plan_amendment.md` rather than left silent.

### 2. Two test-driven bugs surfaced and fixed by the gate cycle

**Bug 1 — `vault_app::keychain` NotFound classifier missed the Windows error variant.** `keychain.rs:174-181` matched on substrings `"no entry"` / `"not found"` / `"no such"` for the empty-entry case (the trigger for first-run-init). Windows Credential Manager (via `windows-native-keyring-store`) actually surfaces empty-entry as `"No matching credential found"` — none of the three substrings match. Result: production startup on a clean Windows box would error out with `KeychainProvenance("get_secret failed (non-NotFound): No matching credential found")` instead of generating + persisting a new master_key. Fix: add `"no matching credential"` to the OR chain with an inline comment naming Windows Credential Manager (`crates/vault-app/src/keychain.rs:173-181`). Preserves fail-closed posture for genuinely-unclassified errors. Test pin: `keychain::tests::read_or_init_master_key_round_trips_byte_identical` + `read_or_init_master_key_first_run_generates_and_persists` both PASS post-fix.

**Bug 2 — `format_keychain_error_dialog` text used lowercase `"reinstall"` mid-sentence.** `vault-tauri/src/lib.rs:121` rendered option 3 as `"3. If the failure persists, reinstall Memory Vault to recover."` Spec-pin test at `vault-tauri/src/lib.rs:281` asserts `dialog.contains("Reinstall")` — capitalised — matching the convention established by the parallel ADR-020 integrity-dialog test (`format_startup_failure_dialog_includes_integrity_details` line 234, `dialog.contains("Reinstall")`). Fix: rewrote option 3 as `"3. Reinstall Memory Vault if the failure persists."` Same meaning, capitalised opening verb, satisfies the locked spec without weakening it. Test pin: `format_keychain_error_dialog_for_keychain_provenance_variant` PASS post-fix.

**Why bug 1 matters as a discipline reinforcement.** This is exactly the spike→production gap `feedback_runtime_confirmation_after_web_spike.md` warns about. Phase 1 Leg 2 spike's first assertion was `set_secret` then `get_secret` — write-then-read, never empty-keychain-read with the production classifier in place. Spike PASSED 4/4 assertions on Windows, ADR-040 was drafted from runtime evidence — but the production-wiring path's first call on a clean keychain hit a code path the spike never exercised. **The runtime-confirmation gate IS load-bearing, not the spike acceptance.** Test author anticipated the bug class (assertion messages literally say "NotFound classifier regression" / "NotFound classifier is misfiring") — the test discipline caught it.

### 3. OOM-recovery process-discipline lesson

DoD-gate session 2026-05-10 hit a parallel-cargo-on-16-GB-Windows OOM event — two concurrent `cargo build --workspace --all-targets` invocations (the second launched after silent-backgrounding obscured the first's still-running state) exhausted the paging file mid-link, corrupting shared workspace dep artifacts across **two distinct file classes**: round 1 corrupted `.rlib` metadata (`libtokio`, `libserde_json`, `libtracing`, `libtracing_subscriber`, `libvault_storage`, `libproptest`, `librpassword`) with `LNK1201` + cascading `E0786 ... os error 1455 paging file too small`; round 2 (after surgical clean of the named `.rlib` set) hit `LNK1285 corrupt PDB file 'initialize_smoke-...pdb'` on a vault-mcp test binary. Round-3 risk (third file class) escalated the response from surgical to full `cargo clean` (114,549 files / 206 GiB removed) → 59m 26s rebuild from clean → green DoD gates → real Phase 1 bugs surfaced.

**Captured lessons:**
- **HANDOFF.md "Open tech-debt"** entry: `Parallel cargo on 16 GB Windows = OOM corruption risk` (added this session, lines ~794) — single-invocation discipline is the only safe path; check `Get-Process cargo|rustc|link` before every cargo invocation, not "is the previous output file growing."
- **`feedback_surgical_cargo_clean_first.md`** escalation-trigger amendment (added this session): surgical-first remains default, BUT if OOM corruption surfaces across two distinct file classes in one session, escalate to full `cargo clean` rather than continue surgical — blast radius likely exceeds what surgical can catch.

### 4. Phase 1 deliverable checklist — verification against iteration-1.5 lines 451-459

| # | Deliverable | Status |
|---|---|---|
| 1 | `vault_app::keychain::read_or_init_master_key` helper module | ✓ — `crates/vault-app/src/keychain.rs` |
| 2 | `vault_app::keychain::derive_sqlcipher_passphrase` (option β) | ✓ — same file (`derive_at_rest_key` co-located per option β symmetry) |
| 3 | `vault-tauri/src/main.rs` AppConfig construction reads keychain → derives both subkeys → builds AppConfig → calls `Application::new` | ✓ |
| 4 | New `VaultError::KeychainProvenance(String)` variant | ✓ — `crates/vault-core/src/error.rs` |
| 5 | VAULT_KEY env var retirement | ✓ — runtime consumption removed from `vault-tauri/src/main.rs` (the only V0.1 consumption site, replaced with keychain helper). Plan-listed files `.env.example` / `setup-dev-env.{sh,ps1}` never existed in this repo (template-borrowed paragraph); `.cargo/config.toml [env]` and `.github/workflows/ci.yml env:` never contained `VAULT_KEY`. `grep -r 'VAULT_KEY' crates/` confirms zero runtime references; doc-comment references at `vault-tauri/src/{lib.rs,main.rs}` correctly frame as retired history. |
| 6 | Workspace dep promotion: `keyring-core`, `windows-native-keyring-store`, `getrandom`, `hex` → `[workspace.dependencies]` + vault-app `[dependencies]` | ✓ |
| 7 | ADR-040 amendment text (production-wiring location + SqlCipher derivation tree per option β) | ✓ — already in HANDOFF.md, lines 615-680 |
| 8 (refinement) | `vault_app::keychain::PRODUCTION_NAMESPACE` const + 2-param helper signature | ✓ — see section 1 above |
| 9 (gate-surfaced) | NotFound classifier widened for Windows variant | ✓ — see section 2, bug 1 |
| 10 (gate-surfaced) | KeychainProvenance dialog text capitalisation fix | ✓ — see section 2, bug 2 |

### 5. DoD gate state at Phase 1 close

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | ✓ exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | ✓ exit 0 |
| `cargo build --workspace --all-targets` | ✓ exit 0 (59m 26s from clean state) |
| `cargo test --workspace --no-fail-fast` | ✓ all crates pass — vault-app 19/0/1, vault-storage **226/0/0**, vault-tauri 6/0/6, vault-mcp 33/0/0, vault-retrieval 30/0/1, vault-core 49/0/0, vault-cli 18/0/0, vault-embedding 13/0/1, all doc-tests pass |
| Test count delta | +5 vault-app (3 keychain regression pins + 2 KDF determinism pins) — within iteration 1.5 floor of +3-5 |

### 6. What Phase 2 inherits

- master_key flow is locked end-to-end at vault-tauri main.rs: keychain read → derive both subkeys → AppConfig carries both forward.
- AppConfig has the staged `at_rest_key` field with `#[allow(dead_code)]` waiting for Phase 2's V0.1-plaintext-migration consumer (per iteration-1.5 deliverable 6 forward-pointer).
- `LanceVectorStore::open()` (V0.1 plaintext) remains alive — Phase 2 is its last caller per close-out plan iteration 1; Phase 3 deletes it.
- ADR-010 hard-gate is **three close-out phases away** (Phase 2 → 3 → 4 → 5).

---

## T0.2.0 Phase 2 — plan iteration 1 (drafted 2026-05-10, post-Phase-1-ship)

Phase 2 implements one-time-on-first-launch detection of V0.1-shape plaintext vault → migrate every row through `LanceVectorStore::open_with_at_rest_key` → delete V0.1 plaintext data + `ALPHA_DO_NOT_STORE_REAL_DATA.txt` marker → distinguishing one-time INFO log. Phase 2 is the LAST CALLER of plaintext `LanceVectorStore::open()` in the codebase; Phase 3 deletes the plaintext path immediately after. Iteration 1 names concrete file paths + function signatures from Phase 2 surface source-read; iteration 2 resolves three open questions.

### 1. Migration module location + signature (vault-storage)

New module `crates/vault-storage/src/migration.rs` (~250-350 LOC estimated). Public surface:

```rust
pub enum MigrationOutcome {
    NoMigrationNeeded,                // clean first-run V0.2 install OR already-migrated
    Migrated { rows_migrated: u64 },  // successful one-shot migration
}

pub async fn migrate_v0_1_to_sealed_if_needed(
    vector_dir: &Path,
    dimension: usize,
    at_rest_key: &[u8; 32],
) -> VaultResult<MigrationOutcome>;
```

**Detection signal:** V0.1-shape data dir = BOTH conditions present:
- `ALPHA_DO_NOT_STORE_REAL_DATA.txt` marker file exists (V0.1 ADR-010 compensating control written by `LanceVectorStore::open` plaintext path, `crates/vault-storage/src/vector_store.rs` step "ADR-010 compensating control #4")
- At least one Parquet file under `vector_dir/<table>/data/` starts with `PAR1` magic bytes (plaintext Lance fragment)

Neither condition → `NoMigrationNeeded`. Marker without Parquet (half-state corruption) → fail-closed `Err`. Parquet without marker (V0.2-shape sealed file already, OR third-party-write) → fail-closed `Err`.

**Migration steps (atomic on success, rollback-on-failure):**
1. Create temp dir `vector_dir.with_extension("v0_1_migration_in_progress")`.
2. Open V0.1 source via `LanceVectorStore::open(vector_dir, dimension)` (still alive at Phase 2; deleted at Phase 3).
3. Stream every row via lance scan API → collect `Vec<RecordBatch>`. **Streaming-vs-collect-into-RAM decision deferred to iteration 2 OQ #2 (lance 4.0 row-iteration API runtime confirmation).**
4. Open temp dir as sealed via `LanceVectorStore::open_with_at_rest_key(temp_dir, dimension, at_rest_key)`.
5. Bulk-insert all rows via the sealed store's writer.
6. Drop both store handles (release LanceDB file locks before any rename).
7. Atomic directory swap: `rename(vector_dir, vector_dir.with_extension("v0_1_backup"))` → `rename(temp_dir, vector_dir)`. **Windows-rename ordering deferred to iteration 2 OQ #1.**
8. Delete `ALPHA_DO_NOT_STORE_REAL_DATA.txt` from the new sealed dir (V0.2 contract: marker absent).
9. Delete the backup dir (post-swap; cleanup-best-effort — failure is WARN-not-Err).
10. Emit one-time INFO log: `"V0.1 → V0.2 migration complete: {rows_migrated} rows migrated, V0.1 plaintext data deleted"`.

**Failure modes + rollback:**
- Read failure mid-iteration → drop both handles, delete temp dir, return Err. V0.1 source untouched.
- Write failure → drop sealed handle, delete temp dir, return Err. V0.1 source untouched.
- Atomic dir swap failure → restore V0.1 by reverse-rename if possible; return Err.
- Backup-dir delete failure (post-swap) → log WARN, return `Migrated { rows_migrated }`. Migration logically succeeded; backup cleanup is best-effort.

### 2. Migration call site (vault-tauri main.rs)

Insert a new step **5b** between AppConfig construction (current step 5, line 226) and Application::new (current step 6, line 256):

```rust
// 5b. V0.1 → V0.2 migration (one-shot, idempotent — NoMigrationNeeded
//     on every launch after the first sealed write). Per HANDOFF.md
//     T0.2.0 close-out plan iteration 1 Phase 2.
let migration_outcome = tauri::async_runtime::block_on(
    vault_storage::migration::migrate_v0_1_to_sealed_if_needed(
        &config.vector_dir,
        EMBEDDING_DIM,
        &config.at_rest_key,
    ),
);
match migration_outcome {
    Ok(MigrationOutcome::NoMigrationNeeded) => {}
    Ok(MigrationOutcome::Migrated { rows_migrated }) => {
        tracing::info!(rows_migrated, "V0.1 → V0.2 migration complete");
    }
    Err(e) => show_fatal_dialog_and_exit(
        app.handle(),
        "Memory Vault — V0.1 Data Migration Failed",
        &format_migration_error_dialog(&e),
        EXIT_STARTUP_FAILURE,
    ),
}
```

Plus new `vault_tauri::format_migration_error_dialog(&VaultError) -> String` in `vault-tauri/src/lib.rs` (parallel pattern to Phase 1's `format_keychain_error_dialog`; same fall-through-to-`format_startup_failure_dialog` discipline for non-migration variants) + spec-pin test.

This step turns AppConfig's `at_rest_key` `#[allow(dead_code)]` field into live consumption — Phase 1 staging fulfilled.

### 3. StorageBackend at-rest-key wiring (vault-storage + vault-app)

`StorageBackend::open` gets a sealed companion (mirrors LanceVectorStore's two-constructor pattern):

```rust
pub async fn open_with_at_rest_key(
    metadata_path: &Path,
    vector_dir: &Path,
    graph_path: &Path,
    key: SqlCipherKey,
    dimension: usize,
    at_rest_key: &[u8; 32],
) -> VaultResult<Self>;
```

`Application::new` (`crates/vault-app/src/application.rs:102`) flips from `StorageBackend::open(...)` → `StorageBackend::open_with_at_rest_key(..., &config.at_rest_key)`. Plaintext `StorageBackend::open` retained for Phase 2's migration source path; **Phase 3 deletes both** `LanceVectorStore::open` and `StorageBackend::open` plaintext constructors.

### 4. Three-tier fixture strategy (per iteration 2 OQ #2 resolution — locked file paths)

**Tier 1 — Scaffolding regression test** at `crates/vault-storage/tests/migration_v0_1_to_sealed.rs`:
- Setup: cargo-test fixture writes synthetic V0.1-shape Parquet via `arrow_array::RecordBatch` + `parquet::arrow::ArrowWriter`, plus the `ALPHA_DO_NOT_STORE_REAL_DATA.txt` marker.
- Assertions: `NoMigrationNeeded` on empty dir / `Migrated` on V0.1-shape / row-count preserved / framing-bytes `0x01 0x00` on every file post-migration / zero PAR1 magic post-migration / marker file deleted from new sealed dir.
- Runs every commit on every CI matrix OS. Catches "migration logic mishandles a known V0.1 file shape."

**Tier 2 — Real V0.1 binary fixture** at `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/`:
- Captured ONCE from V0.1 binary at commit `1d72aac` (= `v0.1.0` tag, V0.1 SHIPPED commit, MSI SHA-256 `03d127371f6a881366e2f048d81f2785de97f68236c5d52747bf0100284d0a06`, Phase 5e build).
- Capture procedure: build V0.1 binary at `1d72aac` → launch with `$env:VAULT_KEY = "fixture-capture-key"` → save 5 throwaway anonymized memories ("fixture row 1" / "fixture row 2" / ... / "fixture row 5", boundary `"default"`) → verify `ALPHA_DO_NOT_STORE_REAL_DATA.txt` marker present → copy entire `%APPDATA%/com.memoryvault.dev/lance/` directory + the marker into the fixture dir.
- `README.md` alongside fixture: capture date, V0.1 commit SHA `1d72aac`, V0.1 tag `v0.1.0`, MSI SHA-256, capture key (intentionally checked in — fixture-only, never used for real data), row count, content of each row.
- Test runs migration on a deep-copy of the fixture (so re-runs are idempotent) and asserts Tier-1 expectations + content-equality on every row (5 known strings).
- Catches "V0.1 actually wrote files we didn't anticipate in scaffolding."

**Tier 3 — Founder one-shot smoke**, before Phase 2 commit:
- Shahbaz creates a snapshot of his actual V0.1 vault dir for safety (`Copy-Item -Recurse %APPDATA%/com.memoryvault.dev/lance/ %APPDATA%/com.memoryvault.dev/lance.pre-v0.2-snapshot/`).
- Runs the Phase 2 binary against the live V0.1 vault.
- Validates row count preserved + spot-checks 2-3 memories via UI search + confirms `ALPHA_DO_NOT_STORE_REAL_DATA.txt` is gone post-migration.
- Documented as a Phase 2 close-out checklist item; HANDOFF.md captures any findings.

### 5. Test count floor (per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`)

Phase 2 floor: **+5 vault-storage tests** (was +3 at iteration 2):
- Tier 1: `migration_returns_no_migration_needed_on_empty_dir`, `migration_succeeds_on_v0_1_shape_with_marker_and_parquet`, `migration_fails_closed_on_marker_without_parquet`, `migration_fails_closed_on_parquet_without_marker`
- Tier 2: `migration_succeeds_on_real_v0_1_fixture_from_1d72aac`

Plus **+1 vault-tauri test**: `format_migration_error_dialog_includes_recovery_options` (spec-pin parallel to Phase 1's `format_keychain_error_dialog_for_keychain_provenance_variant`).

Aggregate: **+6 tests across Phase 2**, taking vault-storage from 231 → ≥236 (post-Phase-1 baseline 231 = 226 + Phase-1's +5 keychain unit tests in vault-app), vault-tauri 6 → ≥7. Floor breach surfaces + plan-amends per discipline.

### 6. DoD acceptance criteria

- All 4 BRD §0.1 gates green (build / test / clippy `-D warnings` / fmt --check).
- Tier 1 scaffolding tests pass on `[ubuntu-latest, windows-latest, macos-latest]` matrix.
- Tier 2 real-fixture test passes on the same matrix.
- Tier 3 founder smoke passes on actual V0.1 vault (manual, before commit).
- Zero regression in vault-storage's existing 226 tests.
- vault-tauri new dialog-format test passes; ADR-020 + ADR-040 dialog tests still pass (no convention regression).

### 7. Open questions for iteration 2

1. **Atomic dir swap on Windows.** `std::fs::rename` is atomic on Windows when source + dest are on the same volume — BUT Windows rejects `rename` if the destination exists (POSIX allows overwrite). Migration step 7 ("rename vector_dir → backup, rename temp → vector_dir") must be sequenced as: (a) rename `vector_dir` → `backup_dir` (works because `backup_dir` doesn't exist yet), (b) rename `temp_dir` → `vector_dir` (works because `vector_dir` no longer exists). Verify this exact ordering survives a kill-mid-step-a / kill-mid-step-b crash + re-launch — does iteration 2 want a recovery path for those partial states (re-detect on next launch + resume)?

2. **lance 4.0 row-iteration streaming API** (per `feedback_runtime_confirmation_after_web_spike.md` discipline). Phase 2 step 3 needs to "iterate every row" from a `LanceVectorStore`. Web research suggests `vector_store.scan().execute().await?` returns a `Stream<RecordBatch>` — but this is web-research-level evidence. Need either a quick spike OR a runtime-confirmation test in iteration 2 that streams the existing `LanceVectorStore` against the real lancedb 0.27.2 API before locking the migration loop's structure. If streaming is unavailable, fall back to bulk-collect-then-insert (RAM-bounded by V0.1 vault size — tens of MB worst-case for a founder-dogfood vault).

3. **Tier 2 fixture capture timing.** Capturing requires building the V0.1 binary at commit `1d72aac` once. Two options: (a) capture during Phase 2 implementation (defers cost, but Tier 2 test is unrunnable until the fixture exists, blocking the test-first discipline), (b) capture at iteration 2 close, before Phase 2 implementation starts (front-loads cost, but Tier 2 test exists from the first commit). Iteration 2 picks.

### 8. Cross-references

- HANDOFF.md "T0.2.0 close-out plan iteration 1" Phase 2 paragraph (line 358) — original framing
- HANDOFF.md "T0.2.0 close-out plan iteration 2" OQ #2 resolution (lines 384-394) — three-tier fixture strategy
- HANDOFF.md "T0.2.0 close-out plan iteration 1.5" Discovery 2 — Phase 2 dependency on Phase 1 master_key flow
- ADR-040 amendment "Updated production-wiring contract" — `at_rest_key` flows from keychain through AppConfig to migration consumer
- ADR-008 amendment (V0.2 at-rest extension lock-in) — migration target shape (per-file granularity, K3 KDF, sealing format)
- ADR-010 — V0.1 plaintext compensating controls (`ALPHA_DO_NOT_STORE_REAL_DATA.txt` marker is detection signal); Phase 3 removes the controls
- BRD §6 T0.2.0 acceptance criteria — Phase 3 acceptance suite consumes Phase 2's migration outcome

---

## T0.2.0 Phase 2 — plan iteration 2 (drafted 2026-05-10, resolves iteration 1's three open questions)

Iteration 1 stands. Iteration 2 resolves the three open questions named in iteration 1 §7, applies one calibration to the iteration 1 detection rule (6-state expansion replaces the 4-state rule), and locks the Tier 2 fixture capture path. After iteration 2 closes, the next concrete action is **Tier 2 fixture capture from the V0.1 binary at commit `1d72aac`** (UI procedure, see §3 below) — Phase 2 implementation work begins after the fixture is checked in.

### 1. OQ #2 — RESOLVED, no spike needed (in-tree runtime confirmation)

The lance 4.0 row-iteration streaming API question is answered by existing vault-storage production code, not a fresh spike. Two call sites already exercise the pattern across the 226 green vault-storage tests:

- `crates/vault-storage/src/vector_store.rs:562-577` — `search()` impl: `query().nearest_to(...).only_if(...).limit(...).distance_type(...).execute().await` returns a stream; `stream.try_collect().await` collects into `Vec<RecordBatch>`.
- `crates/vault-storage/src/vector_store.rs:675-685` — `validate_readable()` impl: `query().limit(row_count).execute().await` → `stream.try_collect()` → `Vec<RecordBatch>`. The doc comment at line 645 explicitly names this as the locked shape: *"The full shape: `query().limit(count).execute().try_collect()`."*

**Lock for Phase 2 migration step 3:**

```rust
let row_count = source_store.table.count_rows(None).await?;
let stream = source_store.table.query().limit(row_count).execute().await?;
let batches: Vec<RecordBatch> = stream.try_collect().await?;
```

**Bulk-collect over streaming.** V0.1 vault scale is bounded by founder-dogfood reality: ADR-029 V0.1 dogfood was 11 memories. Even at hypothetical 10K rows × (16-byte UUID + 1536-byte Float32 embedding + ~50-byte boundary) ≈ 16 MB peak — trivial. Streaming would be premature optimisation; bulk-collect matches the existing `validate_readable()` shape exactly.

**Discipline:** Per `feedback_runtime_confirmation_after_web_spike.md`, in-tree production code that already passes 226 tests is **stronger evidence** than a fresh spike could produce. Per `feedback_source_read_call_graph_upstream_of_empirical.md`, source-read first (before reaching for empirical investigation) — found the pattern, no further investigation needed.

### 2. OQ #1 — RESOLVED with calibration (6-state detection rule + cookie file + locked rename ordering)

Crash-scenario enumeration of the iteration 1 step-7 atomic dir-swap surfaced two real risks against iteration 1's 4-state detection rule: (a) post-step-(a)-pre-step-(b) crash leaves V0.1 data orphaned in `backup_dir` while next-launch detection sees no `vector_dir` and treats as fresh V0.2 install (data-orphaning risk), (b) post-step-(b)-pre-marker-delete crash leaves the new sealed `vector_dir` with the V0.1 marker still inside, tripping iteration 1's "marker without PAR1 Parquet → fail-closed" rule against successful migration (false-fail-closed risk).

**Calibration A — 6-state detection rule (replaces iteration 1 §1 4-state rule).** Six combinations of marker × disk-content map to specific outcomes. Two new failure-mode names introduced for grep-discoverability per `feedback_quote_locked_artefacts_dont_paraphrase.md`: **`half-state corruption fail-closed`** (marker present + disk empty/mixed = aborted-write-mid-creation), **`third-party data fail-closed`** (marker absent + PAR1 Parquet present = V0.1 data without marker, indicates corrupt or third-party-tool-write).

| Marker file | Disk content | Outcome (named) |
|---|---|---|
| Present | PAR1 Parquet | **`v0_1_shape_migrate`** — V0.1-shape detected, run migration |
| Present | Sealed framing (`0x01 0x00`) | **`post_swap_marker_cleanup`** — Phase 2 step-(b)-succeeded crash recovery: delete marker, return `NoMigrationNeeded` |
| Present | Empty / mixed | **`half_state_corruption_fail_closed`** — aborted-write-mid-creation; cannot safely recover; surface to caller |
| Absent | PAR1 Parquet | **`third_party_data_fail_closed`** — V0.1 data without marker; corrupt or non-Memory-Vault-write; surface to caller |
| Absent | Sealed framing | **`v0_2_clean_no_op`** — clean post-migration V0.2 state, return `NoMigrationNeeded` |
| Absent | Empty | **`first_run_install_no_op`** — first-run V0.2 install, return `NoMigrationNeeded` |

**Calibration B — cookie file for the rename-pair window.** A single sentinel file at `vector_dir.parent().join(".vault_migration_in_progress")` containing the resolved temp_dir + backup_dir paths (JSON, two `PathBuf` strings) is written **before** step (a) and **deleted after** step (b). On next launch the cookie-presence check runs **before** the 6-state detection. State machine on cookie-present:

| Visible state | Recovery action |
|---|---|
| `temp_dir` exists with sealed framing + `vector_dir` does not exist | Resume from step (b) — `rename(temp_dir, vector_dir)`; delete cookie; return `Migrated { rows_migrated: <re-derived from new vector_dir> }` |
| `backup_dir` exists + `vector_dir` does not exist + `temp_dir` gone (rare — temp deleted between crash and re-launch) | Restore from backup — `rename(backup_dir, vector_dir)`; delete cookie; restart migration normally |
| `vector_dir` exists with V0.1-shape data | Step (a) didn't happen yet — delete cookie + any orphaned `temp_dir`/`backup_dir` from earlier aborted runs; restart migration normally |
| Any other state | Surface as cookie-recovery-fail-closed; require manual intervention (Tier 3 founder snapshot is the safety net) |

**Calibration C — locked rename ordering (Windows-correct).** `std::fs::rename` on NTFS rejects rename-to-existing-destination (POSIX allows overwrite; Windows does not). Ordering MUST be:

1. `rename(vector_dir, backup_dir)` — succeeds because `backup_dir` doesn't exist yet
2. `rename(temp_dir, vector_dir)` — succeeds because `vector_dir` no longer exists

Reverse ordering fails on Windows; same-volume guarantee preserves NTFS atomicity per `MoveFileEx` semantics.

**Belt-and-suspenders:** Tier 3 founder snapshot (per iteration 1 §4) remains the unhandled-edge-case safety net. The cookie file + 6-state rule handle named failure modes; cookie-recovery-fail-closed surfaces unhandled cases for human triage rather than silent corruption.

### 3. OQ #3 — RESOLVED, UI capture path locked (CLI capability check finding negative)

**Capability check completed 2026-05-10:** vault-cli at `1d72aac` has **no add-memory capability**. Verified by `git show 1d72aac:crates/vault-cli/Cargo.toml` (description: *"Operator CLI for the Memory Vault. Dead-letter recovery + divergence triage. See BRD §5.2 / ADR-009 / T0.1.6."*) + `git show 1d72aac:crates/vault-cli/src/main.rs` (Command enum has only `DeadLetter { action }` + `DivergenceCheck`; no `Add` / `Put` / `Store` / `Create` / `Memory` subcommand).

**Locked capture path: UI capture via the V0.1 Tauri binary built at `1d72aac`.** Procedure for the Tier 2 fixture capture (front-loaded per iteration 1 §7 OQ #3 option (b) per BRD §0.3 test-first discipline):

1. From the founder's Windows machine: `git worktree add ../memory-vault-v0_1 1d72aac` (preserves current working tree at HEAD; isolated capture environment).
2. In the worktree: `cargo build --release -p vault-tauri` (V0.1 binary build).
3. `$env:VAULT_KEY = "fixture-capture-key-do-not-use-in-prod"`; `cargo run --release -p vault-tauri` (or launch the built MSI; either works).
4. Via the Add tab in the Tauri UI, save **5 throwaway memories** with content exactly: `"Tier 2 fixture row 1"`, `"Tier 2 fixture row 2"`, ..., `"Tier 2 fixture row 5"`. Boundary `"default"` (V0.1 hardcoded). No PII, no real data.
5. Close the Tauri UI cleanly. Verify `%APPDATA%/com.memoryvault.dev/lance/memories/_versions/` contains Parquet files; verify `%APPDATA%/com.memoryvault.dev/lance/ALPHA_DO_NOT_STORE_REAL_DATA.txt` exists (per ADR-010 compensating control #4).
6. Copy the entire `%APPDATA%/com.memoryvault.dev/lance/` directory + the marker file into `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/`.
7. Write `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md` with: capture date, V0.1 commit SHA `1d72aac`, V0.1 tag `v0.1.0`, MSI SHA-256 `03d127371f6a881366e2f048d81f2785de97f68236c5d52747bf0100284d0a06`, capture key (intentionally checked in — fixture-only, never used for real data), 5 row contents listed verbatim, **CLI capture fallback rationale** (vault-cli at `1d72aac` had no add-memory subcommand — Cargo.toml + Command enum verified at OQ #3 resolution; UI capture is the only available path; future Phase 2 fixture refreshes follow the same UI procedure unless vault-cli grows add-memory capability later).
8. `git worktree remove ../memory-vault-v0_1` (cleanup; worktree state is captured into the fixture, original isn't needed).
9. `git add crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/` (stage for the Phase 2 first commit alongside the migration code).

**Why the README captures the CLI fallback rationale:** future-session reader looking at the fixture and asking "why was this captured manually instead of scripted?" gets the answer in-place — the capability-check finding is durable evidence, not a silent omission.

### 4. Updated test-count floor (calibration delta)

Iteration 1 §5 forecast +5 vault-storage + 1 vault-tauri = **+6 tests**. Iteration 2 calibrations don't change that floor at the test-count level: the 6-state detection rule still consumes the same 4 tests iteration 1 named (each maps to a named outcome — `v0_1_shape_migrate` / `post_swap_marker_cleanup` / `half_state_corruption_fail_closed` / `third_party_data_fail_closed`), plus the cookie-recovery state machine adds defensive pins.

**Updated floor: +5 vault-storage + 1 vault-tauri + 2 cookie-recovery pins = +8 tests.** Plus 1 Tier-2 real-fixture pin from iteration 1 §5 = **+9 tests total** (was +6 at iteration 1; +3 from cookie-recovery-state-machine pins surfaced at iteration 2 calibration B). Floor breach surfaced + plan-amends per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`.

Cookie-recovery test names (proposed):
- `cookie_recovery_resumes_step_b_when_temp_dir_exists_and_vector_dir_missing`
- `cookie_recovery_restores_from_backup_when_temp_dir_gone`
- `cookie_recovery_restarts_when_step_a_did_not_happen`

### 5. Next concrete action (Phase 2 sequence opener)

Tier 2 fixture capture per §3 above. **Requires founder time on Windows UI** (~15-20 min: V0.1 binary build + UI capture + fixture commit). After fixture is checked in:

1. Tier 1 scaffolding test scaffolds against synthetic Parquet — implementable without the fixture (depends only on arrow_array + parquet crates).
2. Tier 2 real-fixture test wires against the now-checked-in fixture — implementable after step (1).
3. Migration module implementation against the failing tests per BRD §0.3 test-first discipline.
4. Production wiring at vault-tauri main.rs setup() step 5b per iteration 1 §2.
5. StorageBackend at-rest-key wiring per iteration 1 §3.
6. DoD gates green per iteration 1 §6.

### 6. Cross-references

- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1" (lines above) — full Phase 2 surface
- `crates/vault-storage/src/vector_store.rs:562-577` (search) + `:675-685` (validate_readable) — in-tree runtime evidence for OQ #2
- `feedback_runtime_confirmation_after_web_spike.md` — discipline that says in-tree confirmation suffices, no fresh spike
- `feedback_source_read_call_graph_upstream_of_empirical.md` — source-read first, before empirical investigation
- `feedback_quote_locked_artefacts_dont_paraphrase.md` — explicit failure-mode names introduced for grep-discoverability
- `feedback_floor_forecast_is_pre_declaration_not_estimate.md` — test-count floor delta surfaced + plan-amends, not silently absorbed
- `git show 1d72aac:crates/vault-cli/src/main.rs` — vault-cli at V0.1 SHIPPED commit, capability check evidence

---

## T0.2.0 Phase 2 — plan iteration 2.1 (drafted 2026-05-10, post-Tier-2-fixture-capture)

**Trigger.** Tier 2 fixture capture (per iteration 2 §3 procedure) ran 2026-05-10 against the V0.1 binary at commit `1d72aac` and produced 5 Lance data fragments + 6 manifest versions in `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/`. Empirical inspection of the captured `.lance` files surfaced a **runtime drift from iteration 1+2's detection-signal text** that would have caused Phase 2 implementation to ship a detector that NEVER triggers on real V0.1 data. Iteration 2.1 captures the correction; iteration 1 §1 + iteration 2 §2's "PAR1 magic on root files" / "PAR1 Parquet" wording is **SUPERSEDED** by this iteration's LANC-magic-at-file-end detection signal. Iteration 1 + 2 text is preserved intact for history; future-session readers should treat this iteration 2.1 as the authoritative detection rule.

### 1. Detection-signal correction (PAR1-at-start → LANC-at-end)

**What V0.1 actually wrote** (verified by inspecting `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/lance/memories.lance/data/*.lance`):

- Each `.lance` data file ends with **`4C 41 4E 43`** = ASCII `"LANC"` (last 4 bytes; Lance's own format-magic, not Parquet)
- `_latest.manifest` and `_versions/*.manifest` files **also end with `LANC`** magic
- Files do **NOT** start with PAR1 magic (`50 41 52 31`); the first bytes of `.lance` data files are an embedded UUID string (e.g. `019dfced-19c3-7a...`)
- The V0.1 ADR-010 marker file `lance/ALPHA_DO_NOT_STORE_REAL_DATA.txt` is plain text (337 bytes, with the V0.1 ADR-010 wording verbatim — the prior detection-signal anchor remains valid)

**Updated 6-state detection rule (replaces iteration 2 §2's table — LANC where PAR1 was named):**

| Marker file | Disk content | Outcome (named — UNCHANGED from iteration 2 §2) |
|---|---|---|
| Present | At least one `.lance` file in `<table>/data/` ends with `LANC` magic (`4C 41 4E 43`) | **`v0_1_shape_migrate`** — V0.1-shape detected, run migration |
| Present | Files start with sealed framing (`0x01 0x00`) per ADR-008 amendment | **`post_swap_marker_cleanup`** — Phase 2 step-(b)-succeeded crash recovery: delete marker, return `NoMigrationNeeded` |
| Present | Empty / mixed | **`half_state_corruption_fail_closed`** — aborted-write-mid-creation; cannot safely recover; surface to caller |
| Absent | At least one `.lance` file in `<table>/data/` ends with `LANC` magic | **`third_party_data_fail_closed`** — V0.1 data without marker; corrupt or non-Memory-Vault-write; surface to caller |
| Absent | Sealed framing | **`v0_2_clean_no_op`** — clean post-migration V0.2 state, return `NoMigrationNeeded` |
| Absent | Empty | **`first_run_install_no_op`** — first-run V0.2 install, return `NoMigrationNeeded` |

**Detection-implementation note:** check the LAST 4 bytes of `.lance` files in `<table>/data/`, NOT the first 4 bytes (V0.1 puts an embedded UUID at the start; only the trailing magic is reliable). Concrete: `std::fs::read` the file, take `bytes[bytes.len()-4..]`, compare to `b"LANC"`.

### 2. Tier 1 scaffolding strategy implication

Iteration 2 §3 Tier 1 scaffolding plan said: *"cargo-test fixture writes synthetic V0.1-shape Parquet via `arrow_array::RecordBatch` + `parquet::arrow::ArrowWriter`"*. **This was wrong** — Parquet writer would produce PAR1-magic files, not LANC-magic Lance fragments. Tier 1 cannot synthesize V0.1-shape data via raw Parquet writer.

**Two viable Tier 1 strategies** post-correction:

- **Tier 1a (recommended):** Use the workspace's lance 4.0 dep to write `.lance` files via `lance::Dataset::write` or equivalent. lance 4.0's `.lance` files also end with `LANC` magic (Lance's format-magic is consistent across V0.1 era → V0.2 era; what differs is internal fragment-layout schema). Tier 1 scaffolding writes a minimal valid Lance fragment + the ADR-010 marker file, then runs the detector. Tests detection-rule logic without coupling to the Tier 2 fixture's real-file presence.
- **Tier 1b (fallback if 1a's lance 4.0 file isn't recognised by the migration's lance 4.0 reader as backward-compat):** Tier 1 collapses into "shallow regression on the detector logic only" — write raw bytes that match the LANC magic at the end of a fake `.lance` file, plus the marker. Detector tests pass; full migration round-trip happens only at Tier 2 (real fixture).

Iteration 2 §3 Tier 1 file-path target stays at `crates/vault-storage/tests/migration_v0_1_to_sealed.rs`.

### 3. New open question for iteration 3 — lance 4.0 backward-compat read of V0.1 (lance 0.15) fragments

Phase 2 migration step 3 (per iteration 1 §1 + iteration 2 §1) reads V0.1 source via `LanceVectorStore::open(vector_dir, dimension)` then iterates rows via `query().limit(N).execute().try_collect()`. **Open question:** does lance 4.0 (the version in the workspace's Cargo.toml since Phase 0a) successfully read V0.1's lance 0.15 fragments?

Lance has documented backward-compat across V2 file format versions, but lance 0.15 may have used V1 file format — if so, lance 4.0 may refuse to open it OR open it but mis-decode rows. **This is a hard blocker for the migration plan if it fails.**

**Iteration 3 must runtime-confirm** by either:
- Quick spike: write a small Rust program that opens `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/lance/memories.lance/` via lance 4.0 and dumps row count + first row's content. Pass = lance 4.0 reads V0.1 fragments.
- Or implement Tier 1 scaffolding first; if it can write a lance 4.0 `.lance` file that the migration reader picks up correctly, then Tier 2 (real V0.1 fixture) is the final test.

If lance 4.0 cannot read V0.1 fragments, **alternative migration strategies** for iteration 3 to consider:
- Bundle a lance 0.15 reader as a dev-dep just for migration (heavy)
- Build a one-shot migration tool from the V0.1 binary that exports to a lance-version-neutral format (Parquet, JSON), then V0.2 reads that (extra step, but unblocks the migration architecturally)
- Document migration-not-supported for V0.1 founder data; founder accepts data loss; new alpha-cohort installs are first-run-only (acceptable since alpha-cohort hasn't shipped yet)

**Discipline:** This is exactly the spike-or-empirical-confirmation gate `feedback_runtime_confirmation_after_web_spike.md` exists to enforce. Should have been an iteration 1 OQ; surfaces here only because the Tier 2 fixture capture made the version-compat question concrete + reproducible. Iteration 3 carries it.

### 4. Discipline cross-references

- `feedback_quote_locked_artefacts_dont_paraphrase.md` — the iteration 1+2 "PAR1 magic" wording was paraphrased from web-research-level Lance docs without runtime verification. Iteration 2.1's correction (LANC at file END) is the verbatim-from-runtime-evidence form. The fixture README (`crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md` "⚠️ CRITICAL V0.1 file-format finding" section) names the same finding for grep-discoverability.
- `feedback_runtime_confirmation_after_web_spike.md` — Tier 2 fixture capture WAS the runtime confirmation. Without it, Phase 2 implementation would have shipped a detector that never triggers on real V0.1 data. This is the third triple-validation of this discipline (after T0.1.11 Phase 1 ort/ORT version coupling, Phase 5b rmcp stdin-hang, Phase 5c Tauri 2 `window.__TAURI__` default).
- `feedback_source_read_call_graph_upstream_of_empirical.md` — vault-cli capability check (iteration 2 §3 OQ #3 resolution) used `git show 1d72aac:crates/vault-cli/...` instead of working-tree checkout per this discipline; iteration 2.1's detection-rule correction uses the same source-read pattern (inspect captured fixture bytes directly) before constructing the corrected rule.

### 5. Cross-references

- `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/` — the captured V0.1 fixture
- `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md` — fixture provenance + capture procedure + the LANC-vs-PAR1 finding
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2" §2 — the superseded 6-state rule (preserved intact for history)
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1" §1 — the superseded "PAR1 magic" wording (preserved intact for history)
- ADR-008 amendment (V0.2 at-rest extension lock-in) — sealed-framing magic (`0x01 0x00`) referenced in the corrected 6-state rule
- ADR-010 — V0.1 plaintext compensating controls; the `ALPHA_DO_NOT_STORE_REAL_DATA.txt` marker remains the marker-side detection signal (unchanged by this iteration)

---

## T0.2.0 Phase 2 — plan iteration 3 (drafted 2026-05-10, post-spike)

**Trigger.** Iteration 2.1 §3 raised one new open question: does lance 4.0 (lancedb 0.27.2 in workspace since Phase 0a) successfully read V0.1's lance 0.15 fragments? The question is a hard-blocker for the Phase 2 migration plan — step 3 reads V0.1 source via `LanceVectorStore::open(vector_dir, dimension)` then iterates rows. If lance 4.0 cannot read V0.1's fragments, Phase 2 must pick from one of three fallback strategies (bundle a lance 0.15 reader as dev-dep / V0.1-binary export to a neutral format / document-data-loss for V0.1 founder data). Iteration 3 runtime-confirms by writing and running a small spike against the Tier 2 fixture per `feedback_runtime_confirmation_after_web_spike.md`.

### 1. Spike PASS evidence (verbatim from spike run)

**Spike file:** `crates/vault-storage/examples/v0_1_lance_compat_spike.rs` (~190 LOC, deep-copies the checked-in fixture's `lance/` subdir to a tempdir before opening so the fixture is never mutated).

**Run command:** `cargo run --example v0_1_lance_compat_spike -p vault-storage --release`

**Run date:** 2026-05-10, against the Tier 2 fixture captured this session.

**Output (5 sequential checks, all PASS, exit 0):**

```
[1/5] LanceVectorStore::open(dim=384)         OK — store opened
[2/5] validate_readable() (ADR-018)           OK — full decode succeeded
[3/5] count(None)                              count = 5
[4/5] count(Some("default"))                   count(default) = 5
[5/5] search(top=1)                            hit: id=019e1212-564a-7ab1-a97d-52e1eba510ed
                                               cosine_distance=0.9944
PASS — lance 4.0 reads V0.1 (lance 0.15) fragments end-to-end.
```

**Why each check is load-bearing:**
- `LanceVectorStore::open` is the exact production call Phase 2 step 3 will make to read the V0.1 source. The spike doesn't shortcut to a lower-level `lancedb::connect` — it tests the call site that will appear in production code.
- `validate_readable()` is ADR-018's "minimum-cost end-to-end read that exercises data-decode" contract. NOT metadata-only. Per the 2026-04-30 LanceDB-corruption spike (`crates/vault-storage/examples/lance_corruption_spike.rs`), metadata + count can both succeed on a store whose fragment data is corrupted to unreadability. `validate_readable` exercises the full decode path against the row with smallest UUID — the strongest single signal that lance 4.0 actually decodes V0.1's fragment bytes.
- `count(None) = 5` matches the fixture's row count from its README. Wrong count would mean lance 4.0 opened metadata but mis-decoded fragment row counts.
- `count(Some(default)) = 5` confirms boundary-column decode. The fixture's `boundary` column is the V0.1 `Utf8` "default" string for all 5 rows; if lance 4.0 mis-decoded the column, the scoped count would be 0 or wrong.
- `search(top=1)` returns a real UUIDv7 (`019e1212-...`) with a sane cosine_distance (`0.9944` against an L2-normalised all-1/sqrt(384) probe). Wrong UUIDs or NaN distance would mean either id-column decode failure or embedding-column decode failure.

### 2. Implication for Phase 2 implementation

**The 3 alternative migration strategies in iteration 2.1 §3 are moot.** Phase 2 step 3's plan stands as written:

```rust
let source_store = LanceVectorStore::open(&v0_1_vector_dir, EMBEDDING_DIM).await?;
// → iterate rows via source_store's table.query().limit(N).execute().try_collect()
// → batch-write through StorageBackend::open_with_at_rest_key sealed path
// → atomic directory swap (Phase 2 step 7 per iteration 1 §1)
```

**The 6-state detection rule from iteration 2.1 §1 is the authoritative form** — its `LANC` magic at file END check (not PAR1 at file start) is what Phase 2 detector tests assert against.

**Tier 1 scaffolding strategy 1a (recommended, iteration 2.1 §2) is unblocked.** lance 4.0 can write `.lance` files via `lance::Dataset::write` or equivalent for synthetic-fixture detector tests — and since lance 4.0 reads V0.1 (lance 0.15) fragments end-to-end (this iteration's evidence), it will also read its own lance 4.0 output by transitivity. The Tier 1b fallback (raw-bytes-with-LANC-magic) is reserved for the unlikely event that lance 4.0's writer produces something its own reader rejects in this specific build configuration.

### 3. Spike file disposition (kept as executable documentation)

Per `feedback_spike_playbook_for_unknowns.md`, the spike stays in-tree as runtime-evidence. Phase 2 implementation does not delete it. Future-session readers can re-run the same command above to re-confirm the read-compat property if any of these change:
- lancedb / lance dep versions advance (e.g., lance 4.x → 5.x).
- The Tier 2 fixture is re-captured against a different V0.1 commit.
- A future Phase 2 refactor changes `LanceVectorStore::open`'s side-effects in a way that could affect the read path.

The spike's 5-check structure is also a model for the Tier 2 fixture-replay test — same call sequence, but inside a `#[tokio::test]` with `assert_eq!` instead of `eprintln!` + `process::exit`.

### 4. Schema invariant (load-bearing for Phase 2 row-iteration)

**Verified by spike:** the V0.1 schema `(id: Utf8, embedding: FixedSizeList<Float32, 384>, boundary: Utf8)` is byte-stable across V0.1 (lance 0.15) → V0.2 (lance 4.0). lance 4.0's reader decoded all three columns from V0.1 fragments without schema-coercion errors. Phase 2's batch-iteration code can use the V0.2 schema constant (`make_schema(384)` in `vector_store.rs`) directly against V0.1 source data.

This invariant is captured here because it is **not** explicitly named in the V0.1 schema's source-code site — it's implicit in the stable column types. If a future schema-evolution feature lands (e.g., a new optional `provenance` column), this invariant will need to be re-stated and the migration plan amended.

### 5. Discipline cross-references

- `feedback_runtime_confirmation_after_web_spike.md` — this iteration is the runtime confirmation. The web-research-level claim "Lance has documented backward-compat across V2 file format versions" (iteration 2.1 §3) is now empirically verified for the specific version pair (V0.1 lance 0.15 → V0.2 lance 4.0) on the Tier 2 fixture. Without this iteration, Phase 2 would have proceeded on a docs-claim alone and risked shipping a migration that fails on real V0.1 data.
- `feedback_quote_locked_artefacts_dont_paraphrase.md` — the spike's PASS-criteria sentence above quotes the spike file's check sequence verbatim rather than paraphrasing. Future iterations can re-run the spike file as ground truth instead of relying on this paragraph's wording.
- `feedback_spike_playbook_for_unknowns.md` — spike kept in-tree as executable documentation; not promoted to production code (`LanceVectorStore::open` already exists in production); not deleted after PASS.

### 6. Cross-references

- `crates/vault-storage/examples/v0_1_lance_compat_spike.rs` — the spike file (in working tree, bundles with Phase 2 first commit)
- `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/` — the Tier 2 fixture the spike runs against
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2.1" §3 — the open question this iteration resolves
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1" §4 — the three-tier fixture strategy now fully unblocked
- ADR-018 — `validate_readable` data-decode contract that the spike's check [2/5] exercises

---

## T0.2.0 Phase 2 — implementation milestone checkpoint (2026-05-10 session close)

This section captures session deliverables — what shipped, what holds at scaffolding, what next session inherits. Recorded here (rather than as a fresh "iteration 4" plan paragraph) because no new plan decisions were made; this session executed iteration 1 + 2 + 2.1 + 3 plans against locked contracts. Only floor-amendment surfacing was net-new.

### 1. Session deliverables (in dependency order)

1. **CI green confirmed on Phase 1 push (`9c3c24f`)** — run `25628476966` matrix-wide success across `[ubuntu-latest, windows-latest, macos-latest] × [build+test, clippy]` + fmt, completed 2026-05-10T13:22:47Z. Verified via `gh run view 25628476966` per CLAUDE.md per-commit CI standing rule before any new code lands.

2. **Iteration 3 spike SHIPPED + RUN** — `crates/vault-storage/examples/v0_1_lance_compat_spike.rs` (~190 LOC). Five sequential checks against the captured Tier 2 fixture: `LanceVectorStore::open(dim=384)` → `validate_readable()` (ADR-018 full data-decode) → `count(None)=5` → `count(Some(default))=5` → `search(top=1)` returning UUIDv7 `019e1212-564a-7ab1-a97d-52e1eba510ed` (cosine_distance=0.9944). Exit 0. **Resolves iteration 2.1 §3 OQ:** lance 4.0 reads V0.1 (lance 0.15) fragments end-to-end; the 3 alternative migration strategies (bundle-old-reader / V0.1-binary-export / document-data-loss) are all moot.

3. **Detector layer COMPLETE** — `crates/vault-storage/src/migration.rs` (~190 LOC):
   - `MigrationDetectorOutcome` enum (6 variants matching iteration 2.1 §1 verbatim).
   - `detect_v0_1_state(lance_data_dir: &Path) -> VaultResult<MigrationDetectorOutcome>` async impl. Metadata-only per ADR-018 spirit (file presence + magic-byte peeks via seek-based reads; never decodes Lance content). Helpers `file_ends_with` / `file_starts_with` keep the magic-byte checks size-bounded.
   - Exported via `vault-storage/src/lib.rs`.

4. **Detector tests COMPLETE — 7 tests, all PASS** (`crates/vault-storage/tests/migration_v0_1_to_sealed.rs`):
   - `detect_v0_1_shape_migrate` — marker present + LANC magic → V0_1ShapeMigrate
   - `detect_post_swap_marker_cleanup` — marker present + sealed framing → PostSwapMarkerCleanup
   - `detect_half_state_corruption_fail_closed` — marker present + empty → HalfStateCorruptionFailClosed
   - `detect_third_party_data_fail_closed` — marker absent + LANC magic → ThirdPartyDataFailClosed
   - `detect_v0_2_clean_no_op` — marker absent + sealed framing → V0_2CleanNoOp **[+1 amendment]**
   - `detect_first_run_install_no_op` — marker absent + empty → FirstRunInstallNoOp **[+1 amendment]**
   - `tier_2_real_v0_1_fixture_returns_v0_1_shape_migrate` — Tier 2 realism gate (captured V0.1 fixture from MSI commit `1d72aac`)

5. **Migration loop scaffolding HOLDS — 6 tests `#[ignore]`'d**:
   - Public surface stubbed in `migration.rs`: `MigrationOutcome` enum (`NoMigrationNeeded`, `Migrated { rows_migrated: u64 }`) + `migrate_v0_1_to_sealed_if_needed(vector_dir, dimension, at_rest_key)` returning `Err(VaultError::Storage("migration loop not yet implemented..."))`.
   - 6 tests in same test file, all `#[ignore = "scaffolding stub: impl lands in Phase 2 step-(b)"]`:
     - `migration_succeeds_on_v0_1_shape` — V0_1ShapeMigrate path
     - `migration_no_op_on_v0_2_clean` — V0_2CleanNoOp **[+1 amendment]**
     - `migration_no_op_on_first_run_install` — FirstRunInstallNoOp
     - `migration_no_op_on_post_swap_marker_cleanup` — PostSwapMarkerCleanup **[+1 amendment]**
     - `migration_fails_closed_on_half_state_corruption` — substring assertion on error message (so stub error doesn't trivially satisfy `is_err()`)
     - `migration_fails_closed_on_third_party_data` — same substring discipline
   - Run via `cargo test ... -- --ignored` to exercise the stub-driven failure mode (uniform 6/6).

6. **Scoped DoD verified after every milestone** — per per-step discipline:
   - `cargo test -p vault-storage --test migration_v0_1_to_sealed`: 7 passed, 0 failed, 6 ignored (final).
   - `cargo fmt -p vault-storage --check`: clean (after rustfmt auto-applies on each pass).
   - `cargo clippy -p vault-storage --tests -- -D warnings`: clean.

### 2. Floor amendments — surface for the Phase 2 first commit message

Iteration 1 + 2 forecast: 4 detector tests + 1 Tier-2 fixture-replay + 4 migration-loop tests + 3 cookie-recovery + 1 vault-tauri = +13 vault-storage + +1 vault-tauri = 14 total. (Iteration 2 §4's "+8 vs +9" arithmetic discrepancy resolved as +9 per its own test-name list.)

This session lands +13 (7 detector + 6 migration loop scaffolding ignored) toward the +13 vault-storage forecast, BUT:
- **+2 detector layer amendment** (`detect_v0_2_clean_no_op`, `detect_first_run_install_no_op`) — defense-in-depth pinning of all 6 named outcomes from iteration 2.1 §1, surfaced and approved before commit.
- **+2 migration loop layer amendment** (`migration_no_op_on_v0_2_clean`, `migration_no_op_on_post_swap_marker_cleanup`) — covers the 2 no-op outcomes iteration 2.1 §1 added to the 6-state rule that iteration 1 §5's pre-iteration-2.1 4-state framing didn't anticipate.

Net session count: 7 PASS detector tests + 6 IGNORED migration loop scaffolding tests = 13 tests across both layers, +4 over the iteration 1+2 forecast for these two layers. Cookie-recovery (3 tests) + vault-tauri dialog (1 test) stay unattempted; they land with migration loop impl + production wiring next session.

Both amendments surfaced + approved per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`.

### 3. Contract-class commitment (next-session opener)

**The migration loop implementation milestone (next session) MUST remove all 6 `#[ignore]` annotations on migration_v0_1_to_sealed.rs scaffolding tests as part of the impl deliverable.** After removal the tests run live — they MUST pass against the real `migrate_v0_1_to_sealed_if_needed` implementation. Ignore-removal IS the impl-trigger contract.

Concrete next-session sequence:

1. Confirm CI green on this session's commit before any new code lands (per CLAUDE.md per-commit CI standing rule).
2. Migration loop implementation: read-V0.1 → write-sealed → atomic-swap → cleanup loop per iteration 1 §1's 13-step list, with iteration 2 §2's cookie-file calibration baked in.
3. Cookie-recovery state machine + 3 tests per iteration 2 §4's named cases.
4. vault-tauri main.rs setup() step 5b wiring per iteration 1 §2 + dialog format helper + 1 vault-tauri test.
5. StorageBackend `open_with_at_rest_key` wiring per iteration 1 §3.
6. Workspace-level DoD gates (build / clippy / fmt / test --workspace).
7. Phase 2 commit milestone — atomic dir swap + Windows-rename ordering safety + Tier 3 founder smoke test on actual V0.1 vault before commit.

### 4. Cross-references

- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 1" §1 — migration loop spec
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2" §1-§4 — OQ resolutions + cookie-file calibration
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 2.1" §1 — 6-state detection rule (verbatim)
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 3" §1 — spike PASS evidence
- `feedback_admin_changes_ride_with_code.md` — bundling discipline for HANDOFF + fixtures + spike with first Phase 2 code commit
- `feedback_floor_forecast_is_pre_declaration_not_estimate.md` — both +2 amendments surfaced explicitly, neither silently absorbed
- `feedback_quote_locked_artefacts_dont_paraphrase.md` — outcome names match iteration 2.1 §1 verbatim across enum variants, test names, and docstrings

---

## T0.2.0 close-out plan iteration 4 — Phase 3 scope correction (drafted 2026-05-11, post-recon)

**Trigger.** Phase 3 pre-implementation recon (2026-05-11 session) source-read the plaintext-`open()` call graph and found iteration 1 §Phase 3's "Phase 2 was last caller" assumption was false — 10+ live plaintext callers remain across vector_store unit tests, migration test scaffolding, cascading.rs production + tests, vault-cli, vault-app adapter test, divergence test, and 2 spike examples. Per `feedback_flag_review_as_plan_amendment.md`, this is plan amendment, not silent correction. Iteration 4 retracts iteration 1 §Phase 3's scope assumption explicitly + locks the corrected scope. Same shape as iteration 2.1's PAR1→LANC detection-signal correction.

### 1. Retraction of iteration 1 §Phase 3 "Phase 2 was last caller" assumption

Iteration 1 §Phase 3 paragraph (HANDOFF.md line 764) stated: *"Plaintext `LanceVectorStore::open(path, dim)` deleted from `vector_store.rs` (Phase 2 was last caller)."* This is **false** as of 2026-05-11 Phase 3 recon.

**Falsified by:** call-graph source-read found 10+ live plaintext `LanceVectorStore::open` callers:
- `crates/vault-storage/src/cascading.rs:194` — production `CascadingStorage::open` plaintext path (sealed companion at line 234)
- `crates/vault-storage/src/migration.rs` — Phase 2 migration source-path reader
- `crates/vault-storage/tests/migration_v0_1_to_sealed.rs:352` — Phase 2's Tier 1 test scaffolding
- `crates/vault-storage/src/vector_store.rs:1039, 1047, 1060, 1099, 1141, 1160, 1162, 1171, 1181, 1191, 1193` — 11 unit tests
- `crates/vault-storage/examples/v0_1_lance_compat_spike.rs` — Phase 2 iteration-3 spike
- `crates/vault-storage/examples/lance_corruption_spike.rs` — ADR-018 historical-evidence spike

Plus plaintext `StorageBackend::open` callers:
- `crates/vault-cli/src/main.rs:227, 474` — live operator CLI (dead-letter triage + divergence check)
- `crates/vault-storage/src/cascading.rs:730, 761, 812, 1244, 1251` — 5 cascading tests
- `crates/vault-app/src/adapter.rs:389` — adapter test
- `crates/vault-storage/src/divergence.rs:360` — divergence test

**Why the assumption was wrong:** iteration 1 was drafted at Phase 0e (2026-05-09) when Phase 1 + 2 hadn't shipped yet. The "Phase 2 was last caller" framing assumed Phase 2 would migrate the unit-test surface alongside the production source-path. Phase 2 shortcut by leaving the test surface on plaintext + using plaintext `LanceVectorStore::open` for synthetic V0.1 fixture creation in `tests/migration_v0_1_to_sealed.rs` (Tier 1 scaffolding, vs iteration 2 §OQ #2 resolution's `RecordBatch+ArrowWriter` spec). vault-cli was never enumerated in iteration 1's Phase 3 scope.

**Phase 3 scope superseded by this iteration (iteration 4).** Iteration 1 §Phase 3 paragraph (HANDOFF.md line 764) is retracted as the authoritative Phase 3 specification; iteration 4 §2-§9 below is the authoritative form.

### 2. Deletion strategy locked — hard-delete plaintext from production surface; test surface migrates to sealed

- **Production plaintext deletion.** `LanceVectorStore::open(path, dim)` (`vector_store.rs`), `StorageBackend::open(...)` (`lib.rs`), and `CascadingStorage::open` plaintext companion (`cascading.rs:194`) are deleted from their source files. Sealed `open_with_at_rest_key` becomes the sole constructor across all three.
- **Test surface migrates to sealed.** 11 vector_store unit tests + 5 cascading tests + adapter.rs test + divergence.rs test flip from plaintext `open` → sealed `open_with_at_rest_key` with a `TEST_AT_REST_KEY` constant (mirrors `migration_v0_1_to_sealed.rs:486`'s existing test-key pattern).
- **Tier 1 migration test scaffolding** (`migration_v0_1_to_sealed.rs:352`) is restructured to construct V0.1-shape data directly via `arrow_array::RecordBatch` + `parquet::arrow::ArrowWriter` per iteration 2 §OQ #2 resolution Tier 1 spec. Methodology pre-declared in §5 below.
- **migration.rs source-path** restructure: either bypass Lance APIs via raw arrow/parquet read, OR keep a `#[cfg(feature = "v0_1_migration")]`-gated plaintext open callable ONLY by migration.rs. Methodology pre-declared in §4 below (compile-and-run spike required before lock).
- **Spike examples are excluded from deletion scope** (see §6 below).
- **Two distinct grep-tests assert absence (different scopes; not double-counted in §7 floor):**
  - **BRD §6 T0.2.0 acceptance suite criterion (d) grep-test** (per ADR-010 line 763 + BRD §6.2 line 1421 verbatim) asserts the **four ADR-010 control STRINGS** are absent — banner text, persistent-strip text, WARN message text (*"LanceDB data dir is plaintext (V0.1 alpha — see ADR-010). Encryption layer ships in T0.2.0."* per ADR-010 line 761), `ALPHA_DO_NOT_STORE_REAL_DATA.txt` filename string. Counted as part of the +5 acceptance suite in §7 (sub-task §9 (f)).
  - **Sub-task §9 (e) compensating-controls removal grep-test** asserts the **plaintext API SYMBOLS** are absent from production source — `LanceVectorStore::open` (without the `_with_at_rest_key` suffix) and `StorageBackend::open` (without the `_with_at_rest_key` suffix). This is a NEW assertion that iteration 4 §2 adds beyond BRD §6 T0.2.0 (a)-(e); it's the symbol-level companion to the strings-level test above. Counted as +1 in §7 (sub-task §9 (e)).
  - Both tests scoped to `crates/*/src/` ONLY (excluding `crates/*/examples/` per §6 disposition; excluding the §4 cfg-gated migration.rs path if spike outcome (ii) lands and the gate name string `v0_1_migration` is co-located).

### 3. vault-cli decision locked — option (a) migrate to sealed during Phase 3

**Decision audit trail (honest framing per `feedback_flag_review_as_plan_amendment.md`):**

ADR-010 wording does NOT name vault-cli. The four enumerated compensating controls (ADR-010 lines 759-762) are: (1) modal first-run banner (Tauri webview), (2) persistent UI banner (Tauri), (3) WARN log at plaintext LanceDB open (vault-storage), (4) `ALPHA_DO_NOT_STORE_REAL_DATA.txt` file (vault-storage). A defensible-but-aggressive reading of ADR-010's HARD GATE clause (*"No external user receives a build that contains the V0.1 plaintext-LanceDB code path"*, ADR-010 line 756) could have argued vault-cli is operator-only, not external-distributed, and therefore wording-compliant under a `#[cfg(test)] pub(crate)` gate.

**This iteration locks (a) on functional grounds, NOT on a strict reading of the four enumerated controls.** After T0.2.0 lands, on-disk vault data is sealed (`vault-sealed://` URLs, AEAD framing per ADR-008 amendment + Phase 0d production wiring). vault-cli's plaintext `StorageBackend::open` → cascading plaintext `LanceVectorStore::open` chain attempting to read AEAD-sealed bytes as plain Parquet means AEAD authentication failure / parse error / fail-closed on every invocation. Paths (b) `#[cfg(test)]` gate + (c) defer-to-V0.2.x are functionally "vault-cli is dead post-T0.2.0", not "vault-cli is deferred". The migrate-now decision is the only path that keeps vault-cli's operator surface alive across the V0.2 task arc (T0.2.2 consolidator through T0.2.13 sync).

**Second anchor — ADR-010's time-boundary clause (line 742):** *"The exception expires at the moment T0.2.0 lands; no further authorisation extends it."* This is the higher-order property that the four-controls enumeration instantiates. (a) honors this clause cleanly; (b)/(c) would silently violate it for the vault-cli production binary.

**Convergence as lock signal (per `feedback_functional_observation_trumps_wording_interpretation.md`):** Both anchors — functional (AEAD-auth-fail post-T0.2.0) AND time-boundary clause (ADR-010 line 742) — point to option (a) independently. Two interpretive frames converging on the same lock is itself the load-bearing evidence that makes this lock confident, not over-confident. If only one anchor had been named, a future reader could read the decision as fragile (resting on a single interpretive frame); the convergence makes it sturdy. Named explicitly here so the audit trail surfaces the convergence, not just the two anchors.

**Implementation shape (sub-task §9 (a)):** vault-cli adds `vault-app` workspace dep. Auth flow: call `vault_app::keychain::read_or_init_master_key(PRODUCTION_NAMESPACE, VAULT_ID)` — both consts co-located in `vault_app::keychain` (sub-task (a) decision lock 2026-05-11 moves `VAULT_ID = "default"` from vault-tauri main.rs:94 into `vault_app::keychain` alongside existing `PRODUCTION_NAMESPACE`; single source of truth across vault-tauri + vault-cli). Derive subkeys via `derive_sqlcipher_passphrase` + `derive_at_rest_key` (ADR-040 amendment v2 option β derivation tree). Open via `StorageBackend::open_with_at_rest_key`. **`rpassword` removed from vault-cli's `Cargo.toml`** — auth becomes implicit via OS-user keychain access; `windows-native-keyring-store` reads Windows Credential Manager transparently for the running OS user, with no separate keychain-unlock event to prompt for. The V0.1 prompt-for-passphrase UX is vestigial on the keychain model. Sub-task (a) decision lock option α (2026-05-11); matches vault-tauri's keychain-only auth pattern.

### 4. Sub-plan: migration.rs source-path restructure — methodology declared (spike required)

**Methodology: compile-and-run spike** per `feedback_runtime_confirmation_after_web_spike.md`. Web research alone is insufficient — iteration 3's spike confirmed lance 4.0 reads V0.1 fragments **via the Lance API**; the open question is whether the same fragments are readable via **raw arrow/parquet crates without going through Lance APIs**.

**Spike question:** Can `parquet::file::reader::SerializedFileReader` + `parquet::arrow::ArrowReader` (or equivalent) read the V0.1 fixture's `.lance` data files directly, treating them as Parquet? Lance's `.lance` files were Parquet-with-extensions in lance 0.x; lance 2.0+ introduced a "V2" format. Iteration 3 evidence shows lance 4.0 reads V0.1 via Lance, but says nothing about whether raw Parquet readers can decode V0.1 fragment bytes outside of Lance.

**Spike file:** `crates/vault-storage/examples/v0_1_raw_parquet_read_spike.rs` (new). Deep-copies the Tier 2 fixture's `lance/memories.lance/data/` subdir to a tempdir, opens the first `.lance` file via raw `parquet::arrow::ArrowReader`.

**Spike stages (acceptance criteria pre-declared per ADR-041 spike Stages A+B pattern — empirical pass/fail per stage, not heuristic):**

| Stage | Check | PASS criterion | FAIL implication |
|---|---|---|---|
| **A** | Compile `parquet::file::reader::SerializedFileReader::new(File::open(fixture_lance_data_file))` against workspace's pinned `parquet` crate version | Compiles cleanly; no API mismatch errors | API/crate-version incompatibility — outcome (ii) lands trivially (raw-Parquet path unbuildable in our workspace) |
| **B** | Runtime open succeeds; file metadata + row-group count readable | Open returns Ok; `metadata.num_row_groups() >= 1` | Open returns Err / unparseable file — lance fragment file is not a vanilla Parquet container; outcome (ii) |
| **C** | Schema decode + byte-equality against expected V0.1 schema | `parquet_to_arrow_schema(metadata, None)` returns a `Schema` with fields `[("id", Utf8, false), ("embedding", FixedSizeList<Float32, 384>, false), ("boundary", Utf8, false)]` in that order, with V0.1's actual nullability values | Any column type / order / nullability divergence — lance encodes schema differently than vanilla Parquet at the metadata layer; outcome (ii) |
| **D** | Row decode succeeds via `ArrowReader::next() -> Some(Ok(batch))` | Iterator yields 5 rows total across all batches; at least one row's `id` column decode returns a UUIDv7 from fixture's known set (e.g., `019e1212-564a-7ab1-a97d-52e1eba510ed` per iteration 3 §1) | Decode error / wrong row count / wrong `id` value — lance encodes fragment-level data differently than vanilla Parquet at the data-page layer; outcome (ii) |

**Outcome (i) lands ONLY if Stages A+B+C+D ALL PASS.** Any single stage FAIL lands outcome (ii). Spike exit code = 0 on full PASS, non-zero on first FAIL with which-stage-failed diagnostic to stderr.

**Two possible outcomes (informed by the staged results above):**
- **(i) Raw Parquet works (A+B+C+D all PASS):** Refactor `migration.rs` source-path read to bypass `LanceVectorStore::open` entirely, using the same `parquet::arrow::ArrowReader` API path the spike exercised. Plaintext `LanceVectorStore::open` deletion is unblocked across the production codebase (no cfg-gate retention needed).
- **(ii) Raw Parquet does NOT work (any stage FAIL):** Keep plaintext `LanceVectorStore::open` callable from migration.rs ONLY, via `#[cfg(feature = "v0_1_migration")] pub(crate)` gate. The gate is removed when V0.2.x migration-source-path code itself is removed (V0.2.x lifecycle question, not T0.2.0 scope). ADR-010's time-boundary clause is honored at the user-distribution surface — production binaries built without the `v0_1_migration` feature have NO plaintext callable; migration.rs is internal one-shot upgrade code with a documented sunset trigger.

**Spike disposition:** kept in-tree as executable documentation per `feedback_spike_playbook_for_unknowns.md`, regardless of outcome (i)/(ii). Same disposition pattern as iteration 3's `v0_1_lance_compat_spike.rs`.

**README evidence retraction + spike outcome lock (2026-05-11 post-spike amendment):** The compile-and-run methodology framed above is **superseded** by the fixture README's CRITICAL V0.1 file-format finding at `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md` lines 61-73 (pre-dates this iteration 4 §4 by 1 day):

> *"V0.1 lance 0.15 wrote `.lance` files using Lance's own binary format, NOT raw Parquet. Empirical inspection at fixture-capture time: `.lance` data files end with `4C 41 4E 43` = ASCII `"LANC"` (last 4 bytes); files do NOT start with PAR1 magic (`50 41 52 31`)."*

The spike was retained per option (B) lock 2026-05-11 as defensive evidence-doubling: a tiny byte-inspection spike (~150 LOC, no parquet/arrow deps; uses `std::fs` + `std::io::{Read, Seek}` only) at `crates/vault-storage/examples/v0_1_raw_parquet_read_spike.rs` empirically re-confirms README at runtime so future readers can re-run the proof locally. **Spike PASS verified 2026-05-11:** first 4 bytes of `505875b1-...lance` = `30 31 39 65` (NOT PAR1; bonus finding: prefix is the start of UUIDv7 `019e...` — Lance writes the row id near file start, confirming Lance-specific format), last 4 bytes = `4C 41 4E 43` (IS LANC magic). Exit code 0.

**Outcome (ii) empirically locked.** The `parquet::arrow::ArrowReader` refactor path (outcome i) is infeasible. Plaintext `LanceVectorStore::open` retained via `#[cfg(any(test, feature = "v0_1_migration"))] pub` gate (refined from §4's original `#[cfg(feature = ...)] pub(crate)`: (i) the `any(test, ...)` half keeps vault-storage's own internal tests compiling without needing the feature flag, while the `feature = ...` half exposes the gated callable to downstream consumers that opt in; (ii) `pub` (not `pub(crate)`) per the second-amendment retraction below).

> **Retraction of §4 original `pub(crate)` wording (2026-05-11 sub-task (b)+(c) DoD-gate iteration):** the first cargo-check after the source edits failed with E0624 at `crates/vault-retrieval/tests/common/mod.rs:103` + `crates/vault-retrieval/src/strategies/semantic.rs:354` (both `LanceVectorStore::open` callers from a separate crate's test code). iteration 4 §1's enumeration of "10+ live plaintext callers" missed these two — the four other recon-misses (iteration 1 §Phase 3, §2 CascadingStorage, §4 spike methodology, §5 arrow/parquet methodology) were caught upstream; this one surfaced only at cargo-check. Resolution: change `pub(crate)` → `pub` on plaintext `LanceVectorStore::open`. The feature flag (cfg-gate) is the architectural gate that controls EXISTENCE; `pub` vs `pub(crate)` only controls VISIBILITY once it exists. The "callable from migration.rs only" intent (§4 wording) is preserved at the user-distribution surface — vault-cli's binary excludes plaintext open via its per-package build without the feature. vault-retrieval activates the feature via dev-deps so its test builds compile against vault-storage with plaintext open available; sub-task (d) later migrates vault-retrieval's tests to sealed `open_with_at_rest_key` + `TEST_AT_REST_KEY`, at which point the dev-dep feature activation can drop. **Fifth recon-miss-retraction-with-evidence this session** per `feedback_retract_with_falsified_by_when_prior_iteration_wrong_about_future_scope.md` — the pattern is recurring + the discipline is firing consistently.

**Cascade scope expansion (sub-task (b) decision lock 2026-05-11 option α) — with iteration 4 §2 retraction:** iteration 4 §4 only enumerated the `LanceVectorStore::open` gate. But the production call chain in `crates/vault-storage/src/cascading.rs` shows `StorageBackend::open` (line 186) calls `LanceVectorStore::open(...)` internally at line 194 — `StorageBackend::open` fails to compile when the feature is off (referencing a non-existent gated symbol). Sub-task (b) gates both plaintext entry points symmetrically with `#[cfg(any(test, feature = "v0_1_migration"))]`. Plus `pub mod migration;` in `crates/vault-storage/src/lib.rs` becomes `#[cfg(feature = "v0_1_migration")] pub mod migration;` — the entire migration loop module compiles only when the feature is enabled.

> **Retraction of iteration 4 §2 (2026-05-11 sub-task (b) recon):** §2's "Production plaintext deletion" paragraph (in this iteration above) enumerated *three* plaintext APIs: `LanceVectorStore::open` (vector_store.rs), `StorageBackend::open` (lib.rs), and `CascadingStorage::open` plaintext companion (cascading.rs:194). Sub-task (b) recon falsified the third entry — **there is no `CascadingStorage` struct anywhere in vault-storage** (verified by grep). cascading.rs:194 is a body line INSIDE `StorageBackend::open` that calls `LanceVectorStore::open`, not a separate function. Plus the location for `StorageBackend::open` is `cascading.rs:186`, not `lib.rs` (which re-exports the struct but doesn't impl it). Correct two-API enumeration: (1) `LanceVectorStore::open` at `vector_store.rs:247`; (2) `StorageBackend::open` at `cascading.rs:186`. §2's three-API framing was paraphrase-drift from inferred struct naming, not source-read. Same retraction-with-evidence pattern as iteration 4 §1's retraction of iteration 1 §Phase 3 — `feedback_retract_with_falsified_by_when_prior_iteration_wrong_about_future_scope.md` discipline applied a third time this session.

Sub-tasks (c)/(d)/(e) follow up by deleting the gated-but-unreferenced `StorageBackend::open` plaintext companion once its test surface migrates to sealed. iteration 4 §2's "Production plaintext deletion" lock (corrected to two APIs per the retraction above) is preserved as the end-state — sub-task (b) just makes the intermediate state (during the c→d→e migrations) compile cleanly.

**Production binary asymmetry — ADR-010 honored asymmetrically per-binary:**
- **`vault-tauri/Cargo.toml`** enables the feature on its vault-storage dep (`vault-storage = { ..., features = ["v0_1_migration"] }`) so its V0.1→V0.2 migration startup path compiles. vault-tauri's binary CONTAINS plaintext `LanceVectorStore::open` BUT it is reachable only via `migration.rs`'s one-shot V0.1-source-read step at line 433 (which calls `LanceVectorStore::open(vector_dir, dimension).await?` before the sealed dest is created at line 437-438).
- **`vault-cli/Cargo.toml`** does NOT enable the feature. vault-cli never runs migration — sub-task (a) (2026-05-11) migrated it to the keychain-aware `StorageBackend::open_with_at_rest_key` path that opens already-sealed vaults. vault-cli's binary, built without the feature, has NO plaintext callable in its dep tree. ADR-010's "production binaries... have NO plaintext callable" property holds for vault-cli specifically.

**Discipline drift acknowledgment per `feedback_quote_locked_artefacts_dont_paraphrase.md`:** iteration 4 §4 should have quoted the fixture README's CRITICAL finding verbatim when drafting the spike methodology. Instead the original §4 text paraphrased general knowledge of Lance file format (*"Lance's `.lance` files were Parquet-with-extensions in lance 0.x; lance 2.0+ introduced a 'V2' format"*) — drifted from the locked artefact (README) that already documented the empirical truth. The amendment above quotes the README directly + names the drift explicitly. Same retraction pattern as iteration 4 §1's retraction of iteration 1 §Phase 3 — `feedback_retract_with_falsified_by_when_prior_iteration_wrong_about_future_scope.md` pinned this discipline earlier this session; this is its second test-firing in the same session, which is the corroborating signal that the pattern is real and recurring (not a one-off).

**Spike file retained in-tree** per §6 disposition + iteration 3 precedent — provides locally-runnable runtime evidence for any future reader doubting the README finding. The spike's byte-inspection approach (no parquet/arrow deps) means it'll keep working across future cargo-workspace dep updates without breakage.

### 5. Sub-plan: Tier 1 RecordBatch+ArrowWriter restructure — methodology declared (schema-verbatim-mirror)

**Methodology: canonical-source schema mirroring.** The Tier 1 synthetic V0.1-shape data construction must mirror the V0.1 binary's byte-shape exactly — column order, type widths, nullability, field metadata. Iteration 2 §OQ #2 resolution Tier 1 spec was right to call for `arrow_array::RecordBatch` + `parquet::arrow::ArrowWriter`; Phase 2's shortcut (using plaintext `LanceVectorStore::open` to write Tier 1 data) worked because Lance defined the byte-shape — but Lance is the same artifact we're trying to delete from the test surface.

**Schema source-of-truth:** Tier 2 fixture's existing `.lance` files (captured from V0.1 binary at commit `1d72aac`, 5 rows). Inspect schema at test setup time via `parquet::file::reader::SerializedFileReader` + `parquet::arrow::parquet_to_arrow_schema(...)`. Mirror the column types + order + nullability exactly into a `RecordBatch` constructed in Rust.

**Schema-equality assertion:** At Tier 1 test setup, after writing the synthetic RecordBatch to the temp data dir via `ArrowWriter`, the test re-reads the schema from disk and asserts `arrow_schema::Schema::PartialEq` against the schema read from the Tier 2 fixture. If schemas diverge silently, Tier 1 has drifted; the assertion fails loudly at test-setup before any migration logic exercises the synthetic data.

**Coupling to §4 spike:** if §4 outcome (i) lands (raw Parquet works for read), then the migration.rs source-path read uses the SAME `parquet::arrow::parquet_to_arrow_schema` decode path that Tier 1 §5 uses for schema-equality assertion — Tier 1 and migration.rs become a closed loop where each defends the other. If §4 outcome (ii) lands, Tier 1's schema-mirror still stands independently (it's Lance-API-agnostic).

**§5 methodology retraction + Tier 1→Tier 2 collapse (2026-05-11 post-spike, sub-task (b)+(c) bundled per option P4):** the arrow/parquet methodology framed above is **falsified by the same README evidence that invalidated §4** — V0.1's `.lance` files are NOT Parquet (per fixture README lines 61-73, empirically re-confirmed by the §4 spike's runtime byte inspection). Writing Tier 1 data via `parquet::arrow::ArrowWriter` would produce Parquet, which is not V0.1-shape; migration code that expects Lance binary format would fail on Parquet input. Reading the Tier 2 fixture's schema via `parquet::file::reader::SerializedFileReader` also fails because the fixture file isn't a Parquet container.

> **Retraction of iteration 2 §OQ #2 resolution + iteration 4 §5 (2026-05-11 sub-task (b)+(c) recon):** iteration 2 §OQ #2 resolution's Tier 1 spec ("scaffolding-based migration test... synthetic V0.1-shape data dir... Parquet files via `arrow_array::RecordBatch` + `parquet::arrow::ArrowWriter`") was built on the same false "V0.1 = Parquet" premise that drifted into iteration 4 §4 + §5. The fixture README at `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/README.md` (committed in `e27e6dc` 2026-05-11) documented the empirical truth before iteration 4 was drafted; both §4 and §5 paraphrased from inferred Lance-format knowledge instead of reading the canonical artefact. **Third retraction-with-evidence this session** per `feedback_retract_with_falsified_by_when_prior_iteration_wrong_about_future_scope.md` — pattern is real + recurring.

**Locked methodology (option P4):** **collapse Tier 1 into Tier 2.** The existing Tier 2 fixture at `crates/vault-storage/tests/fixtures/v0_1_alpha_data_dir/lance/` IS real V0.1-binary-emitted data (5 rows, captured via UI procedure at `1d72aac`, ALPHA marker + 5 `.lance` data files + manifests + transactions). Sub-task (c) refactors `tests/migration_v0_1_to_sealed.rs`'s `create_v0_1_shape_data(dir: &Path)` helper at line 351 to deep-copy the Tier 2 fixture's `lance/` subdir contents into `dir`, replacing the current plaintext `LanceVectorStore::open` call. Same external contract (dir contains V0.1-shape data on return); different mechanism (fixture copy, not Lance write).

**Why the collapse is correct:** Tier 1 was distinguished from Tier 2 in iteration 2 §OQ #2 as "regression catch (synthetic, runs every commit on every CI matrix OS)" vs Tier 2 as "realism gate (checked-in)". With the fixture committed in `e27e6dc`, Tier 2 IS now "runs every commit on every CI matrix OS" — the distinction collapses. There's no remaining purpose for synthetic V0.1 manufacturing once the real V0.1 capture is in-tree. Iteration 2's three-tier framing (Tier 1 synthetic + Tier 2 fixture + Tier 3 founder smoke) reduces to two tiers (Tier 2 fixture + Tier 3 founder smoke); Tier 3 was already skipped per session-end-2 reasoning (no V0.1 production vault on dev machine).

**No arrow/parquet dev-deps needed.** The fixture deep-copy uses `std::fs` for file I/O — no new crate deps. The cargo-resolution drift in Cargo.lock (rpassword + rtoolbox removed in sub-task (a)) remains the only Cargo.lock change bundled with this commit; no further dep-tree expansion.

**Test floor adjustment:** iteration 4 §7 floor row for sub-task (c) was "0 (one restructured test, same name `tier_1_synthetic_v0_1_migration_round_trip`)". Under P4 the helper is restructured, not a single test; ~12 tests in the file continue calling the refactored helper. Net 0 test count change preserved. iteration 4 §7's Pre-declared total still holds at +8 firm.

### 6. Spike examples — explicit exclusion from Phase 3 deletion scope

Per iteration 3 §3 disposition (HANDOFF.md line 1543-1550): spike files are **executable documentation** + **audit-trail artifacts** that prove the runtime-confirmation that locked iteration 2.1 → iteration 3 → Phase 2 implementation. Deleting them or migrating them off plaintext would lose their evidential value.

**Phase 3 disposition for the 3 in-tree spike examples:**

| Spike file | Phase 3 action |
|---|---|
| `examples/v0_1_lance_compat_spike.rs` | Kept verbatim. Calls plaintext `LanceVectorStore::open` against the Tier 2 V0.1 fixture — that's the entire point of the iteration 3 runtime-confirmation. |
| `examples/lance_corruption_spike.rs` | Kept verbatim. Historical evidence for ADR-018 `validate_readable` design (metadata succeeds + count succeeds on a corrupted store; full decode fails). |
| `examples/v0_1_raw_parquet_read_spike.rs` (new, §4) | Kept verbatim post-spike, same disposition pattern. |

**Compilation gating:** If §4 lands outcome (ii) (`#[cfg(feature = "v0_1_migration")]` on plaintext `LanceVectorStore::open`), the spike examples either inherit the same gate at the example-binary `[features]` declaration in `crates/vault-storage/Cargo.toml`, OR plaintext `LanceVectorStore::open` is exposed via `#[cfg(any(test, feature = "v0_1_migration"))]` to keep examples buildable under the default workspace feature set. Gate granularity decided at Phase 3 implementation milestone, not pre-locked here.

**Cross-link this disposition into BOTH grep-tests defined in §2:** the BRD criterion (d) four-control-strings test AND the §9 (e) plaintext-API-symbols test must both scope their scans to `crates/*/src/` ONLY (excluding `crates/*/examples/`) so neither absence-check fails on the spike examples (which intentionally call plaintext `LanceVectorStore::open` and may reference ADR-010 control strings in their explanatory doc-comments).

### 7. Floor pre-declaration (Phase 3 test count)

Per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`, the floor is a pre-declaration, not an estimate. Net positive surfaces as floor amendment in commit message **before** the commit lands.

| Sub-task | Net test delta |
|---|---|
| §9 (a) vault-cli migration to sealed | +2 firm (variable narrowed by sub-task (a) recon + decision lock 2026-05-11: sealed-open success + keychain-missing fail-closed with generic auth-failed message per BRD §11.7.2). Optional wrong-at-rest-key test subsumed by §9 (f) criterion (c). |
| §9 (b)+(c) bundled (P4 lock 2026-05-11): plaintext cfg-gate cascade + Tier 1→Tier 2 collapse helper refactor | 0 (cfg-gate is shape-change; helper restructure is single-helper, ~12 tests adapt for free) |
| §9 (d) 11 vector_store + 5 cascading + adapter + divergence unit-test migrations to sealed | 0 (migrations, not additions) |
| §9 (e) Compensating-controls removal sweep — **plaintext API SYMBOLS absent** grep-test (scope: `LanceVectorStore::open` + `StorageBackend::open` symbols absent from `crates/*/src/`; distinct from criterion (d)'s four-control-STRINGS test below) | +1 |
| §9 (f) Phase 3 BRD §6 T0.2.0 acceptance suite (a)-(e) — INCLUDES criterion (d)'s four-control-STRINGS grep-test (banner text + persistent-strip text + WARN message text + ALPHA filename) | +5 |
| §4 raw-Parquet spike | +0 (spike is an example binary, not a test) |
| **Pre-declared total** | **+8 net new tests** (variable narrowed to firm by sub-task (a) recon + decision lock 2026-05-11): +6 deterministic [+1 sub-task (e) plaintext-symbols grep-test + +5 sub-task (f) acceptance suite including criterion (d) four-controls-strings grep-test] + 2 firm for vault-cli sub-task (a). Original pre-declaration was +8 to +9 (variable range); narrowing down (NOT breaching) per `feedback_floor_forecast_is_pre_declaration_not_estimate.md` is in-discipline. |

Any breach beyond +9 surfaces in commit message per discipline before the breaching commit lands.

### 8. Iteration depth pre-declaration

Per `feedback_plan_iteration_depth_scales_with_design_surface.md`, contract-establishing tasks warrant 2-3 iterations. Phase 3 is now contract-establishing (the contract being "what plaintext API surface exists after T0.2.0 closes"), not the "mechanical work" iteration 1 framed.

- **Iteration 4 (this)** — Phase 3 plan amendment, contract-establishing. Locks §2 deletion strategy, §3 vault-cli decision, §4-§5 sub-plan methodologies, §6 spike-disposition, §7 floor pre-declaration, §9 sub-task enumeration.
- **Iteration 5 RESERVED** — fires only if scope items surface from §4 spike findings or §5 schema-mirroring inspection that iteration 4's locks cannot accommodate. Example triggers: §4 spike outcome reveals raw Parquet works for V0.1 but lance 4.0 writes a non-Parquet `.lance` extension that breaks the migration source-path differently than §4's two anticipated outcomes; §5 schema-mirror inspection reveals a V0.1-binary field that the workspace's current arrow/parquet crate versions cannot represent without lossy coercion.
- **Iteration 6 RESERVED** — fires only on §4 spike empirical findings if neither (i) nor (ii) is realizable. Triple-iteration reservation matches the contract-establishing-task pattern.

### 9. Phase 3 sub-task enumeration (multi-session arc, not single-session)

Phase 3 is no longer "mechanical work" as iteration 1 framed it. Six named sub-tasks with deliverables-per-sub-task + dependencies + test floor:

| Sub-task | Deliverable | Test floor | Depends on |
|---|---|---|---|
| **(a) vault-cli migration to sealed** | `vault-cli/src/main.rs`: (1) replaces `read_passphrase` + `open_backend(cli, key)` with keychain-aware `open_backend(cli)` that calls `vault_app::keychain::read_or_init_master_key(PRODUCTION_NAMESPACE, VAULT_ID)` → `derive_sqlcipher_passphrase` + `derive_at_rest_key` → `StorageBackend::open_with_at_rest_key`; (2) `make_backend` test helper migrates to `open_with_at_rest_key` with `TEST_AT_REST_KEY` const (single-helper migration covers all 10 vault-cli integration tests for free per sub-task (a) decision lock — folded into (a), not (d)). `Cargo.toml`: adds `vault-app` workspace dep; removes `rpassword` (vestigial on Windows keychain model). `VAULT_ID = "default"` const co-located in `vault_app::keychain` (moved from vault-tauri main.rs:94). | +2 firm (sealed-open success + keychain-missing fail-closed with generic auth-failed message per BRD §11.7.2; sub-task (a) decision lock 2026-05-11 narrowed the +2-3 variable). Optional wrong-at-rest-key test subsumed by sub-task (f) criterion (c). | Phase 1 (shipped); independent of (b)-(f) |
| **(b)+(c) plaintext cfg-gate + Tier 1→Tier 2 collapse — P4 bundle, locked 2026-05-11** | (1) `crates/vault-storage/Cargo.toml` adds `[features] v0_1_migration = []`. (2) `vector_store.rs:247` plaintext `LanceVectorStore::open`: `#[cfg(any(test, feature = "v0_1_migration"))] pub(crate)` gate. (3) `cascading.rs:186` plaintext `StorageBackend::open`: same gate (cascade per option α). (4) `lib.rs` migration mod decl: `#[cfg(feature = "v0_1_migration")] pub mod migration;`. (5) `vault-tauri/Cargo.toml` enables feature on vault-storage dep (vault-cli does NOT — its per-package build excludes plaintext open from the binary). (6) `tests/migration_v0_1_to_sealed.rs` `create_v0_1_shape_data(dir)` helper at line 351 refactored to deep-copy Tier 2 fixture's `lance/` subdir into `dir` (P4 Tier 1→Tier 2 collapse per §5 amendment). (7) Spike `examples/v0_1_raw_parquet_read_spike.rs` retained in-tree per §6 (ran 2026-05-11, PASS exit 0). | 0 (cfg-gate is shape-change; helper restructure is single-helper, ~12 callers adapt for free; spike is example binary, not a `#[test]`) | (a) shipped; Tier 2 fixture committed in (a) at `e27e6dc` |
| **(d) Unit-test surface migration to sealed** | 11 vector_store unit tests (`vector_store.rs:1039, 1047, 1060, 1099, 1141, 1160, 1162, 1171, 1181, 1191, 1193`) + 5 cascading tests (`cascading.rs:730, 761, 812, 1244, 1251`) + `adapter.rs:389` + `divergence.rs:360` flip from plaintext open → sealed `open_with_at_rest_key` with `TEST_AT_REST_KEY` constant. | 0 (migrations) | (b) outcome determines whether plaintext open is reachable from test code at all |
| **(e) Compensating-controls removal sweep** | (1) Modal banner removed from `vault-tauri` Tauri command + dist/index.html. (2) Persistent strip removed from `vault-tauri` dist/index.html. (3) WARN log at plaintext LanceDB open removed alongside the plaintext path itself (the WARN site lives inside the to-be-deleted plaintext `open()`). (4) ALPHA file deletion-on-first-T0.2.0-run lands with one-time INFO log per ADR-010 line 763 removal-trigger spec. **Plus new plaintext-API-SYMBOLS grep-test** per §2 + §7: asserts `LanceVectorStore::open` + `StorageBackend::open` symbols absent from `crates/*/src/`. | +1 (plaintext-API-symbols grep-test, distinct from sub-task (f)'s criterion (d) four-control-STRINGS test) | (a)+(b)+(c)+(d) — production plaintext open must be deleted-able before WARN-emitting site can be removed |
| **(f) Phase 3 acceptance suite (BRD §6 T0.2.0 a-e)** | (a) no plaintext on disk after write/close (entropy ≥ 7.9 + zero PAR1 magic — extends Phase 0d's `sealed_open_writes_framing_bytes_to_disk` to top-level integration). (b) round-trip identity encrypt → decrypt == original on CI matrix `[ubuntu-latest, windows-latest, macos-latest]`. (c) wrong key fails closed (extends Phase 0d's `sealed_open_with_wrong_key_fails_closed`). (d) four ADR-010 control STRINGS absent grep-test (banner text + persistent-strip text + WARN message text + ALPHA filename; distinct from sub-task (e)'s plaintext-API-symbols grep-test). (e) tampered ciphertext returns Err with AEAD authentication message — bit-flip a sealed byte, assert AEAD-auth message in returned VaultError. | +5 (one test per criterion (a)-(e); no double-count with sub-task (e)'s plaintext-symbols grep-test since scopes are distinct) | All preceding sub-tasks complete |

**Sequencing locked (P4 amendment 2026-05-11):** (a) → (b)+(c) bundled → (d) → (e) → (f). Multi-session arc; no commit until a sub-task is independently DoD-green. **Sub-task (a) shipped at `e27e6dc` 2026-05-11** carrying the session-end-2 admin bundle (`.gitignore` negation + Tier 2 fixture binaries `vault.db` + `vault.db-wal` + HANDOFF.md session-end-2 checkpoint + iteration 4 §1-§10 initial draft) per `feedback_admin_changes_ride_with_code.md` — first code commit after those changes were generated. CI red on `ac577f4` (windows-latest fixture-missing) resolved at sub-task (a)'s push (CI run `25678902497` in flight at the time of this amendment). **Sub-task (b)+(c) bundles** Cargo.lock drift from (a) + iteration 4 §4/§5/§9 amendments + sub-task (b)+(c) source/config changes in one commit per P4 reasoning — eliminates the intermediate state where the integration test would have silently skipped without the feature flag (avoids the silent-failure pattern).

### 10. Cross-references

- ADR-010 (HANDOFF_V0.1_ARCHIVE.md:737) — hard-gate text quoted verbatim in §3.
- BRD §6.2 T0.2.0 (`Agent_Build_Specification.txt:1411-1423`) — acceptance criteria + HARD GATE clause quoted verbatim in §2 + §7.
- ADR-040 + amendment v2 (HANDOFF.md:938 + :1019 + this file :471) — keychain wiring + master_key derivation tree consumed by §9 (a).
- HANDOFF.md "T0.2.0 Phase 2 — plan iteration 3" §3 — spike-disposition discipline that §6 inherits.
- HANDOFF.md "T0.2.0 close-out plan iteration 1" §Phase 3 paragraph (line 764) — **retracted by §1 above**.
- HANDOFF.md "T0.2.0 close-out plan iteration 2" §OQ #2 resolution — three-tier fixture strategy that §5 honors.
- `feedback_flag_review_as_plan_amendment.md` — discipline that produced this iteration's existence.
- `feedback_runtime_confirmation_after_web_spike.md` — discipline that produced §4 spike methodology declaration.
- `feedback_floor_forecast_is_pre_declaration_not_estimate.md` — discipline that produced §7 floor pre-declaration.
- `feedback_plan_iteration_depth_scales_with_design_surface.md` — discipline that produced §8 iteration depth pre-declaration.
- `feedback_admin_changes_ride_with_code.md` — bundling discipline that locks §9's session-end-2 admin ride-along.
- `feedback_spike_playbook_for_unknowns.md` — discipline that produced §6's spike-disposition.
- `feedback_quote_locked_artefacts_dont_paraphrase.md` — all ADR-010 / BRD §6.2 phrases in this iteration are quoted verbatim, not paraphrased.

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
| **ADR-039** | Hard-delete API for lance 4.0 tombstoning (Phase 0b production fix + Phase 0c amendment) | **HARD GATE CLEARED — initial implementation Phase 0b commit `2d3c57a`, amended Phase 0c (Compact+Prune)**. Implementation (post-amendment): `LanceVectorStore::delete()` now calls `Table::optimize(OptimizeAction::Compact { options: CompactionOptions::default(), remap_options: None })` followed by `Table::optimize(OptimizeAction::Prune { older_than: zero, delete_unverified: true, error_if_tagged_old_versions: false })` after `table.delete()`, holding the ADR-038 upsert mutex throughout. **Phase 0c amendment reason:** spike Stage E 2×2 diagnostic discovered Phase 0b's Prune-alone implementation was insufficient for partial-fragment deletes (`OptimizeStats { compaction: None, prune: { data_files_removed: 0 } }` — encrypted bytes of deleted rows survive on disk bit-for-bit). Compact rewrites partial fragments dropping tombstoned rows; Prune-after-Compact removes the orphaned original (`data_files_removed: 1` empirically). The Phase 0b regression test passed for the wrong reason — its full-boundary delete pattern triggered fragment-empty special-case, but Memory Vault's actual single-id-delete API hits partial-fragment which Prune-alone leaves untouched. Trade-off (lose time-travel undo) preserved. Regression pins: `delete_physically_removes_content_per_adr_039` (full-fragment) AND `delete_partial_fragment_physically_removes_content_per_adr_039` (partial-fragment, content-hash-set-difference assertion via BLAKE3 — would fail under Prune-alone). Full ADR text drafted at Phase 0e alongside ADR-037 + ADR-008 amendment. |
| **ADR-041 (TBD)** | V0.1 VAULT_KEY → V0.2 keychain SQLCipher passphrase bridge (discovered 2026-05-11 during Phase 2 Tier 3 prep) | **HARD GATE before T0.2.14 (alpha cohort distribution opens) — blocks Tier 3 founder smoke today.** Phase 1's `read_or_init_master_key` doesn't bridge V0.1's VAULT_KEY env-var-derived SQLCipher passphrase → real V0.1 vault would fail at MetadataStore::open after Phase 2 LanceDB migration. Required: ADR-041 text + `read_or_init_master_key` extension (VAULT_KEY env-var fallback when no keychain entry exists, open V0.1 metadata, generate new master_key, persist to keychain, `PRAGMA rekey` to re-encrypt with new passphrase, drop VAULT_KEY support after) + integration test exercising V0.1 fixture → V0.2 sealed end-to-end (SQLCipher rekey + LanceDB migration + sealed re-open + UI verifies memories). See "Open tech-debt" entry above for full deliverables list. |

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
- **ADR-013** — `chrono` pin advanced 0.4.38 → 0.4.39 (T0.1.9 Phase 1, 2026-05-01) → 0.4.44 (T0.2.0 Phase 0a, 2026-05-07 — ADR-037 trigger 1 fired: `arrow-arith 57.2.0` resolved the `ChronoDateExt::quarter()` collision). Pin form (`=0.4.44`) retained per ADR-013 monthly-CVE-check discipline; archive entry unchanged per archive-frozen convention
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

- **V0.1 VAULT_KEY → V0.2 keychain SQLCipher passphrase bridge (Phase 1 follow-on, HARD GATE before T0.2.14)** — discovered 2026-05-11 during Phase 2 Tier 3 founder-smoke prep. Phase 1's `read_or_init_master_key` (`crates/vault-app/src/keychain.rs:97`) generates a new random master_key on first launch when no keychain entry exists — does NOT bridge from V0.1's `VAULT_KEY` env-var-derived SQLCipher passphrase. Result: Phase 2 binary against a real V0.1 vault would succeed at LanceDB migration (plaintext, no key needed) but fail at `Application::new`'s `MetadataStore::open` because the new keychain-derived SQLCipher passphrase doesn't match V0.1's VAULT_KEY-derived passphrase. Phase 1 integration smoke at `crates/vault-app/tests/integration_smoke.rs:146` explicitly punted ("Phase 2/3 wire actual consumption..."). **Required deliverables before alpha cohort opens:** (1) ADR-041 documenting the V0.1→V0.2 SQLCipher passphrase bridge contract; (2) `read_or_init_master_key` extension to detect existing V0.1 SQLCipher metadata file + read VAULT_KEY env if set + open metadata with V0.1-derived passphrase + generate new master_key + persist to keychain + re-encrypt metadata via SQLCipher's `PRAGMA rekey` + drop VAULT_KEY support after success; (3) integration test against captured V0.1 fixture exercising the full V0.1→V0.2 chain (SQLCipher rekey + LanceDB migration + sealed re-open + UI verifies memories searchable). **Blocks Tier 3 founder smoke** (per Phase 2 plan iteration 1 §4) — Tier 3 cannot proceed against any real V0.1 vault until this bridge ships. Phase 2 LanceDB migration ships in `[Phase 2 commit hash TBD]`; SQLCipher bridge is the missing Phase 1 piece for full V0.1→V0.2 user-data continuity.
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
- **Parallel cargo on 16 GB Windows = OOM corruption risk** (surfaced 2026-05-10, T0.2.0 Phase 1 DoD-gate session) — two concurrent `cargo build --workspace --all-targets` invocations on the founder's 16-CPU / 16 GB Windows dev box exhausted the paging file mid-link, corrupting shared workspace dep artifacts (`libtokio`, `libserde_json`, `libtracing`, `libtracing_subscriber`, `libvault_storage`, `libproptest`, `librpassword`) with `LNK1201` + cascading `E0786 ... os error 1455 paging file too small`. Recovery: surgical `cargo clean -p` of the corrupted set per `feedback_surgical_cargo_clean_first.md` (full clean is escalation, not first move). **Discipline:** before every cargo invocation, check `Get-Process | Where-Object { $_.ProcessName -in @('cargo','rustc','link') }` returns empty — not "is the previous output file growing." Silent-backgrounding-without-output is the failure mode that masked the still-running first build; bytes-arriving in the wrapper file is NOT evidence of completion when stdout is piped through `Out-Null`. Single-invocation discipline is the only safe path on this hardware.
- **`vault-embedding test_7_spawn_blocking_does_not_starve_reactor` flakes under workspace-wide concurrent test load** (surfaced 2026-05-10, T0.2.0 Phase 2 commit DoD). Asserts `tokio::sleep` wakes within ~50ms; observed `250.3267ms` during `cargo test --workspace --no-fail-fast` under 300+ parallel tests on the founder's 16-CPU Windows dev box. Isolation re-run (`cargo test -p vault-embedding --test embedding_tests`) PASSED at exit code 0 — same test, same machine, no other changes. **Not a regression from migration work** — vault-embedding source-graph untouched by Phase 2; the failure is OS-scheduler delay under contention, not a real reactor-starvation defect. CI matrix has been green historically (more controlled environment); CI is the canonical test gate per the standing rule. Revisit at the vault-embedding stability pass — candidates: widen the threshold to bound for concurrent-load contention (e.g., 50ms → 500ms with documented headroom rationale) OR restructure to a non-timing-sensitive assertion via `tokio::time::pause` mocking + manual time advance. **Out of scope for T0.2.0.**
- ~~**Phase 0b finding: lance 4.0 delete-tombstoning vs Memory Vault privacy property**~~ — **CLOSED 2026-05-07 (ADR-039 Prune-alone) → AMENDED 2026-05-08 (Phase 0c spike Stage E discovery: Prune-alone insufficient for partial-fragment deletes).** **Final resolution:** `LanceVectorStore::delete()` calls `Compact` then `Prune-with-zero-retention` after `table.delete()`, holding the ADR-038 upsert mutex throughout. Phase 0b's Prune-alone implementation was insufficient because Memory Vault's actual delete API is single-memory-id (partial-fragment) — Lance's `OptimizeAction::Prune` reports `data_files_removed: 0` for that case (verified empirically by spike Stage E 2×2 on both plain file:// and vault-sealed://). The Phase 0b regression test passed for the wrong reason (full-boundary delete = empty fragment = special case); Phase 0c added a partial-fragment companion (`delete_partial_fragment_physically_removes_content_per_adr_039`) using BLAKE3 content-hash-set-difference assertion. Trade-off (lose lance time-travel undo) preserved. See ADR-039 amendment block above for the full Phase 0c discovery + reasoning. **Beta cohort can now receive builds claiming "permanent deletion" — privacy contract preserved with the corrected Compact+Prune sequence.**

**Closed in this commit (T0.2.0 Phase 0a-fix):**
- ~~**Build env vars need persistent home** — `.cargo/config.toml` `[env]` block or `scripts/dev-build.sh` helper~~ — Closed 2026-05-07: `.cargo/config.toml` landed with `[env]` block setting `LANCE_MEM_POOL_SIZE = { value = "268435456", force = true }` (per ADR-038). The persistent-home pattern is now available for any future Lance/datafusion env-knob; further entries can be added as needed.

---

## Competitive intelligence — MemPalace (April 2026)

Open-source AI memory system shipped April 2026 by Milla Jovovich + Ben Sigman. ~47K GitHub stars at time of analysis. Python + ChromaDB + SQLite stack. MIT-licensed. Public benchmark scores: 96.6% R@5 LongMemEval raw mode, 98.4% R@5 held-out 450 questions (clean, generalisable), 100% with LLM rerank (contaminated — three specific failures fixed by inspection).

Validates Memory Vault's verbatim-memory architecture choice: raw verbatim storage + good embeddings beats LLM-based fact-extraction approaches (Mem0, Mastra) on retrieval recall. They're on the same side of that finding as we are.

Where they're meaningfully behind us — the differentiation that matters: no encryption at rest, no hard-delete physical removal (their `delete()` tombstones with default 7-day retention; ours with ADR-039 prunes immediately), no boundary-as-access-control primitive, no concurrent-write safety beyond a single-process Python runtime, MCP write tools without input sanitization. They run locally; they don't enforce a privacy contract.

Where they're ahead today: shipped + viral; coherent UX metaphor (wings/rooms/halls/closets/drawers); 19 MCP tools with on-demand tiered loading (~170 tokens always-on identity layer); auto-save hooks for Claude Code; specialist-agents-with-diaries pattern.

### Forward-pointer items from the gap analysis (candidates, not commitments)

1. **LongMemEval benchmark run on Memory Vault** `[BRD-candidate]` — public benchmark (`xiaowu0162/longmemeval-cleaned` on HuggingFace), MemPalace's runner is open-source, scoring well gives us a defensible competitive number. Estimate: bge-small-en-v1.5 + V0.1 retrieval likely ~96-97% raw, ~97-99% with V0.2 T0.2.7 multi-strategy retrieval. Establish train/dev/held-out splits BEFORE iterating on retrieval improvements per MemPalace's contamination cautionary tale. V0.2 work, before T0.2.16 beta onboarding for marketing/positioning use.

2. **MCP read-side additions** `[tech-debt-candidate]` — `vault_status` (vault overview + boundary list + retrieval policy summary), `vault_check_duplicate` (pre-write duplicate detection), `vault_list_boundaries` (lets agents see granted scopes), `vault_traverse_graph` (BFS over vault-graph relationships from a seed memory). Small surface additions, high leverage for agent UX. V0.2 scope candidate.

3. **Tiered always-loaded layer (L0/L1 equivalent)** `[ADR-candidate]` — ~170 tokens of user-identity context returned alongside any retrieval call, populated from a small pinned-importance subset of memories. Lets agents self-orient on first-message without bloating system prompts. V0.2 or V1.0 ADR.

4. **Claude Code auto-save hooks** `[tech-debt-candidate]` — bundle with T0.2.14 alpha distribution. MemPalace's `mempal_save_hook.sh` (every N messages) and `mempal_precompact_hook.sh` (emergency save before context compression) are good prior art. Lets early users dogfood Memory Vault without manual save commands.

5. **Privacy-primitive MCP tools** `[ADR-candidate]` — `vault_force_compact` (exposes ADR-039 prune-on-demand), `vault_revoke_agent_access` (boundary-scoped access revocation), `vault_audit_log` (surface BLAKE3 audit chain via MCP). These are *the* differentiator surfaced as agent capabilities — agents act as the user's enforcer of their own privacy contract, not just consumer of memory. Worth its own ADR for V0.2.

6. **Boundary-correctness + delete-durability benchmarks** `[BRD-candidate]` — alongside LongMemEval scores, publish benchmarks testing dimensions LongMemEval misses: boundary-correctness (questions paired with boundary scope, scoring whether retrieval correctly excludes out-of-scope memories), delete-durability (write distinctive markers, delete, run forensic recovery, score zero-recoverability). Differentiators we can publish credibly because no competitor implements the underlying primitives.

7. **Temporal validity / `kg_invalidate`** `[tech-debt-candidate]` — vault-graph schema extension for temporal-validity windows on knowledge-graph triples. Real feature for a memory system that watches user life evolve over months ("Kai works_on Orion" valid 2025-06-01 to 2026-03-01). MemPalace has it as a flat triple store; our vault-graph (DuckDB-backed multi-hop) gives a stronger substrate. V0.2 or V1.0 schema work.

### Things to NOT do (lessons from MemPalace)

- **No AAAK-style "lossy compression dialect" framed as lossless.** Their AAAK regresses LongMemEval from 96.6% to 84.2% — public credibility cost. If we ever build compression, it's lossless or clearly-labelled lossy with measured trade-offs.
- **No single-collection centralisation.** They have one ChromaDB collection for everything; O(n) palace graph builds. Our per-boundary partitioning is the right call — keep it.
- **No MCP write tools without input sanitization + confirmation flows for untrusted-context agents.** Their `add_drawer` is a prompt-injection surface. Worth a security ADR before T0.2.14 alpha distribution.
- **No benchmark iteration without held-out splits.** Their hybrid_v4 100% claim was reached by inspecting and fixing three specific failing questions; community caught the contamination within 48 hours; they had to publish a held-out 98.4% to recover credibility. When we benchmark Memory Vault, establish train/dev/held-out splits before iterating.

Cross-link from BRD §6.2 (added alongside this snippet).

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
