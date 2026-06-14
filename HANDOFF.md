# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)

**Last updated:** 2026-06-14 (session 2) — **CI ROOT CAUSE FOUND + FIXED (ADR-079): the Windows red was NOT a DuckDB-version problem — the GHA `windows-2025` runner migrated to Visual Studio 2026 (MSVC 14.51) on 2026-06-08→06-15, which REMOVED `stdext::checked_array_iterator`; DuckDB's bundled fmt still references it → C2061.** Fix = a `/FI` forced-include shim (`.github/msvc_fmt_secure_scl_shim.h`) that undefs `_SECURE_SCL` so fmt uses its raw-pointer branch, wired into both Windows CI jobs via `CXXFLAGS_x86_64_pc_windows_msvc` (cc-rs only; llama/Vulkan build untouched). **Prior session's `=1.4.4` pin correction + graph-filling (ADR-078) were already committed as `d2b9b9b` and pushed — but its CI run `27484651556` was RED (same fmt/format.h compile), NOT the predicted green; the 1.4.4-fixes-CI claim was a misdiagnosis (1.4.4 AND 1.5.3 bundle the same ancient fmt — neither escapes the VS2026 break).** Last actually-green commit: `d613614` (2026-06-10, before the image migration). This CI fix is CI-only; per Shahbaz, committing+pushing without a local build (the failure is CI-image-specific and cannot be reproduced on the local older-MSVC machine — local builds stay green regardless). Graph-filling/encryption-deferred detail from session 1 retained in §1 below.

> **How to read this file:** §1 is the only thing you must act on. §2–§5 are current ground truth (incl. the post-scale roadmap in §5). §6 onward is reference you pull from when planning. Deep detail (full ADR text, session-by-session history, tuning evidence) lives in the three archives — cross-linked by ADR number. **Do not paraphrase archived ADRs — quote them.**

---

## 1 · 🟢 ACTIVE TASK — deploy the self-maintaining worker (Pillar 2: scheduling + the latency wall)

