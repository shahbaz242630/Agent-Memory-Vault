# Memory Vault — Correctness Test Spec (V0.2 dogfood)

**Thesis:** in a memory product the output has to be correct every time. These tests measure correctness on the *agent-read* workload — the differentiator — not just tool plumbing. Structure borrows LongMemEval's five abilities (information extraction, multi-session reasoning, temporal reasoning, knowledge update, abstention), extends them with the cross-agent shape no competitor benchmarks, and adds longitudinal stress + integrity.

Each test is tagged with an execution status:

- **[LIVE]** — I (the connected advisory Claude) can run this right now over the MCP tools, no setup.
- **[CONSOLIDATE]** — needs `vault-cli consolidate run` to have executed on the test boundary first (topics, REPORT, contradiction detection, merge all happen at consolidation).
- **[FIXTURE]** — needs a purpose-built eval fixture + a scoring harness Claude Code writes (can't be judged by eyeballing a handful of facts).
- **[STORAGE]** — must be verified at the storage layer by Claude Code; not observable over MCP.

---

## 0. Preconditions / setup (Claude Code)

1. **Dedicated eval boundary.** Authorize a throwaway boundary (e.g. `test.eval`) in the MCP session config so eval writes don't pollute `personal` / `project.memory-vault`. All seed data below assumes `test.eval`. Tear down after each run (or use a fresh vault dir per run).
2. **Consolidator runnable.** `vault-cli consolidate run --boundary test.eval ...` wired and green (Commit 8 dogfood prereq). Needed for all [CONSOLIDATE] tests.
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
  "boundary": "test.eval",
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

## 7. What I can execute live right now (over MCP, no setup)

Ready to fire this batch against the live vault on your go:

- **C4** content-ceiling probe (5K / 10K / 50K)
- **C5** malformed-id delete
- **C6** update round-trip
- **C7** unicode round-trip
- **C8** boundary-auth rejection (attempt write to an unauthorized boundary)
- **C9** normalization determinism
- **A4** temporal-reasoning smoke (two `as_of`s, query "latest")
- **A6** abstention true-negative
- **A7** abstention no-over-abstain smoke
- **I2** delete-removes-from-retrieval (the retrieval half)
- **I3** boundary read isolation (if a second boundary is seedable this session)

These give immediate signal on the contract surface + abstention while Claude Code wires the consolidator and builds `read_quality_eval.json` for the fixture-gated tiers.

## 8. Suggested sequencing

1. **Now:** I run the §7 live batch → report pass/rough-edge per test.
2. **Claude Code, parallel:** finish Commit 8 (MCP serve + dogfood), then run `consolidate run` on `test.eval` → unblocks K1–K6, A5, X1–X4.
3. **Claude Code:** build `read_quality_eval.json` + scoring harness → unblocks A1–A3, A7, S1–S2. Baseline the numbers; set CI gates.
4. **Ship gate:** A5 (knowledge-update/contradiction) and A6 (abstention) must clear before V0.2 beta — they are the correctness story the category is decided on.

## 9. Benchmarking discipline (non-negotiable for the public story)

When you publish numbers: run **LongMemEval and LOCOMO** with released, reproducible methodology. **Never claim 100%.** A reproducible audited score beats a disputed perfect one — the dev community tears inflated memory-benchmark claims apart, and that becomes the story instead of the product. Lead with knowledge-update / temporal / abstention category scores, since that's where the edge is and where incumbents are weakest.
