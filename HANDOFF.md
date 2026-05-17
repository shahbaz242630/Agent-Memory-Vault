# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)

**Last updated:** 2026-05-16 (T0.2.3 close commit 6 ship, CI watch pending) — **T0.2.3 read-time-pipeline code SHIPPED across commits 3 + 4; commit 5 fix-forward (`humbletim/install-vulkan-sdk@v1.2`) failed CI on Linux + Windows (loader library missing on both OSes, headers also missing on Windows); commit 6 fix-forward SHIPPED — native installers per OS (chocolatey `vulkan-sdk` on Windows, LunarG apt repo on Linux). CI watch pending.** macOS green throughout (Metal via Xcode). Bundled with commit 6 per admin-rides-with-code: vault-storage `create_vector_index_hnsw_sq` method (T0.2.7 Phase 0 production code) + t028a Lance index encryption spike binary + this HANDOFF update. **T0.2.7 Phase 0 t028a security spike PASSED locally: 3078 of 3078 Lance HNSW index files inherit the `vault-sealed://` envelope — BRD §11.5.1 NOT violated, HNSW integration GREEN for envelope compliance.** **Next session opens with: (1) confirm commit 6 CI green across the 3-OS matrix → T0.2.3 CI-side closed, (2) T0.2.7 Phase 1 t028b benchmark spike unblocked. If commit 6 still red, the new error shape determines the commit 7 fix.** See "Next-session opener" below.

**Slim-HANDOFF restart at T0.2.3 commit 2 ship (2026-05-13).** Full pre-restart HANDOFF (T0.2.0 + T0.2.1 + closed-T0.2.2 + T0.2.3 commits 1-2 narrative + ADRs 037-046 full text + all amendments + planning iterations) is frozen at `HANDOFF_V0.2_PART1_ARCHIVE.md` (3,582 lines, 54 sections). See "Archive cross-links" at the bottom of this file.

**Updated by:** Claude (Opus 4.7)

> **📁 Historical archives:** `HANDOFF_V0.1_ARCHIVE.md` (V0.1 alpha era, frozen 2026-05-06) + `HANDOFF_V0.2_PART1_ARCHIVE.md` (V0.2 first half through T0.2.3 commit 2, frozen 2026-05-13). Cross-link out when historical detail is needed; do NOT paraphrase from memory.

---

## Current Status

**Active task:** **T0.2.3 read-time-pipeline code SHIPPED across commits 3 + 4. CI-side close attempted via commit 5 (humbletim action — failed on Linux: loader missing; failed on Windows: BOTH loader + headers missing) and commit 6 (drop humbletim, native installers per OS — chocolatey `vulkan-sdk` on Windows, LunarG apt repo on Linux). Commit 6 CI watch pending.** macOS green throughout (Metal). **Next session opens with: (1) confirm commit 6 CI green across the 3-OS matrix → T0.2.3 CI-side fully closed → T0.2.7 Phase 1 t028b benchmark spike unblocked. If commit 6 still red, the new error shape determines the commit 7 fix.** Also: T0.2.7 Phase 0 t028a security spike PASSED locally (3078/3078 Lance HNSW index files sealed via `vault-sealed://` envelope — BRD §11.5.1 compliance verified). Read the dedicated "Next-session opener" section below before touching any code.

**T0.2.3 arc — code SHIPPED, CI-side BLOCKED on commit 5 fix-forward:**

| Commit | SHA | Scope | CI |
|---|---|---|---|
| 1 | `5aeb5b3` | File-layout refactor + ADR-044 Amendment 1 + Consolidator struct + ConflictReview type + Phase 2 `decide_merge` | ✅ ALL-GREEN run `25798562657` |
| 2 | `17035ec` | ADR-046 + `mark_superseded` primitive + Phase 3 `apply_merge` + orchestrator body + Boundary-Ord recon-amendment | ✅ ALL-GREEN run `25807518081` |
| 3 | `8293716` | ADR-047 + `summary.rs` (`generate_summary_markdown` + 8 unit tests) + 100-memory realism-rewritten fixture + canned LLM fixture + 3 integration tests + 2 property tests + RunState/AMWC field extensions + V0.2 Part 1 archive freeze | ✅ ALL-GREEN run `25923047902` (after one transient-failure rerun on macOS + Ubuntu CI runners — confirmed transient by clean re-run) |
| 4 | `316b553` | **Read-time pipeline production code** (`vault-retrieval::ReadPipeline` + 10 unit tests) + **ADR-048** (read-time pipeline architecture) + **ADR-049** (Qwen-7B model lock) + **HANDOFF section** "V0.2 backend + tuning config locked" + **Cargo.toml platform-conditional backend selection** (Metal/macOS, Vulkan/Windows+Linux) + **acceptance integration test** (`tests/read_pipeline_acceptance.rs`, cron-gated `#[ignore]`) + spike artefacts (t023 + t024 + t025 + t026 + t027a + t027a-ext + t027b + research backup) + `TuningConfig` plumbing (n_gpu_layers, framework_defaults_probe) | ❌ FAILED run `25925478690` — Linux + Windows build/clippy died with `VULKAN_SDK: NotPresent` because the Cargo.toml-side platform-conditional change required a CI workflow update that wasn't in commit 4. Process miss caught by Shahbaz; fix-forward shipped at commit 5 below. |
| 5 | `f2efc9d` | `.github/workflows/ci.yml` fix-forward: add `humbletim/install-vulkan-sdk@v1.2` step to clippy + build-and-test jobs, conditional on `runner.os != 'macOS'`, version pinned to 1.4.350.0, cache on. | ❌ FAILED run `25933341737` — install action ran clean, env var propagated (`VULKAN_SDK_VERSION: 1.4.350.0`, `VULKAN_SDK_PLATFORM: linux`/`windows`), but CMake's `find_package(Vulkan)` errored on Linux: `missing: Vulkan_LIBRARY (found version "1.4.350")` and worse on Windows: `missing: Vulkan_LIBRARY Vulkan_INCLUDE_DIR`. **Action installs SDK headers + glslc on Linux but NOT the runtime loader library; on Windows installs NEITHER headers nor loader.** Confirmed via CI-log re-read at 2026-05-16 session-open. |
| 6 | `<pending push>` | `.github/workflows/ci.yml` fix-forward (replaces commit 5's approach): drop `humbletim/install-vulkan-sdk@v1.2`, install native per OS — chocolatey `vulkan-sdk` on Windows (wraps LunarG installer, lands full SDK under `C:\VulkanSDK\<ver>`, sets `VULKAN_SDK` env via glob-discovered path), LunarG apt repo + `apt install vulkan-sdk` on Linux (full SDK with loader + headers + glslc; Ubuntu codename auto-detected via `lsb_release` for runner-image-update resilience). Adds same Windows-only install step to the cron-only `real-model-smoke` job (was missing entirely from commit 5 — would have failed at next Monday's cron run). Bundled per admin-rides-with-code: vault-storage `create_vector_index_hnsw_sq` method (T0.2.7 Phase 0 production code, t028a-pass-gated) + t028a Lance index encryption spike binary (executable documentation — 3078/3078 PASS locally) + this HANDOFF update. | 🟡 watch pending |

**Empirical anchor for T0.2.3 close (unchanged):** i7-13620H + Intel UHD Graphics + Windows 11 + Vulkan iGPU offload — **mean 86.0s · p99 119.7s · 4/4 contradictions + 2/2 hard-negatives.** Full per-query detail at `crates/vault-retrieval/examples/t027b_qwen_7b_vulkan_results.md`.

**T0.2.7 Phase 0 t028a security spike — PASSED 2026-05-15 (locally, not in CI yet):**
- **Question answered:** Does Lance's HNSW (IvfHnswSq) index emission route through the sealed `vault-sealed://` ObjectStoreProvider?
- **Result:** 3078 of 3078 on-disk Lance files (data fragments + HNSW graph layers + IVF centroid arrays + index manifests) start with the locked `0x01 0x00` VAULT_SEALED prefix. Zero plaintext leaks. Zero empty/short files.
- **Index creation latency:** 0.23s on 1024 synthetic random 384-dim vectors (lance build was efficient).
- **Implication:** No Lance contribution / shim / BRD §11.5.1 amendment needed. The existing T0.2.0 Phase 0e ObjectStoreProvider integration already covers index file emission for free. T0.2.7 HNSW integration is GREEN for envelope compliance. **t028b (HNSW vs IVF benchmark on realism-rewritten fixture) is unblocked on the security axis** but waits on commit 6 + CI green to gate the T0.2.3 close.

---

## Next-session opener — T0.2.3 commit 6 CI verify + T0.2.7 Phase 1 t028b (drafted 2026-05-16 commit 6 ship)

**Read this section first when reopening the session.** Two interlocking workstreams in priority order:

### Priority 1 — Confirm commit 6 CI green (load-bearing; T0.2.7 doesn't ship without this)

