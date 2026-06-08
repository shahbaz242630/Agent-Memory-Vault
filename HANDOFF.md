# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)

**Last updated:** 2026-06-08 (session close) — **HANDOFF archived + slimmed** (history T0.2.3c3 → T0.3.x → `HANDOFF_V0.2_PART2_ARCHIVE.md`). 1k live dogfood (Gemini + Opus, 17 queries) found two read-quality gaps; **ADR-073 Gap-1 fix (`memory_read` false-abstain) is implemented + STAGED but UNVERIFIED** — DoD gates were blocked by the running 10k seed (no parallel cargo). **NEXT SESSION: gate the Gap-1 fix first** (cargo is free now — the seed stopped at terminal close), then re-test 1k, then the harness upgrade + Gap 2, then re-seed 10k, then commit. Full instructions in the 🔚 NEXT SESSION OPENER block in §1. Engine + premium experience are solid; these two gaps are all that's between us and "battle-tested."

> **How to read this file:** §1 is the only thing you must act on. §2–§5 are current ground truth (incl. the post-scale roadmap in §5). §6 onward is reference you pull from when planning. Deep detail (full ADR text, session-by-session history, tuning evidence) lives in the three archives — cross-linked by ADR number. **Do not paraphrase archived ADRs — quote them.**

---

## 1 · 🟢 ACTIVE TASK — scale-validate the retrieval core LIVE (1k → 10k), then lock it

**Goal:** prove correctness is scale-invariant on a REAL vault that Antigravity opens, at 1k then 10k facts. We already proved it on the internal `scale_eval` harness (100/1k/10k identical scorecard) and live in Antigravity at 100 facts. This is the live confirmation at scale + the close of the arc.

### 🔚 NEXT SESSION OPENER (2026-06-08 session close) — **GATE THE STAGED ADR-073 Gap-1 FIX FIRST** — READ THIS FIRST

**What's done this session.** (1) HANDOFF archived → `HANDOFF_V0.2_PART2_ARCHIVE.md`; this slim file opened. (2) Seeded a 1k live vault + verified clean (`divergence-check`: `sqlite==vector`, no findings). (3) Ran a 17-query live dogfood in Antigravity across small Gemini + Opus. (4) **Found two correctness gaps, implemented the fix for Gap 1** (staged, unverified).

**The problem (Gap 1 — the one we fixed).** `memory_read` **false-abstained on stored facts** — the cardinal sin. Root cause confirmed by reading the code: `apply_reranker` (`structured_read_pipeline.rs`) hard-DROPPED every candidate below `reranker.relevance_floor()` (≈logit −2.5), then `abstain = candidates.is_empty()`. Real answers score below that ("runs 10km" for "how do I stay fit" = logit −3.21) → dropped → false-abstain. `memory_search` got the same queries right because it's reorder-only + never empties. (Full evidence: [[project_1k_live_read_false_abstain]].)

**How it's fixed (ADR-073, §8.5).** Made `memory_read` behave like `memory_search`: `apply_reranker` is now **reorder-only** (sigmoid-map, keep ALL, sort DESC — no drop); `read()` computes a recall-safe **abstain HINT** = `search_hint` separation (catches flat clusters like the salary trap) ∪ `READ_NO_SIGNAL_FLOOR` 0.01 (catches lone no-signal facts like cat→dog); **facts are ALWAYS returned even when `abstain=true`** (the floor governs only the hint, never drops a fact → recall safe by construction); response gains `top_relevance`. MCP tool description updated. 3 tests rewritten + 3 new regression tests. **All staged + reviewed-by-eye but NOT machine-verified** — gates were blocked because the 10k seed was a running `cargo test` (no parallel cargo). Exact files in the "Uncommitted working tree" block below.

