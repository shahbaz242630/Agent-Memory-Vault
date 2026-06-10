# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)

**Last updated:** 2026-06-10 — **Gap 2 FIX IMPLEMENTED (ADR-074), all 5 DoD gates green, NOT yet committed.** Document-side alias enrichment built: new `vault-consolidator/src/phases/enrich.rs` (Phi-4 generates a per-fact alias/topic line at consolidation; stored in `metadata.enrichment`; embedded as `"<content> Topics: <aliases>"`, display `content` untouched; idempotent via FNV-1a content fingerprint), `Consolidator::enrich_facts` orchestrator + `EnrichmentReport`, wired into `run_consolidation_with_safety` between consolidation and report-gen. Gates: fmt ✅ / build 0-warn ✅ / clippy 0-lint ✅ / `vault-consolidator` 113 tests (incl. 13 new `enrich`) ✅ / `vault-app` 58 tests ✅. Full ADR in §8.6. **NEXT: live rank-lift validation with the real Phi-4 model (§8.6 "Live validation"), then commit (admin rides with code).** Prior 2026-06-09 note: **Gap 2 RE-DIAGNOSED + fix PROVEN.** CI on `a3e426b` (ADR-073 Gap-1) confirmed **green** (push run 27150216167). A full ground-truth investigation (built a paraphrase-variant ruler, FIXED a greenwash bug in `scale_eval`, then probed the REAL `seeded-vault-1k` across 3 domains) **falsified the 2026-06-08 Gap-2 diagnosis**: it is NOT a BGE paraphrase limit and the fix is NOT query expansion. It's a **vocabulary gap**, and **document-side alias enrichment is the proven fix** (bare Porto ABSENT → enriched rank 1 @ 0.9965 on the killer query; twins rank 5 → 1; no regression). See §4.2. **NEXT SESSION: design + implement document-side alias enrichment in the consolidator (Phi-4 generates the alias/topic line) — open design Qs in §4.2.** Also surfaced: **tech-debt #6** (weekly real-model smoke chronically red since 2026-05-18 — concurrent-download race, CI-infra not a regression; §8). Engine is solid; Gap 2 now has a clear, measured path to closed.

> **How to read this file:** §1 is the only thing you must act on. §2–§5 are current ground truth (incl. the post-scale roadmap in §5). §6 onward is reference you pull from when planning. Deep detail (full ADR text, session-by-session history, tuning evidence) lives in the three archives — cross-linked by ADR number. **Do not paraphrase archived ADRs — quote them.**

---

## 1 · 🟢 ACTIVE TASK — scale-validate the retrieval core LIVE (1k → 10k), then lock it

**Goal:** prove correctness is scale-invariant on a REAL vault that Antigravity opens, at 1k then 10k facts. We already proved it on the internal `scale_eval` harness (100/1k/10k identical scorecard) and live in Antigravity at 100 facts. This is the live confirmation at scale + the close of the arc.

### 🔚 NEXT SESSION OPENER (2026-06-10) — **GAP-2 FIX IMPLEMENTED + GATES GREEN; LIVE-VALIDATE WITH REAL PHI-4, THEN COMMIT** — READ THIS FIRST

**What's done (2026-06-10).** Built the Gap-2 fix — **document-side alias enrichment via Phi-4 at consolidation (ADR-074, §8.6).** New `phases/enrich.rs` (`generate_aliases` / `compose_embed_text` "`<content> Topics: <aliases>`" / `content_fingerprint` FNV-1a / `enrich_one`), `Consolidator::enrich_facts` + `EnrichmentReport`, wired into `run_consolidation_with_safety` (after consolidation, before report-gen). **All 5 DoD gates green** (fmt / build-0-warn / clippy-0-lint / `vault-consolidator` 113 incl. 13 new `enrich` / `vault-app` 58). **NOT yet committed.** Aliases live in `metadata.enrichment`; display `content` untouched (no read-side leak); idempotent (FNV-1a fingerprint skip → first run backfills, steady-state only touches new/changed facts). Confirmed enrichment is already in the real CLI path (`vault-cli consolidate run`).