**State at commit 6 ship (2026-05-16):**
- Read-time pipeline code + ADRs 047/048/049 + Cargo.toml platform-conditional + HANDOFF lock — all on `main` via commits 3 (`8293716`) + 4 (`316b553`) + 5 (`f2efc9d`) + 6 (`<pending push>`).
- Commit 6 replaces commit 5's broken `humbletim/install-vulkan-sdk@v1.2` with native per-OS installers: chocolatey `vulkan-sdk` on Windows (wraps LunarG installer, lands full SDK under `C:\VulkanSDK\<ver>`), LunarG apt repo on Linux (full SDK with loader + headers + glslc; Ubuntu codename auto-detected via `lsb_release` for runner-image-update resilience). Same Windows-only Vulkan install also added to the cron-only `real-model-smoke` job (was missing entirely from commit 5).
- Commit 6 CI watch was pending at session-wrap.

**Diagnostic commands for session-open verify:**
```powershell
# 1. Confirm commit 6 is the latest run + see status across the 3-OS matrix
gh run list --workflow=ci.yml -L 1
gh run view <commit-6-run-id> --json jobs --jq '.jobs[] | {name, conclusion}'

# 2. If all 3 OSes (ubuntu-latest + windows-latest + macos-latest) green:
#    → T0.2.3 CI-side fully closed. Proceed to Priority 2.

# 3. If still red, pull the failure to determine new error shape:
gh run view <commit-6-run-id> --log-failed > commit6_failure.log
```

**If commit 6 still red, hypothesis tree for commit 7:**
- **Linux side:** LunarG repo may not support ubuntu-latest's current codename (e.g., `noble` not yet covered for SDK 1.4.x) → fall back to `jammy` codename override OR pin to known-supported SDK version (e.g., `1.3.290` series via versioned LunarG URL).
- **Windows side:** chocolatey `vulkan-sdk` package may have a different install path or a transient install timeout → check `C:\VulkanSDK` glob result; retry with `--execution-timeout 1800` flag if install timed out.
- **Either side:** `glslc` shader compiler path may not be picked up by `find_package(Vulkan)` even with `VULKAN_SDK` set → explicit `Vulkan_GLSLC_EXECUTABLE` env override added to job env.

**Confirm before commit + push** per the standing rule for commit 7 if needed.

### Priority 2 — T0.2.7 Phase 1 t028b benchmark spike (unblocked after commit 6 CI green)

Per the **T0.2.7 plan iteration 2 lock (2026-05-15)** the Phase 0 → 1 sequence is:
- Phase 0 (t028a security spike): ✅ PASSED locally (3078/3078 sealed). Spike binary + production `create_vector_index_hnsw_sq` method shipped with commit 6 per admin-rides-with-code (executable-documentation pattern).
- **Phase 1 (t028b HNSW vs IVF benchmark): unblocked on the security axis, gated on commit 6 CI close.**

**t028b spike scope (locked iteration 2):**
- File: `crates/vault-retrieval/examples/t028b_hnsw_vs_ivf_spike.rs`
- **Fixture content shape MUST match t026 realism-rewrite** (long-form + paragraph + cross-agent voice, NOT synthetic short) per iteration-2 amendment A. Spike harness doc-comment must include the verbatim string: *"fixture content shape matches t026 realism-rewrite, not synthetic short."*
- Benchmark axes:
  - **Index choice**: HNSW (`IvfHnswSq`) vs IVF-only (`IvfPq` or `IvfSq`) — same `create_index` API surface.
  - **Scale**: 100, 1K, 10K memories (multiply existing `merge_acceptance_100.json` content shape proportionally).
  - **Metrics**: recall@10 + recall@20 vs brute-force-cosine ground truth; p50 + p99 latency; index build time.
- Acceptance: data-only spike (no pass/fail gate at this stage). Partner reviews to decide HNSW vs IVF lock for T0.2.7 implementation phase.

### Working-tree state at commit 6 ship

Working tree empty post-commit-6. All three files that were uncommitted at 2026-05-15 session-wrap (vault-storage `create_vector_index_hnsw_sq` method + t028a spike binary + HANDOFF update) shipped with commit 6.

### Decision tree at session-open

1. Read this opener top to bottom.
2. Verify working-tree state — `git status --short` should be empty.
3. Verify commit 6 CI state — `gh run list --workflow=ci.yml -L 1`.
4. **If commit 6 green across all 3 OS:** T0.2.3 CI-side fully closed. Open T0.2.7 Phase 1 plan-paragraph for t028b spike scope, surface for review, then write.
5. **If commit 6 red:** consult the hypothesis tree above for commit 7 starting point. Pull the failure log, identify which OS / step failed, draft commit 7 fix-forward, surface for review.
6. Skip the historical "T0.2.3 close — architectural reframe + latency optimization narrative" section below — it's the prior-session context; you don't need it for current work.

### Cross-references

- `crates/vault-retrieval/examples/t027b_qwen_7b_vulkan_results.md` — the locked empirical numbers (86s mean, 119.7s p99, 4/4 + 2/2 quality). Background for ADR-048/049 in HANDOFF.
- `crates/vault-retrieval/src/read_pipeline.rs` — V0.2 production read contract (ADR-048 implementation).
- `crates/vault-retrieval/tests/read_pipeline_acceptance.rs` — cron-gated `#[ignore]` acceptance test exercising the locked pipeline against the t026 8-query gauntlet.
- `.github/workflows/ci.yml` — the file commit 6 modified (native Vulkan SDK installers per OS).
- `feedback_surface_ci_implications_before_new_build_features.md` — the rule that should have prevented the gap that landed commit 5; reinforced via commits 5 + 6 commit messages.
- `feedback_admin_changes_ride_with_code.md` — the working-tree-state rule that bundled vault-storage + t028a + HANDOFF with commit 6.
- `feedback_broken_ci_is_regression_not_techdebt.md` — why commit 6 was priority over T0.2.7 Phase 1.

---

## T0.2.3 commit 3 deliverables (staged for commit at 2026-05-14)

**`crates/vault-consolidator/src/summary.rs`** (new file, 601 lines — over the 500-line soft guideline; the file is cohesive with ~250 lines of pure renderer + ~340 lines of co-located unit tests + helper fixtures, splitting tests to a sibling file would be pre-emptive per `feedback_500_line_cap_is_soft.md`) — implements `pub(crate) fn generate_summary_markdown(state: &RunState, checkpoint_id: &str) -> String` per BRD §5.6 lines 959-973. Pure function over `RunState`; section builders for Run header / per-boundary Merges / per-boundary Contradictions / Decay aggregate / Footer. `SNIPPET_MAX_CHARS = 80` char-based truncation with ellipsis (UTF-8 safe). `FOOTER_ROLLBACK_PLACEHOLDER = "rollback ships at T0.2.5"` constant pinned by literal-wording test so T0.2.5 wiring updates the phrase consciously.

**`crates/vault-consolidator/src/consolidator.rs`** (modified) — 3 type promotions from `private` to `pub(crate)`: `RunState` / `BoundarySummary` / `AppliedMergeWithContext` (per ADR-047 §b). 3 `#[allow(dead_code)]` attributes removed. `RunState` gains `started_at: DateTime<Utc>` + `duration: Duration` fields. `AppliedMergeWithContext` gains `merged_text: String` + `pre_merge_contents: Vec<(MemoryId, String)>` (captured from in-scope per-boundary memory enumeration BEFORE `apply_merge` marks members superseded — no extra storage round-trip). `Consolidator::run_consolidation` wires `generate_summary_markdown` into `ConsolidationReport.summary_markdown` (was `String::new()` placeholder at commit 2); checkpoint ID placeholder `"pending-T0.2.5"` until T0.2.5 wires real checkpoints.

**`crates/vault-consolidator/src/lib.rs`** (modified) — added `mod summary;` (private module declaration). Not re-exported — only `consolidator.rs` consumes it via the in-crate path.

**`crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json`** (new, 100 entries). **Realism rewrite per plan iteration 2 (2026-05-14).** Pre-rewrite fixture was 100% short factual content (50-150 chars per entry) which was NOT representative of what LLM/agent integrations (Claude Code, Cursor, Codex, ChatGPT) will actually write to the vault — those produce paragraph-scale session summaries, decision logs, refactor notes. Content-length distribution rewritten to **56 short (50-150 chars) + 30 paragraph (300-1000 chars) + 11 long-form (1000-2000 chars) + 3 BGE-truncation entries (2000-2430 chars)**, preserving all 17 cluster labels, 50+50 boundary partition, and 42-merge / 54-keep / 4-contradiction outcome counts. **Within-cluster length variance on 3 clusters** (`use-postgres` / `bp-reading-132-85` / `learn-spanish`): each carries the same factual content at short, paragraph, AND long-form simultaneously — tests whether BGE embedder + Phi-4 classifier agree across length variance, which IS the production shape (different agents write the same fact in different lengths). **Both contradiction pairs go long-form** (GA-launch-quarter Q1-vs-Q2 + Comcast-bill $89-vs-$109) so Phi-4 sees realistic context paragraphs around the disputed facts rather than short bare statements. **3 BGE-truncation entries** (auth-service architecture log / family-reunion recap / photography-session journal — all 2200-2500 chars) explicitly exceed bge-small-en-v1.5's ~2000-char effective embedding window (512 tokens × ~4 chars/token); merge-time embedding-truncation behavior is now exercised, not just theorized about.