**▶️ DO FIRST next session (in order):**
1. **Gate the Gap-1 fix** — cargo is now free (10k seed was stopped on terminal close). Check disk + confirm with Shahbaz, then run serial: `cargo fmt --all --check` → `cargo clippy --workspace -- -D warnings` → `cargo build --workspace` → `cargo test -p vault-retrieval` → `cargo test -p vault-mcp`. Fix any compile/test surprises (the code is hand-written against a multi-page read; expect possible small fixups).
2. **Re-run the 1k live test** in Antigravity (config still points at `C:\Projects\seeded-vault-1k`) — confirm "how do i stay fit" now ANSWERS (not abstain) and the genuine abstains (blood type / OS) still hold + the salary/cat traps still abstain-with-facts.
3. **Then layer the rest:** the `scale_eval` harness upgrade (call the real `memory_read`/`memory_search` tools + assert `abstain`/`top_relevance` + paraphrase query variants — it greenwashed both gaps) + **Gap 2** (paraphrase recall miss; start with query expansion — the proven mitigation, see [[project_1k_live_paraphrase_recall_miss]]).
4. **Re-seed 10k fresh** (`SEED_N='10000'`, `C:\Projects\seeded-vault-10k`, overnight) for the scale data point + final live re-test.
5. **If 1k + 10k both clean post-fix → commit** Gap-1 + seeder + HANDOFF together (one commit, Shahbaz's go-ahead) → CI green → declare the retrieval core battle-tested.

> **Note on Gap 2 (NOT yet fixed):** "where does the user call home" missed "settled in Porto" entirely ("live" found it at rank #1) — a phrasing-sensitive recall miss at the embedder layer, upstream of the read gate, so the Gap-1 fix does NOT cover it. Deferred to step 3 by design (one change at a time).

> **🟢 Premium experience was EXCELLENT** even with the bug: Opus gave rich answers, graceful abstains on blood-type/salary/OS, never hallucinated the salary-$ or cat→dog traps, and offered to save missing facts. The engine (storage/scale/integrity) is solid. The two gaps are the only things between us and "battle-tested."

### 📌 Reference — seed / verify / repoint commands (1k is DONE + live; reuse these for the 10k re-seed in step 4 above)
1. **Seed** (swap `SEED_N`/`SEED_VAULT_DIR` for 10k): `$env:SEED_N='1000'; $env:SEED_VAULT_DIR='C:\Projects\seeded-vault-1k'; cargo test -p vault-app --test scale_eval seed_live_vault -- --ignored --nocapture` — 1k ≈ 17 min; 10k = multi-hour/overnight (drain rate degrades). Waits for full VECTOR-count drain, then prints the test script.
2. **Verify the seed:** `$env:LANCE_MEM_POOL_SIZE='268435456'; & "C:\Projects\GitHub\Memory Vault\target\debug\vault-cli.exe" --vault-db C:\Projects\seeded-vault-1k\vault.db --vector-dir C:\Projects\seeded-vault-1k\lance --graph-db C:\Projects\seeded-vault-1k\graph.duckdb divergence-check` — expect `sqlite == vector`, **no findings**.
3. **Repoint Antigravity:** edit the REAL config `C:\Users\shahb\.gemini\config\mcp_config.json` (the `~/.gemini/antigravity/mcp_config.json` is a SYMLINK to it — edit the real target). Change the 3 vault paths (`--vault-db`/`--vector-dir`/`--graph-db`) to `seeded-vault-1k`. **Restart Antigravity.** Confirm: `Get-CimInstance Win32_Process -Filter "Name='vault-cli.exe'" | Select CommandLine`.
4. **Run the 15 questions** (`crates/vault-app/tests/fixtures/scale_eval.json`; seeder prints them with expected answers). Watch: #6 cello (subject-less fact), #12 salary + #14 cat-breed (the Thread-2 precision traps), the 5 abstains.
5. **Seed + test 10k** the same way (`SEED_N='10000'`, `C:\Projects\seeded-vault-10k`). **10k seed is MULTI-HOUR** (~1s/vector × 10k, degrading) — plan an overnight run. Verify + repoint + test as above.
6. **If 1k AND 10k both pass live →** (a) commit the seeder (`crates/vault-app/tests/scale_eval.rs` `seed_live_vault` + the vector-count-drain probe) with full DoD gates → CI green; (b) declare the retrieval core "battle-tested at scale," close this arc. Thread 2 (read precision) becomes the next arc.

### ⚙️ Uncommitted working tree (commit at step 6a, after gates + re-test)
- `crates/vault-app/tests/scale_eval.rs` — the `seed_live_vault` `#[ignore]` seeder (production-keychain key, bge-small fixture vectors, env `SEED_N`/`SEED_VAULT_DIR`, drains by VECTOR COUNT) + the `SCALE_EVAL_N` env override + scale-aware readiness poll. Used live. NOT yet committed.
- **ADR-073 Gap-1 fix (STAGED 2026-06-08, UNVERIFIED — gates blocked by the running 10k seed; no parallel cargo).** `crates/vault-retrieval/src/structured_read_pipeline.rs` — `apply_reranker` is now reorder-only (sigmoid-map via shared `relevance_score`, keep all, no floor-drop); `read()` computes a recall-safe abstain HINT (`search_hint` separation ∪ `READ_NO_SIGNAL_FLOOR` 0.01) on the reranker path only, always packs facts, sets new `top_relevance` field; `StructuredReadResponse` gains `top_relevance: f32`; 3 existing reranker tests rewritten for reorder-only + 3 new Group C.7 regression tests (stay-fit / salary-cluster / strong-match). `crates/vault-retrieval/src/reranked_retriever.rs` — `relevance_score` made `pub(crate)`. `crates/vault-mcp/src/server.rs` — `memory_read` tool description updated (6 fields, refined `abstain`, cat→dog guidance). `crates/vault-mcp/tests/common/{mod.rs,mock_adapter.rs}` — `top_relevance: 0.0` added to mock constructions. ADR-073 full text at §8.5.
- **NEXT (post-10k-seed):** run DoD gates (fmt→clippy→build→test, serial) → fix any compile/test surprises → re-run the 1k live test → then the `scale_eval` harness update (call real `memory_read`/`memory_search` + paraphrase variants) + Gap 2 (query expansion).
- `HANDOFF.md` — this slim rewrite + ADR-073 (rides with the eventual commit per admin-rides-with-code).

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

### Thread 2 — retrieval stack is fragile to query paraphrase (TWO gaps, one root) — own arc, NEXT
**Status upgraded 2026-06-08:** the 1k live Antigravity dogfood (small Gemini + Opus, raw JSON captured across 17 queries) found **two distinct correctness gaps that share one root: BGE-small + the reranker don't robustly map paraphrase/idiom queries to the right facts.** This is the gating work before the core can be called battle-tested. Mitigation lever proven all session: **query expansion** — agents that keyword-pad their query ("live home city location", "profession job work") consistently rescue recall; terse natural phrasings ("call home") hit the gaps.

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

#### Gap 2 — paraphrase RECALL miss (recall layer; fact never enters the candidate pool)
**The bug (confirmed live, 1k vault, Opus).** For some natural phrasings the right fact is **not retrieved at all** — neither read nor search can surface it:
- *"where does the user **call home**"* → 11 results, **all location-flavored noise** (Salt Lake City logistics, travel arrangements); *"settled in Porto"* **absent from the pool**; top relevance 0.002; Opus correctly abstained on what it got.
- *"where does the user **live**"* (same vault, same fact) → *"settled in Porto"* at **rank #1, relevance 0.2482**, clear winner.

So it's **phrasing-sensitive recall**, not a total hole: the idiom "call home" steers BGE toward location-shaped noise and the real fact never enters the reranked set; "live" maps cleanly. This is upstream of the reranker (candidate generation), so the Gap-1 fix does NOT cover it — `memory_search` itself missed here. Root is BGE-small's paraphrase limits ([[project_bge_small_cannot_separate_relevant]]) at the recall layer.

**Fix directions for Gap 2 (not yet locked).** **CORRECTION 2026-06-08 (live, both models):** the earlier "(a) query expansion — agent keyword-padding rescued recall every time" claim is **FALSIFIED** — strong agents DO keyword-ize the query themselves (Opus sent `"home location city country lives residence"`, `"cat breed pet animal"`), but it's UNRELIABLE: Flash's `"live home city location"` found Porto (0.2482) while Opus's expansion MISSED it. So the fix must be **vault-side robust**, NOT dependent on agent phrasing: (a) **vault-side query expansion / multi-query / HyDE** (inside the retriever, deterministic); (b) larger candidate pool + rerank-the-full-pool; (c) stronger embedder (last resort — cost/size). Extra risk: a recall miss can become a confident-WRONG answer — Opus guessed the home region from the system timezone (UTC+4) rather than just saying "not found." See [[project_1k_live_paraphrase_recall_miss]] (keywordization finding).

#### Harness gap (fix alongside both)
`scale_eval` scored **"false-abstain: 0"** at 100/1k/10k while read was false-abstaining AND search was recall-missing live — because the harness measures recall in the candidate *pool* with favorable phrasings, not what the live tools return through their gates on natural/idiomatic queries. The harness must (1) call the real `memory_read`/`memory_search` tools and assert on `abstain`/`relevant_facts`, and (2) include paraphrase/idiom query variants per fact, or it keeps scoring these green. No cheap unit test proves the fixes — validate on `scale_eval.rs` with real BGE + reranker.

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
2. **Entity-extraction-at-consolidation + `GraphStore::rewrite_relationships_for_memory(old, new)`.** BRD §5.6 line 950 ("update graph on merge") presupposes entity extraction from `Memory.content` + a relationship-rewrite primitive — neither exists. `apply_merge` skips graph-update with a `tracing::warn!` no-op (honest: graph is empty in V0.2). Files: `cascading.rs:37-50`, `phases/merge.rs::apply_merge`. Do NOT amend the BRD until closed.
3. **`VaultError::Storage(String)` grab-bag → structured variants.** `retry_queue.rs::is_permanent` substring-matches lance error wording (fragile; lance 4.0 wording is inconsistent). Add `SchemaMismatch`/`IoFailure`/etc., re-categorise ~30 call sites, rewrite `is_permanent` as exhaustive `match` + tripwire test. Files: `retry_queue.rs:240-275`, `vault-core/src/error.rs:139`, the ~30 `Storage(format!(...))` sites.
4. **`pending_sync` sweep + migration 0003 cascade payload.** `DivergenceDetector::sweep_pending_sync` is a V0.1 stub (returns 0). Migration 0002 schema lacks the `embedding`+`boundary` payload to reconstruct a `NewRetry`. **SHIP GATE: must land before V0.2 sync beta opens** (cross-device churn makes the stub a silent data-recovery gap). ~80 LoC. Files: `divergence.rs:38-48,200-214`, `vault-cli/src/main.rs:205`, new `migrations/0003_*.sql`, `retry_queue.rs` overflow path.
5. **Cosine NaN-vector lance upstream issue (LOW — community citizenship).** lance 4.0 filters NaN-distance rows from Cosine search (zero-magnitude vectors). Production unaffected (BGE vectors are L2-normalised, never zero). File a minimal-repro issue against `lancedb/lance`. File: `vector_store.rs:1248-1263`.

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

## 9 · 📇 ADR index

Full text of every ADR lives in an archive — cross-link by number, **quote don't paraphrase** ([[feedback_quote_locked_artefacts_dont_paraphrase]]).

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