**The finding it fixes (Gap 2 — re-diagnosed 2026-06-09; see §4.2 for full evidence).** NOT a BGE paraphrase limit (natural idioms find the fact: "call home" → Porto rank 1). It's a **vocabulary gap** — facts phrased without the obvious keyword get outranked/buried; the agent's keyword-soup expansion is the TRIGGER, not the cure (query-side expansion SHELVED). Document-side enrichment PROVEN by `probe_enrichment`: bare Porto ABSENT → enriched rank 1 @ 0.9965; twins rank 5 → 1; no regression.

**▶️ DO FIRST next session (in order):**

1. ✅ **DONE 2026-06-10 — live rank-lift validation with the REAL Phi-4 on the real 1k vault.** All three Gap-2 killers now rank #1 end-to-end (Porto ABSENT→1, hives 4→1, twins 1→1). Required a prompt tweak (single-word generic keywords) caught precisely because we validated before committing. Full table + method in §8.6 "Live validation". The two validation probes (`real_phi4_alias_quality`, `probe_real_enrichment_1k`) ride with the commit.
2. **Commit (admin rides with code).** One commit: `enrich.rs` (incl. tuned prompt + `pub enrich_one`) + consolidator/app wiring + the two real-Phi-4 `#[ignore]` probes + `scale_eval.rs` clippy fixes + the 2026-06-09 ruler + this HANDOFF admin. **Re-run `cargo fmt --all --check` LAST + `git status --short` before `git add`** (drift check). **Confirm with Shahbaz before commit + push.** Then verify CI green before any next commit.
3. **Tech-debt #6** (cheap, optional bundle): `--test-threads=1` on `ci.yml:702` to re-light the weekly smoke.
4. **After Gap 2 commits + CI-green:** re-seed 10k for the scale data point → declare the retrieval core "battle-tested."

> **🟢 The engine is solid.** Gap 1 shipped; Gap 2 fix is built + gate-green. The premium experience (Opus: rich answers, graceful abstains, no hallucinated traps) was excellent throughout. Closing Gap 2 is the last known quality gap before "battle-tested."

### 📌 Reference — seed / verify / repoint commands (1k is DONE + live; reuse these for the 10k re-seed in step 4 above)
1. **Seed** (swap `SEED_N`/`SEED_VAULT_DIR` for 10k): `$env:SEED_N='1000'; $env:SEED_VAULT_DIR='C:\Projects\seeded-vault-1k'; cargo test -p vault-app --test scale_eval seed_live_vault -- --ignored --nocapture` — 1k ≈ 17 min; 10k = multi-hour/overnight (drain rate degrades). Waits for full VECTOR-count drain, then prints the test script.
2. **Verify the seed:** `$env:LANCE_MEM_POOL_SIZE='268435456'; & "C:\Projects\GitHub\Memory Vault\target\debug\vault-cli.exe" --vault-db C:\Projects\seeded-vault-1k\vault.db --vector-dir C:\Projects\seeded-vault-1k\lance --graph-db C:\Projects\seeded-vault-1k\graph.duckdb divergence-check` — expect `sqlite == vector`, **no findings**.
3. **Repoint Antigravity:** edit the REAL config `C:\Users\shahb\.gemini\config\mcp_config.json` (the `~/.gemini/antigravity/mcp_config.json` is a SYMLINK to it — edit the real target). Change the 3 vault paths (`--vault-db`/`--vector-dir`/`--graph-db`) to `seeded-vault-1k`. **Restart Antigravity.** Confirm: `Get-CimInstance Win32_Process -Filter "Name='vault-cli.exe'" | Select CommandLine`.
4. **Run the 15 questions** (`crates/vault-app/tests/fixtures/scale_eval.json`; seeder prints them with expected answers). Watch: #6 cello (subject-less fact), #12 salary + #14 cat-breed (the Thread-2 precision traps), the 5 abstains.
5. **Seed + test 10k** the same way (`SEED_N='10000'`, `C:\Projects\seeded-vault-10k`). **10k seed is MULTI-HOUR** (~1s/vector × 10k, degrading) — plan an overnight run. Verify + repoint + test as above.
6. **If 1k AND 10k both pass live →** (a) commit the seeder (`crates/vault-app/tests/scale_eval.rs` `seed_live_vault` + the vector-count-drain probe) with full DoD gates → CI green; (b) declare the retrieval core "battle-tested at scale," close this arc. Thread 2 (read precision) becomes the next arc.

