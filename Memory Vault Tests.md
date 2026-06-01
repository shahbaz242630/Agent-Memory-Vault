# Memory Vault — Correctness Test Spec (V0.2 dogfood)

**Thesis:** in a memory product the output has to be correct every time. These tests measure correctness on the *agent-read* workload — the differentiator — not just tool plumbing. Structure borrows LongMemEval's five abilities (information extraction, multi-session reasoning, temporal reasoning, knowledge update, abstention), extends them with the cross-agent shape no competitor benchmarks, and adds longitudinal stress + integrity.

Each test is tagged with an execution status:

- **[LIVE]** — I (the connected advisory Claude) can run this right now over the MCP tools, no setup.
- **[CONSOLIDATE]** — needs `vault-cli consolidate run` to have executed on the test boundary first (topics, REPORT, contradiction detection, merge all happen at consolidation).
- **[FIXTURE]** — needs a purpose-built eval fixture + a scoring harness Claude Code writes (can't be judged by eyeballing a handful of facts).
- **[STORAGE]** — must be verified at the storage layer by Claude Code; not observable over MCP.

---

## 0. Preconditions / setup (Claude Code)

1. **Dedicated eval boundary.** Authorize a throwaway boundary (e.g. `testeval`) in the MCP session config so eval writes don't pollute `personal` / `project.memory-vault`. All seed data below assumes `testeval`. **NB: boundary names must be dot-free** — `test.eval` (with a dot) is rejected by input validation (`-32602`) before the auth gate, so use `testeval`. Tear down after each run (or use a fresh vault dir per run).
2. **Consolidator runnable.** `vault-cli consolidate run --boundary testeval ...` wired and green (Commit 8 dogfood prereq). Needed for all [CONSOLIDATE] tests. (Boundary is dot-free `testeval`, never `test.eval`.)
3. **Models on disk.** `scripts/setup-dev-env.ps1` has pulled BGE + ort + tokenizer; Phi-4 GGUF present (needed for consolidation labels + merge classifier).
4. **Two eval fixtures:**
   - `merge_acceptance_100.json` — **already exists** (T0.2.3 realism rewrite: 100 entries, 17 clusters, 50/50 boundary split, 42-merge/54-keep/4-contradiction, within-cluster length variance, BGE-truncation entries). Used by the consolidation-correctness tests.
   - `read_quality_eval.json` — **new, Claude Code builds.** LongMemEval-shaped. Each case = `{ seed_memories: [{content, boundary, source_agent, as_of, confidence}], query, expect: {must_surface: [ids], must_rank_top_k: {id, k}, must_exclude: [ids], abstain: bool} }`. ~80–120 seed memories with deliberate distractors (lexically similar but semantically irrelevant), multi-session facts, temporal variants, and contradiction pairs. Schema below in §6.

---

## 1. Tool-contract tests

Single-tool behavioral correctness. Cheap, deterministic, run every CI cycle.

| ID | Test | Method | Pass criterion | Status |
|---|---|---|---|---|
| C1 | Read-after-write, no restart | write a unique fact → immediately `memory_read` for it, same session | fact appears in `relevant_facts`, `abstain:false`, returned `memory_id` == write id | **[LIVE]** ✅ *passed in dogfood* |
| C2 | Long content accepted + stored intact | write content `LONGOK` + ~2400 char filler + `LONGEND` → read back | write returns id; read-back contains BOTH bracket tokens (no mid/tail truncation) | **[LIVE]** ✅ *passed in dogfood* |
| C3 | Idempotent delete (no-op) | `memory_delete` on a well-formed nonexistent UUID v7 | clean success, no error | **[LIVE]** ✅ *passed in dogfood* |
| C4 | **Content ceiling probe** | write content at 5K / 10K / 50K chars, each bracketed | each either stores intact OR fails with a clear, documented error — never silently truncates while returning success. Record the real ceiling. | **[LIVE]** |
| C5 | **Malformed-id delete** | `memory_delete` with a non-UUID string (`"not-a-uuid"`) | clean rejection (invalid-params) OR documented no-op — not a 500/panic | **[LIVE]** |
| C6 | **Update round-trip** | write fact → `memory_update` same id with new content → read | read returns NEW content; OLD content not surfaced; id stable; provenance/audit preserved | **[LIVE]** |
| C7 | **Unicode round-trip** | write emoji + CJK + Greek + accents + ligature → read | byte-identical content returned (modulo documented canonical normalization, e.g. trailing period) | **[LIVE]** |
| C8 | **Boundary-auth rejection** | `memory_write` to a boundary NOT in the authorized set | `AccessDenied` → JSON-RPC `-32001` ("access denied"), generic message; nothing written. (Access-denial uses `-32001`; malformed *input* uses `-32602` — see C5.) | **[LIVE]** ✅ *passed in dogfood 2026-05-29 (server returned `-32001`, ~30ms; the original `-32602` expectation was the sheet's, not the server's)* |
| C9 | **Normalization is deterministic** | write the same raw content twice; compare stored form | identical canonical output both times; documented transforms only | **[LIVE]** |

**Known open follow-ups from C1–C3 dogfood:**
- Tool description advertises a 2000-char hard limit but server accepted ~2430 (C2). Update the published `content` limit to match real behavior; C4 establishes the true ceiling.
- Delete no-op returns `{"deleted": id}` with no signal that nothing existed (C3). Decide whether to add `existed: bool` so a narrating agent doesn't claim a phantom deletion.
- Fresh-vault reads return `health.status: degraded` / `REPORT_MISSING` on every read until first consolidation. Decide whether never-consolidated should be `ok` + info-note instead of `degraded`.
- **A4 temporal ranking (dogfood 2026-05-29):** a "where does X work *now*" query surfaced the older employer fact alongside/above the newer one. Expected — the read pipeline ranks by semantic relevance, not recency. Not a read-time bug to patch; the fix is the consolidator retiring the superseded fact (this is the A5 mechanism). Verify after first live consolidation.

---

## 2. The five correctness abilities (agent-read core)

This is the tier that actually measures the differentiator. **Cannot be judged on a 4–5 item vault** — needs `read_quality_eval.json` with distractors. Current dogfood baseline: reads return the *entire* small boundary regardless of relevance, so precision is unmeasured. T0.2.3 spike baseline on realistic content was ~24% recall / 0% contradiction detection — these tests exist to move those numbers and gate on them.

| ID | Ability | Test | Pass criterion | Status |
|---|---|---|---|---|
| A1 | Information extraction | bury a target fact among N distractors (incl. lexically-similar-but-irrelevant); query it | target in `relevant_facts` AND ranked in top-3; precision of returned set ≥ bar (set bar after first run; goal = no obvious off-topic facts returned) | **[FIXTURE]** |
| A2 | Lost-in-the-middle | place target in the *middle* of a large seed set, not head/tail | target still surfaces (no positional dropout) | **[FIXTURE]** |
| A3 | Multi-session reasoning | facts needed to answer split across ≥3 writes from different `source_agent`s | read returns ALL pieces required to compose the answer | **[FIXTURE]** |
| A4 | Temporal reasoning | same subject, multiple facts at different `as_of`; query "latest X" | newest `as_of` surfaces and ranks above older; older not presented as current | **[LIVE-smoke]** / **[FIXTURE]** for scoring |
| A5 | Knowledge update / contradiction **(centerpiece)** | write fact A, later write contradicting A′ on same subject+boundary; consolidate; read | consolidator detects contradiction; stale fact `invalidate()`d; read returns CURRENT truth only — not both, not the stale one. `health` flags nothing spurious. | **[CONSOLIDATE]** |
| A6 | Abstention (true negative) | query a subject with zero vault signal | `abstain:true`, empty `relevant_facts`, no fabrication | **[LIVE]** |
| A7 | Abstention (no over-abstain) | query a subject that IS present, phrased loosely | `abstain:false`, target returned — does not abstain on real content | **[LIVE-smoke]** / **[FIXTURE]** |

**A5 is the priority.** It is simultaneously (a) your weakest measured axis, (b) the open problem the whole field admits — staleness in high-relevance memories going "confidently wrong" — and (c) the cleanest edge story. Treat A5's pass bar as a V0.2 ship gate, not a nice-to-have.

---

## 3. Cross-agent shape (the moat — nobody else benchmarks this)

Tests only Memory Vault is positioned to pass. All require consolidation (merge happens there).

| ID | Test | Method | Pass criterion | Status |
|---|---|---|---|---|
| X1 | Same fact, different lengths, different agents | write the same fact 3× — short / paragraph / long-form — each with a different `source_agent`; consolidate | consolidated to a coherent single fact (or correctly clustered); no duplicate noise at read; embedder+classifier agreed across length variance | **[CONSOLIDATE]** |
| X2 | Long-form + terse merge | agent 1 writes a long session summary on topic T; agent 2 writes a one-line fact on T; consolidate; read T | read returns a coherent, non-redundant view combining both | **[CONSOLIDATE]** |
| X3 | Cross-agent attribution survives | seed facts with distinct `source_agent`s; read | `source_agent` preserved per fact through consolidation; attribution not lost or smeared | **[CONSOLIDATE]** |
| X4 | BGE-truncation entries | facts >2000 chars (beyond embedding window); consolidate + read | full text preserved at storage; embedding truncation does not corrupt retrieval/merge | **[CONSOLIDATE]** (storage assert via **[STORAGE]**) |

---

## 4. Consolidation correctness

Run against `merge_acceptance_100.json` (exists). Several already have property tests in `vault-consolidator/tests/` — wire them into the eval report.

| ID | Test | Pass criterion | Status |
|---|---|---|---|
| K1 | Topics populate | after consolidate, reads return non-null `topic` on consolidated facts; REPORT exists per boundary | **[CONSOLIDATE]** |
| K2 | Health → ok path | after consolidate, `health.status: ok` (no `REPORT_MISSING`) on a fresh read | **[CONSOLIDATE]** |
| K3 | Idempotent consolidation | run twice; 2nd run merges 0 / resolves 0 (no further state change) | **[CONSOLIDATE]** (property test exists) |
| K4 | No memory ever lost | every input id is active-or-superseded post-run; row count non-decreasing; ≥1 merged row per merge cluster | **[CONSOLIDATE]/[STORAGE]** (property test exists) |
| K5 | Per-boundary isolation | consolidation of boundary B never reads/writes/merges content from boundary B′ | **[STORAGE]** |
| K6 | Contradiction queue vs auto-resolve | clear-winner contradictions auto-`invalidate()`; ambiguous ones queue to `conflicts_for_user_review`, not silently picked | **[CONSOLIDATE]** |

---

## 5. Longitudinal stress + integrity

| ID | Test | Method | Pass criterion | Status |
|---|---|---|---|---|
| S1 | Scale degradation curve | bulk-load ~1,000 facts over simulated months (vary `as_of`); re-run A1/A4/A6 at N=100/500/1000 | recall + precision do not collapse as N grows; latency stays within target; record the curve | **[FIXTURE]** (needs bulk-load script) |
| S2 | Adversarial staleness | many near-duplicate facts where only the newest is true | read consistently returns current truth; stale near-dupes don't win on recency-blind similarity | **[FIXTURE]/[CONSOLIDATE]** |
| I1 | Encryption at rest | inspect on-disk vault files | memory content not readable as plaintext on disk (T0.2.0 gate) | **[STORAGE]** |
| I2 | Delete removes everything | delete a real memory; inspect storage | row + embedding + cascade rows gone; no orphan; not retrievable incl. `include_archived:true` | **[LIVE]** (retrieval check) + **[STORAGE]** (row check) |
| I3 | Boundary read isolation | with only B authorized, read a query whose best matches live in B′ | B′ content never returned | **[LIVE]** (if multi-boundary seedable) |

---

## 6. `read_quality_eval.json` schema (for Claude Code)

```json
{
  "schema_version": 1,
  "boundary": "testeval",
  "cases": [
    {
      "id": "A1-employer",
      "ability": "information_extraction",
      "seed_memories": [
        {"content": "The user works as a structural engineer at Vega Bridgeworks.",
         "source_agent": "claude-opus-4-7", "as_of": "2026-01-10", "confidence": 0.95},
        {"content": "The user enjoys hiking in the Cascades on weekends.",
         "source_agent": "gpt-5", "as_of": "2026-02-01", "confidence": 0.9}
      ],
      "query": "where does the user work?",
      "expect": {
        "must_surface": ["A1-employer#0"],
        "must_rank_top_k": {"id": "A1-employer#0", "k": 3},
        "must_exclude": ["A1-employer#1"],
        "abstain": false
      }
    },
    {
      "id": "A5-job-change",
      "ability": "knowledge_update",
      "seed_memories": [
        {"content": "As of 2026-01-10 the user works at Vega Bridgeworks.",
         "as_of": "2026-01-10", "confidence": 0.95},
        {"content": "As of 2026-04-01 the user works at Atlas Structures, having left Vega Bridgeworks.",
         "as_of": "2026-04-01", "confidence": 0.95}
      ],
      "query": "where does the user work now?",
      "expect": {
        "must_surface": ["A5-job-change#1"],
        "must_exclude": ["A5-job-change#0"],
        "abstain": false
      },
      "requires_consolidation": true
    },
    {
      "id": "A6-absent",
      "ability": "abstention",
      "seed_memories": [],
      "query": "what is the user's blood type?",
      "expect": {"abstain": true}
    }
  ]
}
```

Harness: seed each case's memories (via MCP `memory_write` or a direct test API), run consolidation for cases flagged `requires_consolidation`, issue the `query` via `memory_read`, then score `relevant_facts` against `expect`. Emit per-ability precision/recall + an abstention confusion matrix (false-abstain vs false-answer). Gate CI on the ability bars once baselined.

---

## 7. Live dogfood runbook — Claude Desktop

**This is the batch to run.** It has two parts. **Part A** is the live contract + abstention surface (run straight through, no setup). **Part B** tests the **consolidation features**: **deterministic dedup of near-identical duplicates (ADR-063 — new this session)**, **knowledge-update contradiction detection (A5) + precision**, and **settable `as_of`**. Part B has a **HANDOFF**: you seed the data, then PAUSE while Claude Code runs `vault-cli consolidate run` (you cannot run consolidation yourself), then you verify.

**Headline this run — A5 knowledge-update contradiction now fires via nearest-neighbor candidates (ADR-065, 2026-06-01):** the consolidator no longer relies on K-means topic grouping (which split the conflicting pair into different buckets so the judge never saw it and A5 silently failed). It now pairs each fact with its nearest cosine neighbors above a **0.70** similarity floor and judges those pairs with the existing Phi-4 + recency logic. **B1 (Tesla→Rivian) is the live ship-gate for this fix.** Expected: the run report shows the contradiction detected and ONLY the older Tesla invalidated. The Tesla/Rivian pair (measured cosine 0.823, mutual #1 neighbors) is the *only* candidate the floor admits in this seed set — every other seeded fact sits ≤0.634, below the floor — so Claude Code confirms a small `candidate_pairs` count + a single invalidation in the log.

**Also confirming fires live (carried from the prior session's arc):**
1. **Deterministic dedup (ADR-063):** near-identical duplicates collapse to one canonical copy with **no LLM call** — the case that used to overflow the merge model and skip forever. Surfaced + counted in the run report (`clusters deduped` / `memories deduped` / `clusters skipped`).
2. **Settable `as_of` on `memory_write`:** an optional date param that seeds the fact's `valid_from`.
3. **Degenerate-query robustness:** an all-stopword search now returns a graceful empty result, not an internal error.
4. **Bug-2 read fix — subject-less facts surface (ADR-064, 2026-06-01):** the read reranker used to reject facts stored WITHOUT a subject (a bare "Plays the cello…" scored below the floor even for near-literal queries) → the read abstained on facts the vault holds. Fixed read-side by framing every candidate as "The user — {fact}" before scoring. **A7 is the live gate** — the seeded cello hobby (item 4 in B-seed) is now deliberately subject-less, and two loosely-phrased reads must surface it.

**Conventions for the whole batch:**
- **Clean slate:** Claude Code wiped the vault data stores (vault.db + lance + graph.duckdb + reports) immediately before this run, so `testeval` (and `personal`) start empty. Seed ONLY the facts listed here — leftover data (especially the giant CAP_OK probes) pollutes clustering and muddies the dedup / A5 verdicts.
- **Boundary:** every write/read/search below uses the `testeval` boundary. Never use a boundary name with a dot.
- **Single-writer:** while this batch runs, no other agent writes to the vault.
- **Ground truth:** the Claude Desktop UI collapses JSON-RPC errors to a generic "Tool execution failed" and can echo stale tool descriptions. For any test that hinges on an error code, the **server log is authoritative** — Claude Code reads `%APPDATA%\Claude\logs\mcp-server-memory-vault.log` and confirms.
- **Record per test:** PASS / FAIL / rough-edge + paste the actual tool response.

### Part A — live contract + abstention (no consolidation needed)

- **C4 — content ceiling (⚠️ ENFORCE THE FULL DUMP — do NOT shorten).** This probes the SERVER's storage ceiling, so the **full literal payload MUST reach the server** — a shortened payload silently invalidates the test. `memory_write` three facts to `testeval`, each of the form `CAP_OK_<n>_START` + filler + `CAP_OK_<n>_END`, where the **filler is the 10-character block `ABCDEFGHIJ` repeated EXACTLY**:
  - probe 1 (5K):  **500** repeats → ~5,000 chars
  - probe 2 (10K): **1,000** repeats → ~10,000 chars
  - probe 3 (50K): **5,000** repeats → ~50,000 chars

  **HARD RULES for the writer (you, Claude Desktop) — this is the part that got skipped last time:** put the FULL literal string in the `content` field. Do **NOT** abbreviate. Do **NOT** write `…`, `[repeated]`, `(50000 chars)`, `... and so on`, or any placeholder. Do **NOT** summarize, sample, or shorten. The literal character count IS the entire test — emitting `ABCDEFGHIJ` 5,000 times is the work, not a formality. **Before each write, state the exact character count you are about to send.** If you genuinely cannot emit the full 50K string in one tool call, SAY SO and stop — never send a shortened version and report success.

  Then `memory_search` each `CAP_OK_<n>` marker. **PASS** = each is either stored intact (both brackets returned, full length, nothing missing mid/tail) OR rejected with a clear, documented error — never a silent success returning truncated content. Note the largest size that stored.

  **Claude Code backstop (ground truth, catches laziness):** for each probe, Claude Code reads the ACTUAL stored content length from storage/log and confirms it matches the intended size. A short read = the writer truncated (a writer FAIL, distinct from a server-truncation FAIL); a full read with both brackets = real PASS. If Desktop cannot emit the 50K payload after trying, Claude Code writes the exact-length probe via the same MCP server path as a fallback so the ceiling still gets measured.

  **THEN, once C4 passes, `memory_delete` all three CAP_OK probes** (capture each returned id at write time). They are giant blobs; left in `testeval` they dominate Part B's clustering and contaminated the A5 verdict last session. The storage-ceiling check is complete the moment they store/reject — delete them before moving on.
- **C5 — malformed-id delete.** `memory_delete` with `id` = `"not-a-uuid"`. **PASS** = clean rejection, session survives (UI may say "Tool execution failed" — that's the client collapsing a structured `-32602`; Claude Code confirms `-32602` in the log). NOT a crash/hang.
- **C6 — update round-trip.** `memory_write` `"C6COLOR The user's favorite color is teal."` → capture the returned id → `memory_update` that id with `"C6COLOR The user's favorite color is amber."` → `memory_read` `"what is the user's favorite color?"`. **PASS** = read returns **amber**, teal not surfaced, id stable.
- **C7 — unicode round-trip.** `memory_write` `"C7UNI 🔐 日本語 Ωβγ café résumé ﬁnesse."` → `memory_search` **`"C7UNI"`** (query the literal marker token — it's the only term guaranteed to be in the content; a query like "unicode probe" shares NO tokens/semantics with a mixed-script string and won't retrieve it). **PASS** = content returned byte-identical (incl. the ﬁ ligature U+FB01 NOT decomposed; modulo a documented trailing-period normalization). NB: `memory_search` is the *raw* hybrid retriever (no reranker — that lives in `memory_read`), so on a tiny vault unrelated large entries can out-rank via RRF; query the marker, don't eyeball top results.
- **C8 — boundary-auth rejection.** `memory_write` to boundary **`secretwork`** — a VALID, dot-free name that is NOT in the authorized set. **Do NOT use a name containing a dot** (a dot trips input-validation `-32602` *before* the auth gate and gives a false pass). **PASS** = rejected, nothing written; the server returns **`-32001` AccessDenied** (Claude Code confirms in the log — `-32001` = unauthorized, distinct from `-32602` = malformed input). Then `memory_search` `"secretwork"` in your authorized boundary to confirm nothing leaked.
- **C9 — normalization determinism.** `memory_write` the SAME raw content twice: `"  C9NORM the user is checking normalization determinism.  "` (note the leading/trailing spaces). **PASS** = both stored in identical canonical form (only documented transforms, e.g. trimmed edges + trailing period).
- **A6 — abstention (true negative).** `memory_read` `"what is the user's blood type?"`. **PASS** = `abstain:true`, empty `relevant_facts`, no fabrication.
- **A7 — abstention (no over-abstain) — THE BUG-2 LIVE GATE (ADR-064).** Run this **after B-seed** (relies on the cello hobby fact, B-seed #4, stored **subject-less** — `"Plays the cello…"`, no "The user" — exactly the phrasing class that made the read abstain before this session's fix). Issue **two** loosely-phrased reads, each of which scored deeply below the reranker floor BEFORE the fix (−4.46 and −5.21 respectively):
  - `memory_read "what does the user do for fun?"` (zero keyword overlap with "cello")
  - `memory_read "what music does the user play?"` (near-literal, yet was the *worst* pre-fix score)

  **PASS** = BOTH return `abstain:false` with the cello fact surfaced. `abstain:true` on either = the subject framing didn't take (Bug-2 regressed). Don't phrase it "outdoors" — the seeded hobby is indoor, so an outdoors query *should* abstain and would mis-score.
- **I2 — delete removes from retrieval.** `memory_write` `"I2DEL zzqx-sentinel single-use deletion probe."` → `memory_search` to confirm present → capture id → `memory_delete` that id → `memory_search` again with `include_archived:true`. **PASS** = the fact is gone from BOTH normal and archived results (hard delete, no orphan).
- **I3 — boundary read isolation.** Only if a second authorized boundary is seedable this session; otherwise skip and note "not run (single boundary)".
- **C10 — degenerate query (new this session).** `memory_search "the a is of and to"` (all stopwords — no searchable terms after the stopword filter). **PASS** = a clean **empty result**, no error, no crash. (Claude Code confirms **no `-32603`** in the log — this query previously returned an internal error.)
- **C11 — settable `as_of` (new this session).** `memory_write` `"C11ASOF The user adopted a rescue dog."` to `testeval` with the optional param **`as_of: "2024-01-15"`**. **PASS** = the write succeeds cleanly (no `-32602`). The `as_of` seeds the fact's `valid_from`; Claude Code confirms the stored `valid_from` is `2024-01-15` (the supplied date, not the write timestamp) if storage inspection is available — otherwise this confirms the live MCP path accepts the param (the date→`valid_from` mapping itself is unit-tested in `as_of_write.rs`). **Delete the C11ASOF probe after** (keeps Part B's set clean).

### Part B — dedup (ADR-063) + contradiction detection (A5) + precision  ← the features under test

**B-seed (Claude Desktop).** `memory_write` each of these to `testeval`, one call each:
1. `"A5CAR The user drives a Tesla Model 3."` — with param **`as_of: "2026-02-01"`** *(older)*
2. `"A5CAR The user sold the Tesla and now drives a Rivian R1T."` — with param **`as_of: "2026-05-01"`** *(newer — supersedes #1)*
3. `"PRECJOB The user works as a data scientist at Helix Labs."`
4. `"Plays the cello in a community orchestra on Sunday afternoons."` *(seed VERBATIM — subject-LESS and **NO marker prefix**, on purpose. This is the exact phrasing the Bug-2 fix (ADR-064) rescues; a marker like "PRECHOBBY " would sit between the subject-frame and the verb and drag the read below the floor — Claude Code pre-verified the clean form surfaces at +3.2/+2.1/+1.3 for the A7/B3 reads. Do not add a marker to this one.)*
5. `"PRECFOOD The user's favourite cuisine is Japanese."`
6. **Dedup triplet (new this session)** — `memory_write` this **exact same content three times** (three separate calls, verbatim): `"DEDUPDOG The user's dog is a Labrador named Biscuit."` *(three near-identical copies → must collapse to one at consolidation, no LLM)*

(Items 1–2 now carry the date as an `as_of` param, not just in the text — this exercises settable `as_of` end-to-end and feeds the fact's `valid_from`.)

Then **STOP and hand off:** tell Shahbaz "B-seed complete — ready for consolidation." Claude Code runs `vault-cli consolidate run` (real Phi-4) and confirms via the log + report. **Do not proceed to B-verify until Claude Code says consolidation is done.**

**B-verify (Claude Desktop, only after consolidation).**
- **B1 — A5 catch (the ship-gate).** `memory_read` `"what does the user drive now?"`. **PASS** = returns **ONLY the Rivian fact (#2)**; the Tesla fact (#1) does NOT appear; `abstain:false`.
- **B2 — reversible, not deleted.** `memory_search` `"Tesla Model 3"` with `include_archived:true`. **PASS** = the Tesla fact IS found (archived / invalidated — retained, recoverable, not destroyed).
- **B3 — precision (no false retirement).** `memory_read` each, and confirm the fact still surfaces (not retired):
  - `"where does the user work?"` → **PRECJOB** present
  - `"what hobby does the user have?"` → **the cello hobby fact (#4)** present
  - `"what food does the user like?"` → **PRECFOOD** present
  **PASS** = all three still present. **The job and hobby facts must BOTH survive** — a weak model previously called "works as an engineer" and "enjoys hiking" *contradictory*; nothing here may be wrongly retired. (Claude Code cross-checks the consolidation log: exactly the Tesla fact invalidated, zero false flags.)
- **B4 — deterministic dedup (the new ADR-063 feature).** After consolidation: `memory_read "what is the user's dog?"` → returns the Labrador/Biscuit fact **exactly once** (not three copies; `abstain:false`). Then `memory_search "Biscuit"` with `include_archived:true` → the duplicate copies ARE still present as **archived/superseded** (collapsed, not deleted — reversible). **PASS** = one canonical copy at read; the other two retained-but-superseded. (Claude Code confirms the run report shows `clusters deduped: ≥1` and `memories deduped: 2`, and that the dedup happened with **zero LLM merge calls** — the survivor + two `memory.superseded` events in the log, no merge skip.)

**Teardown note.** Seeded this run: `CAP_OK_*` (deleted after C4), `C6COLOR`, `C7UNI`, `C9NORM` ×2, `I2DEL` (deleted), `C11ASOF` (deleted after C11), `A5CAR` ×2, `PRECJOB`, the **markerless cello hobby fact (#4)**, `PRECFOOD`, `DEDUPDOG` ×3. Teardown is a **full boundary wipe** by Claude Code (vault.db + lance + graph.duckdb + reports), so no per-fact deletion is needed afterward.

### What Part B proves
B4 = near-identical duplicates collapse **deterministically, with no LLM call**, surfaced + counted in the report (ADR-063 — the overflow/skip class that previously sat unmerged forever is gone). B1 = the consolidator detects a real knowledge-update contradiction and retires only the stale side (A5 ship-gate, on real Phi-4). B3 = the precision fix holds — different-attribute facts are no longer mistaken for contradictions. C11 (Part A) = `as_of` seeds `valid_from`. Together they are the end-to-end confirmation of this session's consolidation work; Part A confirms nothing else regressed.

## 8. Suggested sequencing

1. **Now:** I run the §7 live batch → report pass/rough-edge per test.
2. **Claude Code, parallel:** finish Commit 8 (MCP serve + dogfood), then run `consolidate run` on `testeval` → unblocks K1–K6, A5, X1–X4.
3. **Claude Code:** build `read_quality_eval.json` + scoring harness → unblocks A1–A3, A7, S1–S2. Baseline the numbers; set CI gates.
4. **Ship gate:** A5 (knowledge-update/contradiction) and A6 (abstention) must clear before V0.2 beta — they are the correctness story the category is decided on.

## 9. Benchmarking discipline (non-negotiable for the public story)

When you publish numbers: run **LongMemEval and LOCOMO** with released, reproducible methodology. **Never claim 100%.** A reproducible audited score beats a disputed perfect one — the dev community tears inflated memory-benchmark claims apart, and that becomes the story instead of the product. Lead with knowledge-update / temporal / abstention category scores, since that's where the edge is and where incumbents are weakest.