**`crates/vault-consolidator/tests/fixtures/canned_merge_decisions_nary.json`** (new) — 5 hand-curated `MergeOutcome`-shaped canned responses for `MockLlmProvider` / `ScriptedLlmProvider`: `merge_size_2` / `merge_size_5` / `merge_size_10` (sized for plausible N-ary inputs) + `keep_separate_typical` + `contradiction_typical`. Per ADR-044 §5 single-purpose constraint — hand-curated, not Phi-4-generated.

**`crates/vault-consolidator/tests/common/mod.rs`** (new) — shared helpers for integration + property tests: fixture loaders (`load_merge_acceptance_fixture` + `load_canned_response_as_string`), storage setup (`open_sealed_storage_for_test`), memory constructors, cascading-write-and-drain helper (`insert_and_drain`), BGE provider opener (`open_bge_provider`), and **`ScriptedLlmProvider`** (test-only `LlmProvider` impl that returns a pre-scripted sequence of canned responses; companion to `vault_llm::MockLlmProvider` which returns the same response on every call).

**`crates/vault-consolidator/tests/merge_acceptance.rs`** (new, 3 integration tests):
1. `merge_acceptance_phase_1_to_3_end_to_end_against_100_fixture` — real Phi-4-mini, **cron-gated via `#[ignore]` + `cfg(target_os = "windows")`** (Phi-4 path resolution Windows-only currently per `vault-llm/tests/phi4_mini_smoke.rs`); loads the 100-memory fixture, runs full Phase 1+2+3 pipeline, validates BRD §6.2 line 1441 structural acceptance (merge produces consolidated memories, originals superseded, retrieval surfaces merged version, summary_markdown contains all required sections); logs precision/recall against ground truth as observability only (not a hard gate — Phi-4 quality on long content is the ADR-042 revisit trigger if it materially degrades).
2. `rollback_restores_pre_consolidation_state_exactly` — **`#[ignore]` skeleton** (T0.2.5 dependency; panics loudly with BRD §6.2 line 1451 pointer until T0.2.5 wires `Consolidator::rollback(checkpoint_id)`).
3. `summary_markdown_is_non_empty_and_contains_required_sections` — runs on every CI cycle (Linux + Windows, BGE-gated against macOS). Tiny fixture (4 memories, 2 form tight cluster), `MockLlmProvider` with canned `merge_size_2` response, validates BRD §5.6 line 980 structural contract: markdown non-empty, all 5 section headers present, footer pins.

**`crates/vault-consolidator/tests/properties.rs`** (new, 2 property tests):
1. `consolidation_is_idempotent` (BRD §5.6 line 981) — runs consolidation twice on the same data; asserts run 2 produces `memories_merged == 0` + `contradictions_resolved == 0` (no further state change on stabilized state).
2. `no_memory_is_ever_lost` (BRD §5.6 line 982) — partitions every input memory ID into active OR superseded post-state; asserts no silent drops + storage row count non-decreasing + at least 1 new merged row per merge cluster.