### ⚙️ Working-tree state
- **Last SHIPPED: `a3e426b`** (Gap-1 / ADR-073), CI green (push run 27150216167).
- **Gap-2 fix IMPLEMENTED, all 5 DoD gates green, NOT yet committed** (one commit, admin rides with code). Files:
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
2. **Entity-extraction-at-consolidation + `GraphStore::rewrite_relationships_for_memory(old, new)`.** BRD §5.6 line 950 ("update graph on merge") presupposes entity extraction from `Memory.content` + a relationship-rewrite primitive — neither exists. `apply_merge` skips graph-update with a `tracing::warn!` no-op (honest: graph is empty in V0.2). Files: `cascading.rs:37-50`, `phases/merge.rs::apply_merge`. Do NOT amend the BRD until closed.
3. **`VaultError::Storage(String)` grab-bag → structured variants.** `retry_queue.rs::is_permanent` substring-matches lance error wording (fragile; lance 4.0 wording is inconsistent). Add `SchemaMismatch`/`IoFailure`/etc., re-categorise ~30 call sites, rewrite `is_permanent` as exhaustive `match` + tripwire test. Files: `retry_queue.rs:240-275`, `vault-core/src/error.rs:139`, the ~30 `Storage(format!(...))` sites.
4. **`pending_sync` sweep + migration 0003 cascade payload.** `DivergenceDetector::sweep_pending_sync` is a V0.1 stub (returns 0). Migration 0002 schema lacks the `embedding`+`boundary` payload to reconstruct a `NewRetry`. **SHIP GATE: must land before V0.2 sync beta opens** (cross-device churn makes the stub a silent data-recovery gap). ~80 LoC. Files: `divergence.rs:38-48,200-214`, `vault-cli/src/main.rs:205`, new `migrations/0003_*.sql`, `retry_queue.rs` overflow path.
5. **Cosine NaN-vector lance upstream issue (LOW — community citizenship).** lance 4.0 filters NaN-distance rows from Cosine search (zero-magnitude vectors). Production unaffected (BGE vectors are L2-normalised, never zero). File a minimal-repro issue against `lancedb/lance`. File: `vector_store.rs:1248-1263`.
6. **Weekly real-model smoke red since 2026-05-18 — concurrent-download race (CI-infra, NOT a code regression).** The `real-model-smoke` weekly cron job (`ci.yml:702`, `cargo test -p vault-llm -- --ignored`) has failed every Monday across 4 unrelated commits (`4ae8dbd`/`93d1410`/`2302842`/`a3e426b`). Root cause (source-confirmed): all 3 smoke tests run concurrently (no `--test-threads=1`) and `model_loader.rs::download_with_verify` writes to a **single shared** `.partial` path (`model_loader.rs:131`) then renames to final (`:156`); the winner's rename leaves the losers' rename hitting a vanished `.partial` → `Io NotFound code 2`. The test's own doc (`phi4_mini_smoke.rs:47-48`) assumes serial execution. **Min fix (CI-only):** add `--test-threads=1` to `ci.yml:702` — verifiable only via next Monday cron or a `run-llm-smoke`-labelled PR. **Deeper latent bug (LOW, prod single-writer + pre-download mitigates):** the shared `.partial` path means two cold-starting agent processes could corrupt each other's download — make `.partial` unique per download + treat "final already present after our stream" as success. Matters because this job is the ONLY CI coverage of the real Phi-4 consolidator path (dark for a month); re-light before leaning on the consolidator (roadmap §5 item 2). Files: `ci.yml:702`, `model_loader.rs:95-160`.

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

## 9 · 📇 ADR index

Full text of every ADR lives in an archive — cross-link by number, **quote don't paraphrase** ([[feedback_quote_locked_artefacts_dont_paraphrase]]).

**In-flight (full text in HANDOFF, not yet archived):** **ADR-074** (document-side alias enrichment at consolidation, §8.6 — IMPLEMENTED, gates green, pending commit) · **ADR-073** (recall-safe `memory_read`, §8.5 — SHIPPED `a3e426b`).

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
