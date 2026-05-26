# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)

**Last updated:** 2026-05-26 (T0.3.x **Batch A** pushed at `f0cc158` — Consolidator wired into runtime + `vault-cli consolidate run` subcommand + K-means topic discovery + per-boundary REPORT artifact + `invalidate()` auto-resolution wired into the merge phase. ADR-053 (REPORT shape + storage + lifecycle) shipped with this commit; ADR-052 + ADR-054 ride with Batch B Commit 6. Full local DoD green pre-push: ~610 tests pass workspace-wide / 0 failures / clippy `-D warnings` clean / fmt clean. **CI run #26449167018 `in_progress` for `f0cc158`** — next session opens by confirming CI `success` per [[ci-green-per-commit-vault-code]]. HANDOFF next-session opener rewritten for **Batch B** kick-off (Commits 6-7-8: structured-fact read pipeline + same-day delta log + founder dogfood); the rewrite is uncommitted and rides with the first Batch B code commit per [[admin-changes-ride-with-code]]. Last CI run before `f0cc158`: `08901bf` GREEN matrix-wide. Batch B picks up at Commit 6 (`read_pipeline.rs` Qwen→deterministic surgery) once CI on `f0cc158` confirms green.)

---

## 🆕 Current state

**Live arc:** [[locked-next-arc-t03x]] (amended 2026-05-26) — T0.3.x consolidator-driven structured-fact read pipeline + founder-dogfood. Phase C (write-time decision loop) DEFERRED to V1.0+. Four-step sequence:

1. ✅ **MCP `memory.write` description hardening** — shipped at `93d1410` (2026-05-25). Canonical-save contract in tool description + `vault-app::normalization` server-side helper + JSON-RPC wire-level pin test.
2. ✅ **Consolidator → REPORT (structured per-boundary state)** — shipped at **T0.3.x Batch A** (2026-05-26). K-means topic discovery (`crates/vault-consolidator/src/topics.rs`) + per-boundary REPORT artifact with atomic write (`crates/vault-consolidator/src/report.rs`) + `invalidate()` auto-resolution in the Phase 2 contradiction branch when the LLM surfaces a `clear_winner`. Phi-4-mini placeholder fallback when LLM unavailable → `TOPIC_NAMES_UNAVAILABLE` health-warning surfaces at read time (Batch B). ADR-053 rides here.
3. ⏳ **Read returns structured facts — NO LLM at read** — replaces Qwen-7B's 86s synthesis with cheap code: retrieve top-K (existing BGE + Tantivy + RRF + abstain) → filter by relevance threshold → structure into JSON facts → return via MCP. The calling agent (Claude / GPT / Codex / Kimi) composes its own response from the structured facts. Read latency: ~500ms total. **Batch B (Commit 6) — ADR-052 + ADR-054 ride here.**
4. ✅ **Wire consolidator into runtime + manual trigger** — shipped at **T0.3.x Batch A**. `Application::run_consolidation_with_safety` wraps `Consolidator::run_consolidation` in a cross-process lockfile (RAII guard at `crates/vault-app/src/consolidator_lock.rs`) + 30-min hard timeout + tracing span with `run_id`. CLI entrypoint: `vault-cli consolidate run --bge-model ... --bge-tokenizer ... --ort-lib ... --phi4-model ...` with `VAULT_*_PATH` env-var fallbacks. Founder-dogfood via Claude Desktop's MCP lands at **Batch B (Commit 8)** after the structured-fact read pipeline replaces Qwen.

### 🔒 Architectural lock (2026-05-26)

**The LLM (Qwen-7B) does not belong in the read path.** The vault's read consumer is itself an LLM (Claude / GPT / Codex / Kimi via MCP) — pre-composing prose for it was redundant work the agent re-does anyway in its own voice. Vault returns structured facts; agent composes.

**Three players, plain English:**
- **The agent** (Claude / GPT / Codex / Kimi) — lives OUTSIDE the vault. Talks to the user. Calls our 5 MCP tools. Composes responses. This is the user's choice; we don't run it.
- **Phi-4-mini** — lives INSIDE the vault. Nightly merge classifier (`vault-consolidator::phases::merge::decide_merge`). Cheap, offline, real quality contribution. **Keeps its job.**
- **Qwen-7B** — lives INSIDE the vault today at `vault-retrieval::read_pipeline::ReadPipeline`. Read-time prose synthesis. **Fired.** Replaced by deterministic code.

**Numbers the lock delivers (across all three deployment modes):**

| Mode | Read latency | Per-query cost |
|---|---|---|
| Local | 86s → ~500ms (170×) | GPU/CPU spike → ~zero |
| BYOK ($5/mo) | $0.02-0.05 → ~$0 (~50× cut) | only the agent's own LLM tokens |
| Managed PAYG | $0.001 per Qwen call → ~$0.0001 per read (~10×) | margin healthy across millions of users |

**What this supersedes:**
- ADR-048 (Qwen read-time pipeline) → effectively retired; formal supersession-ADR rides with the first code commit of Step 3
- ADR-049 (Qwen-7B model lock) → still locked formally but no longer ship-blocking for V0.2
- V0.2 backend tuning section (vulkan / metal / n_threads / KV cache / Q19 tail / speculative decoding) → moot for read path. Configuration preserved for any V0.2.x reversal but not load-bearing
- The "120s p99 ceiling" framing → moot

**What stays load-bearing:**
- ADR-051 (bi-temporal `invalidate()` API) — consumed by consolidator
- ADR-044/045/046/047 (consolidator surface) — unchanged
- BGE retrieval + Tantivy + RRF + abstain — the entire Phase 5 hybrid-retrieval architecture
- MCP canonical-save contract on write side (just shipped)
- BRD v1.4 (correctness-is-the-product thesis + three-mode deployment)

**The learning from t023-t027 spikes IS preserved** (we know what 7B does, what tuning knobs matter, what contradictions Qwen surfaces). The IMPLEMENTATION (the synthesis stage in `read_pipeline.rs`) becomes deprecated; the LEARNING informs how the structured-fact filter is designed.

**Last CI run:** `08901bf` (T0.2.3 close commit 16 — `CARGO_TARGET_DIR` at D: drive) — **GREEN matrix-wide** (ubuntu / macos / windows × build+test + clippy + fmt; weekly real-model smoke correctly skipped). Windows-CI disk-exhaustion saga closed; T0.2.3 close commits 14-16 fix-forward chain shipped clean.

**Working tree at this update:** uncommitted HANDOFF cleanup + architectural-lock section + technique map + `crates/vault-app/src/adapter.rs` update-path normalization + `crates/vault-mcp/src/server.rs` update + delete tool description hardening + `crates/vault-mcp/tests/initialize_smoke.rs` pin-test extension. Will ride with the next code commit per [[admin-changes-ride-with-code]].

---

## 📦 Consolidator inventory — what's built vs not (read this FIRST when planning T0.3.x)

The `vault-consolidator` crate already has ~1,000 LOC of production code + ~1,200 LOC of tests across 5 commits (T0.2.2 + T0.2.3). Future sessions should NOT re-discover this — the table below is canonical.

### Built + tested ✅

| Component | File | Status |
|---|---|---|
| **Phase 1 — Clustering** | `phases/cluster.rs` | Cosine ≥ 0.92, top-5 NN per memory, union-find transitive closure, deterministic ordering. Re-embeds at consolidation time because metadata-side `Memory.embedding` is `None`. ADR-045 |
| **Phase 2 — LLM decide** | `phases/merge.rs::decide_merge` | JSON-schema-constrained `LlmProvider::complete_json` call returns `MergeOutcome::{Merge, KeepSeparate, Contradiction}`. ADR-044 + Amendment 1 |
| **Phase 3 — Apply merge** | `phases/merge.rs::apply_merge` | Writes consolidated memory with summed `access_count` + max `confidence`, marks originals superseded via `mark_superseded` (ADR-046), re-embeds. Graph rewrite step is WARN-deferred (see tech-debt) |
| **Orchestrator** | `consolidator.rs::run_consolidation` | Enumerates ALL non-superseded memories → groups by boundary in `BTreeMap` (deterministic order) → per-boundary runs Phase 1 → 2 → 3 → builds `ConsolidationReport` |
| **Run-summary Markdown audit** | `summary.rs::generate_summary_markdown` | Human-readable per-run audit per BRD §5.6 + ADR-047. Per-boundary sub-sections. Privacy invariant tested (no cross-boundary content leak) |
| **`ConsolidatorConfig`** | `consolidator.rs` | BRD defaults: 3 AM run, 0.92 similarity, 180-day decay, 365-day archive, 1000 max memories/run |
| **`ConflictReview`** | `consolidator.rs` | Queue row for contradictions (uuid + boundary + ids + reasoning + flagged_at). Surfaced via `ConsolidationReport.conflicts_for_user_review` — does NOT auto-resolve per BRD §5.6 line 944 |
| **Tests** | `tests/*.rs` + co-located unit tests | Acceptance + property + per-boundary leak prevention; hand-curated 100-memory fixture; canned `MergeOutcome` responses for `MockLlmProvider` / `ScriptedLlmProvider` |

### Not built ❌

| Gap | Originally scoped | Status |
|---|---|---|
| **Phase 4 — Decay + archive** | T0.2.4 | Never started. `src/phases/decay.rs` not created. `memories_archived` field returns 0 |
| **Checkpoint + rollback** | T0.2.5 | Never started. `src/checkpoint.rs` not created. `checkpoint_id` is the literal string `"pending-T0.2.5"` in the run summary |
| **Scheduling** | T0.2.6 | Never started. `src/scheduler.rs` not created. `Consolidator::schedule()` is `todo!("T0.2.6 — vault-consolidator: Scheduling")`. The consolidator runs only when `run_consolidation()` is explicitly invoked |
| **`invalidate()` consumption** | T0.2.7 Phase B (2026-05-24) | Contradictions currently queue to `ConflictReview` only; the new bi-temporal `invalidate()` API (ADR-051) is not yet called by the consolidator. Plan-step for T0.3.x |
| **REPORT as read-pipeline input** | T0.3.x (locked 2026-05-25) | The existing `summary_markdown` is a run audit ("what happened last night"); the locked-next-arc imagines a DIFFERENT artifact — a curated knowledge state ("what's currently true per boundary") that the read pipeline serves from FIRST, vault fallback SECOND. ~5-10K tokens, per-boundary, refreshed nightly. Plan iteration 1 is the next session's first task |

### Open design questions for Step 2 + Step 3 plan iteration 1

Updated for the 2026-05-26 architectural lock. Each is a real architectural decision; plan-iteration depth 2-3 rounds per [[plan-iteration-depth-scales-with-design-surface]].

**Consolidator output side (Step 2):**

1. **REPORT shape** — structured JSON the agent can navigate? Topic-grouped objects with arrays of atomic-fact strings? Locking the schema is now THE central design call because REPORT IS the final structured output an external LLM (the agent) consumes — no internal LLM smooths over messy structure.
2. **REPORT location** — file-on-disk under SQLCipher-encrypted vault directory / SQLite row / Lance artifact?
3. **K-means topic discovery parameters** — fixed K per boundary, adaptive K, or per-vault config? Initial sketch: K = ceil(sqrt(N_memories_in_boundary / 4)) clamped to [3, 20]. Re-cluster from scratch nightly or incremental?
4. **Topic naming** — Phi-4-mini labels each cluster ("name this topic in 2-3 words"), or just cluster IDs, or LLM-free heuristic (e.g., most-frequent-noun)? Phi-4 labels probably worth the ~15 cheap nightly calls.
5. **Contradiction representation in REPORT** — when the consolidator detects an unresolved contradiction, do both facts appear with timestamps + a `contradiction_group_id`, or pick a winner (latest-wins), or surface in a sidecar `conflicts_for_user_review` list?
6. **Hygiene action policy** — when consolidator finds contradictions: `invalidate()` the older one (ADR-051), `mark_superseded()` if there's a clear replacement, archive, or leave-as-is for user review. What's the rule?
7. **What triggers consolidation** — time-based cron (3 AM per BRD default) / memory-count threshold / explicit user trigger? Probably all three for V0.2.

**Read side (Step 3):**

8. **MCP `memory.read` response shape** — what structured JSON does the vault return when the LLM is no longer composing prose? Sketch: `{ boundary, query, relevant_facts: [{fact, topic, memory_id, as_of, confidence, source_agent}], abstain }`. Need to lock the exact schema since it's the agent-facing contract.
9. **Filter logic replacing Qwen's relevance judgment** — what decides "this candidate IS relevant to the query"? Top-N rank? Score threshold? Combined? Existing abstain(threshold=1.0) handles zero-signal; we need a sibling for "include this fact in output."
10. **Same-day delta mechanism** — append-only log file / SQLite table for writes since last consolidation? Read pipeline merges REPORT + today's-deltas into the candidate pool? Or does retrieval over the whole vault subsume the need for a delta layer?
11. **REPORT-vs-vault routing** — simplified since no LLM at read. Probably: always retrieve from vault (top-K via BGE+Tantivy), use REPORT as enrichment layer (topic tags, contradiction markers, supersession chains). Need to confirm.

**Wiring side (Step 4):**

12. **Application startup wiring** — `vault-app::Application::start` constructs the `Consolidator` how? Adds dep on vault-consolidator + Phi-4 model availability check + config plumbing.
13. **CLI subcommand** — `vault-cli consolidate run` (manual) + `vault-cli consolidate report show <boundary>` (inspect) + `vault-cli consolidate dry-run` (preview without mutating)?
14. **Scheduling** (T0.2.6) — separate from this arc but eventually needed. Tokio cron job vs OS-level scheduler vs explicit "consolidate on shutdown" trigger.

**Effort estimate:** ~1 week to consolidator REPORT shape locked + K-means topic discovery shipped (Step 2). ~1 week to structured-fact read pipeline shipped (Step 3). ~3-4 days to runtime wiring + CLI subcommand (Step 4 prereq). ~2.5-3.5 weeks total to founder-dogfood-ready.

---

## 🧰 Technique map — what we use, add, defer, drop (locked 2026-05-26)

Mapped against the vault's six core behaviours: **A** Write · **B** Read · **C** Consolidate · **D** Sync · **E** Scale (Local / BYOK / Managed) · **F** Privacy + integrity.

### ✅ Keeping (already in the code or already-built primitive)

| Tool | Behaviour | Where it lives | Why it stays |
|---|---|---|---|
| **HNSW (hierarchical graph)** | B | LanceDB top-K vector search | Retrieval underpinning at 384-dim; validated at SCALE=10K |
| **Cascading writes / fan-out** | A | `vault-storage::cascading.rs` | One write → SQLite + Lance + DuckDB + audit log atomically. Already the write path |
| **Standard hashing (HashMap/HashSet/BTreeMap)** | A, B, C — everywhere | Boundaries, IDs, in-memory lookups | Zero false positives at our N; simpler than probabilistic structures |
| **Copy-on-write (implicit)** | C | SQLite WAL mode + Lance immutable files | Consolidator-time snapshots come free from underlying stores |
| **Phi-4-mini at consolidation** | C | `vault-consolidator::phases::merge::decide_merge` | Cheap nightly merge classifier; offline so latency doesn't bite. Optional but earns its keep |
| **BGE-small-en-v1.5 embedder** | A, B | `vault-embedding::BgeSmallProvider` | Not an LLM in the generative sense; 32M params; ~50-150ms deterministic embed. Foundation of retrieval |
| **Tantivy BM25 + RRF + abstain (threshold=1.0)** | B | `vault-retrieval::strategies::*` | Phase 5 hybrid retrieval; 9/9 quality at SCALE=10K |

### ➕ Adding for the locked-next-arc (Steps 2-4)

| Tool | Behaviour | What it does | Why MORE important in the new arc |
|---|---|---|---|
| **K-means clustering on BGE embeddings** | C | At consolidation: cluster each boundary's memories into ~8-15 natural topic groups; LLM (Phi-4) labels each cluster | REPORT structure IS the agent-facing output — no internal LLM smooths over messy topic grouping. Clean clusters at consolidation time are what makes the structured JSON navigable for the agent |
| **Token/count-budgeted structured packing** | B | At read: pack top-K retrieved candidates + relevance filter into JSON response payload under a sane size cap | The load-bearing read primitive that replaces Qwen-7B. Just smart engineering — no exotic structure |
| **Append-only delta log** | A → C | Track writes that landed since last consolidation run; read pipeline merges with REPORT at query time | Solves the "stale vault between nightly runs" gap. Plain SQLite table or append-only file |
| **Generational hygiene (concept, not library)** | C | Phase 4 decay: active → decayed → archived as memories age past thresholds | T0.2.4 work. No library to add; just the policy applied to existing fields |
| **Application startup wiring + CLI subcommand** | A, B, C | `vault-app::Application::start` constructs the Consolidator; `vault-cli consolidate run` triggers manually | The consolidator is a working library that nothing currently calls. Wiring it in is prerequisite to dogfood |

### ⏳ Deferring (real fits, wrong timing)

| Tool | Behaviour | When | Why deferred |
|---|---|---|---|
| **Cuckoo filters** | D | V0.2.9-13 sync arc | Compact "what I have" set-difference between devices with deletion support. Strict win over Bloom for sync |
| **DB sharding (per-tenant)** | E | V1.0+ Managed PAYG | Each user vault IS its own shard naturally per [[managed-mode-per-user-vault]]. No Vitess-style work needed |
| **CAS (compare-and-swap)** | A | V1.0+ if contention surfaces | Single-user local + per-vault Managed both stay single-writer; lock contention rare |
| **Replication lag handling** | E | V1.0+ Managed cluster concern | Property to manage if Managed mode runs replicated DB. Not a tool we add — concern that informs which managed DB we pick |
| **Single-brain consensus / Raft** | E | V1.0+ if needed | Per-user-vault sharding sidesteps multi-brain entirely. If Managed ever needs replicated state, prefer managed Postgres/Spanner over hand-rolled Raft |
| **Gossip protocols** | D | V1.0+ if mesh sync | Hub-and-spoke sync doesn't need gossip. Park unless we go peer-to-peer |
| **External sorting** | C, E | V1.0+ if cross-tenant batch ops | For sorting > RAM. We don't have 100M-row single-node workloads |

### ❌ Dropping (wrong tool for our workload — don't reach for these)

| Tool | Why it doesn't fit |
|---|---|
| **Bloom filters** | Cuckoo strictly beats them at the one job they'd do for us (sync set-difference) — better FP/size ratio + native deletion |
| **Z-order curves (Morton codes)** | Low-dim spatial range queries — we're 384-dim NN search. Locality preservation breaks down past ~8 dim |
| **Quad trees** | Same as Z-order — 2D spatial; our data isn't spatial |
| **Skip lists** | SQLite + Lance already cover ordered access; we don't have a LevelDB-style memtable workload |

### What changed because Qwen is out of read

| | Pre-2026-05-26 arc | Post-2026-05-26 arc |
|---|---|---|
| **K-means priority** | Useful for REPORT topic grouping | **More load-bearing** — REPORT structure IS the final output, no LLM to smooth messiness |
| **Token-budgeted packing** | Mattered because Qwen had a context window | **Different constraint** — bounded by MCP response size + agent parsing efficiency, not LLM context |
| **Speculative decoding (Qwen-0.5B draft)** | V0.2.x escape valve if Qwen tail > 120s | **Dead — no Qwen in read path** |
| **Phi-4-mini at consolidate** | Optional polish | **Still optional, even more comfortably so** — not user-blocking |
| **Exotic data-structure menu** | Tempting because chasing read-time latency | **Mostly dropped** — read is now ~500ms with cheap code; no structural breakthrough needed |
| **120s p99 ceiling** | Hard constraint shaping every tuning decision | **Moot for read path** — preserved only for any V0.2.x Qwen-revival contingency |

### Specialist's pick — direction summary

- **Adopt now**: K-means topic discovery + structured filter/pack code + append-only delta log
- **Keep using**: HNSW + cascading writes + hashing + CoW-via-SQLite/Lance + Phi-4-mini at night + BGE embedder + Tantivy/RRF/abstain
- **Park for sync (V0.2.9-13)**: Cuckoo filters
- **Park for V1.0+ Managed**: per-tenant sharding (we get it naturally), the consensus/replication stack (likely use managed DB, don't roll our own)
- **Don't reach for**: Bloom, Z-order, quad tree, skip list, external sorting

The architectural lock **simplified** the menu rather than complicated it. The vault needs brilliant plumbing (filter + structure + pack), not exotic structures.

---

## 🎯 Next-session opener — Batch B (Commits 6 + 7 + 8)

Read this whole block before any new work. **Batch A shipped at commit `f0cc158` (2026-05-26)** — the consolidator write-side of the locked-next-arc is done. Batch B picks up the read-side + same-day deltas + founder dogfood.

### Step 1 — Sanity check working tree + CI

```powershell
git status --short
gh run list --workflow=ci.yml -L 1
```

**Expected working tree:** only this HANDOFF.md (the Batch-B opener rewrite — admin ride-along that bundles with Commit 6 per [[admin-changes-ride-with-code]]). If anything else is uncommitted, investigate before proceeding.

**Expected CI:** the latest run is for `f0cc158` (T0.3.x Batch A). If it shows `success`: proceed. If `in_progress`: wait. If `failure`: STOP — read `gh run view <run-id> --log-failed` and triage before any Batch B code per [[ci-green-per-commit-vault-code]].

Per [[gh-run-watch-exit-not-equal-run-status]] — if `gh run watch` errors, that's network/rate-limit transient, NOT a CI failure. Verify actual run status via `gh run list` before alarming.

### Step 2 — Confirm Batch B scope, no plan re-litigation

**Plan iteration 3 is locked** (this chat session, 2026-05-26). All 5 Contracts + failure semantics are signed off:
- Contract 1: REPORT artifact shape + storage (ADR-053, **shipped at Batch A**)
- Contract 2: MCP `memory.read` response with `health` object (ADR-054, **ships at Commit 6**)
- Contract 3: Consolidator behavior — K-means + Phi-4 labels + contradiction `clear_winner` (**shipped at Batch A**)
- Contract 4: Same-day delta log (**ships at Commit 7**)
- Contract 5: Read pipeline (deterministic filter+pack, no LLM) (**ships at Commit 6**)

**Re-confirm briefly with Shahbaz:** "Plan iteration 3 still holds; we're picking up at Commit 6 (structured-fact read pipeline + ADR-052 + ADR-054). Confirm before code." This prevents silent drift back to LLM-at-read framing.

**Do NOT re-litigate the locked contracts.** If a recon surfaces a falsifying finding, surface it as a plan amendment with falsified-by evidence per [[retract-with-falsified-by-when-prior-iteration-wrong]] — not a quiet redesign.

### Step 3 — Batch B code sequence (3 commits per Shahbaz's batched-DoD direction)

Within Batch B, write all of Commit 6 + 7 + 8 in one stretch, then run gates once. Commit + push happens once at the end. Mirrors the Batch A cadence.

**Commit 6 — Structured-fact read pipeline** (~3-4 days):
- New `StructuredReadPipeline` in `crates/vault-retrieval/src/read_pipeline.rs` (or sibling module) — deterministic filter + pack, no LLM call.
- Inputs: retriever (existing BGE+Tantivy+RRF+abstain wired in `vault-app::Application::new`), per-boundary REPORT loader, optional same-day delta source.
- Output shape (locked, Contract 2): `{ boundary, query, relevant_facts: [{fact, topic, memory_id, as_of, confidence, source_agent}], abstain, health: { status, warnings: [{code, severity, detail, recovery_hint}] } }`.
- 7 locked warning codes (Contract 2): `REPORT_MISSING` / `REPORT_STALE_INFO` (24-72h) / `REPORT_STALE_WARN` (72h-7d) / `REPORT_STALE_CRITICAL` (7d+) / `DELTA_LOG_UNAVAILABLE` / `TOPIC_NAMES_UNAVAILABLE` / `CLOCK_SKEW_DETECTED`.
- Rip Qwen-7B out of `Application::new` step 9 (the existing `ReadPipeline` wiring with `Qwen25_14BProvider`). `AppConfig.qwen_model_path` becomes effectively dead — flag for removal at Commit 8 or leave with `#[allow(dead_code)]` and a comment pointing at the supersession.
- `vault-mcp::server.rs::tool_read` updates to surface the new shape.
- **ADR-052** rides here (supersedes ADR-048 + ADR-049: Qwen retired from read path).
- **ADR-054** rides here (MCP read response health contract + locked warning codes).

**Commit 7 — Same-day delta log** (~1-2 days):
- Schema migration 0004 (or whatever the next migration number is — check `crates/vault-storage/src/migrations/`): `CREATE TABLE delta_log (memory_id TEXT PRIMARY KEY, boundary TEXT NOT NULL, appended_at TIMESTAMP NOT NULL)`.
- Write path append in `vault-storage::cascading::write_memory` (or wherever the canonical write entrypoint lives) after the cascade commits.
- Read path UNIONs delta_log memory IDs with REPORT-bound candidate pool BEFORE the filter step (so today's writes are visible even though they're not in last night's REPORT).
- Consolidator clears delta_log rows older than `generated_at` at end of each successful run.
- Failure to read delta_log → surface `DELTA_LOG_UNAVAILABLE` warning at read time (Contract 2).

**Commit 8 — Founder dogfood + polish** (~1.5 days):
- End-to-end check from Claude Desktop via MCP stdio: write a few memories, run `vault-cli consolidate run`, read them back, verify the structured-fact shape arrives in Claude Desktop and Claude composes a coherent answer from it.
- Tighten any rough edges surfaced during dogfood. Possible items: error-message clarity, REPORT staleness threshold tuning, MCP tool description final polish.
- If Qwen-7B Rust code (`Qwen25_14BProvider` in `vault-llm`) is now fully unused after Commit 6, remove it here. Or defer to a V0.2 cleanup commit.

**Note on cadence:** per Shahbaz's batched-DoD direction at Batch A kickoff (2026-05-26), inside Batch B we write all three commits in one stretch, then run local DoD once (fmt → check → test → clippy → fmt --check → git status), then ask for combined commit+push approval, then wait for CI green. Same shape as Batch A.

### Step 4 — Remaining tech-debt (still not in Batch B scope)

The four open items in the Tech-debt section below are NOT small:
- Entity-extraction-at-consolidation + GraphStore relationship-rewrite — multi-week scope
- `VaultError::Storage(String)` → structured variants — ~30 call sites + new ADR
- `pending_sync` sweep + migration 0003 — ~80 LoC + schema migration + tests (ship-gated with V0.2 sync, not the consolidator arc)
- Lance Cosine NaN community filing — LOW priority

### Frozen vs open going into Batch B

**Frozen (do not re-litigate):**
- 🔒 **Architectural lock 2026-05-26**: LLM out of read; agent composes; vault returns structured facts. Phi-4 stays at consolidation; Qwen-7B fired from read path. See [[architectural-lock-llm-out-of-read-path]] memory.
- [[locked-next-arc-t03x]] — four-step sequence; Steps 2 + 4 shipped at Batch A; Step 3 + final Step 4 dogfood ship at Batch B.
- Phase C (write-time decision loop) DEFERRED to V1.0+
- **Plan iteration 3** (this chat session, 2026-05-26): all 5 Contracts + failure semantics locked.
- **Batch A deliverables** (commit `f0cc158`): `consolidator_lock.rs` RAII guard + 30-min hard timeout + `run_id` tracing span + `Application::run_consolidation_with_safety` + `vault-cli consolidate run` subcommand + K-means topic discovery (`topics.rs`) + per-boundary REPORT artifact (`report.rs`) + `MergeOutcome::Contradiction.clear_winner` auto-invalidate wiring.
- **ADR-053** (REPORT shape + storage + lifecycle) — shipped at Batch A.
- ADR-051 (bi-temporal `invalidate()` API contract) — still load-bearing, consumed by Batch A merge orchestrator.
- MCP `memory.write` + `memory.update` + `memory.delete` canonical-save contract (tool descriptions + field docs + server-side `normalize_for_canonical_save`)
- ADR-044 / 045 / 046 / 047 (consolidator surface) — still load-bearing
- ADR-048 / 049 — formally locked but **superseded by the architectural lock**; **ADR-052** rides with Commit 6 to formalize the supersession.
- Consolidator inventory above (canonical — do NOT re-discover; update in lockstep if new code lands)
- Technique map above (do NOT re-debate Bloom vs Cuckoo, Z-order, quad-tree, etc. — settled)
- BRD v1.4 (correctness-is-the-product thesis + three-mode deployment)

**Open (Batch B):**
- Commit 6: structured-fact read pipeline + ADR-052 + ADR-054
- Commit 7: same-day delta log + migration 0004
- Commit 8: founder dogfood + polish + (optional) Qwen-7B Rust code removal
- The four multi-session tech-debt items in the Tech-debt section
- Eventual: scheduling (T0.2.6), Phase 4 decay (T0.2.4), checkpoint+rollback (T0.2.5) — sequenced after Batch B

### Files to read first in next session

1. **This block** — current state + architectural lock + consolidator inventory + technique map + this opener
2. **Project memories** — [[architectural-lock-llm-out-of-read-path]] + [[locked-next-arc-t03x]] + [[correctness-is-the-product]] + [[mcp-descriptions-cross-platform-lever]] + [[managed-mode-per-user-vault]]
3. **CI status** — `gh run list --workflow=ci.yml -L 1` (confirm `f0cc158` shows `success`)
4. **Batch A outputs (consumed by Batch B)**:
   - `crates/vault-consolidator/src/report.rs` — REPORT artifact producer + shape; Commit 6's read pipeline consumes this.
   - `crates/vault-consolidator/src/topics.rs` — K-means producer; provides the topic labels surfaced in REPORT facts.
   - `crates/vault-app/src/consolidator_lock.rs` — RAII lockfile primitive (reference pattern; not load-bearing for Batch B but useful context).
   - `crates/vault-app/src/application.rs::run_consolidation_with_safety` — shows the safety-wrapper pattern (lockfile + timeout + tracing) for any future runtime hook.
5. **To-be-replaced code (Commit 6 surgery target)** — `crates/vault-retrieval/src/read_pipeline.rs::ReadPipeline`. Current Qwen-7B synthesis stage. Read for context; Commit 6 replaces with deterministic filter+pack.
6. **MCP wiring target** — `crates/vault-mcp/src/server.rs::tool_read`. Where the new structured-fact response shape gets surfaced.
7. **Existing health-degradation pattern in MCP** — `crates/vault-mcp/src/audit.rs::ToolInvokeError::from_vault_error` (for Internal-category mapping reference if Commit 6 adds new error surfaces).

### Three sentences to open next session with

If you're me opening cold: confirm CI green on `f0cc158` first, then re-anchor with Shahbaz "Plan iteration 3 still holds; we're at Commit 6 (structured-fact read pipeline + ADR-052 + ADR-054)." Read `crates/vault-consolidator/src/report.rs` + `crates/vault-retrieval/src/read_pipeline.rs` + `crates/vault-mcp/src/server.rs::tool_read` to ground the surgery before any code. Then proceed with Commit 6 → 7 → 8 as one stretch per Shahbaz's batched-DoD cadence, gates at the end of Batch B.

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

## ADR-053 — Per-boundary REPORT artifact shape + storage + lifecycle (T0.3.x Batch A)

**Status:** Accepted, T0.3.x Batch A (2026-05-26). Rides with the Batch A commit.

**Context.** The locked-next-arc (2026-05-26 architectural lock) replaced Qwen-7B read-time prose synthesis with a deterministic structured-fact read pipeline (Batch B Commit 6). The consolidator now produces a per-boundary REPORT artifact each nightly run that the read pipeline consumes to enrich retrieved candidates with topic tags + provide pre-computed topic groupings. **No LLM ingests this artifact** — it is agent-facing structured JSON, not narrative.

**Decision — shape.**

```json
{
  "schema_version": 1,
  "boundary": "personal",
  "generated_at": "2026-05-26T03:00:00Z",
  "consolidator_run_id": "uuid...",
  "facts_by_topic": {
    "<topic_label>": [
      {
        "fact": "<memory.content verbatim>",
        "memory_id": "<uuid>",
        "as_of": "<memory.valid_from per ADR-051 bi-temporal>",
        "confidence": <f32>,
        "source_agent": "<optional string>"
      }
    ]
  }
}
```

- `schema_version: u32` — pinned at `1`. Read pipeline at Commit 6 refuses unknown higher versions. Forward-compat guard against silent contract drift.
- `facts_by_topic`: `BTreeMap<String, Vec<ReportFact>>` — alphabetical ordering by topic label gives deterministic JSON output so consecutive nightly REPORTs diff cleanly. `HashMap` would break this; pinned by `report_serialisation_uses_deterministic_topic_ordering` test.
- `ReportFact` fields are exactly the agent-facing `memory.read` response shape at Commit 6 — no translation step between Report and the MCP wire format.
- Empty topics (members not resolvable in the supplied `memories` slice, e.g. superseded between topic discovery and report generation) are dropped from the output — `facts_by_topic` never contains an empty array. Pinned by `generate_report_drops_topics_whose_members_are_not_in_memories_slice`.

**Decision — storage layout.**

- **Path**: `<vault_root>/reports/<boundary>.report.json`. One file per boundary so cross-boundary reads don't cascade-fail if one REPORT is corrupt — the read pipeline at Commit 6 surfaces `REPORT_MISSING` per boundary independently.
- **Directory**: `reports/` under the vault root. Created lazily by `write_report_atomic` on first write.
- `<vault_root>` is derived from `AppConfig.metadata_path.parent()` (same root the consolidator lockfile lives under).

**Decision — atomic write protocol.**

Write to `<final>.tmp` → `Write::write_all` (JSON bytes via `serde_json::to_vec_pretty`) → `File::sync_all` → `std::fs::rename` to `<final>`. POSIX `rename(2)` is atomic; Windows `MoveFileEx` with the default `MOVEFILE_REPLACE_EXISTING` is atomic when source + target share a volume (always the case here — both paths live under the vault root). A reader of the REPORT file thus sees either the **old** valid REPORT or the **new** valid REPORT, never a half-written file. No separate file lock needed; the atomic-rename IS the read-safety primitive.

Pinned by `write_report_atomic_round_trips_through_json_serialization` + `write_report_atomic_replaces_previous_report_at_same_path` + `write_report_atomic_creates_reports_dir_if_missing`.

**Decision — versioning.**

Only the latest REPORT per boundary is kept. If a bad REPORT lands, the next nightly run fixes it. No version history at V0.2; the Batch B Commit 6 staleness-tier health-warnings (`REPORT_STALE_INFO` / `REPORT_STALE_WARN` / `REPORT_STALE_CRITICAL`) cover the "nobody re-ran the consolidator in N days" case.

Stale `.tmp` files (process killed between `fsync` and `rename`) persist until the next consolidator run; that run truncates them via `OpenOptions::truncate(true)` so no cleanup-on-acquire step is needed.

**Rejected alternatives.**

- **Per-topic files** (`<vault>/reports/<boundary>/<topic>.json`) — fan-out makes atomic publication of "this nightly's REPORT" impossible (no single rename can atomically swap N files). Single-file-per-boundary keeps the atomic-rename invariant.
- **SQLite table** for REPORT rows — would force the consolidator to write into the same encrypted database the read pipeline reads from. Acceptable but adds lock contention surface; encrypted file-on-disk is simpler and the consolidator is the only writer.
- **Latest + N-history versions** — multi-revision storage adds complexity for V0.2 founder-dogfood scale with no concrete consumer. The audit log already provides historical traceability if needed. Revisit at V1.0+ if a use case surfaces.
- **`facts_by_topic` as `Vec<TopicSection>`** (array of `{label, facts}` objects) — equivalent expressive power but requires custom binary-search to look up by topic name. `BTreeMap` keyed by label is more ergonomic for the read pipeline and serialises with the same alphabetical determinism.

**Cross-refs.** Locked-next-arc plan iteration 3 § Contract 1 (this chat session) · ADR-051 (bi-temporal `invalidate()`, consumed by Phase 2's `clear_winner` branch — orthogonal here) · ADR-052 (Qwen retirement from read path, Batch B Commit 6) · ADR-054 (MCP read response health-warning contract, Batch B Commit 6) · `crates/vault-consolidator/src/report.rs` (production impl + 7 unit tests) · `crates/vault-consolidator/src/topics.rs` (TopicMap producer + 7 unit tests; K-means + Phi-4 labeling + placeholder fallback).

### ADR-053 Amendment 1 — additive `topic_names_unavailable` field (Commit 6, 2026-05-26)

**Status:** Accepted, Commit 6 of locked-next-arc (2026-05-26). Rides with the Commit 6 code commit.

**Context.** During Commit 6 implementation source-read it surfaced that `vault_consolidator::topics::TopicMap` carries a `topic_names_unavailable: bool` signal (set when Phi-4-mini is unavailable and clusters fall back to placeholder `"topic_<id>"` labels) but the persisted `Report` shape locked at Batch A did NOT propagate the field. ADR-054 Contract 2 (Batch B Commit 6) requires surfacing this as the `TOPIC_NAMES_UNAVAILABLE` health-warning — without the producer-side field, the signal silently dies at the disk boundary.

**Decision.** Additive `topic_names_unavailable: bool` field on `Report`, populated from `TopicMap::topic_names_unavailable` by `generate_report`. `#[serde(default)]` makes pre-amendment REPORTs (none exist in practice — Batch A shipped 2026-05-26 with no nightly run yet) deserialize as `false`, preserving backward-compat. **No `REPORT_SCHEMA_VERSION` bump** — purely additive, backward-compatible.

**Rejected alternatives.**
- **Drop `TOPIC_NAMES_UNAVAILABLE` from Contract 2's locked 7 codes** — would shrink the agent-facing health surface to fit the producer's gap; the right direction is to grow the producer, not shrink the contract.
- **Bump `REPORT_SCHEMA_VERSION` 1 → 2** — higher risk: would break any in-flight REPORTs (none exist yet, but adding a version bump for an additive field with serde-default is over-engineering).

**Pin tests.**
- `generate_report_propagates_topic_names_unavailable_true_from_topic_map` (`report.rs::tests`)
- `generate_report_propagates_topic_names_unavailable_false_from_topic_map`
- `report_deserializes_pre_amendment_json_without_topic_names_unavailable_field`
- Read-side mirror: `load_defaults_topic_names_unavailable_to_false_when_field_missing` (`report_io.rs::tests`)

**Cross-refs.** ADR-053 base text (above) · ADR-054 (consumes this signal) · `crates/vault-consolidator/src/report.rs` (producer-side field) · `crates/vault-retrieval/src/report_io.rs::LoadedReport` (consumer-side mirror).

---

## ADR-052 — Qwen-7B retirement from read path (Commit 6 of locked-next-arc)

**Status:** Accepted, Commit 6 (2026-05-26). **Supersedes ADR-048 + ADR-049 in effect** (the LLM read-time pipeline they ship is retired; the model lock they document becomes archival).

**Context.** BRD v1.4 architectural lock (2026-05-26, captured in [[architectural-lock-llm-out-of-read-path]]) reframed the read path: the vault's consumer is itself an LLM (Claude / GPT / Codex / Kimi via MCP). Pre-composing prose for it was redundant work the agent re-does anyway in its own voice. Empirical anchors that drove the rethink:

- **Latency**: 86s mean on Vulkan iGPU (i7-13620H + UHD Graphics) was unshippable for an interactive agent surface (t027b results).
- **Cost** (BYOK and Managed PAYG modes): every read consumed BYOK tokens or PAYG inference cycles for a synthesis the agent immediately re-composes.
- **Quality drift**: the v9/v10 prompt evolution chased prose-elision patterns the agent's own LLM doesn't have at all (it composes in its own voice).
- **Architectural fit**: the agent is the contradiction-surfacer in three-mode deployment; the vault's job is to return the FACTS, not interpret them.

**Decision.** Retire `vault_retrieval::ReadPipeline` + `vault_llm::Qwen25_14BProvider`-in-read-path. Replace with `vault_retrieval::StructuredReadPipeline`: deterministic filter+pack over the existing BGE + Tantivy + RRF + abstain retrieval stack, enriched with per-boundary REPORT topic labels (ADR-053), and surfacing the seven ADR-054 health-warnings. No LLM in the read path.

**What stays (no LLM-removal contagion):**
- **Phi-4-mini at nightly consolidation** (`vault_consolidator::phases::merge::decide_merge`) — cheap binary merge classifier, offline, real quality contribution. Untouched.
- **BGE-small-en-v1.5 embedder** — not an LLM in the generative sense (32M param encoder); ~50-150ms deterministic embed. Foundation of retrieval; untouched.
- **ADR-051 bi-temporal `invalidate()`** — still load-bearing for consolidator Phase 2.
- **ADR-053 REPORT shape** — consumed by the new read pipeline; amended additively per ADR-053 Amendment 1.
- **ADR-044 / 045 / 046 / 047** — consolidator surface, unchanged.

**Implementation surface (delete + add):**

| Surface | Change |
|---|---|
| `crates/vault-retrieval/src/read_pipeline.rs` | **DELETED** (whole file) |
| `crates/vault-retrieval/src/structured_read_pipeline.rs` | **NEW** (~700 lines incl. 21 unit tests) |
| `crates/vault-retrieval/src/report_io.rs` | **NEW** (`LoadedReport` + `FilesystemReportLoader`) |
| `crates/vault-retrieval/tests/read_pipeline_acceptance.rs` | **DELETED** |
| `crates/vault-retrieval/tests/read_pipeline_scale_acceptance.rs` | **DELETED** |
| `crates/vault-retrieval/tests/full_stack_smoke.rs` | **DELETED** (coverage moved to unit tests + adapter integration tests) |
| `crates/vault-retrieval/examples/t025*..t031*.rs` | **DELETED** (13 Qwen-anchored spike examples; `.md` results files preserved) |
| `crates/vault-app/src/application.rs::Application::new` step 9 | Qwen wiring **REMOVED**; StructuredReadPipeline wired |
| `crates/vault-app/src/adapter.rs::VaultAdapter` | `read_pipeline: Option<ReadPipeline>` → `read_pipeline: StructuredReadPipeline` (no Option — always wired) |
| `crates/vault-app/src/config.rs::AppConfig::qwen_model_path` | Marked `#[allow(dead_code)]` (Commit 8 removes the field entirely) |
| `crates/vault-mcp/src/server.rs::tool_read` | Tool description rewritten for new structured-fact contract |
| `crates/vault-mcp/src/adapter.rs::Adapter::read` | Trait return type `ReadResponse` → `StructuredReadResponse` |
| `crates/vault-llm/src/qwen25.rs` | **KEPT** (Commit 8 removes the Rust code if fully unused after dogfood) |

**Numbers the supersession delivers (across all three deployment modes):**

| Mode | Read latency (was → is) | Per-query cost |
|---|---|---|
| Local | 86s → ~500ms (~170×) | GPU/CPU spike → ~zero |
| BYOK ($5/mo) | $0.02-0.05 → ~$0 (~50× cut) | only the agent's own LLM tokens |
| Managed PAYG | ~$0.001 → ~$0.0001 (~10×) | margin healthy across millions of users |

**Rejected alternatives.**

- **Keep ReadPipeline as opt-in via config flag** — adds branching at every MCP read site; complicates the agent contract (which response shape am I getting?); requires V0.2 founders to choose at install time without empirical guidance. Clean cut beats configurable.
- **Deprecate-don't-delete `ReadPipeline`** — `#[deprecated]` markers on a load-bearing type bleed everywhere. The CLAUDE.md no-backwards-compat rule applies: delete the code.
- **Keep Qwen for "high-stakes" reads, structured-fact for "casual"** — invents a heuristic that doesn't exist in the agent's intent. Every read is just a read; let the agent decide what's high-stakes.
- **Defer the architectural lock until Phase C** — defers the BYOK cost-savings + Managed PAYG margin win for no benefit. The lock IS structurally simpler than what it replaces.

**Pin tests (the integration-test removal is replaced by tighter unit coverage):**
- 21 unit tests in `crates/vault-retrieval/src/structured_read_pipeline.rs::tests` covering: query validation + abstain short-circuits (4), boundary field semantics (2), filter+pack with topic lookup (4), 7 warning codes (7), aggregate status rules (4).
- 8 unit tests in `crates/vault-retrieval/src/report_io.rs::tests` covering: file-missing, valid-JSON deserialise, schema-default behavior, malformed-JSON Serde error, path resolution.
- 3 new tests in `crates/vault-consolidator/src/report.rs::tests` pinning ADR-053 Amendment 1's additive field.
- VaultAdapter unit tests (`crates/vault-app/src/adapter.rs::tests`) updated to construct a real `StructuredReadPipeline` with `NoopReportLoader`; pre-existing search/write/update/delete coverage continues unchanged.

**Cross-refs.** [[architectural-lock-llm-out-of-read-path]] (the founder-side framing) · [[locked-next-arc-t03x]] (the work-breakdown) · ADR-048 (V0.2 read pipeline — superseded; archival reference) · ADR-049 (Qwen2.5-7B model lock — superseded for read path; archival reference) · ADR-053 (REPORT shape, consumed) · ADR-054 (MCP read response health contract, ships at same commit) · BRD v1.4 (correctness-is-the-product + three-mode deployment).

---

## ADR-054 — MCP `memory.read` response health-warning contract (Commit 6 of locked-next-arc)

**Status:** Accepted, Commit 6 (2026-05-26). Locks the locked-next-arc Plan Iteration 3 Contract 2 surface.

**Context.** With Qwen-7B retired from the read path (ADR-052), the MCP `memory.read` tool now returns structured facts the agent composes from. But the agent needs to know when the vault state behind those facts is stale, missing, or otherwise compromised — otherwise it'll cheerfully compose answers from a REPORT that hasn't been refreshed in a month. The agent contract needs a structured health surface.

**Decision — response shape.**

```text
{
  "boundary": "<name>" | null,        // null for multi-boundary
  "query": "<echo of trimmed query>",
  "relevant_facts": [
    {
      "fact": "<memory content verbatim>",
      "topic": "<consolidator label>" | null,
      "memory_id": "<uuid string>",
      "as_of": "<RFC3339 DateTime<Utc>>",
      "confidence": <f32>,
      "source_agent": "<name>" | null
    }
  ],
  "abstain": true | false,
  "health": {
    "status": "ok" | "degraded" | "critical",
    "warnings": [
      {
        "code": "<one of seven locked codes>",
        "severity": "info" | "warn" | "critical",
        "detail": "<human-readable specifics>",
        "recovery_hint": "<user-actionable guidance>"
      }
    ]
  }
}
```

**Decision — seven locked warning codes (no eighth without a Contract amendment).**

| Code | Severity | Trigger |
|---|---|---|
| `REPORT_MISSING` | `warn` | No REPORT artifact for the queried boundary. Most common cause: nightly consolidator hasn't run yet on a fresh vault. Also fires when `schema_version` > `SUPPORTED_REPORT_SCHEMA_VERSION` (future REPORT version the binary can't safely interpret). |
| `REPORT_STALE_INFO` | `info` | REPORT `generated_at` age in the 24-72h band. Light signal — fresh enough for most reads. |
| `REPORT_STALE_WARN` | `warn` | REPORT age in the 72h-7d band. Vault state may have drifted; consolidator hasn't run in 3+ days. |
| `REPORT_STALE_CRITICAL` | `critical` | REPORT age ≥ 7d. Major drift; consolidator hasn't run in a week. |
| `DELTA_LOG_UNAVAILABLE` | `warn` | Same-day delta log unavailable. **Reserved for Commit 7 (next session) — Commit 6 NEVER emits this code.** Surfaces when delta-log reads fail and same-day writes may not appear in the response. |
| `TOPIC_NAMES_UNAVAILABLE` | `info` | REPORT carries placeholder `"topic_<id>"` labels (Phi-4-mini was unavailable at consolidation time). Agent should treat `topic` field as opaque cluster identifiers, not semantic labels. Driven by ADR-053 Amendment 1's `topic_names_unavailable: bool`. |
| `CLOCK_SKEW_DETECTED` | `critical` | REPORT `generated_at` is in the future relative to the read-time clock. Indicates clock drift between the consolidator and read hosts (or a deliberate skew). Staleness math becomes unreliable; surfaced as Critical so the agent doesn't silently propagate misleading "fresh" assessments. |

**Decision — staleness threshold values (locked).**

- `STALE_INFO_THRESHOLD = 24 hours`
- `STALE_WARN_THRESHOLD = 72 hours`
- `STALE_CRITICAL_THRESHOLD = 7 days`

Pinned as `pub const` in `crates/vault-retrieval/src/structured_read_pipeline.rs`. Future tuning requires an ADR-054 amendment + test updates.

**Decision — aggregate `status` rule (deterministic).**

1. Any `WarningSeverity::Critical` warning present → `HealthStatus::Critical`.
2. Else if any `Info` or `Warn` warning present → `HealthStatus::Degraded`.
3. Else (no warnings) → `HealthStatus::Ok`.

Pinned by 4 unit tests in `structured_read_pipeline.rs::tests` (one per branch + the no-critical-with-warn case).

**Decision — emission ordering (deterministic).**

For each authorised boundary in input order, the pipeline emits at most one of each warning type in this fixed sequence:
1. Schema-guard (`REPORT_MISSING` via unsupported version) — short-circuits other checks for that boundary
2. `CLOCK_SKEW_DETECTED` — dominates staleness math; when present, the staleness tier check is skipped
3. Staleness tier (`REPORT_STALE_INFO` | `REPORT_STALE_WARN` | `REPORT_STALE_CRITICAL`) — exactly one fires per stale REPORT
4. `TOPIC_NAMES_UNAVAILABLE` — independent of staleness; fires when `topic_names_unavailable: true`

Boundary-order × per-boundary sequence makes consecutive identical reads byte-identical, which simplifies agent-side caching + diffing.

**Rejected alternatives.**

- **Free-form warnings (no locked code set)** — agents can't reliably branch on string contents; locked enum is the contract surface.
- **More codes** (e.g. `REPORT_SCHEMA_UNSUPPORTED` distinct from `REPORT_MISSING`, `RETRIEVER_DEGRADED`, `BOUNDARY_EMPTY`) — over-engineering for V0.2; can amend the Contract if real consumer evidence surfaces.
- **Different severity assignments** (e.g. `REPORT_STALE_CRITICAL` as Warn) — empirically anchored to "consolidator hasn't run in a week is the agent-blocking case". If beta telemetry shows different thresholds, amend.
- **Aggregate-status as max-severity instead of three-tier** — equivalent expressively but `Critical` / `Degraded` / `Ok` reads better in agent prompts than "max severity = warn". Tier names also stable under future severity additions if Contract grows.

**Pin tests.**
- 7 tests in `structured_read_pipeline.rs::tests` exercising each warning code's trigger + severity (`report_missing_*`, `report_age_24_to_72_hours_*`, `report_age_72_hours_to_7_days_*`, `report_age_7_plus_days_*`, `report_with_topic_names_unavailable_*`, `report_generated_at_in_future_*`, `commit_6_never_emits_delta_log_unavailable_warning`).
- 4 tests pinning aggregate-status rules (`aggregate_status_is_ok_*`, `*_degraded_when_only_info_*`, `*_degraded_when_warn_present_*`, `*_critical_when_any_critical_*`).
- 4 boundary-field semantics tests (`single_boundary_*`, `multi_boundary_*`, `zero_authorized_boundaries_*`, `empty_retrieval_*`).
- Tool-description sanity pin: the Commit 6 changes the MCP `tool_read` description; pinned indirectly by `initialize_smoke.rs` `tools/list` contract.

**Cross-refs.** ADR-052 (Qwen retirement, ships at same commit) · ADR-053 + Amendment 1 (REPORT shape consumed) · `crates/vault-retrieval/src/structured_read_pipeline.rs` (production impl + 21 unit tests) · `crates/vault-mcp/src/server.rs::tool_read` (agent-facing description) · [[locked-next-arc-t03x]] Plan Iteration 3 Contract 2.

---

## ADR-051 — Bi-temporal storage semantics + invalidation API contract (T0.2.7 Phase B, merged-consolidator arc)

**Status:** Drafted before code, 2026-05-24, T0.2.7 close. Pre-locks the semantics consumed by Phase B retrieval-filter wiring and the Phase C write-time `ADD/UPDATE/DELETE/NOOP` loop. ADR-050 (V0.2 read-time architecture lock) is the unrelated sibling tracked separately; numbering skips ADR-050 here.

**Context.** Bi-temporal storage fields are not new — they were locked at BRD v1.0 §1.3 bet #1 ("every fact has `valid_from`, `valid_until`, `confidence`, `superseded_by`") and implemented in the schema at T0.1.3 (`crates/vault-storage/src/migrations/0001_initial.sql:14-15`), the domain entity at the same time (`crates/vault-core/src/memory.rs:92-93`), and the SQL persist/load paths in `metadata_store.rs`. The 2026-05-24 product discussion on bloat-defense triggered a source-read that confirmed: the schema is fully present, `valid_from` defaults correctly (now() at create), `superseded_by` has the `mark_superseded` setter from T0.2.3 (`cascading.rs:508`) and is retrieval-filtered at `semantic.rs:192` + `keyword.rs:426` + `metadata_store::list_memories:758` (`include_archived=false` default). What is **missing**: `valid_until` is never set by any API and never filtered by any retrieval strategy. The merged consolidator plan requires both — Phase C's write-time `UPDATE`/`DELETE` decisions, the Zep-pattern bi-temporal invalidation surfaced by the 2026-05-24 research spike, and the future T0.3.x consolidator-driven compression all need a locked contract for `valid_until` semantics + invalidation API before any consumer code lands.

**Decision — semantics of `valid_until`.**

- `valid_until` is **fact-time** — the timestamp at which a memory's content stopped being true in the world. NOT vault-deletion time. NOT garbage-collection time. NOT consolidation-archive time.
- `None`: the memory's content is currently believed to be true. (Default at create.)
- `Some(t)` where `t <= now()`: the fact was true until `t`; currently expired. Retrieval skips by default.
- `Some(t)` where `t > now()`: the fact has a known future expiration (e.g., "Q1 2027 deadline" with `valid_until = 2027-04-01`). Retrieval **includes** these — the fact is still true today.
- Distinct from `created_at` / `updated_at` (vault-time, when the memory was added / last edited) and `last_accessed` (vault-time, retrieval-recency).

**Decision — default retrieval filter (locked across all strategies).**

- Default (`include_archived = false`): exclude memories where `valid_until IS NOT NULL AND valid_until <= now()` (expired). Existing exclusion of `superseded_by IS NOT NULL` (superseded) remains.
- The existing `include_archived` flag semantically expands to **"include both expired AND superseded"** — single flag, both behaviors grow together. Rationale: callers asking for archival visibility want full historical state; splitting the flag into "include_expired" + "include_superseded" doubles the surface for no consumer benefit identified in V0.2.
- Future-dated `valid_until` (`valid_until > now()`) is NOT a filter trigger — the fact is currently true.
- Filter lives at the strategy layer (semantic.rs / keyword.rs) and the list_memories path, mirroring the existing `is_superseded()` filter. No new schema columns. No new indexes in V0.2 (re-evaluate at SCALE=100K+ if `valid_until` lookups dominate the query plan; current SQLite plan is fine for vault sizes through the V0.2 ship).

**Decision — invalidation API surface.**

- New API: `vault-storage::cascading::StorageOps::invalidate(memory_id, valid_until_at, reason)`. Mirrors `mark_superseded` shape — transactional via `with_transaction`, returns `Ack` with `committed_at`. Emits an audit event per BRD §11.9.2 (`event_type: "memory.invalidate"`, `details_json` includes `reason` + `valid_until_at`).
- **Boundary enforcement is the CALLER's responsibility, not the storage primitive's.** This matches existing convention: `mark_superseded` (cascading.rs:508) does NOT do an internal boundary check either — it trusts the memory_id supplied. Boundary checks happen at the MCP layer (`vault-mcp/src/server.rs`) before any storage primitive is invoked, where the request's `authorized_boundaries` slice is available. Internal callers (consolidator, write-time loop) already pre-filter by boundary in their workflows before reaching the storage primitive. Source-read 2026-05-24 corrected the earlier draft of this ADR which incorrectly described `mark_superseded` as boundary-checked.
- **Latest-wins on repeat invalidation:** invalidating an already-invalidated memory updates `valid_until` to the new timestamp. Earliest-wins was considered and deferred — the "we discovered later that the fact actually became false earlier than we recorded" edge case is rare for V0.2; explicit re-write handles it. Document in code comment; revisit if telemetry surfaces the case.
- Does NOT touch `superseded_by`. Orthogonal field.
- Allows `valid_until_at` in the future (planned expirations, e.g., "this fact becomes false after Q1 2027").
- Invalidation does NOT delete or archive the memory — the row stays, retrieval just skips it under default filter.

**Decision — relationship to `mark_superseded` (orthogonality lock).**

`valid_until` and `superseded_by` are **independent fields with independent setters**. Both may be set on the same memory. They answer different questions:

| Field | Question it answers | Set by |
|---|---|---|
| `valid_until` | When did the fact stop being true? | `invalidate()` (NEW, this ADR) |
| `superseded_by` | Which memory replaced this one? | `mark_superseded()` (existing, T0.2.3) |

Composition of the two in the future write-time loop (Phase C, separate work-breakdown):

| Write-time loop decision | Calls | Effect |
|---|---|---|
| `ADD` (genuinely new fact) | (none) | New memory created with `valid_from = now`, `valid_until = None`. |
| `UPDATE` (replaces a contradicting fact) | `invalidate(old_id, now)` + `mark_superseded(old_id, new_id)` in the same transaction | Old memory has both fields set: fact stopped being true + replaced by new memory. |
| `DELETE` (contradicts a fact with no replacement) | `invalidate(old_id, now)` only | Old memory has `valid_until = now`, `superseded_by` untouched. |
| `NOOP` (no-op, duplicate signal) | (none) | No state change. |

The existing T0.2.3 consolidator merge path (`vault-consolidator/src/phases/merge.rs:348`) is unchanged — continues to call `mark_superseded` only when merging duplicates into a new memory. Duplicate-merging is NOT a fact-becoming-false event; `valid_until` should stay `None` on the consolidated members. This preserves existing consolidator semantics.

**Migration: none.** Schema already exists. Existing rows have `valid_until = NULL` by default — they remain currently-valid post-rollout. The retrieval-filter change is forward-compatible (existing memories with `valid_until = NULL` continue to surface). No data migration. No schema migration. The Phase B work is purely code wiring + tests.

**Boundary-of-this-ADR (explicit out-of-scope).**

This ADR locks ONLY: `valid_until` semantics, retrieval filter behavior, `invalidate()` API surface, the orthogonality lock with `mark_superseded`. It does NOT lock:

- The write-time `ADD/UPDATE/DELETE/NOOP` decision loop (Phase C; separate ADR if needed).
- The MCP `vault_capacity_used` + health-metadata signal in tool responses (Phase H).
- The pre-cooked summary format from T0.3.x consolidator-driven read pipeline (Phase G).
- Confidence-decay-over-time on the `confidence` field (T0.2.4 decay phase; uses `last_accessed` + decay function, not `valid_until`).
- Cross-device invalidation semantics under sync (T0.2.9-13; sync arc, deferred).

**Rejected alternatives.**

- **Earliest-wins on repeat invalidation.** Closer to bi-temporal-database academic literature ("we know now that the fact actually became false earlier"). Rejected for V0.2 because: (a) the use case is rare; (b) it requires the caller to know the historically-correct invalidation time, which the write-time loop generally doesn't; (c) latest-wins is simpler and the rare correction case can be done by direct field write + admin tool. Revisit at V1.0 if a real workload surfaces.
- **Splitting `include_archived` into `include_expired` + `include_superseded`.** Two flags. No identified V0.2 consumer needs only one. Doubles the surface. Rejected.
- **Re-using `superseded_by` to mean "invalidated" by pointing to a sentinel `INVALID` memory ID.** Considered. Rejected because it overloads a field with two unrelated meanings + breaks the consolidator's existing supersession-chain invariant.
- **Auto-archiving (physically moving to an archive table) when `valid_until <= now()`.** Considered. Rejected for V0.2 because the row stays cheap to retain, lineage is preserved, and rollback (T0.2.5) becomes simpler. May re-evaluate at SCALE=100K+ if storage cost dominates.

**Cross-refs.** BRD §1.3 bet #1 (confidence-decay knowledge graph, the original spec source) · BRD §5.1 (Memory struct definition, lines 585-601) · BRD §11.9.2 (audit log invariants) · ADR-046 (mark_superseded contract — orthogonal here, not amended) · T0.2.3 commit `17035ec` (mark_superseded primitive shipped) · `crates/vault-core/src/memory.rs:82-100, 198-204` (current schema + invariant) · `crates/vault-storage/src/migrations/0001_initial.sql:7-26` (SQL schema) · `crates/vault-storage/src/cascading.rs:508` (existing mark_superseded) · `crates/vault-retrieval/src/strategies/semantic.rs:192` + `keyword.rs:426` (existing supersession filter) · Merged consolidator plan iteration 1, 2026-05-24 (this chat session — to land in HANDOFF "Active task" block with first Phase B code commit).

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

### ✅ SHIPPED at T0.2.7 Phase 5 Step 2 (2026-05-22) — Promote `bulk_upsert` from t028b spike to `VectorStore` trait + production

**STATUS: shipped in the Phase 5 commit (`c091281`).** This entry is retained as the design record per the closing decision: no formal ADR was drafted because the change is additive (trait method extension), spike-validated at 730× speedup, and ship-gate consumers were documented before promotion. Future amendments (e.g., chunking strategy when SCALE > 100K) can take an ADR-051 sibling if needed.

**Original gap.** `crates/vault-storage/src/vector_store.rs` already contained a working `bulk_upsert` helper authored during the t028b HNSW-vs-IVF spike (2026-05-17 session) measuring **730× faster** insertion vs single-row upsert at 10K. But the helper lived as a `pub fn` on the concrete LanceDB impl, NOT on the `VectorStore` trait — so production code (sync, MetadataStore consumers, connectors) couldn't call it through the trait. Concrete consumers existed (V0.2 cross-device sync; V1.0 Gmail+Calendar connectors) so the forward-compat pull discipline allowed promotion.

**What shipped.**

1. **`async fn bulk_upsert(&self, rows: &[(MemoryId, Vec<f32>, Boundary)]) -> VaultResult<()>` added to `VectorStore` trait** (`crates/vault-storage/src/vector_store.rs`). Trait doc-comment captures the load-bearing contract (empty-input idempotency, atomicity on dimension mismatch, `id`-only merge_insert key, call-site sizing guidance).
2. **Concrete impl moved** from the standalone `pub async fn bulk_upsert` on `impl LanceVectorStore` to inside `impl VectorStore for LanceVectorStore`. Same body — `upsert_lock` ADR-038 mutex, `merge_insert` with `id`-only matching key, dimension validation upfront so atomicity holds. Added `#[instrument(skip(self, rows), fields(n_rows, dim))]` for observability parity with single-row `upsert`.
3. **Six unit tests** in `vector_store.rs::tests` covering: empty-slice no-op, single-row searchable parity, N-row (100) all-searchable, dimension-mismatch writes-zero-rows atomicity, same-id-different-boundary-replaces-not-duplicates security pin (mirrors the single-row test), bulk-then-delete composition.
4. **One property test** added to the existing `proptest::proptest!` block (`bulk_upsert_round_trip_preserves_all_rows_across_random_partitions`).
5. **`read_pipeline_scale_acceptance.rs` setup loop updated** to call `vectors.bulk_upsert(&rows)` once for the whole corpus.
6. **Chunked impl follow-up at T0.2.7 Phase 5 Step 2** (`BULK_UPSERT_CHUNK_ROWS = 2000`) — needed because SealedObjectStore doesn't implement `put_multipart`. Chunk size keeps each sub-batch below the 5MB multipart threshold.

**Affected files (forward-pointer audit trail).**
- `crates/vault-storage/src/vector_store.rs` — trait + impl now live here
- `crates/vault-retrieval/examples/t028b_hnsw_vs_ivf_spike.rs` — original spike consumer (executable documentation per [[spike-playbook-for-unknowns]])
- `crates/vault-retrieval/tests/read_pipeline_scale_acceptance.rs` — consumer using the promoted trait method
- Future: `vault-sync` (V0.2 sync) + `vault-connectors` (V1.0 Gmail/Calendar)

---

### T0.2.x — `VaultError::Storage(String)` grab-bag → structured variants refactor

**Surfaced:** T0.1.8 Phase 3 (2026-04-30, ADR-018 / Phase C plan v2 closing note). **Priority elevated:** T0.2.0 Phase 0b lance 4.0 audit (2026-05-07). **Lifted into current HANDOFF tech-debt at T0.2.7 Phase 5 Step 2 audit (2026-05-22)** after originally tracked in archives — the floating code-comment references in `vault-storage/src/retry_queue.rs:248-265` + `vault-core/src/error.rs:139` lost their HANDOFF.md anchor through the V0.1 → V0.2 archive freeze. Audit lift restores the anchor.

**The gap.** The cascading orchestrator's `is_permanent` classifier in `crates/vault-storage/src/retry_queue.rs::is_permanent` currently substring-matches `Storage(msg)` to recognise permanent-class lance errors:

```rust
VaultError::Storage(msg)
    if msg.contains("schema")
        || msg.contains("CastError")
        || msg.contains("dimension")
        || msg.contains("No vector column found") =>
{
    true
}
```

That works today but defeats type-safe matching, and lance 4.0's Phase 0b audit (2026-05-07) confirmed lance's error wording is inconsistent across schema-shape faults. Without all four substring patterns enumerated, a permanent fault would retry 8 times before dead-lettering instead of going straight there.

**Why priority elevated.** *"Production risk LOW (orchestrator's `eager_validate` catches dim/schema before merge_insert), but landing the structured-variant refactor early-V0.2 is now warranted rather than deferring deep into V0.2.x."*

**What lands at T0.2.x.**

1. **New `VaultError` variants** in `crates/vault-core/src/error.rs`: `VaultError::SchemaMismatch { table: String, detail: String }`, `VaultError::IoFailure(...)`, and other categories surfaced by the audit. Existing `VaultError::Storage(String)` either stays as the catch-all "uncategorised" bucket or gets removed entirely.
2. **Re-categorise every `VaultError::Storage(format!(...))` call site** in vault-storage. Estimated ~30 sites across `metadata_store.rs` / `vector_store.rs` / `graph_store.rs` / `cascading.rs` / `retry_queue.rs`. Each site picks the right structured variant.
3. **Rewrite `is_permanent` as an exhaustive `match`** — no more substring matching. The compiler enforces coverage; new variants must be classified explicitly.
4. **Tests:** existing retry-queue + cascading tests cover the behaviour; add a dedicated `is_permanent_exhaustive_match_covers_all_variants` tripwire test that fails if a new `VaultError` variant lands without being classified.
5. **Per ADR-018 plan:** stand-alone refactor task, NOT a drive-by. Schedule at the start of T0.2.x by then we'll have a fuller picture of which error categories actually matter from the consolidator + sync angles.

**Affected files (forward-pointer audit trail).**
- `crates/vault-storage/src/retry_queue.rs:240-275` — substring-matching workaround currently lives here
- `crates/vault-core/src/error.rs:139` — `VaultError::Storage` variant defined here
- `crates/vault-storage/src/metadata_store.rs` + `vector_store.rs` + `graph_store.rs` + `cascading.rs` — ~30 `Storage(format!(...))` call sites to re-categorise
- ADR-018 — eventual reference; will likely need an amendment when the new variants are locked

---

### T0.2.x — `pending_sync` sweep + migration 0003 cascade payload

**Surfaced:** T0.1.9 Phase A (2026-04-30) when the divergence detector's `pending_sync` sweep was designed but the schema migration that would carry its payload was deferred. **Lifted into current HANDOFF tech-debt at T0.2.7 Phase 5 Step 2 audit (2026-05-22)** after originally tracked in archives — floating code-comment references in `vault-storage/src/divergence.rs:38-48` + `vault-cli/src/main.rs:205` lost their HANDOFF.md anchor through the archive freeze. Audit lift restores the anchor.

**The gap.** Phase A's design intent for the divergence detector's `pending_sync` sweep was to drain rows back into `retry_queue` when capacity returns. But the migration 0002 schema only carries `(memory_id, operation, queued_at)` — it lacks the cascade payload (`embedding` + `boundary`) needed to reconstruct a `NewRetry`. The orchestrator's overflow path drops the payload because Phase B's schema didn't reserve room for it.

**Current V0.1 behaviour (stub).** `DivergenceDetector::sweep_pending_sync` returns 0 unconditionally. A `tracing::warn!` fires if any rows exist with pointer back to this entry (`crates/vault-storage/src/divergence.rs:205-212`). The vault-cli divergence-check subcommand surfaces the (always-zero) count with `(V0.1 stub — see ADR-018 / HANDOFF tech debt)` annotation (`crates/vault-cli/src/main.rs:205`). The stub is acceptable for V0.1 because cap-overflow is unrealistic at V0.1's expected scale (founder dogfood, handfuls of memories).

**Why this MUST land at T0.2.x.** V0.2 cross-device sync (BRD §6.2) materially increases vault size + write churn — 30 beta users × 100s of memories each + cross-device sync events generate enough `pending_sync` accumulation that the V0.1 stub becomes a silent data-recovery gap. **Ship gate: this MUST land before V0.2 sync beta opens.**

**What lands at T0.2.x.**

1. **Schema migration 0003** (`crates/vault-storage/src/migrations/0003_pending_sync_payload.sql` — new file). ALTERs `pending_sync` to add `embedding BLOB NOT NULL DEFAULT X''` (zeroed-default for legacy rows; legacy rows are unreachable in production because V0.1 is local-only and pre-dogfood) + `boundary TEXT NOT NULL DEFAULT ''`.
2. **Orchestrator overflow path writes full payload.** Site: wherever `retry_queue.rs` overflows to `pending_sync` — add embedding + boundary to the insert tuple.
3. **`DivergenceDetector::sweep_pending_sync` real implementation.** Re-enqueues into `retry_queue` while `RetryQueue::len() < MAX_RETRY_QUEUE_DEPTH`. Removes drained rows from `pending_sync`. Returns count drained.
4. **Tests:** migration-applies-to-V0.1-database round-trip, overflow-then-drain integration test, legacy-zero-default-rows skipped-and-warned test.
5. **Update stale code annotations:** remove `(V0.1 stub — see ADR-018 / HANDOFF tech debt)` annotation in `vault-cli/src/main.rs:205`; update `crates/vault-storage/src/divergence.rs:38-48` module-doc to reflect production behaviour.

**Scope estimate:** ~80 LoC + tests. Small.

**Affected files (forward-pointer audit trail).**
- `crates/vault-storage/src/divergence.rs:38-48` + `sweep_pending_sync` — stub site + module-doc references this entry
- `crates/vault-storage/src/divergence.rs:200-214` — runtime WARN log site
- `crates/vault-cli/src/main.rs:205` — V0.1-stub annotation
- `crates/vault-storage/src/migrations/` — new migration 0003 lands here
- `crates/vault-storage/src/retry_queue.rs` — overflow-to-pending_sync path will need to write full payload
- ADR-018 — reference; no amendment needed (already anticipates this work)

---

### V0.2 alpha-distribution — Cosine NaN-vector lance upstream issue (community filing)

**Surfaced:** T0.2.0 Phase 0a-fix (2026-05-07) when the `concurrent_upserts_all_succeed` test failed after the lancedb 0.8 → 0.27.2 upgrade. Three sibling diagnostic tests proved the bug is metric-specific. **Lifted into current HANDOFF tech-debt at T0.2.7 Phase 5 Step 2 audit (2026-05-22)** after originally tracked in archives — the floating code-comment reference in `vault-storage/src/vector_store.rs:1261` lost its HANDOFF.md anchor. Audit lift restores the anchor.

**The finding.** lance 4.0 filters NaN-distance rows from Cosine search where lancedb 0.8 included them. Cosine of `[0,0,0,0]` against any vector is `0 / (0 * ||v||)` = NaN, and lance 4.0's plan filters NaN rows out. **Production unaffected:** BGE-small-en-v1.5 produces L2-normalised vectors with magnitude ≈ 1.0 and never zero — but the lance 4.0 behaviour change is a regression from lancedb 0.8 from the wider community's perspective.

**Why this is tech debt rather than a bug fix on our side.** Our Phase 0a-fix shipped a test-only adjustment. The underlying lance behaviour change still affects any downstream user with zero-magnitude vectors — that's an upstream community contribution opportunity.

**What lands at V0.2 alpha-distribution.**

1. **Build a minimal-repro example** (Python or Rust) demonstrating the lancedb 0.8 → 0.27.2 regression on zero-magnitude vectors with Cosine search. ~50 LoC.
2. **File the issue** against `lance-format/lance` on GitHub. Include: minimal repro, lancedb-0.8-vs-lance-4.0 behaviour diff, link to ADR-038 Layer 4 explaining the discovery context.
3. **Update `crates/vault-storage/src/vector_store.rs:1261` doc-comment** to reference the upstream issue URL once filed.
4. **NO Memory Vault code change required** — production is unaffected and the test-only adjustment already shipped. This entry is closed when the upstream issue is filed.

**Priority:** LOW. Production unaffected; this is community citizenship, not a ship gate.

**Affected files (forward-pointer audit trail).**
- `crates/vault-storage/src/vector_store.rs:1248-1263` — finding documented here + tech-debt pointer
- ADR-038 Layer 4 (`HANDOFF_V0.2_PART1_ARCHIVE.md` line 1820-1828) — full finding narrative
- Future: `https://github.com/lancedb/lance/issues/<TBD>` — upstream issue URL once filed

---

## Live V0.2-era ADRs — cross-link to archive

The following ADRs are LIVE for current V0.2 work. **Full text in `HANDOFF_V0.2_PART1_ARCHIVE.md`.**

- **ADR-044** — `LlmProvider` trait + `Phi4MiniProvider` implementation locks (T0.2.1 Phase 3). Defines the local-LLM contract surface consumed by vault-consolidator Phase 2 + future entity-extraction. §5 single-purpose constraint locks Phi-4 to merge-classifier role only; fixture generation must be hand-curated.
- **ADR-044 Amendment 1** — `CompletionParams::system_prompt: Option<String>` field for non-merge-classifier prompt shapes (T0.2.3 commit 1). Enables future entity-extraction call shape (T0.2.x) without forking the provider.
- **ADR-045** — T0.2.2 Phase 1 Cluster output contract + amendments. N-ary cluster shape, read-cost expectation pin, synthetic acceptance fixture recipe, contract-drift handoff to ADR-044 (resolved at T0.2.3 commit 1), forward-compat notes. §e RESOLVED as of T0.2.3 commit 1.
- **ADR-046** — `mark_superseded` primitive on StorageBackend + new `MemorySuperseded` audit variant (T0.2.3 commit 2). Metadata-only supersession update; preserves BRD §5.6 line 948 provenance fidelity; emits `memory.superseded` audit event distinct from `memory.update`. β-over-α partner-locked decision; rejected `Option<&[f32]>` API extension + rejected `MemoryUpdate`-with-cause-field. Single-supersession assumption documented with V0.3+ forward-revisit.
- **ADR-047** — `summary.rs` file placement + RunState/AMWC field extensions (T0.2.3 commit 3). New `src/summary.rs` file; 3 `pub(crate)` type promotions; `RunState` gains `started_at` + `duration`; `AppliedMergeWithContext` gains `merged_text` + `pre_merge_contents`. Documents BRD §5.6 line 971 vs T0.2.3 iteration 3 divergence as deferred reconciliation. Full ADR text above this section.
- **ADR-048** — Read-time pipeline architecture (T0.2.3 close). Two-stage pipeline (BGE retrieve top-20 → single Qwen-7B synthesis call). **SUPERSEDED-IN-EFFECT by ADR-052 at Commit 6 (2026-05-26)** — read path no longer runs the LLM; kept here as archival reference for the t023-t027b empirical anchors that informed the supersession decision. Full ADR text above this section.
- **ADR-049** — Qwen2.5-7B-Instruct Q4_K_M model lock (T0.2.3 close). Apache 2.0, 128K context, ~4.36 GB. **SUPERSEDED-IN-EFFECT-FOR-READ-PATH by ADR-052 at Commit 6 (2026-05-26)** — `Qwen25_14BProvider` is no longer wired in `Application::new` step 9; the Rust code remains in `vault-llm` until Commit 8 confirms full disuse and removes it. Full ADR text above this section.
- **ADR-051** — Bi-temporal storage semantics + invalidation API contract (T0.2.7 Phase B, 2026-05-24). Locks `valid_until` semantics + retrieval filter + `invalidate()` API surface + orthogonality with `mark_superseded`. No schema migration (fields already exist since T0.1.3). Full ADR text above this section.
- **ADR-052** — Qwen-7B retirement from read path (Commit 6 of locked-next-arc, 2026-05-26). Formally supersedes ADR-048 + ADR-049 in effect: replaces the V0.2-era Qwen-7B single-call synthesis pipeline (mean 86s, p99 119.7s on Vulkan iGPU) with the deterministic `StructuredReadPipeline` (~500ms total). Delivers ~170× local-mode speedup, ~50× BYOK cost cut, ~10× Managed PAYG margin. Phi-4-mini stays at nightly consolidation. ADR-051 + ADR-053 + ADR-044/045/046/047 unchanged. Full ADR text above this section.
- **ADR-053** — Per-boundary REPORT artifact shape + storage + lifecycle (T0.3.x Batch A, 2026-05-26). Locks the structured JSON shape (`schema_version` + `boundary` + `generated_at` + `consolidator_run_id` + `facts_by_topic` keyed by topic label), storage path `<vault_root>/reports/<boundary>.report.json`, atomic `.tmp + fsync + rename` write protocol, and latest-only versioning. Consumed by the Batch B Commit 6 structured-fact read pipeline. **Amendment 1 at Commit 6 (2026-05-26)** adds `topic_names_unavailable: bool` (additive, `#[serde(default)]`) so the read pipeline can surface ADR-054's `TOPIC_NAMES_UNAVAILABLE` warning. Full ADR text + Amendment 1 above this section.
- **ADR-054** — MCP `memory.read` response health-warning contract (Commit 6 of locked-next-arc, 2026-05-26). Locks the structured-fact response shape (`boundary` / `query` / `relevant_facts` / `abstain` / `health`) and the seven warning codes (`REPORT_MISSING` / `REPORT_STALE_INFO` / `REPORT_STALE_WARN` / `REPORT_STALE_CRITICAL` / `DELTA_LOG_UNAVAILABLE` / `TOPIC_NAMES_UNAVAILABLE` / `CLOCK_SKEW_DETECTED`) with their severity assignments + staleness threshold constants + aggregate-status rule. Pinned by 15 unit tests in `crates/vault-retrieval/src/structured_read_pipeline.rs`. Full ADR text above this section.

**V0.1-era ADRs (ADR-001 → ADR-030 + ADR-008 amendments)** — full text in `HANDOFF_V0.1_ARCHIVE.md`.

**Other V0.2-era ADRs in `HANDOFF_V0.2_PART1_ARCHIVE.md`:** ADR-037 (lancedb upgrade), ADR-038 (concurrent-upsert serialisation + LANCE_MEM_POOL_SIZE), ADR-039 amendment (Compact-then-Prune for partial-fragment deletes), ADR-008 amendment (V0.2 at-rest extension lock-in) + ADR-008 amendment v2 (AAD path semantics), ADR-040 + ADR-040 amendment (Keychain crate + master_key derivation) + ADR-040 amendment v2 (Signature fix), ADR-041 + ADR-041 plan iteration 2 (V0.1 VAULT_KEY → V0.2 keychain SQLCipher passphrase bridge), ADR-042 (Phi-4-mini-instruct selection), ADR-043 (model download + integrity verification), ADR-010 hard-gate-cleared note (T0.2.0 Phase 5 close).

---

## Standing rules (CLAUDE.md-promoted defaults)

Per CLAUDE.md project instructions + recurring partner discipline. Memory-stored full rules in `~/.claude/projects/C--Projects-GitHub-Memory-Vault/memory/`.

- **CI verification per-commit.** Every code commit must show CI green matrix-wide before staging the next. `gh run list --workflow=ci.yml -L 1`. Local DoD ≠ CI green (Windows + Ubuntu + macOS clean-room matrix is the canonical surface). Promoted from candidate to default at T0.1.10-close (2026-05-04); 6 vault-code data points then; reinforced through T0.2.0 → T0.2.7.
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