**Test floor accounting.** Commit 3 firm: **+14** (vs plan-iteration-1 forecast of +10). Breakdown: 7 markdown unit tests (`header` / `per_boundary_merges` / `per_boundary_contradictions` / `decay_aggregate_zero` / `footer_emits_checkpoint_AND_literal` / `boundary_separation` / `truncate_snippet`) + 1 ADR-047 pub(crate) pin + 1 footer-literal-wording assertion folded into the footer test + 3 integration tests (1 active + 2 `#[ignore]`'d) + 2 property tests. The +4 over plan-iteration-1 forecast surfaces here per `feedback_floor_forecast_is_pre_declaration_not_estimate.md` — see ADR-047 "Test floor accounting" for per-add reasoning. **Cumulative T0.2.3 firm floor: +29** (commit 1 +8 + commit 2 +7 + commit 3 +14).

**Local DoD gates run before commit.** `cargo check --workspace --all-targets` ✅ | `cargo test -p vault-consolidator` ✅ 31 active tests pass (27 unit + 1 T0.2.2 acceptance + 1 markdown-sections + 2 property), 2 `#[ignore]`'d documented stubs | `cargo clippy --workspace --all-targets -- -D warnings` ✅ | `cargo fmt --all --check` ✅.

---

## ADR-047 — `summary.rs` file placement + RunState/AMWC field extensions (T0.2.3 commit 3)

**Status:** Accepted, T0.2.3 commit 3 (2026-05-14).

**Context.** T0.2.3 commit 3 implements `generate_summary_markdown` per BRD §5.6 lines 959-973. The implementation surfaced three architectural decisions the BRD spec + plan iteration 1 did not pre-decide, plus a recon-amendment-class spec-vs-iteration-lock divergence Shahbaz flagged at iteration 1 review:

1. **File placement.** BRD §5.6 lines 984-993 enumerates the vault-consolidator file layout: `src/lib.rs`, `src/consolidator.rs`, `src/phases/{cluster,merge,decay}.rs`, `src/checkpoint.rs`, `src/scheduler.rs`. No `src/summary.rs` listed. Inline-in-consolidator vs new-module decision needed.
2. **`RunState` field extensions.** The summary header requires `started_at` + `duration` per BRD §5.6 line 965; the existing `RunState` only carried `memories_processed` + `per_boundary`.
3. **`AppliedMergeWithContext` field extensions.** The summary's per-merge entries require pre-merge content snippets + the consolidated text per BRD §5.6 line 966; the existing AMWC only carried `cluster` + `applied` + `reasoning` (IDs only, no content).
4. **BRD §5.6 line 971 vs T0.2.3 iteration 3 §item-4 wording divergence.** BRD line 971 verbatim: *"generate two separate summaries, one per boundary, with clear boundary headers."* T0.2.3 iteration 3 §item-4 lock: *"per-boundary sub-sections inside the outer Run-scoped document."* These describe different document shapes.

**Decision.**

**(a) New file `crates/vault-consolidator/src/summary.rs`.** Reasons:
- `consolidator.rs` was 380 lines pre-commit-3; adding `generate_summary_markdown` + section builders + 8 unit tests (~450 lines) would push it past the 500-line soft guideline per `feedback_500_line_cap_is_soft.md`.
- "Orchestrating phases" and "rendering Markdown" are distinct concerns. Splitting on cohesion grounds + nav-friction signals matches the spirit of BRD §2.5's file-size cap rationale.
- BRD §5.6 lines 984-993 is descriptive of the V0.2 minimum file layout, not prescriptive against additions. Future ADR may amend the BRD section if the layout stabilizes.

Module declaration: `mod summary;` in `lib.rs` (private — not re-exported). Only `consolidator.rs` consumes via the in-crate path `crate::summary::generate_summary_markdown`.

**(b) Three pub(crate) type promotions + field extensions.**

`RunState`: promoted to `pub(crate)`. Added fields: `started_at: DateTime<Utc>` + `duration: Duration`.

`BoundarySummary`: promoted to `pub(crate)`. No field changes.

`AppliedMergeWithContext`: promoted to `pub(crate)`. Added fields: `merged_text: String` (captured from `MergeOutcome::Merge` before `apply_merge` consumes it) + `pre_merge_contents: Vec<(MemoryId, String)>` (captured from the in-scope per-boundary memory enumeration BEFORE `apply_merge` marks members superseded — no extra storage round-trip).

The 3 `#[allow(dead_code)]` attributes that previously suppressed warnings on these types (consolidator.rs lines 338/347/358 at commit 2) are REMOVED in commit 3 — `summary.rs` consumes them via `pub(crate)` visibility.

**(c) BRD-spec-file-list vs actual-files forward-compat.** Documented in this ADR. If a future BRD revision tightens §5.6 lines 984-993 to be prescriptive about file inventory, the additional `summary.rs` file would need either ADR acceptance or BRD amendment. At T0.2.3 the file list is read as descriptive of the V0.2 minimum surface.

**(d) BRD §5.6 line 971 vs iteration 3 §item-4 divergence.** Iteration 3 lock ("per-boundary sub-sections inside the outer Run-scoped document") prevails for T0.2.3 commit 3. Rationale: a single Run-scoped document with per-boundary sub-sections is more usable in the Tauri Consolidation Report viewer (T0.2.15) than separate per-boundary documents — one URL to open, one scroll, structured headers. Forward-compat: if T0.2.15 wiring surfaces a UX reason to switch to separate-per-boundary documents (e.g., per-boundary export to disk as separate REPORT.md files), a future ADR reconciles by either amending BRD §5.6 line 971 or by adding a second rendering function alongside `generate_summary_markdown`. Not re-litigated at T0.2.3 commit 3.

**Pin tests.** ADR-047 §b is pinned by `summary::tests::pub_crate_promotion_for_summary_consumption_compiles` (compile-time visibility check on the 3 types). If `consolidator.rs` reverts any of the 3 types to `private`, the test fails to compile.

**Test floor accounting.** Commit 3 firm test floor: **+14**. Breakdown:
- 7 markdown unit tests in `summary.rs`: `header` / `per_boundary_merges` / `per_boundary_contradictions` / `decay_aggregate_zero` / `footer_emits_checkpoint_AND_literal` / `boundary_separation_no_cross_boundary_content_leak` / `truncate_snippet_clips_at_char_ceiling_with_ellipsis`
- +1 ADR-047 pub(crate) pin: `pub_crate_promotion_for_summary_consumption_compiles`
- +1 footer-literal-wording assertion folded into the footer test (counted as a distinct floor contribution per Shahbaz's plan-iteration-1 directive — T0.2.5 wiring must consciously update BOTH checkpoint-ID format AND literal "rollback ships at T0.2.5" phrase)
- +3 integration tests in `tests/merge_acceptance.rs` (1 active + 2 `#[ignore]`'d)
- +2 property tests in `tests/properties.rs`

Original plan-iteration-1 forecast: +10 firm. The +4 over-forecast surfaces here as plan amendment per `feedback_floor_forecast_is_pre_declaration_not_estimate.md`. Per-add reasoning:
- **+1 boundary-separation unit test** (Shahbaz pushback at plan iteration 1 review): privacy invariants need dedicated tests per [[privacy-invariants-need-dedicated-tests]] memory; per-boundary rendering correctness ≠ cross-boundary leakage invariant.
- **+1 ADR-047 pub(crate) pin** (this ADR's own pinning requirement, Shahbaz directive at plan iteration 1 review).
- **+1 footer literal-wording assertion** folded into the footer test (Shahbaz directive: T0.2.5 wiring must consciously update both format AND phrase together).
- **+1 truncate_snippet unit test** (surfaced at plan iteration 2 fixture-realism rewrite — pre-rewrite no test exercised the truncation path because all fixture content was below the 80-char cap; the rewrite added 14 entries >800 chars which now trigger the truncation path in test #2 transitively, but a dedicated unit test pins the contract explicitly).

**Cumulative T0.2.3 firm floor: +29** (commit 1 +8 + commit 2 +7 + commit 3 +14).

**Live BRD references.**
- §5.6 lines 959-973: human-readable summary spec.
- §5.6 lines 975-982: Heavy test requirements (BRD origin of commit-3's test floor).
- §5.6 lines 984-993: vault-consolidator file layout (descriptive at T0.2.3; ADR-047 §c documents the `summary.rs` addition).
- §6.2 line 1441: T0.2.3 acceptance criterion.
- ADR-045 §a: Cluster shape (consumed by AppliedMergeWithContext).
- ADR-046: `mark_superseded` primitive (consumed by Phase 3's `apply_merge`).

---

## ADR-048 — Read-time pipeline architecture (single-call Qwen-7B synthesis)

**Status:** Accepted, T0.2.3 close (2026-05-15).

**Context.** T0.2.3 four-spike arc (t023→t026) established that retrieval IS the product surface for agent-shaped workloads; consolidation is housekeeping. Empirical findings: BGE recall@20 = 1.00 across realistic query shapes (t023); Phi-4-mini fails contradiction synthesis at 1/8 (t024); Pipeline A Phi-4+Qwen split hurts BOTH quality and latency (t025); Qwen2.5-7B-Instruct standalone passes 4/4 contradictions + 2/2 hard-negatives (t026, reconfirmed at t027b).

**Decision.** Read-time pipeline is exactly two stages:
1. **Stage 1** — BGE retrieval top-20 via existing `SemanticRetriever`. No change.
2. **Stage 2** — Single Qwen2.5-7B-Instruct synthesis call (filter + flag contradictions + write narrative) with GBNF-constrained JSON output.

Production implementation: `crates/vault-retrieval/src/read_pipeline.rs::ReadPipeline`. Concrete struct (NOT a trait yet — defer trait surface to V0.3 cloud-tier per `feedback_forward_compat_concrete_vs_hypothetical.md`).

**Rejected (one line each, empirical evidence in linked result files).**
- Phi-4-mini stage 2/2.5 split — fails contradiction surfacing (`crates/vault-retrieval/examples/t024_readtime_viability_spike.rs`; 1/8 contradictions).
- Two-model split (Phi-4 + Qwen) — hurts BOTH quality and latency vs Qwen-7B alone (`crates/vault-retrieval/examples/t025_qwen_vs_split_results.md` Pipeline A vs B).
- Qwen2.5-14B — quality acceptable but unshippable latency 4.5–11 min/query (`t025_qwen_vs_split_results.md`).

**Consolidation reframed as housekeeping (folds the proposed ADR-045 Amendment 1 in).** The vault-consolidator (T0.2.2 + T0.2.3 commits 1-3) continues to deduplicate, merge near-duplicates, mark superseded entries, and emit run summaries — these SHAPE what retrieval finds. The canonical product-quality surface is now this read-time pipeline (4/4 + 2/2 on the t026 gauntlet), NOT the consolidator's clustering / merge quality gates from ADR-045 §c. BRD §5.6 verbatim contracts on consolidator primitives stay unchanged; ADR-044 / ADR-046 / ADR-047 stay unchanged; T0.2.3 commits 1-3 staged work ships as-is. Consolidator failure-recovery is still rigorous (per existing contracts), but a consolidation-run failure is no longer "the product is broken" — it's "the substrate gets dirtier, retrieval still works."

**Latency budget.** Read-time stage 2 has its OWN budget. BRD §5.5 line 869's 200ms applies to `Retriever::retrieve` (stage 1) ONLY, NOT to the synthesis stage.

**Quality contract.** 4/4 contradictions surfaced (Q11, Q13, Q25, Q26) + 2/2 hard-negatives correctly rejected (Q21, Q22), measured on the t026 8-query gauntlet and reconfirmed at t027b. Pinned by `crates/vault-retrieval/tests/read_pipeline_acceptance.rs::read_pipeline_acceptance_8_query_gauntlet` — cron-gated `#[ignore]` integration test that runs the production `ReadPipeline` against the locked Qwen-7B model with the locked `TuningConfig`.

**Pin tests.**
- 10 unit tests in `crates/vault-retrieval/src/read_pipeline.rs::tests` cover pipeline wiring (empty-retrieval short-circuit, LLM call invocation, error propagation, retriever-query construction, system-prompt override, JSON schema validity, system-prompt content tripwire). Use `MockLlmProvider` + a test-local mock `Retriever`; on every CI cycle.
- 1 integration test in `crates/vault-retrieval/tests/read_pipeline_acceptance.rs` — cron-gated `#[ignore]` + `cfg(target_os = "windows")` (Vulkan SDK + GGUF path are Windows-only in CI today; Linux/Vulkan + macOS/Metal need a t027c-equivalent spike to unlock).
- Query fixture `crates/vault-retrieval/test-fixtures/merge_acceptance_100_queries.json` (26 queries) promoted from spike-only to vault-retrieval's acceptance fixture surface; the 8-query subset is the canonical gauntlet.

**Forward-compat.** Speculative decoding (Qwen2.5-0.5B draft + Qwen-7B target — Family B) is the documented V0.2.x escape valve if real-world tail breaches the 120s ceiling. Mathematically lossless, ~50% gen-phase speedup, 2–3 week llama-cpp-sys-2 FFI work. Triggered by beta telemetry showing real-world p99 > 120s, NOT deferred indefinitely.

**Cross-refs.** ADR-044 / ADR-046 / ADR-047 (consolidator surface, unchanged) · ADR-049 (model lock, below) · `crates/vault-retrieval/examples/t023_retrieval_diagnostic_results.md` · `t025_qwen_vs_split_results.md` · `t026_qwen_7b_results.md` · `t027b_qwen_7b_vulkan_results.md`.

---

## ADR-049 — Qwen2.5-7B-Instruct Q4_K_M model lock

**Status:** Accepted, T0.2.3 close (2026-05-15).

**Context.** V0.2 read-time synthesis model lock. Empirical evidence from t023-t027b plus Shahbaz's hands-on testing rule out sub-7B candidates on this contradiction-surfacing workload.

**Decision.** Qwen2.5-7B-Instruct **Q4_K_M GGUF**, ~4.36 GB on disk, **Apache 2.0**, 128K native context. Quantization floor is Q4_K_M.

**Rejected candidates (one line each, empirical evidence in result files).**
- Phi-4-mini-instruct 3.8B Q4_K_M — fails t024 (1/8 contradictions). Kept in vault-consolidator for merge classification (where it scores 100% precision on binary classification).
- Qwen2.5-14B-Instruct Q4_K_M — passes quality but unshippable latency 4.5–11 min/query. Rejected at t025; GGUF deleted from disk during t026.
- Sub-7B candidates (Qwen 3B / 1.5B / 0.5B, Phi-4-mini, Llama-3.2-3B, Gemma-3-4B) as primary read-time models — Shahbaz hands-on testing confirms "rubbish" output per `feedback_no_sub_7b_models_for_synthesis.md`. Standard benchmarks (MMLU, HumanEval) understate the quality cliff on nuanced agentic reasoning. **Exception:** Qwen2.5-0.5B as speculative-decoding draft for the 7B target — NOT a primary substitution; output is byte-identical to running the 7B alone because every draft token is verified by the target.

**Distribution.** Productionization follows ADR-043 (download + SHA + revision-pin verification pattern). When the production download chain lands (post-T0.2.3 in a vault-llm Phase X commit), the Qwen-7B `Qwen25Config` mirrors `Phi4MiniConfig`'s shape with Qwen-specific SHA + revision pins. Today the spike + acceptance test consume a pre-downloaded GGUF at `$APPDATA\com.shahbaz242630.memory-vault\models\Qwen2.5-7B-Instruct-Q4_K_M.gguf`.

**Cross-refs.** ADR-042 (V0.1 model selection — superseded for the read-time role; V0.1-era CPU-only framing updated in the HANDOFF "V0.2 backend + tuning config locked" section below, NOT via a formal amendment per iteration-2 shrink scope) · ADR-043 (download chain) · ADR-048 (pipeline, above) · `feedback_no_sub_7b_models_for_synthesis.md`.

---

## V0.2 backend + tuning config locked (HANDOFF section — NOT an ADR)

Plain HANDOFF content documenting the configuration choices locked at T0.2.3 close. Per iteration-2 shrink scope: the Cargo.toml diff and the tuning literal are **configuration, not architecture** — they belong here, not as standalone ADR amendments.

**Backend selection — per-target-OS Cargo.toml shape (replaces unconditional `llama-cpp-2 = { workspace = true }`):**

```toml
[target.'cfg(target_os = "macos")'.dependencies]
llama-cpp-2 = { version = "=0.1.146", features = ["metal"] }

[target.'cfg(any(target_os = "windows", target_os = "linux"))'.dependencies]
llama-cpp-2 = { version = "=0.1.146", features = ["vulkan"] }
```

Lives at `crates/vault-llm/Cargo.toml` lines 39-49 (the `[dependencies]` table contains only platform-neutral entries; the per-target llama-cpp-2 declarations follow). CPU fallback happens at runtime: if `n_gpu_layers > 0` in `TuningConfig` doesn't light up (no usable iGPU/dGPU on this host), llama.cpp returns 0 offloaded layers and the same binary runs CPU-only. One binary per platform; no separate CPU-only Cargo profile required.

**Locked production tuning config:**

```rust
TuningConfig {
    n_threads:        Some(12),
    n_threads_batch:  Some(12),
    n_batch:          None,      // n_ctx default
    type_k:           None,      // KV cache f16 — Q8_0 hurt 34% on AVX2-without-VNNI; do NOT override
    type_v:           None,
    n_gpu_layers:     Some(99),  // offload all (llama.cpp clamps to actual model layer count)
}
```

Per-knob evidence: `crates/vault-retrieval/examples/t027a_qwen_tuning_results.md` (n_threads sweep + KV Q8_0 rejection) · `t027a_ext_t14_t16_results.md` (t12 wins, t14/t16 regress on HT contention) · `t027b_qwen_7b_vulkan_results.md` (29/29 layer offload, 36% drop vs t12 CPU baseline). The `TuningConfig` literal above is the V0.2 production default; consumers of `Qwen25_14BProvider::open_with_tuning()` pass this struct verbatim.

**Empirical numbers (single hardware data point — i7-13620H + Intel UHD Graphics + Windows 11 + Vulkan):** **mean 86.0s · p50 84.9s · p99 119.7s · 4/4 contradictions + 2/2 hard-negatives.** Full per-query detail at `crates/vault-retrieval/examples/t027b_qwen_7b_vulkan_results.md` — not restated here.

**Hardware honesty (V0.2 free-tier framing — locked wording).**

> *"V0.2 free-tier ships at 86s mean on a representative Intel iGPU. Pure-CPU fallback is 134s mean and breaches the 120s ceiling. Metal autodetect on macOS is still deferred (per V0.1 archive ADR-042 scope-amendment trail)."*

| Hardware class | Code path | Expected | Status |
|---|---|---|---|
| Modern Intel laptop + Vulkan iGPU (this measurement) | Vulkan, full GPU offload | **86s mean (measured)** | ✅ Shippable |
| Modern laptop with NO usable iGPU | CPU runtime fallback (t12 config) | **134s mean (measured)** | ❌ Breaches 120s ceiling — UX framing must reflect this |
| Older Intel iGPUs (UHD 620 / HD 4000) | Vulkan, partial or full offload | Unknown — 100–180s likely | ❌ Untested; V0.2.x measurement required |
| Apple Silicon Macs (M1 / M2 / M3 / M4) | **Metal backend (entirely different code path)** | Projected 30–60s per research playbook | ❌ Untested; **promotion gated on first Apple Silicon beta user OR borrowed-Mac t027c gauntlet pre-V0.2-launch.** Do NOT promise Mac latency in product copy. |
| Discrete GPU (NVIDIA / AMD) | Vulkan auto, CUDA opt-in deferred | Projected <30s | ⚪ Works automatically on free tier; CUDA opt-in is V0.2.x |

**Q19 tail-latency margin (load-bearing).** p99 = 119.7s = **0.3s under the 120s ceiling.** Q19 (multi-cluster narrative spanning 3 clusters / 8 memories) is the worst-case query in the t026 gauntlet by design. Margin erodes under: denser BGE top-K (>8 relevant candidates), longer output (>400 generated tokens), heavier system prompts (per-tenant context). **Escape valve:** speculative decoding (Qwen2.5-0.5B drafting for Qwen-7B target — Family B), mathematically lossless, ~50% gen-phase speedup, 2–3 week implementation via raw `llama-cpp-sys-2` FFI. Deferred to V0.2.x; **triggered if beta telemetry shows real-world p99 > 120s, NOT deferred indefinitely.**

**V0.2.x revisit triggers (deferred forward-compat notes).**
- **Opt-out CPU-only build feature** (`gpu-vulkan` / `gpu-metal` as opt-out workspace features) — revisit if a beta user reports a GPU driver bug that runtime fallback doesn't handle cleanly. Until then, runtime fallback IS the answer.
- **`gpu-cuda` opt-in feature for NVIDIA discrete** — revisit when (a) a real NVIDIA prosumer / dev user requests it, OR (b) the paid-cloud tier ships and needs server-side CUDA builds. Vulkan covers NVIDIA discrete adequately on the free tier until either trigger fires.
- **Apple Silicon empirical gauntlet (t027c)** — required before promoting Mac latency claims in product copy. Tracked as V0.2.x scope.
- **Older Intel iGPU measurement (UHD 620 / HD 4000 class)** — required before broader marketing claims.

---

## T0.2.3 close — architectural reframe + latency optimization narrative (historical, drafted 2026-05-14, closed 2026-05-15)

**Read this first when reopening.** This session locked the read-time architecture after a 4-spike arc that reframed T0.2.3 from "fix consolidation recall" to "ship the read-time pipeline as the product surface." Next session: **drive Qwen2.5-7B local CPU latency from current mean 187s down to 150-180s (2:30-3:00 per query) using llama.cpp tuning knobs.**

### What this session established (the 4-spike arc, 2026-05-14)

**The architectural reframe**: retrieval IS the product surface; consolidation is housekeeping. Agents read the vault on every boot / context switch / thread pickup. If reads return coherent context with contradictions surfaced, the product wins. If reads return noise, the product loses. The differentiator is the agent-shaped read workload (long-form session writes merging with terse fact-writes; contradictions surfacing not hiding; cross-topic synthesis on catch-up queries).

**Spike arc evidence:**

- **t023 — retrieval diagnostic** (`crates/vault-retrieval/examples/t023_retrieval_diagnostic_results.md`). Established BGE semantic retrieval recall@20 = 1.00 across every real-query shape on the realism-rewritten 100-memory fixture. The pairwise-clustering 24% recall finding does NOT translate to retrieval recall — query-anchored retrieval recovers what pairwise clustering misses. Hard-negative score-distribution analysis showed score-threshold gating fundamentally cannot work (band-floor 0.6779 < FP-ceiling 0.7169). Content-aware reading required at read time.

- **t024 — Phi-4-mini read-time viability**. Crashed on Q22 degeneracy loop; patched + re-run was cancelled. **7/8 complete data points proved Phi-4-mini synthesis fails the contradiction-surfacing differentiator (1/8 structural passes). Phi-4-mini cannot do open-ended synthesis at our content shape.**

- **t025 — Pipeline A (Phi-4 + Qwen split) vs Pipeline B (Qwen-14B standalone)** (`crates/vault-retrieval/examples/t025_qwen_vs_split_results.md`). Pipeline B wins decisively on quality (3/4 contradictions vs 1/4) AND latency (mean 406s vs 415s). Pipeline A's Phi-4 stage 2.5 pairwise gate at cosine ≥ 0.85 misses contradiction pairs in 3 of 4 queries. **The split architecture (Phi-4 helping Qwen) doesn't help — it hurts both quality and latency.**

- **t026 — Qwen-7B standalone** (`crates/vault-retrieval/examples/t026_qwen_7b_results.md`). **4/4 contradictions PASS** (better than 14B's 3/4 — Qwen-7B beats Qwen-14B on quality including Q26 oblique Comcast). **2/2 hard-negatives correctly reject** (`vault_has_no_relevant_content=true`). Q19 narrative caught 2 embedded contradictions (bonus signal). **Latency mean 187s — still over 2-min ceiling but ~55% faster than 14B.**

### Architectural decision LOCKED this session

**Read-time pipeline (V0.2 final shape — agreed but not yet ADR-drafted):**

1. **Stage 1 — Semantic retrieval** (BGE-small, top-20). Already shipped per `vault-retrieval::SemanticRetriever`. No changes.
2. **Stage 2 — Qwen2.5-7B-Instruct synthesis (single call, does everything: filter + flag contradictions + write narrative).** No Phi-4 stage 2/2.5 split — t025 proved it hurts. Same model, same prompt as t026 Pipeline B.

**Phi-4-mini stays in the system for `vault-consolidator` merge classification only** (where it scored 100% precision per the original cron test on what it saw — that role works). It is NOT in the read-time pipeline.

**Qwen-14B GGUF deleted from disk this session** (8.37 GB freed). Cache currently holds:
- `Phi-4-mini-instruct-Q4_K_M.gguf` — 2.32 GB (consolidator merge classifier)
- `Qwen2.5-7B-Instruct-Q4_K_M.gguf` — 4.36 GB (read-time stage 2 synthesis)

**Cloud-tier architecture (Shahbaz's direction, deferred to V0.3):** Same Qwen-7B running on our own GPU servers (NOT third-party Anthropic/OpenAI APIs). Vault stays local; for each query, client sends `(query + retrieved candidates)` over TLS to our zero-log inference server; server does synthesis; returns result. Same model as free local tier = same answers, just faster (estimated 10-20s on cloud GPU vs 187s on local CPU). Better economics than third-party APIs (~$0.0005/call vs $0.02-0.05/call). Better privacy story than third-party APIs (we control infrastructure + policy). Apache 2.0 means no vendor lock-in. **V0.3 work, not V0.2.**

### The latency gap — and the next-session objective

| Reality (Qwen-7B local CPU on i7-13620H) | Target (next session) | Hard ceiling (Shahbaz-locked) |
|---|---|---|
| Mean 187s, p99 224s | Mean 150-180s, p99 ≤ 180s | 120s — broken above this for V0.2 free local tier |

We need to close ~10-25% of latency on local CPU. The path: tune llama.cpp inference parameters that are currently at defaults. Realistic gain estimate: 20-40% with the right tuning. If we hit 150s mean, V0.2 free local tier ships at "AI is doing real work" UX. If not, we either accept slower-than-ideal free tier OR push cloud-tier-only earlier than V0.3.

### Working tree state — UNCOMMITTED, do NOT discard

No commits this session per Shahbaz's direction ("no push or commit for now until we nail this"). All preserved on disk:

**T0.2.3 commit-3 staged (still preserved from previous session):**
- `crates/vault-consolidator/src/consolidator.rs` (modified)
- `crates/vault-consolidator/src/lib.rs` (modified)
- `crates/vault-consolidator/src/summary.rs` (new — 595 lines markdown renderer + 8 unit tests)
- `crates/vault-consolidator/tests/common/mod.rs` (new — shared helpers + ScriptedLlmProvider)
- `crates/vault-consolidator/tests/fixtures/canned_merge_decisions_nary.json` (new)
- `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json` (new — 100-memory realism-rewritten fixture)
- `crates/vault-consolidator/tests/merge_acceptance.rs` (new — 3 integration tests)
- `crates/vault-consolidator/tests/properties.rs` (new — 2 property tests)
- `HANDOFF_V0.2_PART1_ARCHIVE.md` (new — 3,582-line archive freeze from 2026-05-13)

**This session's spike artefacts:**
- `crates/vault-retrieval/Cargo.toml` (modified — added `vault-llm`, `anyhow`, `chrono` as dev-deps for spike examples)
- `crates/vault-retrieval/test-fixtures/merge_acceptance_100_queries.json` (new — 26-query t023 fixture, becomes the read-time acceptance fixture)
- `crates/vault-retrieval/examples/t023_retrieval_diagnostic_spike.rs` + `t023_retrieval_diagnostic_results.md`
- `crates/vault-retrieval/examples/t024_readtime_viability_spike.rs` (Phi-4-only test, crashed at Q22; no results.md)
- `crates/vault-retrieval/examples/t025_qwen_vs_split_spike.rs` + `t025_qwen_vs_split_results.md`
- `crates/vault-retrieval/examples/t026_qwen_7b_spike.rs` + `t026_qwen_7b_results.md`
- `crates/vault-llm/src/qwen25.rs` (new — spike-scoped `Qwen25_14BProvider`; misleading name, works for any Qwen2.5-family GGUF including the 7B we locked on)
- `crates/vault-llm/src/lib.rs` (modified — added `pub mod qwen25;` + re-export)
- `crates/vault-llm/src/phi4_mini.rs` (modified — made `get_or_init_backend` `pub(crate)` so `qwen25` can share the LlamaBackend singleton)
- `HANDOFF.md` (this file, modified)

**Do NOT run `git reset --hard`, `git restore .`, `git clean -fd`, or any destructive cleanup.** Do NOT run `cargo clean` (warm cache is ~30 GB, full rebuild is 30+ min).

### Next-session objective — Qwen-7B latency optimization

**Goal:** drive Qwen-7B mean per-query latency from current 187s down to 150-180s (or as close to the 120s ceiling as we can get).

**Reference numbers from t026 baseline (Qwen-7B Q4_K_M, llama.cpp at defaults, single thread for inference, mmap on, KV cache f16):**
- Min 156s · p50 187s · p99 224s · max 224s · mean 187s
- Quality: 4/4 contradictions PASS, 2/2 hard-negatives PASS — **do not regress quality while optimizing latency.**

**Tuning knobs to test (llama.cpp / llama-cpp-2), roughly in order of expected gain:**

1. **CPU thread count.** Current default is `std::thread::hardware_concurrency()` (16 on i7-13620H with 10 cores / 16 threads). Hyperthreaded contention on AVX2 inference often makes 8-10 threads faster than 16. Test n_threads = 8, 10, 12, 16. Expected gain: 5-15%.

2. **KV cache quantization.** Currently f16. Switching to `q8_0` halves KV-cache memory and is often slightly faster on CPU. `q4_0` is faster again but typically degrades output. Test q8_0 first. Expected gain: 5-15%.

3. **Batch size / n_batch.** Currently sized to needed.max(2048).min(32_768) in `qwen25.rs::run_one_inference_qwen` line 162. Larger batches improve prompt-eval throughput. Test n_batch = 512, 1024, 2048, 4096 against prompts of ~6-12K tokens. Expected gain: 5-15%.

4. **Flash Attention.** llama.cpp log shows it's already auto-enabled ("Flash Attention was auto, set to enabled"). No-op unless we want to force-disable for comparison.

5. **n_ctx sizing.** Currently sized to `needed.max(2048).min(32_768)`. Tighter sizing reduces KV cache allocation cost. Measure actual prompt+output token counts per query and tune. Expected gain: 0-5%.

6. **--threads-batch separate from --threads.** Sometimes the prompt-eval batch phase benefits from a different thread count than the generation phase. Worth a quick test.

7. **CPU SIMD: AVX-512.** i7-13620H does NOT support AVX-512 (Intel restricts that to certain server/HEDT SKUs). AVX2 is the ceiling. Already used. Skip.

**Methodology proposal for next-session spike:**

1. Add knob-passing to `vault_llm::Qwen25_14BProvider::open` (or extend with an `open_with_params` variant) so the spike can pass `n_threads`, `n_threads_batch`, `n_batch`, and KV cache type. Currently these are hardcoded to defaults.
2. Run a small spike (e.g., 2 queries, not full 8) for each knob configuration to get fast per-config latency numbers.
3. Pick the best config, then run full 8-query t026-style spike to validate quality didn't regress.
4. Capture the winning config + new latency numbers in `crates/vault-retrieval/examples/t027_qwen_7b_tuned_results.md`.
5. If we hit ≤180s mean: lock the config + draft ADR-050 documenting the latency tuning. Surface to Shahbaz for approval, then commit the architecture-lock arc.
6. If we don't hit ≤180s: surface the gap honestly + propose either (a) accept slower free local tier with explicit UX framing, or (b) accelerate the cloud-tier work into V0.2.

**Files to focus on during the next session:**
- `crates/vault-llm/src/qwen25.rs::run_one_inference_qwen` — the inference loop; add knob plumbing
- `crates/vault-llm/src/provider.rs::CompletionParams` — may need new fields for the tuning knobs (or pass via a config struct on the provider)
- New spike: `crates/vault-retrieval/examples/t027_qwen_7b_tuned_spike.rs` — copy t026 pattern

### Three-model summary (settled this session — do not re-litigate)

| Model | Role | Status |
|---|---|---|
| Phi-4-mini-instruct (3.8B, Q4_K_M, 2.32 GB) | Consolidator merge classifier (vault-consolidator) | KEEP — works for binary classification at 100% precision |
| Qwen2.5-14B-Instruct (14B, Q4_K_M, 8.37 GB) | (Tested as read-time stage 3; rejected) | **DELETED from disk this session** — Qwen-7B beats it on quality + speed |
| Qwen2.5-7B-Instruct (7B, Q4_K_M, 4.36 GB) | Read-time stage 2 synthesis (vault-retrieval) | **LOCKED** — 4/4 contradictions, 2/2 hard-negatives, mean 187s |

### Architectural decisions deferred to ADR drafts (do NOT draft yet — pending latency work)

- **ADR-048** — Read-time pipeline architecture (Qwen-7B single-call synthesis; no Phi-4 stage 2/2.5 split per t025 evidence; layers above `Retriever::retrieve` with its own latency budget — BRD §5.5 line 869's 200ms applies to retriever only, NOT read-time stage)
- **ADR-049** — Qwen2.5-7B model selection (Apache 2.0, 128K context, ~4.36 GB Q4_K_M, beats Phi-4-mini on synthesis and beats Qwen-14B on this workload per t025/t026 evidence)
- **ADR-045 Amendment 1** — Consolidator reframed as best-effort housekeeping (read-time pipeline is the load-bearing product surface; consolidation shapes what retrieval can find but is not itself the differentiator)
- **ADR-050** (pending next session) — Latency tuning config locked for Qwen-7B (after the tuning spike)

**These ADRs land in the same commit arc that introduces the production read-time pipeline. Do NOT draft them in isolation — they ride the code.**

### What T0.2.3 close looks like (revised)

T0.2.3 ships as ARC of 4+ commits:
- commits 1 (`5aeb5b3`) + 2 (`17035ec`) shipped + CI-green ✓ (consolidator Phases 1-3)
- commit 3 (staged today, preserved in working tree) — summary renderer + acceptance tests + ADR-047. Ships within T0.2.3 close, not standalone.
- New commits (4+) — read-time pipeline production code:
  - vault-retrieval `read_pipeline.rs` (Qwen-7B single-call synthesis wrapping `Retriever::retrieve`)
  - vault-llm productionized Qwen-7B provider (with SHA + revision pins per ADR-043 pattern, download chain, integrity verification)
  - ADRs 048 + 049 + ADR-045 Amendment 1 + ADR-050 (latency config)
  - Acceptance fixture: promote `crates/vault-retrieval/test-fixtures/merge_acceptance_100_queries.json` to test-suite (currently spike-only)
  - Read-time acceptance tests with structural assertions on Q11/Q13/Q25/Q26 contradiction surfacing + Q21/Q22 hard-negative rejection + latency floor

T0.2.4 (Phase 4 decay & archive) opens after T0.2.3 close. T0.2.3 commit-3's `summary_markdown` renderer becomes the historical record of consolidation runs the read pipeline ignores.

### Decision tree at session-open (latency optimization)

1. Read this opener top to bottom.
2. Verify working tree state — `git status --short` should show all files listed above. Do NOT clean anything.
3. Confirm models on disk: `Phi-4-mini-instruct-Q4_K_M.gguf` (2.32 GB) + `Qwen2.5-7B-Instruct-Q4_K_M.gguf` (4.36 GB). Qwen-14B was deleted this session.
4. Re-read the three spike result files for context: `t023_retrieval_diagnostic_results.md`, `t025_qwen_vs_split_results.md`, `t026_qwen_7b_results.md`.
5. Plan iteration 1 in chat (NOT HANDOFF.md, per `feedback_plan_iterations_inline_not_handoff.md`): which tuning knobs to test first, what spike methodology (compile-and-run, single-query micro-bench, full 8-query confirmation).
6. Surface plan for Shahbaz approval before writing the t027 tuning spike.
7. Once Shahbaz approves: write `t027_qwen_7b_tuned_spike.rs`, run quick per-knob micro-benches, pick winning config, full 8-query run.
8. If mean ≤ 180s AND quality matches t026 baseline (4/4 contradictions, 2/2 hard-negatives): draft ADRs 048/049/050 + ADR-045 Amendment 1 + production read-time pipeline code. Stage as commit arc. Ask Shahbaz for combined commit + push approval.
9. If mean > 180s OR quality regressed: surface gap honestly + propose either (a) ship free local at slower-than-ideal with explicit UX framing, or (b) pull cloud-tier work into V0.2 scope, or (c) further latency-optimization rounds.

### Cross-references

- `crates/vault-retrieval/examples/t023_retrieval_diagnostic_results.md` — retrieval is healthy (recall@20 = 1.00 across real-query shapes)
- `crates/vault-retrieval/examples/t025_qwen_vs_split_results.md` — Pipeline A (Phi-4 split) is broken; Pipeline B (Qwen standalone) wins quality + speed
- `crates/vault-retrieval/examples/t026_qwen_7b_results.md` — Qwen-7B baseline: 4/4 contradictions, 2/2 hard-negatives, mean 187s — the number to beat
- BRD §5.5 line 869 — retriever 200ms latency contract applies to `Retriever::retrieve` ONLY; the new read-time stage has its own budget
- BRD §5.5 line 856 — V1 query classifier is heuristic, NOT LLM. The new Phi-4-free read-time stage is downstream of strategy execution, not in the classifier role.
- ADR-042 — Phi-4-mini selection (still valid for consolidator merge classification; not in read-time pipeline)
- ADR-043 — Model download + integrity verification pattern (Qwen-7B productionization will follow this when read-pipeline code lands)
- ADR-044 + Amendment 1 — LlmProvider trait + system_prompt option (consumed by spike code; production read-pipeline will reuse)
- ADR-045 §c — clustering quality gates (now superseded by ADR-045 Amendment 1 reframe — pending draft)
- ADR-046 — `mark_superseded` primitive (consolidator-side, unchanged by read-time work)
- ADR-047 — `summary.rs` file placement + RunState/AMWC field extensions (commit-3 staged content)
- `feedback_dont_propose_relaxation_for_speed.md` — don't propose relaxing the 2-min ceiling unless we've genuinely exhausted optimization
- `feedback_runtime_confirmation_after_web_spike.md` — empirical confirmation gates trump theoretical reasoning; t026 numbers are the canonical evidence

---

## Tech debt — open items

### T0.2.x — entity-extraction-at-consolidation + GraphStore relationship-rewrite primitive on merge

**Surfaced:** T0.2.3 commit 1 iteration 3 source-read of `crates/vault-storage/src/graph_store.rs:99-161` + `crates/vault-storage/src/cascading.rs:37-44`. **Logged:** T0.2.3 commit 1 (`5aeb5b3`). **Reaffirmed:** T0.2.3 commit 2 (`17035ec`) — `apply_merge` emits the `tracing::warn!` no-op pointing here.

**The gap.** BRD §5.6 line 950 verbatim: *"Update graph: relationships pointing to old memories now point to new merged memory."* That sentence presupposes two contract surfaces that don't exist yet:

1. **Entity extraction from `Memory.content` at consolidation time** — there is no production path that creates `graph_store::Entity` rows for memories. V0.1's `cascading.rs` graph-cascade scope was a no-op for memory writes; T0.2.3 `cascading.rs:37-50` comment block points here.
2. **A `GraphStore::rewrite_relationships_for_memory(old_id, new_id)` primitive** — the `GraphStore` trait has `create_entity` / `create_relationship` / `traverse` / `supersede_relationship` / `validate_readable`. None of them rewrite a batch of relationships when a source memory is superseded. Relationship endpoints are `EntityId` (not `MemoryId`) — a memory↔entity mapping doesn't exist either.

**T0.2.3 commit 2 disposition (shipped at `17035ec`).** `apply_merge` executes steps 1-3 of BRD §5.6 lines 947-950 verbatim (new memory creation + supersession + re-embed via cascade) but **skips step 4 (graph update) with `tracing::warn!`** and a doc-comment pointing here. The graph stays empty in V0.2 because the V0.1 cascade never wrote to it — no relationships exist to rewrite, so the no-op is honest about scope. β (also ship entity extraction at T0.2.3) was rejected as +2-3 weeks scope creep; γ (`todo!()` panic) was rejected because production runs would hit it on first merge.

**What lands at T0.2.x (this entry).**
1. **Entity-extraction primitive** in vault-consolidator (or vault-core if shared with future write-time extraction): given `&str` content, returns `Vec<EntityRef>` for ingestion. Likely Phi-4-mini-driven with custom system prompt (now possible per ADR-044 Amendment 1).
2. **Entity-row writes at consolidation time** through `GraphStore::create_entity` + relationships between co-occurring entities.
3. **`GraphStore::rewrite_relationships_for_memory(old_id, new_id)` new trait method** + DuckDB-backed impl. Additive to `GraphStore` trait.
4. **Phase 3 `apply_merge` graph-update step lights up** — `tracing::warn!` no-op replaced with the actual rewrite call. Existing Phase 3 unit tests get a graph-coverage extension.
5. **Tests:** entity-extraction unit tests (mock-LLM scenarios + edge cases), relationship-rewrite unit tests on DuckDbGraphStore, integration tests for the full Phase 3 path with non-empty graph state.

**Eventual contract reference.** BRD §5.6 line 950 verbatim is the locked spec contract; this entry tracks V0.2 deferral. BRD itself stays unamended — spec captures the eventual surface; this entry captures the V0.2 deferral.

**Affected files (forward-pointer audit trail).**
- `crates/vault-storage/src/cascading.rs:37-50` — comment block points here
- `crates/vault-consolidator/src/phases/merge.rs::apply_merge` — Phase 3 WARN-no-op site (T0.2.3 commit 2)
- BRD §5.6 line 950 — eventual-contract reference; do NOT amend the BRD until this tech-debt entry is closed

---

## Live V0.2-era ADRs — cross-link to archive

The following ADRs are LIVE for current V0.2 work. **Full text in `HANDOFF_V0.2_PART1_ARCHIVE.md`.**

- **ADR-044** — `LlmProvider` trait + `Phi4MiniProvider` implementation locks (T0.2.1 Phase 3). Defines the local-LLM contract surface consumed by vault-consolidator Phase 2 + future entity-extraction. §5 single-purpose constraint locks Phi-4 to merge-classifier role only; fixture generation must be hand-curated.
- **ADR-044 Amendment 1** — `CompletionParams::system_prompt: Option<String>` field for non-merge-classifier prompt shapes (T0.2.3 commit 1). Enables future entity-extraction call shape (T0.2.x) without forking the provider.
- **ADR-045** — T0.2.2 Phase 1 Cluster output contract + amendments. N-ary cluster shape, read-cost expectation pin, synthetic acceptance fixture recipe, contract-drift handoff to ADR-044 (resolved at T0.2.3 commit 1), forward-compat notes. §e RESOLVED as of T0.2.3 commit 1.
- **ADR-046** — `mark_superseded` primitive on StorageBackend + new `MemorySuperseded` audit variant (T0.2.3 commit 2). Metadata-only supersession update; preserves BRD §5.6 line 948 provenance fidelity; emits `memory.superseded` audit event distinct from `memory.update`. β-over-α partner-locked decision; rejected `Option<&[f32]>` API extension + rejected `MemoryUpdate`-with-cause-field. Single-supersession assumption documented with V0.3+ forward-revisit.
- **ADR-047** — `summary.rs` file placement + RunState/AMWC field extensions (T0.2.3 commit 3). New `src/summary.rs` file (BRD §5.6 lines 984-993 forward-compat note); 3 `pub(crate)` type promotions (`RunState` / `BoundarySummary` / `AppliedMergeWithContext`); `RunState` gains `started_at` + `duration`; `AppliedMergeWithContext` gains `merged_text` + `pre_merge_contents`. Documents BRD §5.6 line 971 ("separate summaries per boundary") vs T0.2.3 iteration 3 ("per-boundary sub-sections inside outer Run-scoped document") divergence as deferred reconciliation. Test floor +14 firm with per-add reasoning. Full ADR text above this section.
- **ADR-048** — Read-time pipeline architecture (T0.2.3 close). Two-stage pipeline (BGE retrieve top-20 → single Qwen-7B synthesis call). Rejects Phi-4 stage 2/2.5 split (t024 evidence) + Phi-4/Qwen two-model split (t025) + Qwen-14B (latency at t025). Folds the proposed ADR-045 Amendment 1 housekeeping reframe in as one paragraph: consolidator stays load-bearing for substrate hygiene but the canonical product-quality surface moves to this pipeline. Quality contract 4/4 + 2/2 pinned by `tests/read_pipeline_acceptance.rs`. Forward-compat: speculative decoding (Family B) as V0.2.x escape valve. Full ADR text above this section.
- **ADR-049** — Qwen2.5-7B-Instruct Q4_K_M model lock (T0.2.3 close). Apache 2.0, 128K context, ~4.36 GB. Rejects Phi-4-mini 3.8B (t024 1/8 contradictions), Qwen2.5-14B (t025 latency), sub-7B candidates (Shahbaz hands-on rubbish output per `feedback_no_sub_7b_models_for_synthesis.md`). Exception: Qwen2.5-0.5B as speculative-decoding draft for 7B target — output byte-identical to running 7B alone. Distribution follows ADR-043 download + SHA + revision-pin pattern when productionised. Supersedes ADR-042 for the read-time role; ADR-042's "CPU-only on all platforms for V0.2" framing updated in the HANDOFF "V0.2 backend + tuning config locked" section above (NOT via a formal amendment per iteration-2 shrink scope). Full ADR text above this section.

**V0.1-era ADRs (ADR-001 → ADR-030 + ADR-008 amendments)** — full text in `HANDOFF_V0.1_ARCHIVE.md`.

**Other V0.2-era ADRs in `HANDOFF_V0.2_PART1_ARCHIVE.md`:** ADR-037 (lancedb upgrade), ADR-038 (concurrent-upsert serialisation + LANCE_MEM_POOL_SIZE), ADR-039 amendment (Compact-then-Prune for partial-fragment deletes), ADR-008 amendment (V0.2 at-rest extension lock-in) + ADR-008 amendment v2 (AAD path semantics), ADR-040 + ADR-040 amendment (Keychain crate + master_key derivation) + ADR-040 amendment v2 (Signature fix), ADR-041 + ADR-041 plan iteration 2 (V0.1 VAULT_KEY → V0.2 keychain SQLCipher passphrase bridge), ADR-042 (Phi-4-mini-instruct selection), ADR-043 (model download + integrity verification), ADR-010 hard-gate-cleared note (T0.2.0 Phase 5 close).

---

## Standing rules (CLAUDE.md-promoted defaults)

Per CLAUDE.md project instructions + recurring partner discipline. Memory-stored full rules in `~/.claude/projects/C--Projects-GitHub-Memory-Vault/memory/`.

- **CI verification per-commit.** Every code commit must show CI green matrix-wide before staging the next. `gh run list --workflow=ci.yml -L 1`. Local DoD ≠ CI green (Windows + Ubuntu + macOS clean-room matrix is the canonical surface). Promoted from candidate to default at T0.1.10-close (2026-05-04); 6 vault-code data points then; reinforced through T0.2.0/T0.2.1/T0.2.2/T0.2.3 commit 1.
- **Strictly-serial cargo.** Never parallel cargo invocations on the same workspace — kills incremental cache, requires 30GB+ wipe + 30+ min rebuild. Order: check → test → clippy → fmt → git status.
- **Cargo on Windows = PowerShell.** ADR-006's bundled-sqlcipher-vendored-openssl chain needs Strawberry Perl path order (PowerShell has it; Bash MSYS2 perl lacks the modules). `LIBCLANG_PATH = $env:USERPROFILE\scoop\apps\llvm\current\bin` + `$env:PATH = "$env:LIBCLANG_PATH;$env:PATH"` every fresh shell.
- **Confirm before commit + push.** Single combined approval covers both per `feedback_confirm_before_commit_push.md`. Co-Authored-By: bare `Claude <noreply@anthropic.com>`, **no model qualifier**.
- **Admin-only changes ride with code.** HANDOFF.md edits + ADR-only updates + tech-debt notes bundle with next code commit. Saves a ~45-min CI cycle per admin commit.
- **fmt runs LAST.** Final `cargo fmt --all --check` must have no edits between it and `git add`. `git status --short` between final fmt and `git add` catches drift (e.g., Cargo.lock changes from cargo gate runs).
- **Surface plan amendments BEFORE code.** Recon-class amendments + signature changes + new primitives = partner-approval before implementation, not silent slip. `feedback_floor_forecast_is_pre_declaration_not_estimate.md`.
- **Read crate spec before drafting recommendations.** CLAUDE.md spec-read rule extends to recommendation drafting stage, not just code-writing. `feedback_read_spec_before_recommending_not_just_before_coding.md`.
- **HANDOFF line 4 is a lagging indicator.** For any current-state question, source-read the deepest "next-session opener" or "deliverables" block first + cross-check `git log`; line 4 only refreshes on next admin ride-along. `feedback_handoff_top_metadata_is_lagging_indicator.md`.

---

## Archive cross-links

- **`HANDOFF_V0.1_ARCHIVE.md`** — frozen 2026-05-06. T0.1.1 → T0.1.12 phase narratives, ADRs 001-036 full text, V0.1 alpha tech-debt closures, V0.1 plan-iteration histories. Cross-link out when V0.1 detail is needed; do NOT paraphrase.
- **`HANDOFF_V0.2_PART1_ARCHIVE.md`** — frozen 2026-05-13 (T0.2.3 commit 2 ship). T0.2.0 + T0.2.1 + T0.2.2 + T0.2.3 commits 1-2 narratives, ADRs 037-046 full text (including ADR-044 Amendment 1 + ADR-008 amendments + ADR-040 amendments + ADR-041 plan iteration 2 + ADR-041 final), all V0.2-era plan iterations, T0.2.0/T0.2.1/T0.2.2 commit 2 historical next-session openers. **Slim-restart point for V0.2 Part 2 work begins here.** Cross-link out for V0.2-Part-1 detail; do NOT paraphrase.

When V0.2 closes (T0.2.13 ship + V0.2 hard-gate clearance), an additional `HANDOFF_V0.2_PART2_ARCHIVE.md` will freeze V0.2 Part 2 (T0.2.3 commit 3 onwards through T0.2.13), and a fresh slim HANDOFF.md will open for V1.0 work per BRD §6.3.