> **🆕 SESSION 2026-06-14 — GRAPH-FILLING SHIPPED-pending-commit · encryption DEFERRED (premise falsified) · DuckDB corrected to 1.4.4 LTS.** All DoD gates green LOCALLY on a fresh `cargo clean` full-workspace rebuild (build `--all-targets` ✅ · workspace tests ✅ 0-fail · clippy `-D warnings` ✅ · fmt ✅). NOT committed; CI unverified.
>
> **1. Graph encryption — DEFERRED (ADR-077's "pure code now" premise FALSIFIED by spike).** DuckDB native encryption CANNOT meet our offline requirement on ANY version. A spike (an isolated throwaway crate + a vault-storage example, both since removed) proved: the bundled **mbedtls crypto module is READ-ONLY** — writing an encrypted DB demands `LOAD httpfs` (the OpenSSL extension), a **network-fetched** extension, which breaks local-first / offline / zero-knowledge. Confirmed on BOTH `1.4.4` LTS and `1.5.3`: *"read-only crypto module loaded … ensure httpfs is loaded"*. The only offline-write path is `force_mbedtls_unsafe` (insecure RNG — unacceptable). **Low risk to defer:** the graph holds ZERO user data in V0.2 + all real data (memories in SQLite, embeddings in Lance) is already sealed. **The real path when it matters: bundle the httpfs/OpenSSL helper INSIDE the app and `LOAD` it from a local file (no network)** — packaging work for the encryption task, deferred to when the graph holds shippable data. tech-debt #7 stays open, now with this finding.
>
> **2. DuckDB pin corrected `=1.10503.1` → `=1.4.4` LTS (Cargo.toml + NEW `vault-storage/build.rs`).** ADR-077 believed `=1.10503.1` was "1.4 LTS"; it is actually **DuckDB 1.5.3** (the crate's `1.105xx` scheme), which (a) is off the supported LTS line and (b) **fails to compile its bundled C++ on the Windows CI runner** (`fmt/format.h`) — that is why `a1c0ff9`'s CI run `27469473142` is RED. `=1.4.4` (the crate's pre-`1.105xx` simple scheme = libduckdb 1.4.x) is the real LTS, compiles clean locally on the full `--all-targets` workspace, and is the version we'd bundle the encryption helper for. The new `build.rs` emits `cargo:rustc-link-lib=dylib=rstrtmgr` + `rustc-link-arg=rstrtmgr.lib`: DuckDB 1.4's `AdditionalLockInfo` (the "which process holds the lock" feature) calls the Windows **Restart Manager**, which `libduckdb-sys`'s bundled build forgot to link — load-bearing for `--all-targets` + any future encrypted-`ATTACH` target.
>
> **3. GRAPH-FILLING BUILT — ADR-078 (§8.10), closes the tech-debt #2 "entity-extraction-at-consolidation" gap.** The consolidator now fills the knowledge graph: each fact's entities (people / places / things, typed) + relationships are extracted by the **SAME Phi-4 enrichment call** that already generates search aliases → **no second per-fact LLM call, no worse latency** (validated by a live tuned Phi-4 probe: combined aliases + entities + relationships in one call, single-word keywords preserved, correct typing, sensible directed links). New `phases/extract.rs` (best-effort parse + cleanup — maps labels, drops dangling links + dups — then **get-or-create** graph write, boundary-scoped, idempotent via the existing `content_fp`). New `GraphStore::get_entity` (the get-or-create lookup). `enrich.rs` makes the combined call (`generate_enrichment`); `enrich_facts` writes the graph AFTER the vector persists (so a transient graph failure never re-extracts into duplicate edges). Tests: vault-consolidator 134 + vault-storage 84, all green, 0 regressions; the real-Phi-4 quality probe is `#[ignore]`d in `phases::enrich`.
>
> **Working tree (uncommitted, awaiting commit + push approval):** `Cargo.toml` (DuckDB 1.4.4 + rationale comment) · NEW `crates/vault-storage/build.rs` (rstrtmgr) · `graph_store.rs` (`get_entity` + 4 tests) · NEW `crates/vault-consolidator/src/phases/extract.rs` (+8 tests) · `phases/enrich.rs` (combined call + probe) · `phases/mod.rs` · `consolidator.rs` (graph-write wiring + `EnrichmentReport` graph fields + e2e test) · this HANDOFF. **CI must be verified green after push** (cold DuckDB-1.4.4 build; should also clear the `a1c0ff9` Windows red).
>
> ### 🔚 NEXT SESSION OPENER — DEPLOY THE WORKER (Pillar 2)
>
> **STEP 0 — confirm the ADR-079 CI fix went GREEN** (`gh run list --workflow=ci.yml -L 1`). This is the FIRST thing to verify: `main` was RED for two commits (`a1c0ff9`, `d2b9b9b`) because the GHA `windows-2025` image migrated to VS 2026 (removed `stdext::checked_array_iterator`; ADR-079 §8.11). The fix is a `/FI` `_SECURE_SCL`-undef shim. **If the Windows leg is STILL red on the same `fmt/format.h` C2061:** the `<yvals.h>`-defines-`_SECURE_SCL` assumption was wrong — escalate to the heavier shim (include `<vector>` instead of `<yvals.h>`, or hand-provide the type gated on `_MSC_VER`). Do NOT start the worker task on a red `main`.
>
> **THE TASK — make the nightly consolidator run on its own AND actually finish.** Two halves (BRD §5.6 / roadmap §5 Pillar 2):
> - **Scheduling (T0.2.6):** `Consolidator::schedule()` is still `todo!()` (`consolidator.rs` ~line 940) and nothing calls it at app startup (`vault-app::Application::start_with_mcp`). Wire a daily run-at-`run_at` trigger that calls `run_consolidation_with_safety` (already built: 30-min timeout + cross-process lockfile + tracing span — it runs `run_consolidation` → `enrich_facts` (now also graph-fills) → `generate_reports`).
> - **The latency wall (the real work):** the full nightly run does NOT finish at ~90 facts on this hardware (Phi-4 ~9.8s/call CPU; enrich/extract is ONE call/fact, plus merge + per-pair contradiction). The fix that fits the laptop: **incremental runs** — enrich/extract is already fingerprint-gated (`content_fp` skips unchanged facts), so steady-state nightly runs touch only new/changed facts (seconds) and the backlog self-heals over a few nights. Add a per-run work cap if needed. Do NOT chase a full-90-facts-in-30-min run.
>
> **THEN:** real single-device dogfood of the self-maintaining vault → Pillar 3 (sync) or beta. Graph encryption (bundle httpfs locally) folds into the sync security review OR whenever the graph first holds shippable data.
>
> ### 🧹 Scratch to clean (NOT in repo)
> - `C:\Projects\duckdb-enc-spike\` — throwaway isolated crate that proved the 1.4.4 encryption blocker; `target/` already removed, source can be deleted.
> - `C:\Projects\mcp-probe\client.py` — prior session's MCP probe harness (still useful for dogfood).
>
> The original scale-validation opener below is historical context.

**Goal:** prove correctness is scale-invariant on a REAL vault that Antigravity opens, at 1k then 10k facts. We already proved it on the internal `scale_eval` harness (100/1k/10k identical scorecard) and live in Antigravity at 100 facts. This is the live confirmation at scale + the close of the arc.

### 🔚 NEXT SESSION OPENER (2026-06-12 close) — **RUN GATES + COMMIT the staged gap-#7 steer (bundle with more code); then Pillar 2 path** — READ THIS FIRST

**▶️ PRIMARY ACTION next session: run the DoD gates + commit the UNCOMMITTED gap-#7 agent-steer code** (3 MCP tool-description edits in `crates/vault-mcp/src/server.rs`, staged this session, NOT yet built/gated — Shahbaz deferred gates to "tomorrow with more code" to avoid a CI cycle on a tiny change). The edits: `memory_read` gains a **decompose-multi-intent + natural-phrasing** steer AND a **single-valued-conflict** steer (the car — prefer newer `as_of`/explicit replacement signal, "say which is current", "don't assume conflict when both can be true"); `memory_search` gains the **one-topic-per-call** steer (it already had natural-phrasing). These encode today's findings ([[project_reranker_brittle_on_terse_queries]] + the car decision). Reach every agent via `tools/list`. **Gate (workspace build 0-warn → clippy → fmt → `vault-mcp` tests) then commit + CI-green-verify.** Bundle with whatever the next code task is.

**THEN — the sequencing Shahbaz + I agreed (2026-06-12):** **(1)** the gap-#7 steer above (knocks out #7 + the car steer #4). **(2) Pillar 2 — auto-run consolidator** (scheduling + decay/archive #6 + checkpoint) — BUT it has a hardware wall: the full nightly run does NOT finish at ~90 facts on this machine (contradiction phase ~20s/Phi-4 call blows the 30-min budget), so Pillar 2 = *"make the nightly run complete (latency/perf) THEN schedule it"*, not just schedule. **(3) real single-device dogfood** of the self-maintaining vault. **(4) Pillar 3 — cross-device sync** (biggest/most security-sensitive; fold gap #5 graph-crypto into its security review). Lean: dogfood-first before sync.

**Wave 3 is COMPLETE — full results + vault-level replay in §13.1; Arc B (car/temporal) spiked + reverted in §13.2.** Both Flash (weak) and Opus 4.6 (strong) landed correct answers on essentially every trap. **KEY REFRAME (2026-06-12, Shahbaz):** since the agent produces CORRECT OUTPUT across every tested trap (incl. salary/allergy/wrong-neighbour), gaps **#1/#2 (read precision) are NOT confirmed-broken — they are agent-handled today**, same logic that closed the car. They drop from "must-fix" to **🟡 insurance** (build only if a correct fact gets truncated out of the agent's view at scale, or to harden Managed-mode where unknown weak agents connect). **No confirmed-broken output exists in the gap table.** Updated gap classification: §13.3 (NEW). **The PRIMARY ACTION further below (re-run Wave 3) is SUPERSEDED — Wave 3 is done.** Original Wave-3 instructions kept below for reference.

**What's done (2026-06-11).** (1) CI **green** on `d613614` (Gap-2, run `27277023260`). (2) Gap-2 **live-confirmed through the real MCP read path at 1k** — fresh `seeded-vault-1k` copy, bare vs enriched A/B both via `memory_read`/`memory_search`: buried Porto **ABSENT → rank 1** in search + present in read facts; twins/hives weren't buried. Nuance: hardest keyword-soup query enriched read still `abstain=True` (reranker scores the wording-mismatch low — enrichment fixes recall-into-pool, NOT the reranker score; never-empty returns the fact anyway → recall-safe). (3) **Full-aspect live test campaign on a NEW messy+clean dogfood vault** (`seeded-vault-mixed`, ~94 facts) via a scripted MCP stdio client — Antigravity quota hit (~10h reset) so I drove the MCP server directly. **Scorecard + failure root-causes in §13 (NEW).** No code change this session; CI stays green on `d613614`.

**▶️ PRIMARY ACTION — Wave 3: live-agent test in Antigravity once quota resets.** Config ALREADY repointed to `seeded-vault-mixed`. Restart Antigravity, confirm via `Get-CimInstance Win32_Process -Filter "Name='vault-cli.exe'" | Select CommandLine`, then run the 10 planted-trap questions (verbatim in `C:\Projects\mcp-probe\client.py` → `GRADE_QUERIES`; grading key in `SEED_NOTES`) on **BOTH a weak model (Gemini Flash) and a strong model (Gemini Pro)**. The question: does each model compose a CORRECT answer from the structured output — esp. the wrong-neighbor cases where a distractor ranks #1 (live→Lisbon, kids→Marcus, allergy→Marcus's peanut, salary→$450, car→Tesla)? Strong agent expected to recover; weak agent at risk — that delta IS the read-precision evidence.

**Optionally first:** enrich `seeded-vault-mixed` (surgical, like the 1k proof — extend `probe_real_enrichment_1k` to loop `enrich_one` over ALL active facts, no contradiction phase, no 30-min cap; one build) so Wave 3 also exercises the Gap-2 lift on messy data (Porto buried on keyword-soup). **Full `consolidate run` does NOT work on this hardware at ≥~90 facts** — the contradiction phase alone (~20s/Phi-4 call on the Intel UHD Vulkan GPU) blows the 30-min budget before enrichment runs (proven twice: the 100-probe + this session). The **TINY-vault path (≤~6 facts) DOES complete (27.6s)** — that's how merge/REPORT/enrichment were verified this session.

**Then the open arcs.** The retrieval *plumbing* is proven correct on messy data; the work now is the *precision/abstain* layer. **The 6 non-pass items from §13, each → its fix/build + priority (so nothing is lost):**

| # | Gap (§13) | Fix / build | Priority | Tracked |
|---|---|---|---|---|
| 1 | Salary $-trap (answers instead of abstaining on money-shaped noise) | read-precision: add a category/ownership veto + per-candidate filter so a confident wrong-*kind* match is rejected | 🔴 HIGH — but gate on Wave 3 first (see if a strong agent already recovers) | read-precision arc, roadmap §5 item 1; tech-debt #1 |
| 2 | Wrong-neighbor #1 ordering (mother/Marcus/dog out-ranks the user's own fact) | read-precision: a subject/ownership signal so "about the user" beats "about an associate" | 🔴 HIGH — same arc as #1 | roadmap §5.1; relates [[project_reranker_subjectless_facts_framing]] |
| 3 | Blood/OS marginal abstain (squeak over the no-signal floor) | read-precision: tune/curve the no-signal floor or per-candidate gate | 🟠 MED — same arc as #1 | roadmap §5.1 |
| 4 | Contradiction not resolved (Tesla/Rivian both stay active) | temporal: fact-time `as_of` (date extraction or settable) + tune the Phi-4 contradiction judge | 🟠 MED — own arc | §4 carried follow-up; [[project_as_of_write_time_blocks_a5_temporal]] |
| 5 | `graph.duckdb` plaintext | verify ADR-010 DuckDB-encryption status; wire it if truly unshipped | 🟢 LOW (graph empty in V0.2) | tech-debt #7 |
| 6 | Decay / archive not built | BUILD Phase 4 (age-out + archive old memories) | 🟢 planned BUILD (not a bug) | roadmap §5 item 2; §6 "Not built"; T0.2.4 |

**Honest sequencing:** #1–#3 are ONE arc (read precision, roadmap §5.1) and are the highest-value fix — but **run Wave 3 first**, because if the strong agent already composes correct answers despite the wrong-neighbor ordering, that re-prioritises how hard we push #2. #4 is its own (temporal) arc. #5 is low-pri tech-debt. #6 is a scheduled build, not a defect.

### 🧰 Scratch state (NOT in repo — clean up when done)
- **MCP probe client:** `C:\Projects\mcp-probe\client.py` — the scripted MCP stdio test harness built this session (modes: discover / inspect / measure / grade / crud_test / auth_test / search_test / isolation_test / seed_mixed / seed_tiny / car_check / write_killers). Run: `$env:PROBE_VAULT=<vaultdir>; $env:BOUNDARIES='personal,testeval'; python client.py <mode>`.
- **Scratch vaults (all throwaway dogfood):** `seeded-vault-mixed` (~94 messy+clean, Wave-3 target), `seeded-vault-tiny` (6-fact consolidation demo — MERGED + REPORT written), `seeded-vault-1k-probe` (3 killers enriched), `seeded-vault-1k-bare` (3 killers bare), `seeded-vault-100-probe`. Real evidence vaults `seeded-vault-{100,1k,10k}` untouched.
- **Antigravity config** points at `seeded-vault-mixed`. Backups: `mcp_config.json.bak-1k` (was 1k), `mcp_config.json.bak-realvault` (real production vault). **Restore real vault when fully done:** set the 3 paths back to `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\{vault.db,lance,graph.duckdb}`, restart.

**Tech-debt #6** (cheap, ride with the next code commit): `--test-threads=1` on `ci.yml:702` to re-light the weekly smoke. **Tech-debt #7 (NEW):** verify `graph.duckdb` encryption — ADR-010 scoped it for T0.2.0 but the store still opens PLAINTEXT (`DUCK` magic bytes confirmed) and the runtime still WARNs "ships in T0.2.0"; low risk (graph empty in V0.2) but claim/reality diverge.

> **🟢 Plumbing solid on messy data.** Storage / retrieval / security / structural aspects all PASS (§13). Remaining work is the precision/abstain layer + temporal resolution — known, scoped, the 85→100 arc. Founder thesis is *correct output*, so Wave 3 (does a real agent land the right answer from the structured output) is the next acceptance.

### 📌 Reference — seed / verify / repoint commands (1k is DONE + live; reuse these for the 10k re-seed in step 4 above)
1. **Seed** (swap `SEED_N`/`SEED_VAULT_DIR` for 10k): `$env:SEED_N='1000'; $env:SEED_VAULT_DIR='C:\Projects\seeded-vault-1k'; cargo test -p vault-app --test scale_eval seed_live_vault -- --ignored --nocapture` — 1k ≈ 17 min; 10k = multi-hour/overnight (drain rate degrades). Waits for full VECTOR-count drain, then prints the test script.
2. **Verify the seed:** `$env:LANCE_MEM_POOL_SIZE='268435456'; & "C:\Projects\GitHub\Memory Vault\target\debug\vault-cli.exe" --vault-db C:\Projects\seeded-vault-1k\vault.db --vector-dir C:\Projects\seeded-vault-1k\lance --graph-db C:\Projects\seeded-vault-1k\graph.duckdb divergence-check` — expect `sqlite == vector`, **no findings**.
3. **Repoint Antigravity:** edit the REAL config `C:\Users\shahb\.gemini\config\mcp_config.json` (the `~/.gemini/antigravity/mcp_config.json` is a SYMLINK to it — edit the real target). Change the 3 vault paths (`--vault-db`/`--vector-dir`/`--graph-db`) to `seeded-vault-1k`. **Restart Antigravity.** Confirm: `Get-CimInstance Win32_Process -Filter "Name='vault-cli.exe'" | Select CommandLine`.
4. **Run the 15 questions** (`crates/vault-app/tests/fixtures/scale_eval.json`; seeder prints them with expected answers). Watch: #6 cello (subject-less fact), #12 salary + #14 cat-breed (the Thread-2 precision traps), the 5 abstains.
5. **Seed + test 10k** the same way (`SEED_N='10000'`, `C:\Projects\seeded-vault-10k`). **10k seed is MULTI-HOUR** (~1s/vector × 10k, degrading) — plan an overnight run. Verify + repoint + test as above.
6. **If 1k AND 10k both pass live →** (a) commit the seeder (`crates/vault-app/tests/scale_eval.rs` `seed_live_vault` + the vector-count-drain probe) with full DoD gates → CI green; (b) declare the retrieval core "battle-tested at scale," close this arc. Thread 2 (read precision) becomes the next arc.

### ⚙️ Working-tree state
- **Last SHIPPED: `d613614`** (Gap-2 / ADR-074), pushed to `main`; **CI `27277023260` was `in_progress` at session close — verify `success` first thing next session.** Prior green: `a3e426b` (ADR-073, run 27150216167).
- **Uncommitted: HANDOFF.md ONLY** (this session-close opener rewrite + "Last updated") — admin-only, rides with the next code commit per admin-rides-with-code. The working tree is otherwise clean (the fix below is committed in `d613614`).
- **Shipped in `d613614`** (one commit, admin rode with code):
  - **NEW `crates/vault-consolidator/src/phases/enrich.rs`** — `generate_aliases` (Phi-4, JSON `{"aliases":[...]}`, tuned to single-word generic keywords), `compose_embed_text` (`"<content> Topics: <aliases>"`), `content_fingerprint` (FNV-1a), `set_enrichment_metadata`, **`pub enrich_one`** (exposed for the live probe) + 11 mock-LLM unit tests + `real_phi4_alias_quality` `#[ignore]` probe.
  - **`crates/vault-consolidator/src/consolidator.rs`** — `Consolidator::enrich_facts` + `EnrichmentReport` + 2 idempotency tests; `phases/mod.rs` (+`pub mod enrich`); `lib.rs` (export `EnrichmentReport`).
  - **`crates/vault-app/src/application.rs`** — `enrich_facts` wired into `run_consolidation_with_safety` (after consolidation, before `generate_reports`, under the 30-min budget).
  - **`crates/vault-app/tests/scale_eval.rs`** — NEW `probe_real_enrichment_1k` `#[ignore]` (real-Phi-4 end-to-end 1k rank-lift A/B) + the two `map_or(true, …)`→`is_none_or` clippy fixes. The ruler (this + `scale_eval.json` `_phrasing` variants + drain-poll fix + 3 `#[ignore]` probes from 2026-06-09) rides with this commit per commit-only-with-tested-fix.
  - **`HANDOFF.md`** — §8.6 ADR-074 full text + §9 index + this state + opener + "Last updated".
- **Build cache:** full `cargo clean` + cold rebuild done this session (disk had hit 0 GB during the first gate run; clean reclaimed 137 GB). Build was 36m38s; gates ran clean after.
- **Memories updated (outside repo, 2026-06-09):** `project_1k_live_paraphrase_recall_miss` REWRITTEN (Gap-2 re-diagnosis + proven fix) + MEMORY.md index line.
- **Memories updated (outside repo):** `project_1k_live_paraphrase_recall_miss` REWRITTEN (Gap-2 re-diagnosis + proven fix; old framing marked falsified) + MEMORY.md index line.
- **Scratch on disk (not repo):** `C:\Projects\seeded-vault-1k-probe` (a throwaway copy used by the probes — safe to delete; `Remove-Item` before re-copy since `Copy-Item -Force` MERGES). The real evidence vault `C:\Projects\seeded-vault-1k` is untouched.

### 🔧 Antigravity config — state + revert
- **Backup of the original real-vault config:** `C:\Users\shahb\.gemini\antigravity\mcp_config.json.bak-realvault`.
- **To restore the real vault when done:** set the 3 paths back to `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\{vault.db, lance, graph.duckdb}` in `~/.gemini/config/mcp_config.json`, restart Antigravity.

### 🧠 Seeder gotcha (don't regress)
Confirm drain by **VECTOR ROW COUNT** (`LanceVectorStore::count`), NOT by searching for a sentinel fact — search finds a fact via the keyword channel BEFORE its vector lands in LanceDB, which once shipped a **1-of-101-vector vault** (caught only by `divergence-check`). The seeder polls a re-opened `LanceVectorStore` count each tick until `== total`. A freshly-seeded vault shows a cosmetic `REPORT_MISSING` / `status: degraded` health warning (consolidator hasn't run) — harmless, does NOT affect answers; clear later via `vault-cli consolidate run` (needs `--phi4-model`; do it when the MCP server is NOT holding the vault — single-writer).

---

## 2 · 🧭 Where the build is

V0.2 read/consolidate core is functionally complete and CI-green. The work since T0.2.3 was a long correctness-at-the-output arc (the founder thesis: *"memory is only useful if the output is correct"*). Net result:

- **Read path** returns structured facts, NO LLM at read (`StructuredReadPipeline`, ~500ms). The calling agent composes the answer. Recall-first by lock: never false-empty.
- **`memory_read`** is the primary answer path (returns structured `abstain`); **`memory_search`** is reorder-only + recall-safe (never false-empties) with an additive `weak_match` hint. (ADR-066/069/071)
- **Reranker** (Qwen3-Reranker-0.6B, cross-encoder) is the read relevance authority, lazily loaded off the MCP handshake. (ADR-059/070)
- **Consolidator** produces a per-boundary REPORT (structured knowledge state) nightly; contradiction detection is nearest-neighbor based. (ADR-053/065)
- **Cross-agent proven:** Claude, Cursor, Antigravity all read the vault correctly. Validated at 100 facts live across both tools and both model tiers.
- **Scale:** `scale_eval` harness shows correctness is scale-invariant 100→1k→10k (identical scorecard). The one 10k internal crash (a flaky, data-safe storage-worker race) is fixed + shipped (ADR-072).

**Last shipped commit:** `da10c0f` (ADR-072, 10k TOCTOU fix), CI-green run `27096332980`. Recent chain: `a3c938b`→`661d391`→`a1e4dac`→`da10c0f` all matrix-clean.

**The locked arc** ([[locked-next-arc-t03x]], amended 2026-05-26) — all four steps SHIPPED:
1. ✅ MCP `memory.write` description hardening (`93d1410`)
2. ✅ Consolidator → REPORT (Batch A, `f0cc158`, ADR-053)
3. ✅ Read returns structured facts, no LLM at read (Batch B Commit 6, `99052f2`, ADR-052/054)
4. ✅ Consolidator wired into runtime + manual CLI trigger (`f0cc158`)

Phase C (write-time decision loop) DEFERRED to V1.0+.

---

## 3 · 🔒 Architectural locks (do not relitigate without explicit founder sign-off)

- **LLM is OUT of the read path** (2026-05-26). The read consumer is itself an LLM (the agent); pre-composing prose was redundant. Vault returns structured facts; agent composes. Delivered ~170× local speedup, ~50× BYOK cost cut, ~10× Managed PAYG margin. Phi-4-mini stays at nightly consolidation only. [[project_architectural_lock_llm_out_of_read_path]]
- **Recall is sacrosanct.** A false-abstain (vault has the answer but says "I don't know") is the cardinal sin — far worse than a false-answer. Every read/search change is recall-safe by construction: reorder-only, never false-empty. [[project_memory_read_primary_search_recall_safe]]
- **Correctness of output IS the product.** Storage + retrieval are table stakes; correct output to the agent is the differentiator. Don't burn cycles on prose polish when the structured field is already correct. [[project_correctness_is_the_product]] · [[feedback_structured_contract_user_sees_via_agent]]
- **Correctness before latency** (V0.2). Get core quality to 100% first; don't preempt latency work until the founder signals the core is structurally solid. [[project_correctness_before_latency]]
- **Three-mode deployment** (Local $10 / BYOK $5mo / Managed PAYG) shares one codebase; every architectural decision must be mode-agnostic. Managed = per-user vault + per-user key. [[project_three_mode_deployment]] · [[project_managed_mode_per_user_vault]]
- **Zero-knowledge guarantee:** the server cryptographically cannot read vault contents. No crypto-path change without re-reading BRD §11 + an ADR-SEC entry.
- **Never recommend sub-7B models for read-time synthesis** (Qwen2.5-7B is the quality floor) — moot now that the LLM is out of read, but stands if read-synthesis is ever revisited. [[feedback_no_sub_7b_models_for_synthesis]]

---

## 4 · 🟠 Open threads (next arcs, NOT blockers for the 1k/10k validation)

### Thread 2 — retrieval vocabulary gap (Gap 1 SHIPPED; Gap 2 IMPLEMENTED — ADR-074, gates green, pending live validation + commit) — own arc, ACTIVE
**Status RE-DIAGNOSED 2026-06-09** (ground-truth probe on the real `seeded-vault-1k`, 3 domains — see §4.2 below; falsifies the 2026-06-08 framing). Gap 1 (read false-abstain) is SHIPPED (ADR-073). **Gap 2 is NOT "BGE can't handle paraphrase/idiom"** — natural idioms work fine ("call home" → Porto rank 1). The real root is a **vocabulary gap**: a fact phrased without the obvious keyword ("settled in **Porto**", "raising **twins**", "comes out in **hives**") gets outranked by — or in a dense-distractor field drops below — facts that carry the literal keyword. **The proven fix is document-side alias enrichment, NOT query expansion** (which backfires — it IS the keyword-soup that triggers the miss). Full evidence + fix validation in §4.2.

#### Gap 1 — read-gate false-ABSTAIN (gate layer; fact IS retrieved, gate drops it)
**The bug (confirmed live, 1k vault).** `memory_read` **false-abstained** — returned `relevant_facts: []`, `abstain: true` — on facts that ARE in the vault:
- *"how do I stay fit"* and *"exercise running cycling"* → both `abstain: true`, even though *"runs ten kilometres three times a week"* AND *"cycles to the office"* are stored (the 2nd query literally contained "running"/"cycling"). The agent only recovered by falling back to `memory_search`. A weaker agent would have told the user "I don't have that" — the exact cardinal sin the recall-safe lock exists to prevent ([[project_memory_read_primary_search_recall_safe]]).

**Root cause (measured).** `memory_read` abstains on an **absolute reranker floor (ADR-059: logit 0 = relevance 0.50)**. But the reranker scores real answers far below that — and is sometimes actively wrong:

| live query | top relevance | #2 | separation | `weak_match` | truth |
|---|---|---|---|---|---|
| "stay fit" | 0.0388 | 0.0061 | ~6× clear winner | false | real answer (runs 10km) — **read abstained** ❌ |
| "morning routine" | 0.5256 (cycles) / 0.18 (flat white) | — | — | false | both real; flat white below 0.5 floor |
| "what does the user eat" | 0.0639 | 0.0473 | ~1.3× murky | true | real answer (*Japanese cuisine* didn't even make search top-10 — ranked below cafeteria-noise) |
| "operating system" (absent) | 0.000065 | 0.000055 | flat | true | genuinely nothing — abstain correct ✅ |
| "cat breed" (absent; dog present) | 0.00028 (dog) | 0.00003 | ~9× | false | no cat — dog is no-signal-level; agent correctly said "no cat, but a dog" ✅ |

Two takeaways: (1) **real answers live at relevance 0.04–0.99; no-signal/wrong-neighbor lives at 0.00006–0.0003** — a ~100× gap. The logit-0 (0.50) floor sits on top of the real answers and mows them down. (2) `memory_search` already gets all these RIGHT (separation-based, never empties); only `memory_read`'s gate is broken. ADR-066 said "reranker is a re-orderer, NOT a precision authority" — yet ADR-059 still uses its absolute score as the abstain gate. That contradiction IS the bug.

**The fix (3 parts, every threshold backed by the live data above).**
1. **Kill the logit-0 abstain floor** in `memory_read` (the whole false-abstain).
2. **Adopt `memory_search`'s separation-based logic** + a *much* lower no-signal floor (~relevance 0.001). Real answers (≥0.04) clear it; C7/C8 (≤0.0003) don't. (Separation alone is insufficient — C8's dog separated 9× yet is no-signal-level — so combine separation with the low absolute floor.)
3. **Never hard-empty `relevant_facts`.** Even when `abstain`-leaning, return the top candidates + a `weak_match`/confidence hint and let the agent judge. Proven live: given the dog fact, the agent correctly abstained on "cat" while surfacing the dog. `abstain` becomes a *hint*, not a fact-shredder.

Net: make `memory_read` behave like `memory_search` already does. The over-inclusion/false-answer side (salary→$, cat→dog, keyboards-leak) is the *same* root (absolute reranker score is an unreliable gate) and the weak-match hint covers it too — the agent judges instead of the vault hard-deciding.

#### §4.2 Gap 2 — RE-DIAGNOSED 2026-06-09 (ground-truth probe, fix proven)
**What it is NOT.** The 2026-06-08 framing ("the idiom 'call home' misses Porto; fix = vault-side query expansion") is **FALSIFIED**. Ground-truth probing of the real `seeded-vault-1k` (new `probe_live_vault` / `probe_family_domain` / `probe_enrichment` tests in `scale_eval.rs`, run live across 3 domains — location, relationships, health) shows the bare idiom finds the fact fine: *"where does the user **call home**"* → Porto **rank 1** (0.4339); *"live"* → rank 1 (0.95); *"is the user married?"* → rank 1.

**What it actually is — a VOCABULARY GAP, two failure modes.** A fact phrased *without* the obvious keyword — "settled in **Porto**" (not "lives in"), "raising **twins**" (not "kids"), "comes out in **hives**" (not "allergy") — is outranked by, or (in a vault with a DENSE field of lexically-overlapping distractors) drops out of the candidate pool below, facts that carry the literal keyword:
1. **Recall miss** under dense matching-domain noise. The ONE outright miss was the agent's **keyword-soup** query `"home location city country lives residence"` → Porto **ABSENT**, top 0.0013, buried under Salt-Lake-City/travel distractors. (That Salt-Lake-City pool is exactly what 2026-06-08 mis-pinned on "call home".) Sparse domains (family/health) don't bury the target, but →
2. **Confidence collapse.** Keyword-soup queries score ~0.008–0.03 (no-signal level) → `memory_read` abstains even when recall holds. And a 3rd-party fact carrying the keyword ("Marcus carries an epipen for his peanut allergy", 0.96) outranks the user's own answer ("comes out in hives", 0.18) for "is the user allergic?".

**So keyword-padding is the TRIGGER, not the cure** — vault-side query expansion would replicate the harmful soup. The fix is **document-side**.

**The fix — PROVEN by A/B probe (`probe_enrichment`).** Enrich each fact's *embedded text* with normalized aliases/topics. Measured on the hardest case: bare Porto **ABSENT** → enriched Porto (`"…Topics: home, lives, residence, city, country, location"`) **rank 1 @ 0.9965** on the exact killer keyword query, with **no regression** on natural ("where does the user live": enriched #1 / bare #2). Twins: bare rank 5 → enriched rank 1 (natural AND keyword). **Where it lives:** the consolidator's Phi-4 pass already touches every fact → generate the alias/topic line there (fits [[project_locked_next_arc_t03x]] consolidator arc; keeps the LLM out of the read path). **Query-side expansion SHELVED; stronger embedder = last resort.** Full detail: [[project_1k_live_paraphrase_recall_miss]].

**Decision LOCKED 2026-06-09: generate aliases with Phi-4 at consolidation (Option B), NOT write-time agent aliases (Option A).** Rationale + recon in §1 opener step 1. Remaining ADR-074 specifics to lock: (a) Phi-4 alias prompt + output format; (b) `metadata` storage key + embed-text composition (`content + aliases`); (c) when it runs / re-embed cost (backfill of existing facts is the point). A deterministic synonym map was rejected — "settled in Porto" → "home/residence" needs comprehension, not a thesaurus.

#### Harness gap — root was DEEPER than "favorable phrasings" (FIXED 2026-06-09)
`scale_eval`'s `scale_correctness_eval` scored **"false-abstain: 0 / recall perfect"** at 1k/10k for TWO reasons: (1) favorable fixture phrasings (added plain/idiom/keyword `_phrasing` variants + a per-phrasing recall scorecard), and — the deeper one — (2) its readiness poll broke at *"Rivian searchable"* (BM25 hits before the vector lands), so the query pass ran against a **half-drained vector store** (`ready after 0s` vs the honest `1546s`) — almost no distractor competition → artificially perfect recall. **Fixed:** the poll now waits for `LanceVectorStore::count == total` (mirrors `seed_live_vault`). NOTE: even fully-drained at 1k the in-process harness can't reproduce the keyword miss without the dense-distractor condition — the faithful repro is the real-vault probe (`PROBE_VAULT_DIR`). The ruler (variants + drain fix + 3 probe tests) is uncommitted; it rides with the Gap-2 fix commit per commit-only-with-tested-fix.

**Verdict:** engine solid + premium experience excellent (Opus: rich answers, graceful abstains on blood-type/salary/OS, never hallucinated the salary-$ or cat→dog traps, offered to save missing facts). But these two recall-robustness gaps gate the "battle-tested" call. **Full evidence:** [[project_1k_live_read_false_abstain]] + [[project_1k_live_paraphrase_recall_miss]] + this session's 1k Antigravity transcript (17 queries). Related tech-debt #1 (carry-cosine-through-fusion + per-candidate filter, §8) is the same surface as Gap 1.

### Carried follow-ups (not blockers)
- **REPORT_MISSING cleanup** — run the consolidator on the live seeded vaults to clear the cosmetic `status: degraded` warning (needs `--phi4-model`, server not holding the vault).
- **`max_results` 10 → 5** — proven safe at top-5; one change at a time.
- **Antigravity `instructions.md` rewrite** — steer agents to prefer `memory_read`; empty result = not in vault.
- **`as_of` is write-time, not fact-time** — content dates aren't parsed; blocks the A5/A4 temporal contradiction cases. Open decision: settable `as_of` vs date-extraction. [[project_as_of_write_time_blocks_a5_temporal]]

---

## 5 · 🗺️ Post-scale roadmap (V0.2 remaining) — pick the start point

Once the 1k/10k live test passes (§1), the retrieval **core** is proven correct + scale-solid. These four pillars complete V0.2. Founder picks where to start; my recommended order is **1 → 2 → (fork: 3 or 4)**.

**1. Read precision (Thread 2) — close the last known quality gap.** 🟢 *recommended first*
The vault sometimes returns a confident wrong-neighbor instead of abstaining ("salary?" → catering $; "cat?" → the dog; "instrument?" → cello-correct + keyboards leaked). Fix = recall-safe `weak_match` hint on `memory_read` (let the agent judge, never drop a fact). Contained, high-value, squarely the "correctness IS the product" thesis. Full detail in §4 (Thread 2). Related: tech-debt #1 (carry-cosine-through-fusion + per-candidate filter) in §8.

**2. Sleep consolidator — make it run on its own.** 🌙
Today the nightly brain (merge duplicates, surface contradictions, build the REPORT) only runs when manually triggered. For real users it must run automatically + safely. Three pieces unbuilt (see §6 "Not built"): **Scheduling** (T0.2.6 — nightly cron), **Phase 4 decay + archive** (T0.2.4 — old memories fade gracefully), **Checkpoint + rollback** (T0.2.5 — undo a bad run). This turns a manually-poked library into a self-maintaining vault — the headline word in the V0.2 spec. Pair with sustained founder dogfood.

**3. Cross-device sync (`vault-sync`) — the big multi-device feature.** 🔄
The V0.2 promise: your memory on every device, readable by any agent, **without the server ever reading it** (zero-knowledge sync). Largest + most security-sensitive surface → re-read BRD §11 first, ADR-SEC entries required. **Ship gate:** tech-debt #4 (`pending_sync` sweep + migration 0003 payload, §8) MUST land before sync beta opens.

**4. Beta packaging + 30 real users.** 🚀
Onboarding flow, desktop-app polish, getting it into hands. The V0.2 finish line (BRD §6.2: 30 beta users).

**The one real fork (a couple weeks out, founder's call):** after 1 + 2, do **sync first** (full multi-device vision before anyone tries it, longer to first users) **or beta-on-one-device first** (real users + feedback sooner; even single-device the vault is genuinely useful; sync follows). Recommendation leans beta-first per the bootstrap reality — get one device perfect + dogfood-proven before taking on the heavy sync surface.

---

## 6 · 📦 Consolidator inventory — what's built vs not (read FIRST when planning consolidator work)

`vault-consolidator` has ~1,000 LOC production + ~1,200 LOC tests. Do NOT re-discover.

**Built + tested ✅**
| Component | File | Notes |
|---|---|---|
| Phase 1 — Clustering | `phases/cluster.rs` | Cosine ≥ 0.92, top-5 NN, union-find transitive closure, deterministic. Re-embeds (metadata `Memory.embedding` is `None`). ADR-045 |
| Phase 2 — LLM decide | `phases/merge.rs::decide_merge` | JSON-schema `LlmProvider::complete_json` → `MergeOutcome::{Merge, KeepSeparate, Contradiction}`. ADR-044 |
| Phase 3 — Apply merge | `phases/merge.rs::apply_merge` | Summed `access_count` + max `confidence`, marks originals superseded (ADR-046), re-embeds. Graph rewrite WARN-deferred (tech debt §7) |
| Orchestrator | `consolidator.rs::run_consolidation` | All non-superseded → group by boundary (`BTreeMap`, deterministic) → Phase 1→2→3 → `ConsolidationReport` |
| Topic discovery | `topics.rs` | Connected-components (NOT K-means — ADR-068) |
| REPORT artifact | `report.rs` | Per-boundary structured JSON, atomic write. ADR-053 |
| Run-summary audit | `summary.rs` | Per-boundary Markdown, privacy-leak tested. ADR-047 |
| Runtime wiring | `vault-app::run_consolidation_with_safety` | Cross-process lockfile + 30-min timeout + tracing span |

**Not built ❌**
| Gap | Scoped | Status |
|---|---|---|
| Phase 4 — Decay + archive | T0.2.4 | Never started; `memories_archived` returns 0 |
| Checkpoint + rollback | T0.2.5 | Never started; `checkpoint_id` = literal `"pending-T0.2.5"` |
| Scheduling | T0.2.6 | Never started; `Consolidator::schedule()` is `todo!()`. Runs only when `run_consolidation()` is invoked |
| `invalidate()` consumption | T0.2.7 Phase B | Contradictions queue to `ConflictReview`; bi-temporal `invalidate()` (ADR-051) not yet called. Partially addressed via REPORT auto-resolution on `clear_winner` |

---

## 7 · 🧰 Technique map (locked 2026-05-26) — summary

Mapped against: **A** Write · **B** Read · **C** Consolidate · **D** Sync · **E** Scale · **F** Privacy. Full table in PART2 archive.

- **Keeping:** HNSW (LanceDB top-K), cascading writes, std hashing, CoW-via-SQLite-WAL+Lance, Phi-4-mini at consolidation, BGE-small-en-v1.5 embedder, Tantivy BM25 + RRF + abstain.
- **Added this arc:** connected-components topic discovery (C), token-budgeted structured packing at read (B), startup wiring + CLI subcommand.
- **Deferring:** Cuckoo filters (sync, V0.2.9-13); per-tenant sharding / consensus / replication (V1.0+ Managed — prefer managed Postgres/Spanner over hand-rolled Raft).
- **Dropped (wrong tool):** Bloom filters, Z-order/Morton, quad trees, skip lists, external sorting.
- **Dead:** speculative decoding + the 120s p99 ceiling (Qwen is out of the read path).

The lock SIMPLIFIED the menu. The vault needs brilliant plumbing (filter + structure + pack), not exotic structures.

---

## 8 · 🐛 Tech debt — open items (live forward-pointers)

Full narrative for each in PART2 archive ("Tech debt — open items"). File pointers kept here so they don't lose their anchor.

1. **Read-relevance: per-candidate cosine filter + carry-cosine-through-fusion + retire vestigial BM25 gate.** Carry raw semantic cosine through `HybridRetriever` fusion onto `RetrievedMemory` (today `hybrid.rs:221-247` discards it), then filter per-candidate → removes double-embed, enables per-candidate precision filtering, lets the BM25 gate be retired. Closely related to Thread 2. Files: `vault-retrieval/src/strategies/hybrid.rs:221-247`, `structured_read_pipeline.rs`, `strategies/abstain.rs`. (Surfaced ADR-057)
2. **🟢 LARGELY CLOSED 2026-06-14 (ADR-078, §8.10).** Entity-extraction-at-consolidation is now BUILT — the consolidator extracts + writes entities + relationships per fact via the combined Phi-4 enrichment call (`phases/extract.rs` + `enrich_facts`). **Remaining tail:** `GraphStore::rewrite_relationships_for_memory(old, new)` for the merge path — a fact whose *content* changes re-extracts but leaves the prior content's relationships behind; `apply_merge` still has its graph-update `tracing::warn!` no-op (`phases/merge.rs::apply_merge`). Low priority while the graph is dogfood-only. Do NOT amend the BRD until the merge-rewrite tail closes.
3. **`VaultError::Storage(String)` grab-bag → structured variants.** `retry_queue.rs::is_permanent` substring-matches lance error wording (fragile; lance 4.0 wording is inconsistent). Add `SchemaMismatch`/`IoFailure`/etc., re-categorise ~30 call sites, rewrite `is_permanent` as exhaustive `match` + tripwire test. Files: `retry_queue.rs:240-275`, `vault-core/src/error.rs:139`, the ~30 `Storage(format!(...))` sites.
4. **✅ CLOSED 2026-06-13 (ADR-076, §8.8).** `pending_sync` sweep + migration 0003 cascade payload. Migration 0003 added `sequence_id` + `payload`; the overflow path persists the full cascade and `StorageBackend::drain_pending_sync` re-enqueues it (the `DivergenceDetector` Tier-0 sweep). The V0.2-sync ship-gate is met. (Note: stored the raw cascade `payload` rather than the sketched `embedding`/`boundary` columns — more faithful + version-agnostic.)
5. **Cosine NaN-vector lance upstream issue (LOW — community citizenship).** lance 4.0 filters NaN-distance rows from Cosine search (zero-magnitude vectors). Production unaffected (BGE vectors are L2-normalised, never zero). File a minimal-repro issue against `lancedb/lance`. File: `vector_store.rs:1248-1263`.
6. **🟡 Min-fix LANDED 2026-06-13 (`--test-threads=1` added to `ci.yml:702`); verify on the next Monday cron. Deeper unique-`.partial` fix still open (LOW).** Weekly real-model smoke red since 2026-05-18 — concurrent-download race (CI-infra, NOT a code regression). The `real-model-smoke` weekly cron job (`ci.yml:702`, `cargo test -p vault-llm -- --ignored`) has failed every Monday across 4 unrelated commits (`4ae8dbd`/`93d1410`/`2302842`/`a3e426b`). Root cause (source-confirmed): all 3 smoke tests run concurrently (no `--test-threads=1`) and `model_loader.rs::download_with_verify` writes to a **single shared** `.partial` path (`model_loader.rs:131`) then renames to final (`:156`); the winner's rename leaves the losers' rename hitting a vanished `.partial` → `Io NotFound code 2`. The test's own doc (`phi4_mini_smoke.rs:47-48`) assumes serial execution. **Min fix (CI-only):** add `--test-threads=1` to `ci.yml:702` — verifiable only via next Monday cron or a `run-llm-smoke`-labelled PR. **Deeper latent bug (LOW, prod single-writer + pre-download mitigates):** the shared `.partial` path means two cold-starting agent processes could corrupt each other's download — make `.partial` unique per download + treat "final already present after our stream" as success. Matters because this job is the ONLY CI coverage of the real Phi-4 consolidator path (dark for a month); re-light before leaning on the consolidator (roadmap §5 item 2). Files: `ci.yml:702`, `model_loader.rs:95-160`.

7. **`graph.duckdb` plaintext + native-encryption dead-end (LOW — graph empty in V0.2).** DuckDB native encryption can't write an encrypted DB offline on any bundled version (mbedtls is read-only; secure write needs the network `httpfs`/OpenSSL extension). Real path: bundle the httpfs/OpenSSL helper INSIDE the app and `LOAD` it from a local file. Fold into the Pillar-3 sync security review or whenever the graph first holds shippable data. (ADR-078 §8.10.)
8. **Remove the VS2026 `_SECURE_SCL` `/FI` shim (LOW — CI-infra workaround).** `.github/msvc_fmt_secure_scl_shim.h` + the two Windows CI steps + `CXXFLAGS_x86_64_pc_windows_msvc` exist only because DuckDB's bundled fmt references the removed `stdext::checked_array_iterator` (ADR-079 §8.11). Delete once `libduckdb-sys` vendors a newer fmt (or drops the `stdext` usage). Files: `.github/msvc_fmt_secure_scl_shim.h`, `.github/workflows/ci.yml` (clippy + build jobs).

Also tracked as SHIPPED-design-record in PART2 archive: `bulk_upsert` promotion to the `VectorStore` trait (730× faster bulk insert, shipped `c091281`).

---

## 8.5 · 🆕 ADR-073 (IN FLIGHT) — recall-safe `memory_read`: reorder-only + separation/no-signal abstain hint, never hard-empty

**Status:** SHIPPED 2026-06-08 (committing; CI pending). All 5 DoD gates green (fmt/build-0-warn/clippy-0-lint/vault-retrieval 80+6 tests/vault-mcp 41 tests). **Live-verified on the 1k vault across BOTH model tiers** (Flash + Opus): "how do i stay fit" now ANSWERS via `memory_read` (was `abstain:true` empty); blood-type/OS/salary still abstain with no fabrication; cat→dog surfaces the dog helpfully. Fixes Thread-2 Gap 1 (§4). Amends ADR-054 (read response shape, additive) + ADR-066 (recall-first read) + supersedes the ADR-059 read-side floor-drop. Full text stays here until the next archive freeze. (Gap 2 still open — see §4.)

**Context.** 1k live dogfood proved `memory_read` false-abstains on stored facts: `apply_reranker` (`structured_read_pipeline.rs`) hard-drops every candidate below `reranker.relevance_floor()` (≈ logit −2.5) and sets `abstain = candidates.is_empty()`. Real answers score below that floor ("runs 10km" for "stay fit" = logit −3.21) → dropped → false-abstain. Meanwhile `memory_search` (`RerankedRetriever`) is reorder-only + never empties and got these right. The two paths diverged; read must converge to search's recall-safe behavior. (Evidence: [[project_1k_live_read_false_abstain]].)

**Decision.**
1. **`apply_reranker` becomes reorder-only** — mirror `RerankedRetriever::rerank_pool`: sigmoid-map each logit to `[0,1]`, keep ALL candidates, sort by relevance DESC. No floor-drop. (The `RERANK_CANDIDATE_CAP` truncation stays — it bounds reranker cost, doesn't hide answers.)
2. **`abstain` is computed by a combined hint, not a drop** — `abstain = candidates.is_empty() || weak_match`, where `weak_match` is TRUE when EITHER (a) `top_relevance < READ_NO_SIGNAL_FLOOR` (≈0.01; catches lone/few no-signal facts the Lisbon-guard + cat→dog class), OR (b) the top is not separated from the pool per `search_hint`'s rule (top < `STRONG_RELEVANCE` 0.5 AND top < `SEPARATION_RATIO` 3× the runner-up; catches flat clusters like the salary trap). Separation alone is insufficient (a lone no-signal fact reads as "separated") — hence the floor; the floor alone is insufficient (the 0.025 salary cluster clears it) — hence separation. Both, combined.
3. **Never hard-empty `relevant_facts`** — when retrieval returned candidates, they are ALWAYS returned (reordered, truncated to `max_candidates`), even when `abstain=true`. The floor governs only the abstain HINT, never whether a fact is shown — so a mis-set floor can never hide a real answer (recall-safety by construction; the cardinal rule holds regardless of floor placement). The agent judges; the cat→dog live case proved a capable agent abstains-in-prose correctly when given facts + an honest hint.
4. **Response gains `top_relevance: f32`** (the rank-1 relevance for agent transparency). `abstain` IS read's weak-match signal — refined from "facts empty" to "no confident match; the facts shown (if any) are low-confidence." (No separate `weak_match` field — it would be identical to `abstain`; `top_relevance` carries the nuance, mirroring `memory_search`'s hint.)

**Thresholds (all backed by the 1k live data, none guessed):** real answers ranged relevance 0.0388–0.99; no-signal/wrong-neighbor 0.00006–0.004; salary distractor cluster 0.014–0.025 flat. `READ_NO_SIGNAL_FLOOR = 0.01` sits in the ~10× gap between the lowest real answer (0.0388) and the highest no-signal (0.004); `STRONG_RELEVANCE`/`SEPARATION_RATIO` reuse `search_hint`'s pinned 0.5 / 3×.

**Not chosen:** (a) lowering the existing floor-drop threshold — still drops real answers below it, still hard-false-abstains; rejected because recall must be unconditional. (b) Pure `search_hint` separation (no floor) — false-ANSWERS on lone no-signal facts (Lisbon-guard / cat→dog). (c) A redundant `weak_match` field — identical to `abstain`. (d) Touching the no-reranker cosine-gate fallback — out of scope; that path's `Vec::new()`-on-below-floor still maps to `abstain=true`+empty, unchanged.

**Security:** no crypto/boundary-filter change; boundary authorization upstream is untouched.

**Harness note:** `scale_eval` greenwashed this (measured pool recall, not the live `memory_read` gate, with favorable phrasings). Part of this work updates the harness to call the real tool + assert on `abstain`/`relevant_facts`/`top_relevance` + add paraphrase query variants.

**Tests changed (contract change — surfaced, not silent):** `reranker_filters_candidates_below_floor` (now: both kept, keep-ranked-first, reorder-only), `reranker_abstains_when_all_candidates_below_floor` (now: abstain=true BUT fact still present + low top_relevance), `union_semantic_recall_rescues_keyword_starved_fact` (now: distractor also kept, cello ranked #1, abstain=false). New tests: stay-fit-class (low-but-separated → abstain=false + fact present), salary-class (flat cluster → abstain=true + facts present), no-signal-floor (lone deep-negative → abstain=true + fact present), top_relevance field population.

---

## 8.6 · 🆕 ADR-074 (IN FLIGHT) — document-side alias enrichment at consolidation (Gap-2 vocabulary-gap fix)

**Status:** IMPLEMENTED 2026-06-10; **all 5 DoD gates green** (fmt / build-0-warn / clippy-0-lint / `vault-consolidator` 113 tests incl. 13 new `enrich` / `vault-app` 58 tests). NOT yet committed; live rank-lift validation with the real Phi-4 model is the next step. Fixes Thread-2 Gap 2 (§4 / §4.2). Honours [[project_architectural_lock_llm_out_of_read_path]] (Phi-4 at consolidation only) + [[project_locked_next_arc_t03x]] (consolidator arc). Full text stays here until the next archive freeze.

**Context.** Ground-truth probing of the real `seeded-vault-1k` (2026-06-09) re-diagnosed Gap 2 as a **vocabulary gap**: a fact phrased without the obvious keyword ("settled in **Porto**", "raising **twins**") is outranked by — or in a dense-distractor field drops below — facts carrying the literal keyword. The agent's keyword-soup query is the *trigger*, so query-side expansion was FALSIFIED (it replicates the harmful soup). The `probe_enrichment` A/B proved the fix is **document-side**: bare Porto ABSENT → enriched Porto rank 1 @ 0.9965 on the killer query; twins rank 5 → 1; no regression. Evidence: [[project_1k_live_paraphrase_recall_miss]].

**Decision — Option B (Phi-4 at consolidation), NOT write-time agent aliases (Option A):** the proven miss is an *existing* fact (write-time aliases only help future writes), and write-time leans on agent-generated aliases (the lever this session proved unreliable). Three locked parts:

1. **Alias generation (`phases/enrich.rs::generate_aliases`).** One `LlmProvider::complete_json` call per fact — mirrors `topics::label_one_cluster` (temp 0, fixed seed `0x0A11_A5E5`, `max_tokens` 64, JSON schema `{"aliases":[4..8 strings]}`, JSON-only system prompt). Asks for alternative search keywords NOT already prominent in the text (synonym / category / type). Output normalised to trimmed lowercase, de-blanked. Empty/malformed → `Err` (skip-and-retry, never run-abort).
2. **Storage + embed-text (`metadata` key + composition).** Aliases stored on `Memory.metadata.enrichment = {"aliases": "a, b, c", "content_fp": "<fnv1a-hex>"}` (no schema migration — `metadata` is free-form `serde_json`; existing keys preserved). The **embedded** text is `compose_embed_text` = `"<content> Topics: <aliases>"` (the proven probe shape, pinned by `compose_embed_text_matches_probe_shape`). **`Memory.content` (display text) is NEVER modified** → the alias line cannot leak into the read response (read returns `content`). Aliases are a **vector-channel boost only**; BM25 still indexes clean `content`. The persisted vector is replaced in-place via `StorageBackend::update_memory` (atomic metadata + vector update, by id).
3. **When it runs / cost (`Consolidator::enrich_facts`).** A new consolidator step over the active (non-superseded, non-invalidated) set, wired in the app-layer safety wrapper AFTER `run_consolidation` and BEFORE `generate_reports` (parallels `generate_reports`; under the same 30-min timeout). **Idempotent:** each fact records an FNV-1a `content_fp`; a fact already enriched for its current content is skipped, so the first run backfills the whole vault and steady-state runs only re-embed newly-written / changed facts (a merge or update mints fresh content → fresh fingerprint → re-enrich). FNV-1a (not `DefaultHasher`) is stable across toolchain versions → no spurious whole-vault re-embed after a Rust upgrade.

**Failure + operational semantics (locked-next-arc Step 4):** a per-fact LLM / embed / `update_memory` failure is logged-and-counted (`EnrichmentReport::facts_failed`) and the loop continues — one bad fact never aborts the run, and the fact retries next cycle (no fingerprint written). Two operational notes from tracing the real path: (a) **first backfill on a large vault can exceed the 30-min consolidator timeout** (~1k facts × ~3–5s/Phi-4 call); because each `update_memory` commits immediately and the pass is idempotent, a timed-out run still makes durable progress and **re-running resumes** (self-heals over 2–3 runs — no per-run cap added; alpha-scale vaults of a few hundred facts finish in one run). (b) **Re-embeds drain async** through the cascade queue (like merges today), so the one-shot `vault-cli consolidate run` exits before the new vectors land; they apply when a worker next opens the vault (restart Antigravity / MCP server).

**Not chosen:** (a) write-time agent aliases (Option A — doesn't fix existing facts; relies on the unreliable agent-alias lever). (b) Vault-side query expansion (FALSIFIED — IS the keyword-soup that triggers the miss). (c) A deterministic synonym map ("settled in Porto" → "home" needs comprehension, not a thesaurus). (d) Putting aliases into `content` (would leak into display + pollute BM25). (e) A per-run enrichment cap (YAGNI at alpha scale; timeout-resume already bounds risk — revisit if the live 1k run shows timeout pain).

**Security:** no crypto / boundary-filter change. Enrichment operates within a single boundary's facts via the existing storage traits; the alias text is derived from the fact's own content by the local Phi-4 (no cross-boundary read, no network).

**Live validation — DONE 2026-06-10, real Phi-4, real 1k vault.** Two `#[ignore]` probes ride with this commit: `vault-consolidator` `real_phi4_alias_quality` (loads the real GGUF, prints aliases for the killer facts) and `vault-app` `scale_eval::probe_real_enrichment_1k` (drops the 3 keyword-poor killers into a throwaway `seeded-vault-1k` copy, records bare rank, enriches ONLY them via the real `enrich_one` path, re-measures by direct LanceDB vector search — fast A/B, no full-vault enrichment / merge-cost). **Result (real Phi-4 aliases, 1k dense field):**

| killer | killer query | bare | → enriched |
|---|---|---|---|
| Porto ("settled in Porto") | "home location city country lives residence" | **ABSENT (>top-50)** | **rank 1** |
| twins ("raising twins") | "children kids son daughter offspring family" | rank 1 | rank 1 |
| hives ("comes out in hives") | "is the user allergic to anything" | rank 4 | **rank 1** |

**Prompt-tuning finding (the reason to validate-before-commit):** the *first* real-Phi-4 run lifted Porto only ABSENT→rank 6 — Phi-4 returned Portugal-anchored *phrases* (`portugal residence change`) instead of the generic single words the query uses. Tuned `generate_aliases` to ask for **single-word generic category/type keywords** (neutral job/pet examples, NOT the eval cases) → Porto's aliases became `portugal, settlement, residence, city, relocation, migration` → **rank 1**, hives/twins unchanged. All three killers now #1 end-to-end. Run cmd: `$env:PROBE_VAULT_DIR=<throwaway 1k copy>; $env:PHI4_MODEL_DIR=<models dir>; cargo test -p vault-app --test scale_eval probe_real_enrichment_1k -- --ignored --nocapture`.

---

## 8.7 · 🆕 ADR-075 (IN FLIGHT) — Phase 4 confidence decay (T0.2.4)

**Status:** SHIPPED 2026-06-13; all 4 DoD gates green (fresh DuckDB-1.4 build 0-warn / `vault-storage` + `vault-consolidator` tests 0-fail / clippy 0-lint / fmt). Implements BRD §5.6 Phase 4 line 994 (the *decay* half; cold archive deferred). Honours [[project_architectural_lock_llm_out_of_read_path]] (no LLM in decay).

**Context.** Phase 4 was unbuilt (`memories_archived` hardcoded 0; no decay pass). The sleep consolidator must fade stale knowledge so retrieval (which weights by confidence) demotes it over time without ever deleting it.

**Decision.**
1. **Policy (`phases/decay.rs::plan_decay`)** — a fact not accessed in `decay_after_days` (180) has `confidence ×= 0.9` (BRD line 994 verbatim). Pure planner over the active set; skips superseded / invalidated facts and 0.0-confidence no-ops.
2. **Metadata-only application (`StorageBackend::apply_decay`)** — sets confidence + an idempotency marker (`metadata.decay.last_decay_at`); **never re-embeds** (re-embedding from raw `content` would clobber the ADR-074 enriched vector). New `memory.decayed` audit event distinguishes a decay from a user edit.
3. **Idempotency (BRD line 1022)** — the marker means a back-to-back run does not re-decay; a fact re-decays only after a full decay period elapses.
4. **Wiring** — runs as Phase 4 in `run_consolidation` (after contradiction, before report); `ConsolidationReport.memories_decayed` + the summary Decay section carry the count.

**Cold archive (BRD lines 995-996) DEFERRED** — a first-class `Memory` state change (schema + retrieval-filter reach) far larger than decay; its own batch keeps this one debuggable. `memories_archived` stays 0.

**Tests:** 10 planner + 3 `apply_decay` + 2 summary + 1 real-BGE end-to-end (`cold_fact_decays_through_consolidation_and_is_never_lost`). The "no memory ever lost" property holds — decay only mutates confidence.

---

## 8.8 · 🆕 ADR-076 (IN FLIGHT) — sync ship-gate: `pending_sync` cascade payload (migration 0003)

**Status:** SHIPPED 2026-06-13; 4 gates green. **Closes tech-debt #4** (V0.2 sync ship-gate).

**Context.** `DivergenceDetector::sweep_pending_sync` was a V0.1 stub returning 0 — the cap-overflow catch-up table carried only `(memory_id, operation, queued_at)`, not enough to reconstruct a `retry_queue` row. Cross-device churn (V0.2 sync) makes a silently-dropped overflow entry a real data-recovery gap.

**Decision.**
1. **Migration 0003** adds `sequence_id INTEGER` + `payload BLOB` to `pending_sync` (nullable / defaulted — legacy rows read NULL payload and are *skipped*, never re-enqueued broken).
2. **Overflow path persists the full cascade** — both overflow call sites pass the in-scope `audit_seq` + `payload_bytes` to `tx_upsert_pending_sync`.
3. **Real sweep (`StorageBackend::drain_pending_sync`)** — oldest-first, atomically per entry: while `retry_queue` < cap, re-insert the stored cascade + delete the pending row in one tx. Stops at cap; payload-less rows skipped. `DivergenceDetector` calls it as Tier-0.

**Deviation from the handoff sketch:** stored the cascade **payload (+ `sequence_id`)** rather than separate `embedding`/`boundary` columns — more faithful (the stored bytes hand straight back to the retry insert) and schema-version-agnostic.

**Security:** payload lives in the SQLCipher-encrypted `vault.db` — encrypted at rest, no new plaintext surface, no crypto-path change.

**Tests:** full overflow → drop-vector → sweep → worker-reapply → vector-restored loop; payload-less legacy skip; payload round-trip; migration-columns check.

---

## 8.9 · 🆕 ADR-077 (IN FLIGHT) — DuckDB 1.2.2 → 1.10503.1 (libduckdb 1.4 LTS) upgrade

**Status:** SHIPPED 2026-06-13; 4 gates green on a **fresh full-workspace cold build** (`cargo clean` first).

**Context.** DuckDB 1.4 LTS (Sept 2025) adds native database encryption (`ATTACH … (ENCRYPTION_KEY …)`, AES-256-GCM over the main file + WAL + temp files) — the clean path to closing the V0.2 graph-encryption gap (`graph_store.rs:41-42`), which pinned 1.2.2 could not do.

**Decision.** Adopt the dependency upgrade **now** (de-risked on a clean rebuild of the whole workspace), but **DEFER the encryption wiring** (the `ATTACH ENCRYPTION_KEY` in `graph_store.rs` + ADR-SEC + §11 threat-model review + security tests) to its own task. Lands the heavy/risky dep bump on a verified clean tree so the later encryption work is pure code, not a dep gamble.

**Verification.** Spike built `vault-storage` clean (17m36s, exit 0). Then a full `cargo clean` + fresh `cargo build --workspace -D warnings` compiled **all 12 crates** against 1.4 (29m57s, 0 warnings); tests + clippy green.

**Cost accepted (`Cargo.lock` churn):** arrow 54→58 (workspace now carries arrow 57 **and** 58 — lance stays on 57; they don't cross paths), strum 0.25→0.27, + new crossterm / zip / zopfli / zlib-rs. The Cargo.toml CRT-conflict note (esaxx-rs `/MT` vs duckdb-sys `/MD`) is unaffected — `esaxx_fast` is already dropped.

**Next task (graph encryption — still deferred):** wire `ATTACH 'graph.duckdb' (ENCRYPTION_KEY <derived from master key>)` + ADR-SEC entry + §11 threat-model walk + security tests.

---

## 8.10 · 🆕 ADR-078 (IN FLIGHT) — graph-filling: entity + relationship extraction at consolidation

**Status:** SHIPPED 2026-06-14; all DoD gates green on a fresh `cargo clean` full-workspace rebuild (DuckDB 1.4.4). **Closes tech-debt #2** (entity-extraction-at-consolidation). Honours [[project_architectural_lock_llm_out_of_read_path]] (Phi-4 at consolidation only) + [[project_locked_next_arc_t03x]] (consolidator arc). Full text stays here until the next archive freeze.

**Corrects ADR-077 (§8.9):** that ADR's "libduckdb 1.4 LTS" label was WRONG — `=1.10503.1` is **DuckDB 1.5.3** (off-LTS; its bundled C++ fails the Windows CI `fmt/format.h` compile → `a1c0ff9` is CI-RED). Pin corrected to `=1.4.4` (the real LTS). ADR-077's encryption goal is **falsified by spike**: NO bundled DuckDB version can securely write an encrypted DB offline (mbedtls is read-only; secure write needs the network `httpfs`/OpenSSL extension → breaks offline/zero-knowledge — confirmed on 1.4.4 AND 1.5.3). Graph encryption deferred to "bundle the helper locally, when the graph holds shippable data" (tech-debt #7). A `rstrtmgr` link fix (`vault-storage/build.rs`) covers DuckDB 1.4's `AdditionalLockInfo` → Windows Restart-Manager dependency that `libduckdb-sys` forgot to link.

**Context.** The DuckDB `GraphStore` (entities + bi-temporal relationships) shipped at T0.1.5 but nothing ever FILLED it — `apply_merge` skipped the graph with a `tracing::warn!` no-op (tech-debt #2), so `graph.duckdb` held zero data. Product reason to fill it now (Shahbaz, 2026-06-14): the graph must hold real data before it (and its eventual encryption) is worth anything; "it's empty so don't encrypt it" is unbuilt work, not a feature.

**Decision — extract via the EXISTING enrichment call, not a new pass.** The nightly enrichment (ADR-074) already sends every fact to Phi-4 once (for search aliases). A separate extraction pass would DOUBLE the per-fact LLM cost and worsen the ~90-fact latency wall. Instead the one call now returns three products: `aliases` + `entities` + `relationships`. **Validated by a live tuned Phi-4 probe** (`phases::enrich::real_phi4_combined_extraction_quality`, `#[ignore]`): combined output keeps single-word keyword quality (no recall regression) and produces correctly-typed entities + sensibly-directed links. Three parts:

1. **Combined call (`phases/enrich.rs`).** `generate_aliases` → `generate_enrichment` returning `{aliases, graph}`; one `complete_json` against a schema carrying all three arrays (entity `type` enum = `EntityType` snake_case names). Aliases stay recall-critical (empty aliases ⇒ `Err`/retry); the graph is best-effort (empty ⇒ no error). `EnrichedFact` gains a `graph: ExtractedGraph` field.
2. **Parse + cleanup + write (NEW `phases/extract.rs`).** `parse_extracted` is best-effort (NEVER errors): maps the type label (unknown ⇒ `Concept`, never `Custom` junk), drops empty/over-long names, dedups entities, normalises relations to snake_case, and **drops any relationship whose endpoints are not in the entity list** (the model occasionally references an unlisted name). `write_extracted_to_graph` **gets-or-creates** each entity (new `GraphStore::get_entity` lookup) so nightly re-runs reuse ids instead of hitting the `(name, type, boundary)` UNIQUE constraint, then creates the relationships — all scoped to the memory's own `Boundary` (ADR-015 privacy holds).
3. **Wiring (`consolidator.rs::enrich_facts`).** After `update_memory` persists the enriched vector, the graph is written. **Ordering is load-bearing:** vector first (writes the `content_fp` fingerprint), graph second — so a transient graph-write failure is never re-extracted into DUPLICATE edges on the next run. `EnrichmentReport` gains `entities_created` / `entities_reused` / `relationships_created` / `relationships_failed` / `graph_write_failures`.

**Idempotency.** Extraction rides inside the fingerprint-gated `enrich_one`, so a steady-state run never re-extracts an unchanged fact → no duplicate entities/relationships (proven by the `enrich_facts_fills_graph_with_entities_and_relationships` e2e: fact → linked entities, traversable, second run = zero duplicates).

**Not chosen / deferred:** (a) a separate extraction LLM pass (doubles latency); (b) a local NER model (no NER lib in-tree; Phi-4 already loaded at consolidation); (c) **relationship-rewrite-on-merge** — a content change (merge/update) re-extracts but leaves the prior content's relationships behind; retiring them needs the `rewrite_relationships_for_memory` primitive (tech-debt #2's tail) — out of scope for this milestone, harmless while the graph is dogfood-only; (d) graph encryption (deferred — see above).

**Security:** no crypto / boundary-filter change. Extraction operates within a single boundary's facts via the existing `GraphStore` traits; entity/relationship text is derived from the fact's own content by the local Phi-4 (no cross-boundary read, no network). `create_relationship`'s ADR-015 cross-boundary guard is untouched.

**Tests:** `vault-storage` `get_entity` ×4 (absent / full-fidelity / type+boundary scoping / get-or-create no-dup); `vault-consolidator` `phases::extract` ×8 (label mapping, relation normalisation, dangling-drop, case-insensitive endpoint resolve, dedup + self-loop drop, malformed-safe) + the `enrich_facts` e2e graph-fill + existing enrichment tests green (no regression).

---

## 8.11 · 🆕 ADR-079 (IN FLIGHT) — Windows CI fix: VS2026 removed `stdext::checked_array_iterator` (bundled-DuckDB fmt break)

**Status:** committing now; CI-only change, NOT locally testable (see below). Restores `main` to green after two consecutive RED commits (`a1c0ff9`, `d2b9b9b`). Corrects the ADR-078/§1 misdiagnosis that the DuckDB pin caused the Windows red.

**Root cause (proven from CI run `27484651556` logs + cross-checked upstream).** GitHub's `windows-2025` runner image migrated to **Visual Studio 2026 (MSVC 14.51.36231)** during the 2026-06-08→06-15 rollout (the build log path is `Microsoft Visual Studio\18\Enterprise`). VS 2026 **removed** `stdext::checked_array_iterator` from the MSVC STL headers entirely (a long-deprecated non-Standard extension; confirmed removed, not merely deprecated — see o3de/o3de#19754: *"these functions literally do not exist anymore"*). DuckDB's bundled `fmt` (~v5.x, vendored in `libduckdb-sys`) still references it under a bare `#ifdef _SECURE_SCL`; VS 2026 **still defines** `_SECURE_SCL`, so the bundled C++ build takes that branch and fails:

```text
fmt/format.h(326): error C2061: syntax error: identifier 'checked_array_iterator'
```

This is independent of DuckDB version — `1.4.4` AND `1.5.3` bundle the same ancient fmt, so neither the `=1.10503.1→=1.4.4` correction nor any crate bump escapes it. The last green commit (`d613614`, 2026-06-10) predates the image migration; nothing in our code regressed. `_SILENCE_STDEXT_ARR_ITERS_DEPRECATION_WARNING` does NOT help (the type is gone, not deprecated; and the build already uses `-W0`).

**Decision.** A forced-include (`/FI`) shim header (`.github/msvc_fmt_secure_scl_shim.h`) `#include`s `<yvals.h>` (which sets `_SECURE_SCL` and has an include guard) then `#undef _SECURE_SCL`; later STL includes are guard-no-ops, so the macro stays undefined and fmt falls back to its raw-pointer `checked_ptr = T*` branch — the exact path Linux/macOS already compile (known-good; DuckDB builds clean there). Wired into BOTH Windows CI jobs (clippy + build/test) via `CXXFLAGS_x86_64_pc_windows_msvc`, which **cc-rs (libduckdb-sys) reads but CMake (llama-cpp-sys-2's Vulkan build) does not** — so the llama/Vulkan build, the reason we are on `windows-2025` at all, is untouched.

**Not chosen:** (a) the silence macro (type removed, not deprecated); (b) a DuckDB crate bump (same bundled fmt across versions); (c) reverting to `windows-2022` (re-breaks the llama `vulkan-shaders-gen` C1083 build — the documented reason for `windows-2025`); (d) pinning an older MSVC v143 toolset (re-introduces toolset/CMake interaction risk, larger blast radius); (e) hand-writing a `checked_array_iterator` replacement (error-prone vs. just disabling the dead branch).

**Local-test relaxation (per Shahbaz, 2026-06-14 session 2).** The failure is specific to the CI runner's VS 2026 image and **cannot be reproduced on the founder's local machine** (older MSVC that still ships the type — local builds were green throughout). So local DoD gates verify nothing here; CI is the only meaningful verification. Committed + pushed without a local build run by explicit founder direction; CI-green is the gate. **Risk if wrong:** the `<yvals.h>`-defines-`_SECURE_SCL` assumption is the one empirical link not provable locally — if a different header defines it, CI fails the same way and we iterate.

**Security:** none — build-time compiler flag only, no runtime/crypto/boundary surface.

**Tech debt:** remove the shim + CI step once `libduckdb-sys` vendors a newer fmt (or drops the `stdext` usage). Tracked as tech-debt #8.

---

## 9 · 📇 ADR index

Full text of every ADR lives in an archive — cross-link by number, **quote don't paraphrase** ([[feedback_quote_locked_artefacts_dont_paraphrase]]).

**In-flight (full text in HANDOFF, not yet archived):** **ADR-079** (Windows CI fix: VS2026 removed `stdext::checked_array_iterator` → `/FI` `_SECURE_SCL`-undef shim for bundled-DuckDB fmt, §8.11 — corrects the ADR-078/§1 "1.4.4 fixes CI" misdiagnosis) · **ADR-078** (graph-filling: entity + relationship extraction at consolidation, §8.10 — closes tech-debt #2; corrects ADR-077 to DuckDB 1.4.4 + defers encryption) · **ADR-077** (DuckDB dep upgrade — corrected to `=1.4.4` LTS, §8.9) · **ADR-076** (sync ship-gate `pending_sync` payload, §8.8) · **ADR-075** (Phase 4 confidence decay, §8.7) · **ADR-074** (document-side alias enrichment at consolidation, §8.6) · **ADR-073** (recall-safe `memory_read`, §8.5 — SHIPPED `a3e426b`).

**Most relevant to current/next work (full text in `HANDOFF_V0.2_PART2_ARCHIVE.md`):**
| ADR | Title | Status |
|---|---|---|
| **072** | sealed-store `get_opts` never returns a short buffer for a bounded range (10k TOCTOU fix) | SHIPPED `da10c0f` |
| **071** | reranked + recall-safe `memory_search`; `memory_read` is the primary answer path | SHIPPED `661d391` (+ Option B `a1e4dac`) |
| **070** | lazy reranker load off the handshake path | SHIPPED `a3c938b` |
| **069** | read recall-union: hybrid ∪ semantic candidate pool | SHIPPED `a2cee13` |
| **068** | topic discovery by connected-components, not K-means | SHIPPED `76ffc9b` |
| **067** | `memory_search` recall-first: hybrid candidates, no hard BM25 gate | SHIPPED `76ffc9b` |
| **066** | recall-first read: reranker as re-orderer + no-signal floor, not precision authority | SHIPPED |
| **065** | contradiction candidate generation by nearest neighbor, not K-means topics | SHIPPED |
| **064** | read-side subject framing for the reranker (`DOC_SUBJECT_FRAME "The user — "`, Bug-2 fix) | SHIPPED |
| **061** | clustering robustness to vector-store / metadata divergence | SHIPPED |
| **060** | topic-level contradiction detection (A5 ship-gate) | SHIPPED |
| **059** | cross-encoder reranker (Qwen3-Reranker-0.6B) as the read relevance gate (supersedes ADR-057 cosine floor) | SHIPPED `87d0b72` |
| **058** | wire per-boundary REPORT generation into the consolidation run | SHIPPED |
| **057** | deterministic cosine relevance gate for `memory_read` | SUPERSEDED by ADR-059 |
| **056** | dogfood-surfaced correctness fixes (Commit 8) | SHIPPED |
| **055** | `vault-cli mcp serve` subcommand-split design | SHIPPED |
| **054** | MCP `memory.read` response health-warning contract (6 codes; Amendment 2 dropped `DELTA_LOG_UNAVAILABLE`) | SHIPPED `99052f2` |
| **053** | per-boundary REPORT artifact shape + storage + lifecycle (+ Amendment 1: `topic_names_unavailable`) | SHIPPED `f0cc158` |
| **052** | Qwen-7B retirement from read path (supersedes ADR-048/049 in effect) | SHIPPED `99052f2` |
| **051** | bi-temporal storage semantics + `invalidate()` API contract | SHIPPED |
| **047** | `summary.rs` placement + RunState/AMWC field extensions | SHIPPED |
| 048, 049 | Qwen-7B read pipeline + model lock | SUPERSEDED by ADR-052 |

**Live V0.2-era ADRs, full text in `HANDOFF_V0.2_PART1_ARCHIVE.md`:** ADR-044 (+Amendment 1, `LlmProvider`/`Phi4MiniProvider`), ADR-045 (Cluster output contract), ADR-046 (`mark_superseded` + `MemorySuperseded` audit), plus ADR-037–043 (lancedb upgrade, concurrent-upsert serialisation, Keychain/master-key derivation, V0.1→V0.2 SQLCipher bridge, Phi-4-mini selection, model download/integrity).

**V0.1-era ADRs (001–036):** full text in `HANDOFF_V0.1_ARCHIVE.md`.

---

## 10 · 📐 Standing rules (CLAUDE.md-promoted defaults)

Full rules in `~/.claude/projects/C--Projects-GitHub-Memory-Vault/memory/`.

- **Confirm before every commit + push.** One combined approval covers both; per-action (yes-commit ≠ yes-push for the *next* task). Co-Authored-By: bare `Claude <noreply@anthropic.com>`, no model qualifier.
- **CI green per-commit.** Every code commit shows CI green matrix-wide (`gh run list --workflow=ci.yml -L 1`) before staging the next. Local DoD ≠ CI green. Relaxation is the founder's to invoke per-batch, acknowledged in the commit body.
- **Confirm before any cargo build/test/clippy/check/run + check disk first** (laptop freezes during compile; disk runs tight). Report disk + target size in the ask. Only `cargo fmt` is safe. Run gates in background (`run_in_background=true`).
- **Strictly-serial cargo.** Never parallel cargo on the same workspace (kills incremental cache → 30GB+ wipe + 30-min rebuild). Order: check → test → clippy → fmt → `git status`.
- **Cargo on Windows = PowerShell** (Strawberry Perl path order for the sqlcipher/openssl vendoring; MSYS2 perl in Bash lacks the modules). Set `LIBCLANG_PATH` + prepend to PATH each fresh shell.
- **fmt runs LAST**, with `git status --short` between final `cargo fmt --all --check` and `git add` to catch drift (esp. `Cargo.lock`).
- **Admin-only changes ride with the next code commit** (HANDOFF/ADR/tech-debt/doc edits never get their own commit — saves a ~45-min CI cycle). Spike examples + eval harnesses + baselines bundle with the tested code that consumes them, never alone.
- **No drive-by refactoring.** Log it under Tech Debt (§7) and continue.
- **Surface plan amendments BEFORE code** (recon-class changes, signature changes, new primitives, floor-forecast breaches). Inline architectural decisions produce an ADR in the same commit.
- **Plain English when asking the founder questions** (non-coder product owner); reserve technical density for code/commits/ADRs/HANDOFF.
- **Never commit the project-level CLAUDE.md** (gitignored, local-only).
- **HANDOFF line "Last updated" is a lagging indicator.** For current-state questions, source-read §1 + cross-check `git log --oneline`.
- **Definition of Done (BRD §0.1):** build zero-warnings + affected-crate tests pass + clippy `-D warnings` clean + `fmt --check` passes + HANDOFF updated. All five or it's not done.

---

## 11 · 🗂️ Archives

- **`HANDOFF_V0.1_ARCHIVE.md`** — frozen 2026-05-06. T0.1.1–T0.1.12 narratives, ADRs 001–036, V0.1 tech-debt closures.
- **`HANDOFF_V0.2_PART1_ARCHIVE.md`** — frozen 2026-05-13 (T0.2.3 commit 2). T0.2.0–T0.2.3c2 narratives, ADRs 037–046 + amendments.
- **`HANDOFF_V0.2_PART2_ARCHIVE.md`** — frozen 2026-06-08 (this split). T0.2.3c3 → T0.3.x narratives, ADRs 047–072 full text, the read-correctness + consolidator-REPORT + A5-contradiction arcs, full tech-debt narratives, technique map, consolidator inventory, V0.2 backend/tuning config.

Cross-link out for detail; **do not paraphrase** archived ADRs or spec text — quote them.

When V0.2 closes (T0.2.13 ship + hard-gate clearance), a fresh slim HANDOFF.md opens for V1.0 per BRD §6.3.

---

## 12 · 🔧 Key reference (paths, models, commands, env)

**Repo:** https://github.com/shahbaz242630/Agent-Memory-Vault.git · **Local:** `C:\Projects\GitHub\Memory Vault` · **Spec:** `Agent Build Specification.txt` (BRD, canonical).

**Binary:** `C:\Projects\GitHub\Memory Vault\target\debug\vault-cli.exe`
**Models / fixtures:** bge-small + qwen3-reranker fixtures under `crates/vault-embedding/test-fixtures/`.
**Real vault (production):** `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\{vault.db, lance, graph.duckdb}` (Tauri bundle id `com.shahbaz242630.memory-vault`). Dev vault is throwaway dogfood data — safe to wipe. [[project_dev_vault_is_throwaway_test_data]]
**Seeded test vaults:** `C:\Projects\seeded-vault-{100,1k,10k}`.

**Env (fresh PowerShell shell):**
```powershell
$env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"; $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
$env:LANCE_MEM_POOL_SIZE = '268435456'   # matters for heavy concurrent WRITES, not read-only tests
```

**Scale harness:** `cargo test -p vault-app --test scale_eval` (set `SCALE_EVAL_N` to size; real BGE + Qwen3-reranker, own temp vault). Live seeder: the `seed_live_vault` `#[ignore]` test (env `SEED_N` + `SEED_VAULT_DIR`).

**Disk note:** C: runs tight (~20 GB free at this session; `target/` ≈ 129 GB). Always check before a build. Surgical `cargo clean -p <crate>` first; full `cargo clean` is escalation.

---

## 13 · 🧪 Full-aspect live test campaign — scorecard + failure root-causes (2026-06-11)

Driven via a scripted MCP **stdio** client (`C:\Projects\mcp-probe\client.py`, NOT in repo) against `seeded-vault-mixed` (~94 messy+clean dogfood facts) + `seeded-vault-tiny` (6-fact consolidation demo). Antigravity quota was down so I acted as the MCP client directly (the structured contract the agent receives). **No production code changed.**

| Aspect | Verdict | Evidence |
|---|---|---|
| Write / Read / Update / Delete | ✅ | CRUD round-trip: write→read→update(content replaced)→delete(gone) |
| Search + recall-safety + `weak_match` | ✅ | never empty (even nonsense query → n=5, `weak_match=true`); `weak_match=false` only on real hits |
| Access control — reject unauthorized | ✅ | write to `secret` → `{-32001, "access denied"}` |
| Access control — accept authorized | ✅ | write to `testeval` → id returned |
| Boundary isolation | ✅ | `testeval` marker visible w/ testeval authorized, invisible w/ personal-only (n=10) |
| Encryption at rest — `vault.db` | ✅ | header = random bytes, not `SQLite format 3` (SQLCipher) |
| Graph encryption — `graph.duckdb` | ❌/⚠️ | `DUCK` magic = PLAINTEXT (tech-debt #7) |
| Merge / dedup | ✅ | tiny vault: 2 near-dup run-facts → 1, both originals superseded |
| REPORT (structured knowledge state) | ✅ | `personal.report.json` 4 auto-named topics, dates captured |
| Enrichment (Gap-2 recall lift) | ✅ | 1k MCP A/B (Porto ABSENT→1) + tiny-vault consolidate (4 enriched, 0 failed) |
| Abstain — clear absence (cat) | ✅ | `abstain=true`, surfaces dog Biscuit, invents no cat |
| Abstain — salary | ❌ | `abstain=false`, surfaces "$450 room booking" (conf 0.41) |
| Abstain — blood type / OS | ⚠️ | `abstain=false` but top_rel ~0.01–0.02 (marginal) |
| Wrong-neighbor precision | ⚠️ | distractor ranks #1: live→"mother in Lisbon", kids→"Marcus's kids", allergy→"Marcus's peanut" |
| Contradiction **resolution** | ⚠️ | Tesla/Rivian both stay active (0 resolved, 0 queued) even with `as_of` set |
| Decay / archive | ❌ | not built (T0.2.4) |

**One-line root-cause per non-pass item:**
- **Graph plaintext** — ADR-010's DuckDB encryption layer (scoped T0.2.0) never actually shipped; the store still opens plaintext (runtime still WARNs). Low risk only because the graph is empty in V0.2 (entity extraction unbuilt, tech-debt #2).
- **Salary $-trap** — the reranker scores money-shaped facts ("$450 booking", "rent 1200") as relevant to "salary" and there is no per-candidate category/precision filter to veto a confident wrong-category match; the abstain gate is purely reranker-score-driven and the score cleared the no-signal floor.
- **Blood/OS marginal abstain** — the no-signal floor (~0.01) sits just below where a couple of barely-related distractors score (0.011–0.019), so they squeak over and `abstain` stays false even though nothing relevant exists.
- **Wrong-neighbor #1 ordering** — the reranker ranks a semantically-adjacent fact about *someone/something else* (the mother, Marcus, the dog) above the user's own fact; there is no subject/ownership signal distinguishing "about the user" from "about an associate."
- **Contradiction not resolved** — NN-pair + Phi-4 judge did not flag Tesla vs Rivian as a contradiction (two cars can coexist / pair not surfaced), and `as_of` is write-time so there is no fact-time recency signal to force supersession; both remain active.
- **Decay/archive** — simply not implemented yet (Phase 4 / T0.2.4 never started; `memories_archived` returns 0).

**Verdict:** storage / retrieval / security / structural plumbing is **correct on messy data**; every gap is in the **precision/abstain** layer (read-precision arc, roadmap §5 item 1) or **temporal resolution** (`as_of`/A5) or **unbuilt nightly features** (decay/archive). Wave 3 (live Flash vs Pro on `seeded-vault-mixed`) is the remaining acceptance — does a real agent land the right answer from this structured output.

## 13.1 · 🧪 Wave 3 — DONE (live Flash + Opus 4.6 in Antigravity, 2026-06-12)

Live-agent run on `seeded-vault-mixed` (un-enriched). **Both models landed correct answers on essentially every trap** — the agent layer rescues a genuinely messy vault ranking. No code changed; CI stays green on `d613614`.

- **Gemini Flash (weak):** 14/15 atomic clean + 1 expected temporal partial (car: listed Tesla+Rivian, didn't resolve). On a multi-intent *sentence* Flash **mashed all 4 intents into one query** (`"languages sports teams reading holiday"`, top_relevance 0.040) → McLaren + The Expanse **buried out of the result window** → answer complete but **partly papered over with lucky-correct guesses**.
- **Claude Opus 4.6 (strong):** **decomposed** the same sentence into 4 focused `memory_search` calls → fully grounded, fully correct, both category traps held (Blade Runner out of "reading", Madrid framed as work not holiday), even synthesized accurate cross-links (Portuguese↔Porto, City↔Manchester from the wide recall pools).

**Probe replay — vault-level ground truth (raw `memory_read`, natural-question GRADE_QUERIES, agent stripped away).** Only **2 of 10 traps are vault-clean** (cat→abstain=True@0.25; instrument→cello #1@0.98). The other 8 are messy at the source:
- **Wrong-neighbour #1 at high confidence (0.88–0.99):** "where do you live" → **#1 = mother/Lisbon (0.99)**, Porto not even top-5; "have kids" → #1 = Marcus's kids (0.88), twins #3; "allergic" → #1 = Marcus's peanut (0.95), user's penicillin/shellfish #2/#3. The reranker confidently ranks an *associate's* fact above the user's own.
- **Salary trap fires at vault level:** abstain=False, #1 = "$450 booking" (0.41). Flash/Opus both rescued by reasoning from self-describing content.
- **Marginal abstain misses:** blood-type top 0.011, OS top 0.019 — both squeak over the `READ_NO_SIGNAL_FLOOR` 0.01, abstain stays False.
- **Contradiction unresolved:** car → Tesla(0.997)+Rivian both active, no supersession (`as_of` is write-time).

**Two findings (1 kept, 1 retracted):**
1. **KEPT — reranker brittle on terse keyword queries.** Natural questions score 0.88–0.99; terse fragments collapse to noise (Opus's `"sports teams follow"` → top 0.0022, "supports manchester city" ranked **#8 below junk**). Two query-style failure modes both → noise: weak-agent *mash* (dilution, facts buried) + strong-agent *keyword-strip* (facts present, ranked below noise). Fix = steer agents to **decompose AND phrase as natural questions** (`instructions.md` follow-up, §4). Memory: [[project_reranker_brittle_on_terse_queries]].
2. **RETRACTED — `search_hint.rs` weak_match is NOT buggy.** A mid-run hypothesis that `weak_match=false` on a noise-level separated top needed the ADR-073 no-signal floor was **falsified by a code read**: the separation-based (not magnitude) design is deliberate and documented (canonical example "cello 0.0469"), and `weak_match=false` is honest because matches genuinely exist in the pool. Do not change it.

**Net:** outcomes are good on both model tiers, but the **vault's ranking is genuinely messy** — the agent rescue is a crutch (a model weaker than Flash would faceplant on salary/allergy). This is the strongest evidence yet for **roadmap §5 item 1 (read precision): a subject/ownership signal so "about the user" beats "about an associate" + a category veto for the salary-shape trap.** Recall-safety ([[project_memory_read_primary_search_recall_safe]]) is the hero that makes the messy ranking survivable. The Gap-2 enrichment lift was NOT exercised here (mixed vault un-enriched); optionally enrich it (surgical `enrich_one` loop) to also test Porto-in-soup.

### 13.2 · Gap #4 (car/temporal) — ADR-075 fact-time SPIKED + REVERTED 2026-06-12; route to agent-steering, not vault resolution

Attempted Arc B (gap #4): a consolidation-time Phi-4 **fact-time extractor** (Option B, vault-owned; new `phases/fact_time.rs` + `effective_fact_time` recency input + Phase-2b wiring) to break the write-time recency tie that leaves Tesla+Rivian both active. Scaffolded, compiled clean (0 warnings), and **gated on a real-Phi-4 end-to-end spike** (`real_phi4_car_resolution`) **before any commit**. Spike result (110s) — **the car does NOT cleanly auto-resolve, for two independent reasons:**
1. **The conservative judge correctly refuses.** Real Phi-4 returned `contradiction=false` / `stale=[]` for "Drives a Tesla Model 3." + "Finally picked up my Rivian R1T last month." — owning two cars is genuinely possible; the judge only flags with an explicit replacement signal ("having sold the Tesla"), which the real content lacks. Making it more aggressive risks wrongly retiring coexisting facts (recall cardinal sin).
2. **The date-less old fact inverts recency.** Phi-4 DID extract the Rivian's "last month" → 2026-05-11 correctly, but the Tesla (no date in its text) falls back to write-time (today) → it looks *newer* than the Rivian → recency would retire the **wrong** (Rivian) car. `effective_fact_time`'s write-time fallback is unreliable for mixed dated/undated pairs.

**Decision (Shahbaz): reverted the scaffold; do NOT force vault-side car resolution.** This is the genuinely-ambiguous case the agent-decides lock ([[project_architectural_lock_llm_out_of_read_path]]) is *for* — both Flash & Opus presented both cars correctly above. **Re-route gap #4 to agent-steering** (the car steer, bundled with the gap-#7 terse-query steer — both landed this session as MCP tool-description edits, NOT an `instructions.md`: no such file exists; the tool descriptions are the cross-platform lever per [[project_mcp_descriptions_cross_platform_lever]]). Cheap, safe, no recall risk. The fact-time *extraction tech works* (Phi-4 nailed the relative date) — it's just the wrong lever for this case; the agent-settable `as_of` (2026-05-30 decision) remains the safe write-time path for explicit dates. Spiking caught this in 110s, before a build+commit+live-test cycle. Arc B code reverted (working tree back to CI-green `d613614` for the consolidator). Memory: [[project_as_of_write_time_blocks_a5_temporal]] (UPDATE 2026-06-12).

### 13.3 · 🆕 Gap-table reclassification (2026-06-12, Shahbaz) — NO confirmed-broken output; #1/#2 are insurance, not must-fix

**The reframe (Shahbaz caught the inconsistency):** Wave 3 showed the agent produces CORRECT OUTPUT on *every* tested trap — salary, allergy, wrong-neighbour, instrument, car. So the same logic that closed the car (#4 — "agent handles it, don't force a vault fix") applies to #1/#2/#3 too. They were over-stated as "must-fix." **By the founder thesis (correctness of OUTPUT is the product) there is NO confirmed-broken item in the gap table.** Distinction that survives: #4 (car) has *no single correct answer* (ambiguous → fixing is *wrong*); #1/#2 *have* a correct answer the vault mis-ranks (fixing is *safe* — reorder-only, no deletion — but *not urgent* since output is already correct).

| # | Gap | Output correct today? | Status | Note |
|---|---|---|---|---|
| 1 | Wrong-neighbour #1 ranking | ✅ agent rescues | 🟡 **Insurance** | Build only if a correct fact gets truncated out of the agent's ~20-candidate view at scale, OR to harden Managed-mode (unknown weak agents). Measured at vault level §13.1. Roadmap §5.1. |
| 2 | Salary $-trap | ✅ agent rescues | 🟡 **Insurance** | Same arc as #1. |
| 3 | Blood/OS marginal abstain | ✅ agent handles | 🅿️ **Parked** | Tightening the floor risks killing real low-score answers; recall lock wins. |
| 4 | Car / contradiction | ✅ agent shows both | ✅ **Decided — agent-steer** | Ambiguous; fact-time spiked + reverted (§13.2). Steer SHIPPED-pending-gates this session. |
| 5 | `graph.duckdb` plaintext | n/a | 🟢 **Low-pri** | Fold into Pillar 3 (sync) security review; graph empty in V0.2. |
| 6 | Decay / archive | n/a | 🟢 **Planned build** | Part of Pillar 2 (T0.2.4) — not separate work. |
| 7 | Reranker brittle on terse queries | ✅ Opus decomposed | 🟠 **Steer SHIPPED-pending-gates** | MCP tool-description edits this session (staged uncommitted). |

**Pillar reclassification:** Pillar 1 (read precision = #1/#2) **de-prioritised to insurance** — was "the #1 arc," downgraded today because output is already correct via the agent. Pillars 2 (consolidator auto-run — has the ~90-fact hardware wall), 3 (sync), 4 (beta/daily-use) unchanged. **Product call pending:** keep hardening (insurance) vs pivot to real daily dogfood (lean: dogfood-first, the core produces correct output and is ready to *use*).

**Working-tree state at this close:** (a) `crates/vault-mcp/src/server.rs` — gap-#7 + car steer tool-description edits, **staged, NOT gated/committed** (Shahbaz: gates tomorrow bundled with more code). (b) `HANDOFF.md` — this update. (c) Consolidator Arc B fully reverted (matches `d613614`). (d) Out-of-repo: memory `project_as_of_write_time_blocks_a5_temporal` UPDATE + NEW `project_reranker_brittle_on_terse_queries` + MEMORY.md index line. CI still green on `d613614`; next commit must gate the server.rs change + CI-verify.
