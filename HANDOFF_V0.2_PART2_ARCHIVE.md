<!-- ============================================================================
     ARCHIVE — FROZEN 2026-06-08. DO NOT EDIT.
     Covers: T0.2.3 commit 3 → T0.3.x (2026-05-14 → 2026-06-07), spanning the
     read-pipeline correctness arc (reranker ADR-059, recall-first ADR-066/067,
     ADR-069/070/071, the 10k TOCTOU fix ADR-072) + the consolidator REPORT arc
     (ADR-052/053/054/055) + the A5 contradiction arc (ADR-060/064/065).
     Full ADR text (ADR-047–072), all next-session-opener histories, tech-debt
     forward-pointers, technique map, consolidator inventory, and V0.2 tuning
     config live here verbatim. The slim active HANDOFF.md cross-links back to
     this file by ADR number and section. Do NOT paraphrase — quote.
============================================================================ -->

# Memory Vault — Build Handoff

**Current version:** V0.2 Closed Beta (BRD §6.2 — sleep consolidator, boundaries hardening, cross-device sync, 30 beta users)

**Last updated:** 2026-06-07 #2 (session close) — **Thread 1 (10k storage-worker panic) SHIPPED & CI-GREEN (`da10c0f`, ADR-072, run 27096332980 ✅). 100-fact vault LIVE-VALIDATED clean in Antigravity across BOTH tools × BOTH model tiers. Live-vault seeder BUILT (uncommitted working-tree). NEXT: seed 1k → 10k, live-test each in Antigravity; if both pass, commit the seeder + declare the retrieval core BATTLE-TESTED.** Full 1k/10k action plan + all paths/commands in the 2026-06-07 #2 opener below. (ADR-072 root-cause/fix detail retained in that opener's DONE section + the superseded 2026-06-07 #1 opener; ADR-071 line further below HISTORICAL.)

<details><summary>Prior Last-updated (2026-06-06, ADR-071) — HISTORICAL</summary>

**2026-06-06:** **ADR-071 RERANKED + recall-safe `memory_search` SHIPPED — all 5 DoD gates GREEN (fresh from `cargo clean`), 3-model live-dogfood validated in Antigravity, committed this session.** `RerankedRetriever` brings the cross-encoder + ADR-069 recall-union to `memory_search` as REORDER-ONLY (never false-empty); sigmoid logit→[0,1] score; tool descriptions steer question-answering to `memory_read` (the primary answer path, returns structured `abstain`). Live-validated: Flash one-shots `memory_read` (cello + blood-type abstain, no 4-min spin); Opus 4.6 `memory_search` reorders cello #4→#1, never `[]`. **Option B (additive `weak_match`/`top_relevance` hint on `memory_search`) ALSO shipped + 2-case live-validated this session** (instrument → `weak_match:false`; blood-type → `weak_match:true`; separation-based, survives the reranker's terse-query brittleness). ADR-071 committed `661d391` (CI ✅ 27064305259); Option B committed `a1e4dac` (CI ✅ 27067515185). **THEN ran the scale-correctness ladder 100→1k→10k: correctness is SCALE-INVARIANT (identical scorecard at 100× scale), but the 10k run surfaced (1) a storage-worker PANIC (bytes range-OOB in the write path) and (2) a measured read-precision gap (false-answers + near-miss leaks). NEXT: fix the panic FIRST, then the precision gate, then build the live-vault seeder + seed 100→1k→10k for the Antigravity multi-agent live test. See the 2026-06-06 #2 opener below.** Prior openers HISTORICAL.)

</details>

---

## 🟢 NEXT SESSION OPENER (2026-06-07 #2 — SESSION CLOSE) — **SEED 1k → 10k, LIVE-TEST EACH IN ANTIGRAVITY; IF BOTH PASS → COMMIT THE SEEDER + DECLARE THE RETRIEVAL CORE BATTLE-TESTED.** — READ THIS FIRST

> This session: Thread 1 (the 10k panic) is SHIPPED + CI-GREEN (`da10c0f`, ADR-072). Then built a live-vault seeder and **live-validated the 100-fact vault in Antigravity — clean across `memory_read` (small Gemini) AND `memory_search` (powerful model, `weak_match` hint), incl. the cello subject-less case (#1 @ 0.79, keyboards a distant 0.047) and correct abstains on blood-type/salary/cat-breed.** Reranker separation wide (real +1.3/+5.0 logit, noise −3 to −11). Verdict: core solid, no engine red flags; Thread 2 (read precision) is the one open yellow flag (dodged live via phrasing+`weak_match`, NOT solved). See memory [[project_100scale_live_validation_antigravity]]. Goal now: prove scale-invariance LIVE at 1k + 10k, then lock the core.

### ▶️ NEXT ACTIONS (in order)
1. **Seed 1k** (fresh folder): `$env:SEED_N='1000'; $env:SEED_VAULT_DIR='C:\Projects\seeded-vault-1k'; cargo test -p vault-app --test scale_eval seed_live_vault -- --ignored --nocapture` — ~17 min (single-row cascade drain ≈ 1s/vector; the seeder waits for full VECTOR-count drain, prints the test script at the end).
2. **Verify the seed**: `$env:LANCE_MEM_POOL_SIZE='268435456'; & "C:\Projects\GitHub\Memory Vault\target\debug\vault-cli.exe" --vault-db C:\Projects\seeded-vault-1k\vault.db --vector-dir C:\Projects\seeded-vault-1k\lance --graph-db C:\Projects\seeded-vault-1k\graph.duckdb divergence-check` — expect `sqlite == vector`, **no findings**.
3. **Repoint Antigravity**: edit the REAL config file `C:\Users\shahb\.gemini\config\mcp_config.json` (the `~/.gemini/antigravity/mcp_config.json` is a SYMLINK to it — edit the real target, the Edit tool refuses the symlink). Change the 3 vault paths (`--vault-db`/`--vector-dir`/`--graph-db`) to the `seeded-vault-1k` folder. **Restart Antigravity** (it won't pick up the change while running). Confirm the server launched at the right vault: `Get-CimInstance Win32_Process -Filter "Name='vault-cli.exe'" | Select CommandLine`.
4. **Run the 15 questions** (in `crates/vault-app/tests/fixtures/scale_eval.json`; the seeder also prints them with expected answers). Watch: #6 cello (subject-less), #12 salary + #14 cat-breed (Thread-2 traps), the 5 abstains.
5. **Seed + test 10k** the same way (`SEED_N='10000'`, `C:\Projects\seeded-vault-10k`). **NOTE: 10k seed is MULTI-HOUR** (~1s/vector × 10k, degrading) — plan an overnight/long run. Verify + repoint + test as above.
6. **IF 1k AND 10k BOTH PASS LIVE → (a)** commit the seeder (`crates/vault-app/tests/scale_eval.rs` `seed_live_vault` + the vector-count-drain probe) with full DoD gates → CI green; **(b)** declare the retrieval core "battle-tested at scale" and close this arc. THEN Thread 2 (read-precision) becomes the next arc.

### ⚙️ Uncommitted working-tree state (commit when the core is declared battle-tested, per step 6a)
- `crates/vault-app/tests/scale_eval.rs` — the `seed_live_vault` `#[ignore]` seeder (production-keychain key, bge-small fixture vectors, env `SEED_N`/`SEED_VAULT_DIR`, drains by VECTOR COUNT). Compiles clean (0 warnings); used live this session. NOT yet committed.

### 🔧 Antigravity config — current state + how to revert
- **Currently points at `C:\Projects\seeded-vault-100`** (boundaries `personal` + `testeval`; planted facts live in `personal`).
- **Backup of the original (real-vault) config:** `C:\Users\shahb\.gemini\antigravity\mcp_config.json.bak-realvault`.
- **To restore the real vault** (when done testing): set the 3 paths back to `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\{vault.db, lance, graph.duckdb}` in `~/.gemini/config/mcp_config.json`, then restart Antigravity.
- vault-cli binary: `C:\Projects\GitHub\Memory Vault\target\debug\vault-cli.exe`. Models: bge-small + qwen3-reranker fixtures under `crates/vault-embedding/test-fixtures/`. `LANCE_MEM_POOL_SIZE=268435456` not set in his config (works without; matters only for heavy concurrent WRITES, not the read-only live test).

### 🧠 Seeder gotcha (learned this session — don't regress)
Confirm drain by **VECTOR ROW COUNT** (`LanceVectorStore::count`), NOT by searching for a sentinel fact — search finds a fact via the keyword channel BEFORE its vector lands in LanceDB, which shipped a **1-of-101-vector vault** on the first attempt (caught only by `divergence-check`). The seeder now polls a re-opened `LanceVectorStore` count each tick until `== total`. Also: a freshly-seeded vault shows a cosmetic `REPORT_MISSING` / `status: degraded` health warning (consolidator hasn't run) — harmless, does NOT affect answers; clear later via `vault-cli consolidate run` (needs `--phi4-model`; do it when the MCP server is NOT holding the vault — single-writer).

### 🟠 Still open (own arcs, NOT blockers for the 1k/10k validation)
- **Thread 2 — read precision** (`memory_read` over-includes adjacent content; salary→catering-$, cat→dog). Dodged live via phrasing + `weak_match` but NOT solved. Likely fix: recall-safe "weak match" hint on `memory_read` mirroring Option B's search hint (let the agent judge, never drop a fact). Validate on `scale_eval` scorecard.
- Carried: REPORT_MISSING cleanup (run consolidator on the live vaults); `max_results` 10→5 (proven safe @top-5, one change at a time); Antigravity `instructions.md` rewrite (prefer `memory_read`, empty = not in vault).

## 🟢 NEXT SESSION OPENER (2026-06-07 #1, SUPERSEDED by #2 above — Thread 1 SHIPPED & CI-green as `da10c0f`; retained for the ADR-072 root-cause/fix detail) — **THREAD 1 (storage-worker panic) — REAL FIX in place (ADR-072), DoD-gating. Flaky/data-safe/self-healing race, not a hard crash. NEXT: gate → commit → CI. Then (A) Thread 2 read-precision pass, or (B) live-vault seeder — Shahbaz's call.**

> The 10k panic was chased to ground the hard way: first theory (head-only empty body) was FALSIFIED by a 10k run that still panicked. Captured the full backtrace (`RUST_BACKTRACE=full`) + read Lance's source: the crash is a TOCTOU in `get_opts`'s bounded-range path — Lance caches a fragment size, then under load the unsealed content is momentarily shorter, we returned a short buffer, Lance's scheduler over-sliced it. **It's a flaky, data-safe, self-healing race** (two 10k runs: one panicked, one clean; identical perfect scorecard both times; Lance `catch_unwind`s it → cascade retry-queue self-heals). Real fix: return a retryable error instead of a short buffer. Deterministic unit-test pin (no flaky 10k needed). **Confirm before commit+push per the standing rule.**

### ✅ DONE THIS SESSION (ADR-072 — the REAL fix)
- **The journey (honest record):** first hypothesis — `head: true` returned an empty body while reporting a non-zero size — was a real *consistency* gap but NOT the panic: a 10k run WITH that fix still panicked twice. Kept that change as a minor, separately-tested improvement (`get_opts_head_only_body_matches_declared_size_and_range`), it is not load-bearing for the panic.
- **Real root cause (full backtrace + Lance source-read):** backtrace = `bytes::slice` ← `lance_io::scheduler::submit_request` (scheduler.rs:984 `bytes_vec[i].slice(start..end)`) ← `lance_file::reader::read_tail` (reader.rs:437) ← `get_file_metadata`. `read_tail` requests `begin..file_size` where `file_size = reader.size()` = our `head` (disk − `SEAL_OVERHEAD`), cached once via `get_or_try_init`. Under heavy write load at scale the unsealed content for that path is momentarily SHORTER than the cached size, so our `get_opts` bounded-range arm returned a clamped SHORT buffer; Lance's scheduler trusts the requested length and slices the short buffer → `range end out of bounds: N <= 0`. A TOCTOU (time-of-check `size()` vs time-of-use `get`).
- **Why flaky + low-severity:** two 10k `scale_eval` runs, identical code — run A panicked ×2, run B totally clean (`finished in 2286s`, `test result: ok`). BOTH: recall 10/10, 0 false-abstain, identical scorecard → **no data loss / no corruption**. Lance wraps the metadata read in `catch_unwind`, so a hit becomes a transient error → the cascade write reschedules in our retry-queue → retried → succeeds. User impact = an ugly internal hiccup + a silent retry under heavy write load at thousands of memories; never a user-facing crash or lost memory.
- **Real fix (`get_opts` bounded-range arm):** if `r.end > plaintext_len`, return a retryable `ObjectStoreError::Generic` instead of a short slice. Lance's get-retry (`do_get_with_outer_retry`, 3×) and our cascade retry-queue both re-fetch with a fresh size and self-heal — the over-slice mechanism is removed by construction. In normal operation `r.end == plaintext_len` exactly, so the guard never fires. (Bounded is the only path the backtrace exercises; Offset/Suffix clamping left as-is.)
- **Pin (deterministic, no flaky 10k):** `get_opts_bounded_range_beyond_content_errors_not_short_read` — seal 100 bytes, request `0..500` → asserts `Err` (not a short `Ok`); plus an in-bounds `10..60` over-correction guard that must still return exactly 50 bytes. The mechanism is provably gone regardless of whether the flaky race triggers.
- **No crypto change** (seal/unseal/AAD/key untouched; zero-knowledge property intact).
- **Rides this commit:** `crates/vault-app/tests/scale_eval.rs` (the `SCALE_EVAL_N` env override + scale-aware readiness poll — the harness that surfaced the panic; ships with the fix it supports). The diagnostic `eprintln` + the slow `sealed_concurrent_stress.rs` repro attempt were REMOVED (the stress harness couldn't reach trip-scale: single-row writes are O(n²), it crawled to ~1k in ~30 min; the deterministic unit test supersedes it).
- **Gates:** the earlier full DoD sweep (fmt/clippy/build/test) was green on the head-only change; the real fix needs a RE-GATE before commit (in progress).

### ADR-072 — sealed-store `get_opts` never returns a short buffer for a bounded range (fixes the 10k TOCTOU panic)
**Context.** 10k-scale `tokio-rt-worker` panic `range end out of bounds: N <= 0`, in Lance's scheduler slicing a buffer our sealed store returned for a `read_tail` bounded-range request. Flaky (race), data-safe (identical scorecards across a panicking and a clean run), self-healing (Lance `catch_unwind` → cascade retry-queue). Root cause: Lance caches `size()` (our `head` = disk − `SEAL_OVERHEAD`); under load the unsealed content is momentarily shorter than the cached size; our bounded-range arm clamped and returned a short buffer; the scheduler over-sliced it.

**Decision.** In `get_opts`, a bounded range whose `end` exceeds the current unsealed `plaintext_len` returns a retryable `ObjectStoreError` rather than a clamped short buffer. Lance's get-retry + our cascade retry-queue re-fetch with a fresh size and self-heal. Removes the over-slice mechanism by construction; pinned by a deterministic unit test.

**Not chosen:** (a) trying to fully eliminate the underlying TOCTOU race (deep Lance-interaction change, flaky to repro/confirm, and unnecessary — the data is safe and the retry self-heals); (b) re-reading the file inside `get_opts` on short-read (more code + extra I/O; the retry layers already exist). **Secondary, separate change kept in this commit:** `head: true` now streams the full already-decrypted body so body-len == range-span == meta.size (a consistency improvement found while chasing the first wrong theory; pinned by its own test; NOT the panic fix). Frozen `examples/at_rest_spike.rs` untouched.

**Security:** no crypto-path change — seal/unseal/AAD/key untouched; the returned plaintext is what the local Lance reader already receives on every normal read.

### 🟠 THREAD 2 — read precision (false-answers + near-miss leaks) — STILL OPEN, own pass
NOT a crash; bounded; scale-invariant. The read path is recall-first by lock (ADR-066): it returns everything above a coarse no-signal floor and trusts the agent. Cost: it sometimes returns a confident wrong-but-adjacent fact instead of abstaining ("salary" → catering "$6,500 annual"; "cat breed" → the dog; "instrument" → cello-correct + mechanical-keyboards leaked). **Why not a quick fix:** tightening to catch these also hides real low-scoring answers (cello scored 0.047 and was CORRECT) → false-abstain, the cardinal sin. **Likely recall-safe direction (Shahbaz's instinct, 2026-06-07):** mirror Option B — attach a "weak match / may not really answer this" hint to `memory_read` facts and let the agent say "I don't have it," NEVER drop a fact. Needs: measure exact reranker scores for these adjacent cases, design the hint, validate on `scale_eval.rs` (real BGE+reranker — no cheap unit test proves it).

### 🌱 THREAD 3 — live-seeder + Antigravity multi-agent test — NOW UNBLOCKED
Thread 1 (the panic) was the gate; it's fixed. Seeder design is locked in the 2026-06-06 #2 opener below (a new `#[ignore]` test in `scale_eval.rs` writing a REAL vault with the production keychain derivation + bge-small vectors, seeded 100→1k→10k). **Need from Shahbaz:** his current Antigravity `vault-cli mcp …` launch command (to copy model paths + hand back an edited version pointing at the new vault). Note: a clean 10k `scale_eval` run does NOT *prove* the panic is gone (the race is flaky — it ran clean once with the bug still present); the deterministic unit test proves the over-slice mechanism is removed. Seeding 10k live is safe regardless — even if the race fires it's data-safe + self-heals via the cascade retry-queue.

---

## 🔴 NEXT SESSION OPENER (2026-06-06 #2) [SUPERSEDED by 2026-06-07 — Thread 1 FIXED; Threads 2+3 carried forward above] — **SCALE LADDER DONE (100→1k→10k: correctness scale-INVARIANT). TWO OPEN THREADS: (1) storage-worker PANIC at 10k (bytes range-OOB in the write path) — a CRASH, fix FIRST; (2) read precision gate (false-answers + near-miss leaks) — bounded, not scale-driven. THEN: build live-vault seeder, seed 100→1k→10k, repoint Antigravity, multi-agent LIVE test at 10k.** — READ THIS FIRST

> Shipped ADR-071 (`661d391`, CI ✅) + Option B (`a1e4dac`, CI ✅ 27067515185). Then ran the scale-correctness ladder 100 → 1k → 10k via `scale_eval.rs` (new `SCALE_EVAL_N` env override). **Correctness is identical across 100× scale.** The 10k run surfaced a background-worker panic in the storage write path, and the read precision gap (the keyboards-for-instrument family Shahbaz caught live) is now measured + bounded. Next session: fix the panic, then the precision gate, then live-seed + Antigravity.

### 📊 Scale ladder results (`scale_eval.rs`, real BGE + Qwen3-reranker, own temp vault)
| metric | 100 | 1k | 10k |
|---|---|---|---|
| correct-answer | 10 | 10 | 10 |
| correct-abstain | 3 | 3 | 3 |
| **FALSE-ABSTAIN (the cliff)** | 0 | 0 | 0 |
| FALSE-ANSWER (precision) | 2 | 2 | 2 |
| read recall (targets / rank-1) | 10/10 (9@1) | 10/10 (9@1) | 10/10 (9@1) |
| search recall @5 / @20 | 10/10 | 10/10 | 10/10 |
| near-miss leaks (diagnostic) | 6 | 6 | 6 |

**Takeaway: core retrieval is scale-invariant.** 100× the distractors changed *nothing* — recall never degrades, the right answer ranks #1, false-abstain stays 0. Precision issues are a FIXED, bounded set (specific adjacent-content confusions), NOT growing with scale. Runtimes: 100 ≈ 18min (ran both file tests), 1k ≈ 12min, 10k ≈ 38min (main test only, filtered). **100k DEFERRED until Thread 1 is fixed** (would just crash harder + waste a ~hours-long run).

### 🔴 THREAD 1 (do FIRST) — storage-worker PANIC at 10k
```
thread 'tokio-rt-worker' panicked at bytes-1.11.1/src/bytes.rs:392:9:
range end out of bounds: 2385 <= 0
```
- Surfaced ONLY at 10k (1k was clean) → scale/timing-dependent (likely a race or buffer edge under heavy concurrent writes).
- `bytes` underlies Arrow/LanceDB → almost certainly the **vector-store write path**, on the cascade write-drain worker spawned by `app.start()`.
- Detached worker thread → did NOT fail the test (assertions passed, recall 10/10), but in production **a write-drain worker can die under load**. **Blocks the live seeder** (it writes 10k through this same path → same crash).
- Investigate: (a) source-read `crates/vault-storage/src/vector_store.rs` write / record-batch path for a slice/advance where an Arrow/embedding buffer can be empty (FixedSizeList construction, batch sizing); (b) if not pinned, re-run 10k with `RUST_BACKTRACE=full` for the exact frame — **no backtrace was captured this run**.

### 🟠 THREAD 2 — read precision gate (false-answers + near-miss leaks)
The read path OVER-INCLUDES adjacent content. Measured, bounded, scale-invariant:
- **2 FALSE-ANSWERS** (answered when it should abstain): *"what is the user's salary?"* → catering bookings (*"…rate approximately $6,500 annual"*); *"what breed is the user's cat?"* → *"golden retriever named Biscuit"* (the dog; user may not even have a cat).
- **6 NEAR-MISS LEAKS** (extra wrong-but-similar facts riding with correct answers): the **keyboards-for-"instrument"** case (cello #1 correct, *"vintage mechanical keyboards"* leaked at rank 2) — the one Shahbaz caught live.
- Root: the read abstain/precision gate (reranker relevance floor, ADR-059/066) is too loose on **adjacent** no-signal/near-miss content. The reranker scores UNRELATED distractors low (so leaks don't grow with scale) but is fooled by genuinely-adjacent facts (keyboard=instrument polysemy, $-amount=salary, dog≈cat-pet).
- Note: the SEARCH path scored these correctly low (keyboards 0.0027) — so the gap is the **read path's inclusion rule**, not the reranker's raw scores.
- Fix direction (TBD): tighten read precision on adjacent content **WITHOUT re-introducing false-abstains** — recall stays sacrosanct (a false-abstain is the worse failure).

### 🌱 THREAD 3 — LIVE-SEEDING TEST PLAN (the Antigravity multi-agent step)
Goal: populate a REAL vault Antigravity can open, with KNOWN planted answers, to watch real agents behave at scale. Seed in steps **100 → 1k → 10k**; live multi-agent test at 10k (Shahbaz's call).
- **Seeder design (locked this session):** a new `#[ignore]` test in `scale_eval.rs` (reuses `generate_distractors` + planted facts directly — zero refactor) that writes to a REAL vault (env: paths + `SEED_N`) using the SAME Credential-Manager key derivation the MCP server uses (`vault_app::keychain::read_or_init_master_key(PRODUCTION_NAMESPACE, VAULT_ID)` → `derive_sqlcipher_passphrase` + `derive_at_rest_key`) — so Antigravity opens it natively. Embed with the SAME bge-small model the MCP server uses (vectors must match). Seed into the `personal` boundary (Antigravity default). Exit without running queries.
- **Approach:** seed a FRESH vault at NEW paths + repoint Antigravity (current vault untouched, reversible). **Need from Shahbaz:** his current Antigravity `vault-cli mcp …` launch command (to copy the `--bge-model`/`--rerank-model`/etc. paths + hand back an edited version pointing at the new vault).
- **Sequence:** seed 100 → quick agent spot-check → seed 1k → spot-check → seed 10k → full multi-agent live test.
- **BLOCKER:** Thread 1 (the panic) gates this — seeding 10k hits the same write path. Fix the panic first.

### ⚙️ Uncommitted working-tree state (rides with the next code commit, per admin-rides-with-code)
- `crates/vault-app/tests/scale_eval.rs` — test-infra: `SCALE_EVAL_N` env override + scale-aware readiness poll (`max_attempts = scale/10`, floor 60). Used to run the ladder. NOT yet committed.
- `HANDOFF.md` — this opener.

### 🅿️ Carried follow-ups (not blockers)
- REPORT_MISSING on `personal` (run `vault-cli consolidate run`); REPORT_STALE_INFO on `testeval` (25h band, refresh).
- Antigravity `instructions.md` rewrite (list BOTH tools, prefer `memory_read`, empty = not in vault).
- `max_results` 10→5 (proven safe @top-5; one change at a time).

---

## 🟢 NEXT SESSION OPENER (2026-06-06) [SUPERSEDED by the 2026-06-06 #2 opener above — ADR-071 + Option B both SHIPPED & CI-GREEN (`661d391` / `a1e4dac`); retained for the ADR-071/Option B detail] — **ADR-071 RERANKED + recall-safe `memory_search` SHIPPED & CI-GREEN (`661d391`). Option B (additive `weak_match` hint) SHIPPED (`a1e4dac`, CI ✅). NEXT: carried follow-ups (REPORT_MISSING cleanup, Antigravity `instructions.md`, `max_results` revisit).** — READ THIS FIRST

> Picked up opener #4 (code written, unverified, uncommitted). Reclaimed 102 GB of stale `target/` (`cargo clean`), ran every gate fresh, then Shahbaz live-dogfooded in Antigravity across 3 model tiers. All green → committed ADR-071 (`661d391`, CI ✅ run 27064305259). Then built **Option B** (the additive `weak_match` hint that closes ADR-071's known no-signal-judgment limitation), gated it green, and live-re-validated both cases. Option B rides this commit.

### ✅ DONE THIS SESSION (ADR-071)
**All 5 DoD gates green, fresh from `cargo clean` (reclaimed 102 GB; disk 42 GB → 133 GB free):**
- `cargo fmt --all --check` ✅
- `cargo clippy --workspace -- -D warnings` ✅ (15m28s, 0 warnings)
- `cargo build --workspace` ✅ (21m31s, 0 warnings)
- `cargo test` ✅ — vault-retrieval / vault-mcp / vault-app all pass (8 new reorder-only unit tests incl. `never_returns_false_empty_even_when_all_score_negative`, `recall_union_rescues_a_base_missed_semantic_match`, `explicit_score_threshold_filters_on_relevance_scale`)

**3-model live dogfood (Antigravity, live `testeval` vault) — the validation that matters:**
| Model | Tool chosen | Query | Result |
|---|---|---|---|
| Gemini Flash (small) | `memory_read` | "what instrument do I play" | ✅ cello, `abstain:false`, conf 0.9 |
| Gemini Flash (small) | `memory_read` | "what is my blood type" | ✅ `[]`, `abstain:true` — **NO 4-min spin (the headline fix)** |
| Opus 4.6 | `memory_search` | instrument | ✅ cello reordered to **#1** (was #4), score +0.047 (sigmoid), **never `[]`** |
| Opus 4.6 | `memory_search` | blood type (no-signal) | ✅ returns ranked list (no false-empty), all scores ~0.0002 — recall-safe |

**Both catastrophic failures from opener #4's dogfood are fixed:** false-empty→abandon-vault, and no-signal→4-min-spin. Weak models now self-route to `memory_read` (description-steering works); `memory_search` is reorder-only recall-safe. Commit hash to be referenced in the next admin ride-along (per admin-rides-with-code).

### ADR-071 — reranked + recall-safe `memory_search`; `memory_read` is the primary answer path
**Context.** `memory_search` shipped as the raw `HybridRetriever` (BGE ∪ BM25 via RRF, no reranker) — our weakest ranking path, and the one agents reach for unpredictably. Live dogfood: correct answer ranked #4/10; at 100+ facts it falls out of the returned window entirely. An earlier in-session fix added the reranker WITH a drop-below-floor (so search could honestly return "nothing found"), but the 4-model dogfood proved a **false-empty is the WORST failure for a memory product**: a bare empty list is ambiguous ("wrong query" vs "not in vault"), so weak agents either ABANDON the vault for competing memory (Opus 4.6 went hunting the IDE's own brain) or SPIN re-querying for minutes (Flash on a no-signal query).

**Decision.**
1. **`memory_read` is THE primary answer path** — returns a structured DECISION (facts, or `abstain:true` + "do not fabricate"): unambiguous + actionable. Tool descriptions steer question-answering there.
2. **`memory_search` = reranked, recall-safe browse path** via new `RerankedRetriever` (wraps hybrid base → ADR-069 semantic recall-union → cross-encoder rerank → **REORDER-ONLY** → top-K). NEVER drops candidates → never a false-empty; empty ONLY when the base pool is genuinely empty. Reuses the warm reranker `Arc` from the read path (no second model load).
3. **Score domain:** reranker logit → `[0,1]` via sigmoid (`relevance_score`), strictly monotonic (rank order preserved), bounded so a returned result never reads as a negative/irrelevant score (the earlier `tanh` showed the correct cello as −0.95). Raw logit kept in `explanation`. Alpha-permitted wire change (V0.x; frozen at V1.0).
4. **Graceful fallback:** no reranker model configured → raw hybrid wired directly (recall-first preserved for lightweight deployments).

**Not chosen:** making search bulletproof (the reranker is brittle on terse keyword queries — adding "music" to "play" pushed the cello below any floor; only literal "cello" scored positive); removing search (weak models still reach for it, so it must be safe).

**Consequence / known limit (→ Option B):** on a true no-signal query, `memory_search` returns ranked-but-irrelevant facts with uniformly near-zero scores — the calling agent must judge. The clean "not in vault" signal lives in `memory_read.abstain`. Option B adds an additive hint to help agents judge search output without reintroducing a floor.

**Files (this commit):** `crates/vault-retrieval/src/reranked_retriever.rs` (NEW, +8 unit tests), `crates/vault-retrieval/src/lib.rs` (exports), `crates/vault-app/src/application.rs` (step 7b builds the reranker once, shared by search+read; step 8 wires search; step 9 reuses it), `crates/vault-mcp/src/server.rs` (tool descriptions: search de-cosine'd + "prefer `memory_read`" + "thin/empty = not in vault, don't rephrase"; read marked PRIMARY), `HANDOFF.md`.

### ✅ DONE: ADR-071 Option B — additive, separation-based `weak_match` hint on `memory_search`
**Goal:** give `memory_search` a recall-safe relevance signal so weak agents can tell "strong match" from "nothing really matched" — WITHOUT ever dropping or false-emptying (the floor we deliberately removed in ADR-071).

**The load-bearing design choice — separation, NOT absolute magnitude.** A naive `weak_match = top < floor` repeats the exact mistake ADR-071 removed: the reranker is brittle on terse queries and scores even a *correct* answer low (the cello scored only **0.047** on "instrument"). An absolute floor would mislabel that correct answer as weak. The reliable signal is how much the top *separates* from the pool: a real answer stands out (cello 0.047 vs runner-up 0.0027 ≈ **17×**) while a no-signal query is flat (blood-type 0.00020 vs 0.00016 ≈ **1.25×**). So `weak_match` keys on separation:
- `false` (strong) when top ≥ `STRONG_RELEVANCE` (0.5 = sigmoid(0), the reranker's own "yes" boundary — also guards two-genuinely-strong matches) **OR** top ≥ `SEPARATION_RATIO` (3.0) × the runner-up;
- `true` otherwise. A lone candidate counts as separated. Empty → weak.

**Wire change:** `memory_search` now returns `{ results: [...], top_relevance: f32, weak_match: bool }` (was a bare array). `results` is the FULL reordered set — the hint NEVER truncates it. Alpha-permitted (V0.x). Shahbaz approved the shape change before code.

**Why additive, not a floor:** a floor drops results → false-empty → the exact failure ADR-071 fixed. The hint preserves full recall (every candidate still returned) while restoring the judgment aid weak agents need. Belt-and-braces with the read-primary steering. The contract stays "never empty when candidates exist."

**Files:** `crates/vault-retrieval/src/search_hint.rs` (NEW — pure `search_hint()` + `SearchHint` + `STRONG_RELEVANCE`/`SEPARATION_RATIO` consts, **6 unit tests** tied to the live dogfood numbers), `lib.rs` (exports), `reranked_retriever.rs` (stale line-105 doc comment fixed: "drop-below-floor" → "reorder-only"), `vault-mcp/src/server.rs` (new `SearchResponse` wrapper, `handle_search` attaches the hint, `tool_search` serializes it, tool description teaches `weak_match`), `vault-mcp/tests/tool_invoke.rs` (wire-shape pin updated to the object + `top_relevance`/`weak_match` presence). `application.rs` unchanged (search goes through the unchanged adapter trait).

**Gates (incremental):** fmt ✅ / clippy --workspace -D warnings ✅ (14.96s) / build --workspace ✅ / test vault-retrieval ✅ (77 unit, +6) / vault-mcp ✅ / vault-app ✅.

**Live re-validated (Antigravity, Opus 4.6, `testeval`):** "instrument" → `top_relevance 0.047`, **`weak_match:false`** (cello #1, correct-but-low NOT mislabelled); "blood type" → `top_relevance 0.0002`, **`weak_match:true`** (no-signal correctly flagged). The separation heuristic distinguishes the two despite near-identical absolute scores.

### 🅿️ Carried follow-ups (not blockers)
- **REPORT_MISSING on `personal` boundary** — `memory_read`/`memory_search` return `health.status: degraded` with `no REPORT artifact exists for boundary 'personal'`. Cosmetic (facts still returned correctly). Clear with `vault-cli consolidate run` on `personal`. Offered this session; not yet run.
- ~~**Stale doc comment** `reranked_retriever.rs:105` ("drop-below-floor → top-K")~~ — ✅ FIXED in Option B ("reorder-only → top-K").
- **Antigravity `instructions.md` rewrite** (Shahbaz's local file, NOT in repo) — list BOTH tools, "prefer `memory_read`, empty = not in vault, don't rephrase forever."
- **`max_results` 10→5** — proven safe by the scorecard (10/10 @top-5) but kept at 10 (one change at a time). Revisit as its own change.

---

## 🟦 NEXT SESSION OPENER (2026-06-05 #4) [SUPERSEDED by the 2026-06-06 opener above — ADR-071 SHIPPED, all gates green, 3-model live-validated, committed; retained for the design-pivot detail] — **ADR-071 RERANKED `memory_search` (reorder-only, recall-safe) + `memory_read`-primary strategy. CODE WRITTEN, UNVERIFIED. NEXT: run gates → live-validate → commit.** — READ THIS FIRST

> Long session. We removed the flaky Kimi MCP server, ran a consolidation experiment (ruled out consolidation as the search-ranking fix), built the reranked-search fix, validated it on the real-model scorecard (green) — and then a **4-model live dogfood in Antigravity reversed a key design choice.** Code is revised to the new design but **the DoD gates have NOT been run and nothing is committed.** Pick up by running the gates.

### ⚠️ STATE: code written, UNVERIFIED, UNCOMMITTED
The working tree has uncommitted changes (below). **Gates fmt/clippy/build/test have NOT run on the final reorder-only code.** Do NOT assume green — run them first.

**Changed files (all uncommitted):**
- `crates/vault-retrieval/src/reranked_retriever.rs` — **NEW** `RerankedRetriever`: base hybrid → ADR-069 recall-union → cross-encoder rerank → **reorder-only (NEVER drops, never false-empty)** → top-K. Sigmoid `relevance_score()` maps logit → [0,1] (replaced tanh, which showed the correct cello as −0.95). 7 unit tests (reorder-only semantics incl. `never_returns_false_empty_even_when_all_score_negative` + explicit-threshold test).
- `crates/vault-retrieval/src/lib.rs` — exports `RerankedRetriever`, `SEARCH_CANDIDATE_FANOUT`, `RERANK_CANDIDATE_CAP`.
- `crates/vault-app/src/application.rs` — step 7b builds the reranker ONCE (shared by search+read); step 8 wires `memory_search` to `RerankedRetriever` (fallback = raw hybrid if no model); step 9 read pipeline reuses the shared reranker. (This wiring compiled green earlier; the reorder-only change is internal to the retriever so application.rs is unaffected.)
- `crates/vault-mcp/src/server.rs` — tool descriptions (the cross-platform steering lever): `memory_search` no longer claims "cosine" (now "0-1 relevance, reranked"), tells agents to PREFER `memory_read` for questions, query in natural-language phrases not keywords, and treat thin/empty results as "not in vault — don't keep rephrasing"; `memory_read` marked "THE PRIMARY TOOL"; `SearchToolParams` param docs de-cosine'd. (No existing test pins search/read descriptions — only write/update/delete are pinned in `initialize_smoke.rs`.)
- `HANDOFF.md` — this opener (admin, rides with the next code commit).

### ▶️ NEXT STEPS (in order)
1. **Run the gates** (laptop: confirm before `cargo build`, check disk; run BACKGROUND + serial fmt→clippy→build→test per [[no-parallel-cargo-invocations]] + [[cargo-on-windows-use-powershell]]). Test crates: **`-p vault-retrieval` (7 new tests), `-p vault-mcp` (descriptions changed — initialize_smoke), `-p vault-app`.** Note: tests use the tee-to-file pattern (`Tee-Object` + `Out-Null`) — read the tee file for results, the task output only keeps the exit line.
2. **Live-validate in Antigravity again** (Shahbaz drives; binary rebuilt by the build gate). Re-run the exact failing cases: terse `"instrument"` / `"music"` (must now return the cello reordered, **NOT `[]`**); `"blood type"` (must return a list, NOT empty → no 4-min spin). Confirm the score reads ~0.x (positive), not −0.95.
3. **If green: write ADR-071, commit** (confirm with Shahbaz per [[feedback-confirm-before-commit-push]]) + CI-green check.
4. **Write Shahbaz a better `instructions.md`** for Antigravity (his local file, NOT in repo): list BOTH tools incl. `memory_read`, "search the vault before answering questions about the user, prefer `memory_read`, empty = not in vault." The bare 2-liner he had omitted `memory_read` entirely.

### 🔑 WHY THE DESIGN CHANGED — the 4-model live dogfood (the load-bearing finding)
The real-model scorecard used FULL-SENTENCE queries ("what instrument does the user play") and was green. But real agents send TERSE keywords, and that exposed a fatal flaw in the first design (drop-below-floor = "honest nothing found"):

| Model | Tool chosen | Query style | Result |
|---|---|---|---|
| Gemini **Pro** (high) | `memory_read` | full question | ✅ **flawless, one-shot** (cello, abstain:false, structured) |
| Gemini **Flash** (small) | `memory_search` | terse keywords | `"instrument"`→`[]`, `"music"`→`[]`, `"play"`→cello, `"orchestra"`→cello |
| **Opus 4.6** | `memory_search` | `"instrument play music"`→`[]` → **ABANDONED the vault, went hunting the IDE's own brain**; only found cello when literal `"cello"` was in the query (logit +0.38) |
| Flash on `"blood type"` (true no-signal) | `memory_search` | **spun ~4 min, ~20 keyword variations, all `[]`** — force-stopped |

**Insights (durable — see memory [[project_memory_read_primary_search_recall_safe]]):**
1. **A false-empty from `memory_search` is the WORST failure for a memory product.** Its empty result is AMBIGUOUS — a weak agent can't tell "wrong query" from "not in vault", so it either abandons the vault for competing memory OR spins re-querying forever. It teaches the agent the vault is unreliable → it routes around us.
2. **The reranker is reliable on full-sentence queries but BRITTLE on terse keywords** — adding "music" to "play" pushed the cello BELOW the floor; only literal "cello" scored positive. It rewards literal token overlap, which the user's query rarely has. Our scorecard's full-sentence queries hid this.
3. **Tool choice tracks model capability:** stronger model → `memory_read` (great); weaker → `memory_search` (struggles).
4. **`memory_read` is the well-designed interface:** it returns a DECISION (structured facts OR `abstain:true` + "do NOT fabricate") — unambiguous + actionable. `memory_search` hands back raw results and makes the agent judge (weak agents fail at this).

**→ LOCKED (Shahbaz, this session): `memory_read` is THE primary retrieval tool (steer agents there via descriptions); `memory_search` stays as a recall-safe browse fallback (reorder-only — never false-empty). NOT chosen: making search bulletproof (reranker too brittle on terse queries), and NOT removing search (weak models still reach for it, so it must be safe).**

### 🧪 Consolidation experiment result (ruled out, this session)
Ran `vault-cli consolidate run` on the live `testeval` vault: **0 merges / 0 dedups / 0 contradictions / 0 archived** (already consolidated last session) but it DID write a fresh `testeval.report.json` (clears `REPORT_MISSING`). Confirmed: consolidation does NOT touch query-time ranking → the `memory_search` weakness is structural/upstream, exactly as predicted. (Note: `memory_read` on cross-boundary still warns `REPORT_MISSING` for `personal` — that boundary has no REPORT; run consolidate on `personal` too if we want it clean.)

### 🅿️ Deferred / follow-ups (not blockers)
- **`max_results` 10→5:** proven safe by the scorecard's 10/10 @top-5, but Shahbaz chose to KEEP 10 for now (one change at a time). Revisit as its own change.
- **Reranker brittleness on terse queries:** mitigated by reorder-only + steering to read, NOT solved. Live as a known limitation.
- **The green scorecard was on the OLD drop-floor code** — its search-recall numbers (10/10) reflect ranking, but the reorder-only revision changes abstain behavior; re-run it next session if we want fresh numbers (it now exercises reorder-only via `Application::new`).
- **Kimi MCP removed** this session (it kept breaking on its old client — protocol + DuckDB single-writer). `kimi mcp list` = empty. Not our bug; re-add only if old-client support becomes a goal.

---

## 🟦 NEXT SESSION OPENER (2026-06-05 #3) [SUPERSEDED by #4 above — its "search-recall design" task is now DONE (ADR-071); retained for the cross-agent dogfood detail] — **CROSS-AGENT DOGFOOD: Claude+Cursor+Antigravity all read the vault correctly; Kimi blocked by its OLD MCP client. NEW TOP PRIORITY: `memory_search` recall quality (agents use it + it ranks the right answer #4/10). NEXT: search-recall design.** — READ THIS FIRST

> Big hands-on dogfood session (Shahbaz drove four MCP clients). **Two fixes shipped + committed this session, BOTH CI-GREEN:** ADR-069 read recall-union (`a2cee13`, CI ✅ run 27007426640) + ADR-070 lazy reranker (`a3c938b`, CI ✅ run 27013044496, 48m). Then cross-agent dogfooded the live ~15-fact `testeval` vault.

### ✅ Cross-agent result (the product thesis, proven on real third parties)
| Client | Connect | Reads correctly | Tool used | Notes |
|---|---|---|---|---|
| Claude Desktop | ✅ | ✅ | **`memory_read` mostly + `memory_search` sometimes** | needs nudging to use vault |
| Cursor | ✅ | ✅ 5/5 | `memory_search` | auto-discovers from natural Qs |
| Antigravity (Google) | ✅ | ✅ | `memory_search` | auto-discovers; **shows raw tool I/O** |
| Kimi CLI | ⚠️ connects | ❌ | — | old MCP client rejects protocol `2025-11-25` + DuckDB single-writer |

Full detail + config paths + the corrected tool-choice analysis: **[[project_cross_agent_proven_on_cursor]]** (memory). ADR-070 lazy-reranker also live-validated here (handshake ~40s→~9s on every client; servers spawned at ~1.6 GB = BGE + reranker, warm-up confirmed).

### 🎯 THE REFRAMED TOP PRIORITY — `memory_search` recall quality
Antigravity surfaced the RAW `memory_search` output for query "instrument": the correct answer **"Plays the cello…" ranked #4 of 10**, BEHIND "vintage mechanical keyboards" (#1), "structural engineer" (#2), "fluent in Mandarin/English" (#3); RRF scores barely separated (0.0164→0.0156); no keyword match. The answer was still correct ONLY because the agent (recall-first design) picked cello from the list. **Why this is now #1:** agents pick BOTH tools unpredictably (per-agent AND per-query — Claude leans read, Cursor/Antigravity lean search); we do NOT control the choice; `memory_search` has our WEAKEST ranking (no reranker, no ADR-069 recall-union — those are `memory_read`-only). So whenever an agent picks search (constantly), they hit weak ranking. On the 15-fact vault the answer stays in the top-K window so it works; at 100+ facts it could fall OUT of the window → wrong/empty answer. **Decision B is no longer shelved "browse polish" — it's the top correctness item for the scale arc.** This is the [[project_bge_small_cannot_separate_relevant]] weakness, on the path agents actually use.

### ▶️ NEXT STEPS (search-recall arc)
1. ~~Verify CI green~~ — DONE: both `a2cee13` + `a3c938b` CI-green (verified session end). Working tree has only this HANDOFF opener uncommitted (rides with next code commit).
2. **Cheap experiment FIRST — does the consolidator move the ranking?** Run `vault-cli consolidate run` on the live vault, then re-run the Antigravity "instrument" query and compare the ranked list. **Prediction (Claude): NO change to relative ranking** — consolidation does dedup/contradiction/topics/REPORT, none of which touch query-time BGE/RRF ranking; the cello stays ~#4. Worth measuring to rule it out cheaply (Shahbaz's instinct) + it clears the `REPORT_MISSING` health warning. If prediction holds, ranking weakness is confirmed upstream of consolidation → needs a real fix.
3. **Design the search-recall fix** (has a real speed-vs-quality tension — search's value is being fast ~70-240ms vs read's ~17s reranked). Options to weigh: (a) bring the reranker + ADR-069 recall-union to `memory_search` (slow), (b) a lighter/faster reranker on the search path, (c) a stronger base embedder than BGE-small, (d) steer agents toward `memory_read` via MCP tool DESCRIPTIONS (cheap, untested, partial — fights speed). Likely a combination. Bring Shahbaz a short plan before code (contract-establishing, new arc → one plan iteration).
4. **Use Antigravity as the inspection window** — it's the only client that shows the raw `.md` instructions it reads + the raw `output.txt` it receives, so drive search-recall measurements through it to SEE the actual ranked list the agent gets (not just the final answer). Cursor/Claude hide the raw tool I/O.

### 🅿️ De-prioritized (confirmed not blockers)
- **Kimi / old-MCP-client support** — protocol-`2025-11-25` down-negotiation (rmcp) + DuckDB single-writer multi-connection. Every CURRENT client (Claude/Cursor/Antigravity) works, so this only buys laggard clients. Revisit only if old-client support becomes a goal. NOT our server's bug.
- Latency (#2) — agents lean on the fast search path; reranker latency (~17s on `memory_read`) bites less than feared. Still deferred.

### 🧾 Admin note
This opener (HANDOFF.md edit) is admin-only and is UNCOMMITTED — it rides with next session's first code commit per [[feedback_admin_changes_ride_with_code]]. Cross-agent MCP configs were written OUTSIDE the repo (`~/.cursor/mcp.json`, `~/.gemini/config/mcp_config.json`, Kimi `~/.kimi/mcp.json`) — not part of the repo, left in place for next session's continued testing.

---

## 🟦 NEXT SESSION OPENER (2026-06-05 #2) — **LAZY RERANKER (ADR-070): MCP handshake ~40s → ~9s, live-verified. DoD-GREEN, committed (`a3c938b`). [SUPERSEDED by the #3 opener above; retained for ADR-070 detail.]** — READ THIS FIRST

> Deferred the 1.2 GB reranker load off the MCP `initialize` handshake so MCP clients — Kimi CLI especially — connect without timing out. **Live Claude Desktop: handshake dropped ~40s → ~9s**; the background warm-up loads the model AFTER the transport binds (process settled at ~1.6 GB; first warm read returned the cello fact at rank 1 in 17.7s, `error=None`). **Honest scope note: the prior plan's "<1s" was NOT met** — measurement showed BGE + the keyword-index build are a *second* eager ~9s cost the plan didn't account for (the reranker was the biggest blocker at ~40s, not the only one). Both concrete goals (Kimi <40s connect, Claude 60s timeout) ARE resolved. **FIRST THING NEXT SESSION: confirm CI green for THIS commit AND the prior `a2cee13` (ADR-069), then re-run the Kimi independent validation** (it should connect now — see the 2026-06-04 evening opener below for the re-add command + the known DuckDB single-writer multi-client quirk).

### ADR-070 — lazy reranker load off the handshake path (SHIPPED in tree, committing this session)
**Decision:** wrap `Qwen3RerankerProvider` in a new `LazyQwen3Reranker` (`vault-embedding/src/reranker_lazy.rs`, implements `RerankProvider`) that loads the ~1.2 GB model lazily on first use via `tokio::sync::OnceCell` (the blocking `open` runs on `spawn_blocking`). Construction does ZERO disk I/O; `relevance_floor()` returns the `RERANK_NO_SIGNAL_FLOOR` constant WITHOUT loading — so nothing on the `initialize` handshake path can trigger the load. `Application::start_with_mcp` calls `spawn_warmup()` right after the stdio transport binds, so the model warms in the background off the critical path; the first read then pays inference only, not the load.
**Why:** the eager `Qwen3RerankerProvider::open` in `Application::new` ran BEFORE the server could answer `initialize` (~40s on CPU) → timed out Kimi CLI's connect retries (<40s) and sat close to Claude Desktop's 60s init window. The reranker is only needed at read time, never for the handshake.
**Wiring:** `application.rs` (~line 320) swaps the eager `Arc::new(Qwen3RerankerProvider::open(...)?)` for `Arc::new(LazyQwen3Reranker::new(...))` (infallible, no `?`); one clone coerces to `Arc<dyn RerankProvider>` for the pipeline, the concrete handle is kept on a new `Application.reranker_warmup: Option<Arc<LazyQwen3Reranker>>` field for the serve-path warm-up. Both share the same `OnceCell`. NO change to `StructuredReadPipeline`.
**Integrity-timing note (security):** the model's SHA-256 integrity check (ADR-020) moves from startup to first-load. Verify-before-use is preserved (it still runs before the model produces any result); only the timing shifts startup → first-read/warm-up. Not a weakening — recorded as an explicit decision. A corrupt/missing model now surfaces at first read instead of at launch (for a local single-user tool, the moment the user would notice anyway).
**Live result (Claude Desktop, 2026-06-05, MCP log ground truth):** `initialize` request 11:34:01.624Z → response 11:34:10.779Z ≈ **9.1s** (was ~40s+); process resident **~1.6 GB** (BGE + reranker both loaded → warm-up confirmed); forced `memory_read "what instrument does the user play"` → **cello fact at rank 1**, `result_count=1`, `error=None`, `duration_ms=17657` (warm inference, not cold load).
**Gates:** fmt ✅ · clippy --workspace --all-targets -D warnings ✅ 0 · build --workspace ✅ · test -p vault-embedding ✅ (13 lib incl. 4 NEW lazy tests: floor-without-load, no-disk-construction, empty-docs-no-load, error-surfacing-on-bogus-path) · test -p vault-app ✅ 58.
**Tech debt / follow-up (logged, NOT chased):** handshake is ~9s, not <1s — the residual is BGE model load + keyword-index build, still eager in `Application::new` before `serve()` binds. Defer those off the handshake only if true sub-second is needed; BGE is used by read + search + write-path embedding, so lazy-loading it has a wider blast radius and low payoff at V0.2 beta (~9s is comfortably within every client's timeout).

### 📦 WHAT THE ADR-070 COMMIT CONTAINS (2026-06-05 #2)
- `crates/vault-embedding/src/reranker_lazy.rs` — **NEW**: `LazyQwen3Reranker` + module docs (why/how/integrity-timing) + 4 unit tests + 1 real-model `#[ignore]` parity test.
- `crates/vault-embedding/src/lib.rs` — `pub mod reranker_lazy;` + `pub use reranker_lazy::LazyQwen3Reranker;`.
- `crates/vault-app/src/application.rs` — eager→lazy swap; new `reranker_warmup` field; `spawn_warmup()` trigger in `start_with_mcp` after the transport binds; import `Qwen3RerankerProvider`→`LazyQwen3Reranker`.
- `HANDOFF.md` — this opener (admin, rides with the code per [[feedback-admin-changes-ride-with-code]]).

---

## 🟦 NEXT SESSION OPENER (2026-06-05) — **SCALE TEST + read recall-union fix (ADR-069). DoD-GREEN, committed (`a2cee13`). [SUPERSEDED by the 2026-06-05 #2 opener above; retained for the ADR-069 CI-verification + the ADR-069 detail.]** — READ THIS FIRST

> Built the first **"prove correctness at scale"** harness, found a real recall bug at 100 facts, fixed it (ADR-069), and validated the fix three ways (unit tests + 100-fact scorecard + live Claude Desktop). **2026-06-05: all five DoD gates ran green and the work was committed** (ADR-069 fix + scale harness + this HANDOFF together). **FIRST THING NEXT SESSION: confirm this commit's CI run went `success`** (`gh run list --workflow=ci.yml -L 1`) before staging anything new ([[broken-ci-is-regression-not-techdebt]]). Then pick up at "▶️ NEXT STEPS" step 2 (lazy-reranker).

### 📦 WHAT THIS COMMIT CONTAINS (2026-06-05)
The files below were the working-tree state; all are now committed. Changed/new files:
- `crates/vault-retrieval/src/structured_read_pipeline.rs` — **ADR-069 fix**: new `union_semantic_recall()` called in `read()`'s reranker branch; `RERANK_CANDIDATE_CAP` raised `DEFAULT_MAX_CANDIDATES` → `2 * DEFAULT_MAX_CANDIDATES`; `semantic` field doc updated (dual role); 2 new unit tests (`union_semantic_recall_rescues_keyword_starved_fact`, `union_semantic_recall_dedups_overlap`) — both PASS (`cargo test -p vault-retrieval --lib union_semantic_recall` green).
- `crates/vault-app/src/application.rs` — wires `.with_relevance_gate(semantic.clone())` onto the reranker path so the pipeline has the semantic channel for the union.
- `crates/vault-app/tests/scale_eval.rs` — NEW scale harness: `scale_correctness_eval` (the scorecard, `#[ignore]`) + `subject_frame_depth_probe` (fast BGE-only diagnostic, `#[ignore]`). Ported t029 distractor generator (duplicated by design; rule-of-three not yet hit).
- `crates/vault-app/tests/fixtures/scale_eval.json` — NEW: 22 planted facts (10 synonym-gap recall targets + near-miss must-excludes + 2 guard facts) + 15 queries; `scale: 100`.
- `.gitignore` — added `.playwright-mcp/`.
- `HANDOFF.md` — this opener (admin, rides with the next code commit).
- (Outside repo) `C:\Users\shahb\.kimi\mcp.json` — was registered for the cross-agent test, then **removed at session end** at Shahbaz's request (`kimi mcp list` = empty). Re-add next session (command below).

`target/` was fully `cargo clean`ed mid-session (disk was at 20 GB free / 96%; now ~133 GB free). **First build next session is a full ~36-min rebuild.**

### ADR-069 — read recall-union: feed the reranker a hybrid ∪ semantic candidate pool (SHIPPED in tree, UNCOMMITTED)
**Decision:** `StructuredReadPipeline::read` (reranker path) unions the semantic channel's top-`max_candidates` onto the hybrid (RRF) hits, deduped by id, before reranking. `RERANK_CANDIDATE_CAP = 2 * DEFAULT_MAX_CANDIDATES` so the union isn't truncated. No-op when no semantic channel is wired (unit tests / no-reranker fallback unaffected).
**Why:** At 100 facts the hybrid's RRF fusion **starves a strong pure-semantic match**. Measured via `subject_frame_depth_probe`: the subject-less cello ranked **pure-BGE #6/100** but fell **>20/100 in the hybrid** — because every "The user …" fact earns a 2nd RRF term from the incidental "user" keyword overlap, while the keyword-less cello gets only its semantic term. It dropped below the top-20 cap, so the reranker (the relevance authority) never saw it → `memory_read` mis-abstained on a fact the vault holds. (First hypothesis — "BGE can't embed subject-less facts" — was FALSIFIED by the probe; measure before fixing paid off again, cf. [[feedback-source-read-call-graph-upstream-of-empirical]].)
**Result:** scorecard read recall **9/10 → 10/10**, cello at read rank 1, false-abstain still **0**, no new false-answers, no regressions. Live-confirmed in Claude Desktop.
**Scales:** semantic top-N always contains strong matches regardless of vault size, so this holds at 1000+ (unlike just raising the cap). Reranker cost grows to ≤2× candidates (latency deferred, post-correctness).

### Validation (3 ways, all green for READ)
- **Unit:** `cargo test -p vault-retrieval --lib union_semantic_recall` → 2 passed.
- **100-fact scorecard** (`cargo test -p vault-app --test scale_eval scale_correctness_eval -- --ignored --nocapture`, real BGE + real Qwen3-reranker): read **10/10 recall, 8→9/10 at rank 1, 0 false-abstain, 2 false-answers** (salary→vendor-invoices, cat→dog — see Decision A), search 9/10@top-20 (cello still missing → Decision B).
- **Live Claude Desktop** (rebuilt `target\debug\vault-cli.exe`, 15 facts seeded into `testeval`, fresh session, forced tool use): Q1 instrument→**cello surfaced, abstain:false** ✅; Q2 cat→vault returned dog, **Claude said "no cat"** ✅; Q3 salary→vault returned 4 vendor invoices, **Claude said "no salary"** ✅; Q4 blood type→**clean abstain** ✅.

### 🟢 Decision A — SETTLED (this session)
Recall-first read's internal "false-answers" (returning a topically/lexically adjacent fact instead of abstaining) are **harmless at the user layer** — a competent agent (Claude, live) correctly declines every time, across BOTH failure channels (semantic-adjacency: cat→dog; BM25 lexical-overlap: salary→"annual" invoices). **DECISION: leave the −2.5 no-signal floor alone.** Tightening it to kill the false-answers would re-introduce the recall cliff (the genuinely dangerous failure — a vault that hides facts it holds). The −2.5 floor is now considered validated-by-agent, not just provisional. See [[project-correctness-is-the-product]] + [[feedback-structured-contract-user-sees-via-agent]].

### ▶️ NEXT STEPS (do in this order)
1. ~~**Finish DoD + commit ADR-069 + the scale harness.**~~ **DONE 2026-06-05.** All five gates green (fmt ✅ · clippy --workspace --all-targets -D warnings ✅ 0 · build --workspace ✅ · test -p vault-retrieval ✅ 63 · test -p vault-app ✅ 58). One clippy `doc-lazy-continuation` nit in the ADR-069 doc-comment fixed (no logic change). Committed (code + scale harness + this HANDOFF together) + pushed per [[feedback-commit-only-with-tested-fix]]. **→ FIRST THING NEXT SESSION: verify the CI run went `success` before staging anything new.**
2. **Lazy-load the reranker for MCP-client compatibility (CHOSEN next task).** Today the MCP server loads the 1.2 GB Qwen3 reranker BEFORE answering `initialize` (~40s). This (a) blocks Kimi CLI from connecting (its connect patience < 40s on retries) and (b) is the same risk behind the known Claude-Desktop 60s-init-timeout note. Fix: load the reranker lazily (after `initialize`, on first `memory_read`) so the server answers the handshake in <1s. First read pays the load (within the 60s tool timeout). Re-verify Claude Desktop still reads fine.
3. **Re-run the Kimi independent validation** (after step 2). Kimi server was **removed at session end** — re-add it first: `kimi mcp add --transport stdio memory-vault -- "<target\debug\vault-cli.exe>" <same vault-db/vector-dir/graph-db + mcp --bge-* --ort-lib --rerank-* --boundary personal --boundary testeval serve args as the Claude Desktop config>` (use PowerShell `--%` stop-parsing so the paths pass verbatim; verify with `kimi mcp test memory-vault`). Launch pattern: `cd $HOME; $env:PYTHONUTF8=1; kimi` (UTF-8 avoids a cp1252 glyph crash in Kimi's output). Ask the same 4 questions. **Known Kimi quirk:** it spawns TWO connections (main agent + sub-agent), and our DuckDB graph store is single-writer → the 2nd spawn fails ("File is already open"). Lazy-load helps the timeout half; the single-writer/multi-client half may need a follow-up (open graph DB read-tolerant, or a single-connection Kimi config). This is a real V0.x **multi-client** finding — log it.
4. **Decision B — `memory_search` recall (BACKLOG, low priority).** The cello is still missing from `memory_search` top-20 (search has no reranker, so ADR-069 doesn't reach it). Read is the answer path (Claude used `memory_read` for all 4 live questions); search is browse. Fix only if we want browse 10/10 — would mean touching the shared `HybridRetriever` fusion (wider blast radius). Defer.
5. **Continue the scale arc:** 1000-fact stretch run (`scale_eval.json` `scale` field → 1000; expect slower BGE seed + reranker); calibrate the 0.70 topic floor (ADR-068, still provisional). The −2.5 floor is now settled (Decision A).

### Live-test infra notes (reuse next session)
- Vault data dir: `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\` (`vault.db*` + `graph.duckdb*` + `lance/` + `reports/`; keep `models/`). Wipe for clean slate with Claude Desktop + Kimi BOTH closed (single-writer locks). This session's 15 test facts are in `testeval` — wipe before any consolidate/clean run.
- MCP server binary = `target\debug\vault-cli.exe` (rebuild with the fix BEFORE any live test — Desktop/Kimi must be closed so the .exe isn't locked).
- Claude Desktop config `%APPDATA%\Claude\claude_desktop_config.json` (reranker + bge wired, boundaries `personal`+`testeval`). MCP log (ground truth) = `%APPDATA%\Roaming\Claude\logs\mcp-server-memory-vault.log`.
- Kimi log = `C:\Users\shahb\.kimi\logs\kimi.log` (rust server tracing is interleaved as `ERROR | threading:run`). Kimi server tracing has no separate clean log — read `kimi.log`.
- **Single-writer:** never run two MCP clients (Claude Desktop + Kimi) against the vault at once. Check `Get-Process vault-cli` before launching (don't guard on `Get-Process Claude` — matches Claude Code too).

---

## 🟦 NEXT SESSION OPENER (2026-06-04) — **`memory_search` RECALL-FIRST (ADR-067) + topics K-means→connected-components (ADR-068) + fixture grown. DoD-GREEN + FULL §7 DOGFOOD GREEN, committed+pushed.** — HISTORICAL

> This session closed three of the four rough edges queued by the 2026-06-02 opener — in one build batch (per Shahbaz's "don't build again and again" direction): **#1 `memory_search` empty**, **#3 K-means topic mislabels**, **#4 grow the calibration fixture**. **#2 latency stays deferred** (post-correctness, locked). All five DoD gates green AND the **full §7 live dogfood ran GREEN in Claude Desktop** before commit.
>
> **§7 LIVE DOGFOOD (2026-06-04, real BGE + real Phi-4, clean testeval slate) — FULLY GREEN.** Part A: C4 ✅ (100K-char store intact, automated), C5/C6/C8/C9/A6/I2/C10/C11 ✅, **C7 `memory_search "C7UNI"` → 2 hits in 226ms (was `[]` in 2ms — ADR-067 proven live)**, A7 ×2 ✅. Consolidation: `candidate_pairs=1 → stale_count=1` (only older Tesla invalidated, ADR-051 `valid_until`), `clusters deduped: 2, memories deduped: 3, memories_merged: 0` (DEDUPDOG ×3 + C9NORM ×2 collapsed, zero LLM, ADR-063 `superseded_by`), **REPORT: 8 distinct correctly-attached topics — job→`job_title`, dog→`pet_labrador`, cello→`community_orchestra`, cars→`electric_vehicle_ownership` (ADR-068 proven live; the K-means mislabel failure mode is gone)**. Part B: B1 only-Rivian ✅, B2 Tesla recoverable ✅, B3 PRECJOB+cello+PRECFOOD all survive ✅, B4 Biscuit-once + 2 superseded ✅. Topic-label calibration caveat stands (tiny single-boundary set; not a substitute for the labeling eval). Committed + pushed after the run.

### ✅ CI for `76ffc9b` — GREEN (verified end of session 2026-06-04)
Commit **`76ffc9b`** (pushed to `main`) is **CI-green** (run `26948896661`, all jobs success across the ubuntu/windows/macos matrix). NOTE: the *first* attempt failed on `ubuntu build --all-targets` with `collect2: ld terminated with signal 7 [Bus error], core dumped` — a **transient runner link-stage resource crash**, NOT a code defect (every crate compiled; the failing link targets were `vault-cli`/`vault-mcp` test bins we never touched; clippy-ubuntu + win + mac all green). A `gh run rerun --failed` cleared it. **If this recurs** (the extra `search_recall_first` test binary may nudge ubuntu's parallel-link memory), the fix is a CI-config tweak (lower link parallelism / `cargo build --all-targets -j1` on ubuntu, or split the build), NOT a source change. The weekly `real-model smoke` cron is separately still red on the pre-existing Phi-4 GGUF download race (fix: `--test-threads=1` on the 3 smoke tests; deferred).

### 🔍 ROOT CAUSE (grounded in the live MCP log, not the handoff phrasing)
`memory_search` was **not** empty across the board. The 2026-06-02 MCP log shows multi-token queries WORKED (`finesse résumé café`→2 hits in 72ms, `CAP_OK_1`→2 in 161ms); only **single-token / no-lexical-overlap** queries returned `[]` — and they returned in **~0-2ms**, the signature of the BM25 abstain gate (`AbstainingRetriever`) short-circuiting BEFORE the ~70ms semantic channel ran. Live proof: at 17:50 `memory_read "favorite color?"` surfaced "amber", but `memory_search "amber"` 90s later returned `[]` in 2ms. Same bug class as the read Bug-2 — a hard BM25 gate eating semantically-findable facts. (`secretwork`-style empties in the log were CORRECT — those were rejected/never-stored writes.)

### ✅ SHIPPED THIS SESSION (committed `76ffc9b` + pushed; DoD-green + full §7 dogfood green)
- **ADR-067 — `memory_search` recall-first.** `vault-app/src/application.rs`: search wired to the RAW `hybrid` (semantic+keyword RRF), dropping the `AbstainingRetriever` BM25 gate (mirrors the read ADR-066 stance). BM25 still contributes to RRF ranking; it is no longer a hard gate. `AbstainingRetriever` is now production-unreferenced (kept public + unit-tested in vault-retrieval; tech-debt to remove). New test `vault-app/tests/search_recall_first.rs` (real BGE, `#[ignore]`): `"food"`→Japanese-cuisine + single-token `"amber"`→favorite-color. **Ran live: 1 passed (11.5s).**
- **ADR-068 — topic labels: K-means → connected-components.** `vault-consolidator/src/topics.rs`: replaced K-means (which force-bucketed N facts into a fixed K and mislabeled unrelated ones — live: a job fact + a dog fact both tagged `vehicle_transitions`) with top-K cosine connected-components over the union-find DSU (`union_find_components` made `pub(crate)`). Floor `TOPIC_NN_SIMILARITY_FLOOR = 0.70`, K=5; singletons kept as honest per-fact topics. Reworked the K-means-specific unit test + added `discover_topics_groups_related_pair_and_isolates_unrelated_facts` pinning the fix. Display-only — never affects read recall.
- **Fixture grown (#4).** `vault-retrieval/tests/fixtures/read_quality_eval.json` 18→24 cases: +4 true-negative guards against POPULATED vaults (the "does the −2.5 floor still abstain as distractors grow" worry) + 2 synonym recall cases. The eval harness scores `cases.len()` dynamically. The actual −2.5 + 0.70 re-measurement is a real-model eval run (the `#[ignore]` `read_quality_eval` / weekly smoke) — flagged, not done this session.
- **Gates:** `fmt` ✅ · `clippy --workspace --all-targets -D warnings` ✅ 0 · `build --workspace` ✅ 0 · `test -p vault-consolidator` ✅ (100 unit + integration, incl. real-BGE report_generation) · `test -p vault-app` ✅ (58) · `search_recall_first --ignored` ✅ (real BGE).

### ADR-067 — `memory_search` recall-first: hybrid candidates, no hard BM25 gate (SHIPPED, uncommitted)
**Decision:** `memory_search` returns the raw `HybridRetriever` (BGE semantic + Tantivy BM25 fused by RRF) candidates; the `AbstainingRetriever` top-1 BM25 gate is removed from the production search path.
**Why:** The BM25 gate returned `[]` whenever the lexical top-1 scored below 1.0, short-circuiting in ~0-2ms before the semantic channel — so single-token (`amber`, `C7UNI`) and no-lexical-overlap queries dropped facts the vault holds and surfaces via `memory_read` (live MCP log, 2026-06-02). Recall-first matches the read fix (ADR-066) and what Mem0/Letta/Zep do: return ranked candidates, let the calling agent judge.
**Mechanics:** `application.rs` step 8 = `hybrid.clone()` instead of `AbstainingRetriever::new(hybrid, keyword)`. `keyword` is moved into the hybrid (step 7); the import is dropped. Search now never hard-abstains (semantic always returns nearest neighbors on a non-empty vault); scores let the agent judge.
**Live-confirmed:** `search_recall_first.rs` real-BGE test green — `"food"`→cuisine, `"amber"`→color.
**Follow-up:** remove the now-unreferenced `AbstainingRetriever` (+ `AbstainConfig`) in a focused cleanup; it stays for now (no drive-by).

### ADR-068 — topic discovery by connected-components, not K-means (SHIPPED, uncommitted)
**Decision:** `discover_topics` groups a boundary's facts into topics by connected components over the cosine-similarity graph (per-fact top-K neighbors ≥ floor → union-find transitive closure), replacing K-means. Singletons are kept as their own topic.
**Why:** K-means is *forced* to fill a fixed K buckets and cannot leave a fact ungrouped, so on small/diverse vaults it jams unrelated facts together and mislabels them (live §7: PRECJOB + Biscuit → `vehicle_transitions`). Same false premise that retired K-means from contradiction detection (ADR-065). Topic membership is a similarity question, not a partitioning one.
**Mechanics:** `TOPIC_NN_TOP_K = 5`, `TOPIC_NN_SIMILARITY_FLOOR = 0.70` (provisional — reuses the measured 0.634-noise / 0.823-related gap from the contradiction spike; topic breadth is fuzzier, so re-measure on real fill data). Reuses `phases::cluster::union_find_components` (now `pub(crate)`). Display-only — REPORT topic labels; never gates read recall. The `n < 3 → "general"` short-circuit is preserved.
**Verified:** topics.rs unit tests (incl. new dogfood-scenario test) + real-BGE `report_generation` green.

### 🎯 NEXT ARC — PROVE CORRECTNESS AT SCALE + CALIBRATE (chosen with Shahbaz, 2026-06-04)

**Where we are:** the correctness core (recall-first read ADR-066 + search ADR-067, A5 contradiction ADR-065, dedup ADR-063, topics ADR-068, honest abstention) is **structurally solid and live-dogfood-proven** — the existential "do we have a product" risk is resolved. BUT every proof so far is on a **tiny ~12-fact, single-boundary, hand-curated vault**, and the two newest floors (0.70 topic, −2.5 no-signal) are **provisional guesses, not calibrated on real data**. So the core is *correct on toy data* — NOT yet proven *correct at scale*. This arc earns (or refutes) the "core ready" badge before we spend on latency / sync / distribution. Shahbaz picked this over latency/sync/distribution because it's the cheapest way to de-risk the founder thesis ("correct output every time") and it directly produces the floor calibration we owe.

**The arc, in three workstreams:**
1. **Scale test + scoring harness.** Seed a realistic large vault — 100 then 1,000 facts across several boundaries — with *planted* hard cases (contradictions, near-duplicates, synonym-gap questions, genuine no-signal questions). Run read / search / consolidation over it and SCORE: does the right fact surface? false-abstain (the cliff)? false-answer (over-correction)? does the consolidator over-retire or miss contradictions at scale? **Build on what exists:** `crates/vault-retrieval/examples/t029_scale_1000_retrieval_diagnostic.rs` (1K-scale diagnostic) + the `read_quality_eval` harness (`crates/vault-app/tests/read_quality_eval.rs`, `recall_first_go_no_go`) + the 24-case fixture. Not starting from zero.
2. **Calibrate the two provisional floors on that data.** −2.5 no-signal floor (ADR-066): as distractors pile up, does it still abstain on junk AND surface real synonym matches? 0.70 topic floor (ADR-068): does it group related facts without over-merging once there are genuinely many topics? Measure, adjust, pin.
3. **Fix whatever breaks.** It will break somewhere — each break is a real correctness gap we'd otherwise ship to beta blind.

**▶️ CONCRETE FIRST STEP (do this first next session):** *design* the scale fixture + scoring harness — what facts, what planted cases, what correctness properties we assert (= the definition of "correct at scale") — and bring Shahbaz a short plan to review BEFORE writing it (contract-establishing; new arc warrants one plan iteration per [[feedback-plan-iteration-depth-scales-with-design-surface]]). Then implement → run → scorecard → calibrate.

**Memories to load:** [[correctness-is-the-product]], [[anchor-on-measured-not-projected]], [[project-read-relevance-conformal-not-absolute-threshold]], [[bge-small-cannot-separate-relevant]], [[reference-mcp-dogfood-log-is-ground-truth]], [[confirm-before-cargo-build-and-check-disk]], [[feedback-confirm-before-commit-push]].

### 🅿️ BACKLOG (deferred, NOT this arc — revisit after scale-proof)
- **Latency (#2, locked post-correctness)** — read ~4-46s on CPU (Qwen3 reranker is the cost); search now runs the hybrid (~70-240ms, fine). Options: int8 reranker quant / GPU / DirectML ORT EP / 4B-on-GPU for managed mode. The biggest *beta-UX* blocker once correctness-at-scale is proven.
- **Cross-device sync** — the big remaining V0.2 feature (zero-knowledge encrypted). Survey existing sync scaffolding before scoping.
- **Distribution / onboarding** — get it to the first beta users (alpha-distribution task).
- **Remove `AbstainingRetriever`** (+ `AbstainConfig`) — production-unreferenced after ADR-067. Focused cleanup; check vault-retrieval re-exports + tests.
- **Re-dogfood `memory_search`** single-token cases live (already unit+live-proven via C7 this session; lowest priority).

### Tech debt logged
- `AbstainingRetriever` / `AbstainConfig` production-unreferenced (ADR-067) — remove in a focused pass.
- 0.70 topic floor + −2.5 no-signal floor both provisional — **this is now the NEXT ARC's calibration target.**
- Carried: N-ary contradiction fallback unreferenced; Phase 2b invalidations not summed into `contradictions_resolved` (cosmetic).

---

## 🟦 NEXT SESSION OPENER (2026-06-02 evening) — **[HISTORICAL — superseded by the 🟦 2026-06-04 opener above. Rough edges #1 (`memory_search`), #3 (K-means topics), #4 (grow fixture) are now CLOSED via ADR-067 + ADR-068 + fixture growth, DoD-green. #2 latency still deferred.]** RECALL-FIRST READ SHIPPED (ADR-066), §7 DOGFOOD FULLY GREEN. NEXT: fix `memory_search` (same recall-first fix) + 3 rough edges. — READ THIS FIRST

> This session resolved the read over-abstention that nearly ended the project. Root cause was architectural, not a model gap: the 0.6B reranker was being used as a strict *precision authority*, and it provably **cannot** separate hard synonym leaps (e.g. allergy→"avoid", settled→"home") from ambiguous guards at any single floor (measured: interleaved, gap −0.33 even at the v5 instruction; the 4B fixed quality but is ~80s/read on local CPU — unusable, see ADR-066). **Fix = recall-first read:** the reranker re-orders + applies only a coarse *no-signal* floor; the calling agent does fine relevance. Deep-research confirmed this is what Mem0/Letta/Zep all do (return top-k, let the agent judge — they don't gate locally). The full §7 dogfood ran GREEN on real Phi-4 in Claude Desktop. All older openers are HISTORICAL.

### ⏳ FIRST THING NEXT SESSION — verify CI
The recall-first commit was pushed to `main`; confirm its CI run went `success` (`gh run list --workflow=ci.yml -L 1`) before staging anything new ([[broken-ci-is-regression-not-techdebt]]). Note the weekly **`real-model smoke` cron** was red on a *pre-existing* flaky reason (Phi-4 GGUF download race on the fresh runner — `phi4_mini_smoke.rs` runs 3 tests that race to download the same 2.49 GB file; 1 passes, 2 fail with "cannot find the file specified"). NOT caused by this arc. Fix when convenient: run those 3 smoke tests with `--test-threads=1` in the CI workflow so the first downloads and the other two cache-hit. (Diagnosed 2026-06-02; deferred — it's the weekly cron, not the push gate.)

### ✅ SHIPPED + DOGFOOD-CONFIRMED THIS SESSION
- **Recall-first read (ADR-066).** `crates/vault-embedding/src/reranker.rs`: `QWEN3_RERANKER_INSTRUCT` → v5 synonym-aware wording; `RERANK_RELEVANCE_FLOOR` (0.0, precision) **renamed** `RERANK_NO_SIGNAL_FLOOR` (**-2.5**, coarse no-signal cut). `structured_read_pipeline.rs::apply_reranker` now recall-first (re-order + drop only deep junk). `lib.rs` re-export updated. Pins: `no_signal_floor_is_pinned`.
- **Regression (real models, `#[ignore]` weekly smoke):** `read_no_keyword_overlap.rs::synonym_gap_reads_surface_recall_first_and_still_abstain_on_no_signal` — food→cuisine + languages→fluent + pet→dog surface, blood-type abstains. GREEN.
- **C4 closed:** `vault-app/tests/content_ceiling.rs` — 5K/10K/50K/**100K** all store INTACT via the real `Adapter::write` path (read back from metadata by id; +1 trailing-period normalization only, zero truncation). The test Desktop could never run, now automated + permanent.
- **Calibration assets:** `read_quality_eval.json` grew +10 synonym-gap cases (food/commute/pet/occupation/drink/age/exercise/home/language/avoid), each with a hard same-topic guard. `read_quality_eval.rs::recall_first_go_no_go` (raw-BGE pooled recall probe). `reranker_fun_diagnostic.rs` temp eval scaffolds removed (conformal instrument kept).
- **§7 dogfood (real Phi-4, Claude Desktop, 2026-06-02) — FULLY GREEN:** Part A C4–C11 ✅, A6 abstain ✅, A7 cello ×2 ✅; B1 drive-now→only-Rivian ✅, B3 work→PRECJOB ✅, **B3 food→PRECFOOD ✅ (the headline)**, B4 dog→Biscuit-once ✅. Consolidation: `candidate_pairs=1, stale_count=1` (only Tesla invalidated), `memories deduped: 3` (2 DEDUPDOG + 1 C9NORM) with `memories_merged: 0` (zero LLM).

### 🟦 NEXT SESSION — FIX THESE (priority order)

**1. `memory_search` returns EMPTY across the board — TOP PRIORITY.** Live this session: `memory_search "amber"` (a clean lexical hit `memory_read` surfaced one call earlier) returned `[]`; so did every other search incl. C7's `"C7UNI"`, B2's `"Tesla Model 3"`, B4's `"Biscuit"`. `memory_read` worked throughout. **Root cause is almost certainly the SAME class we just fixed for read:** `memory_search` still routes through the `AbstainingRetriever` BM25 **keyword gate** (`crates/vault-app/src/application.rs` ~line 280 — search keeps the gate; read was switched to raw hybrid in the Bug-2 fix). When the BM25 leg doesn't hit (or the Tantivy index lags the write), the gate returns empty even though the data is present and semantically findable. **Two hypotheses to disambiguate first (Desktop-Claude flagged both): (a) the keyword/BM25 gate abstaining; (b) boundary-scoping or embeddings-not-indexed-on-write** (every `memory_read` came back `boundary:null` = multi-boundary; confirm which boundary `search` queries + whether the vector/Tantivy index serves same-session writes). **Fix = apply recall-first to search too:** drop the hard BM25 gate, return the hybrid (semantic-backed) ranked candidates. Pin with a real-model test. This is a CORE tool broken — high impact.

**2. Read latency ~4–70s on CPU (Qwen3 reranker).** First read after model load was ~70s; warm reads ~4–12s. The reranker on CPU is the cost. Post-correctness workstream (locked stance: correctness before latency). Options: int8 reranker quant; GPU/DirectML ORT EP; or (managed mode) the 4B on a GPU server. Decide per deployment mode (the 4B-on-GPU is the managed-mode quality lever per ADR-066).

**3. K-means topic mislabels (cosmetic, display-only).** Live: PRECJOB + Biscuit both got topic `vehicle_transitions`; PRECFOOD correctly `japanese_cuisine`. Does NOT affect read correctness (the right facts surfaced regardless). Folds into the deferred clustering rework (threshold connected-components / HDBSCAN). Same rough edge carried from last session.

**4. Re-validate the -2.5 no-signal floor + grow the labelled fixture as the vault fills.** -2.5 was calibrated on a small (16-case) fixture: real should-surface facts ≥ -0.35, true no-signal ≤ -4.4, so -2.5 sits in the gap (recall-leaning). As production fills with more distractors, re-measure (the conformal instrument + `recall_first_go_no_go` are the tools) and adjust if real facts start scoring lower. Capture labelled (query→relevant/guard) judgments in prod to feed this.

**5. C4 upper bound (low priority).** 100K confirmed intact; the true max + clean-reject-above-ceiling behaviour untested. Extend `content_ceiling.rs` (e.g. 500K/1M) if a real need surfaces.

### ADR-066 — recall-first read: reranker as re-orderer + no-signal floor, not precision authority (SHIPPED)
**Decision:** The read relevance gate is recall-first. The Qwen3 reranker (1) re-orders retrieved candidates and (2) drops only candidates below a coarse **no-signal** floor (`RERANK_NO_SIGNAL_FLOOR = -2.5`); everything else is returned to the calling agent, which makes the fine relevance judgment. Supersedes the ADR-057-amendment precision floor (logit 0).
**Why:** The 0.6B cross-encoder provably cannot be a precision authority on this data — measured 2026-06-02 over a grown 16-case synonym fixture: real should-surface logits and ambiguous-guard logits INTERLEAVE (min-real -0.35 < max-guard +2.44; gap -0.33) even with a v5 synonym-aware instruction, so NO single floor separates them (conformal calibration confirmed dead). The 4B reranker DOES separate (16/16 recall at floor 0) but costs ~80s/read on local CPU (8GB f16) — unusable in local/BYOK mode; viable only on GPU (managed mode). Deep-research (Mem0/Letta/Zep/Cognee) found none of them run a local cross-encoder precision gate — they return top-k and let the calling LLM judge; our over-abstention was a self-imposed consequence of gating. The vault's real moat (zero-knowledge, A5 contradiction/recency, dedup, provenance) is unaffected.
**Mechanics:** instruction → v5 ("answer yes even when the question and fact use different words for the same idea"); floor 0 → -2.5 (real ≥ -0.35, no-signal ≤ -4.4, recall-leaning). `apply_reranker` filters at the no-signal floor + re-sorts. The no-signal floor PRESERVES A6 abstention (the reranker scores true no-signal deeply negative).
**Live-confirmed:** §7 dogfood fully green incl. the headline food→cuisine surface + A6 abstain. **4B model + integrity-bypass eval seam removed** (not shipped — security surface); findings live here + in the transcript.
**Follow-up:** apply the same recall-first fix to `memory_search` (rough edge #1).

### Tech debt logged (NOT addressed — no drive-by refactor)
- **`memory_search` keyword-gate brittleness** (rough edge #1) — the headline next task.
- **K-means topic mislabels** (rough edge #3) — display-only.
- **Reranker latency** (rough edge #2) — post-correctness.
- Carried from last session: N-ary contradiction fallback unreferenced by production; Phase 2b invalidations not summed into `contradictions_resolved` (cosmetic counter gap — the live run showed `contradictions_resolved: 0` despite `stale_count=1`).

### How to get up to speed next session (fast)
- Re-read this opener + ADR-066 above + [[project-architectural-lock-llm-out-of-read-path]], [[correctness-is-the-product]], [[bge-small-cannot-separate-relevant]], [[project-reranker-qwen3-solves-model-fit]], [[project-read-relevance-conformal-not-absolute-threshold]] (now: conformal was NOT the fix — recall-first was), [[feedback-confirm-before-commit-push]], [[confirm-before-cargo-build-and-check-disk]], [[reference-mcp-dogfood-log-is-ground-truth]].
- Live dogfood harness (reuse it): wipe stores at `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\` (`vault.db*` + `graph.duckdb*` + empty `lance/` + `reports/`; keep `models/`) with **Claude Desktop fully quit** (note: `Get-Process Claude` matches Claude *Code* too — don't use it as a guard). MCP server binary = `target\debug\vault-cli.exe` (Desktop config `%APPDATA%\Claude\claude_desktop_config.json`, boundaries `personal`+`testeval`, reranker + bge wired, no SqlCipher key arg = dev default). MCP log (ground truth) = `%APPDATA%\Roaming\Claude\logs\mcp-server-memory-vault.log` — it captures BOTH client requests AND server responses/tracing, so abstain/surface + error codes are confirmable there. Live log watcher: `tail -n 0 -f <log> | grep -iE "tools/call|abstain|rerank|-32601|-32602|-32001|..."` via the Monitor tool (persistent). **Startup loads the 1.2GB reranker before answering `initialize`; under machine load this can exceed Desktop's 60s init timeout → tools missing. Relaunch when the box is idle.** Consolidate cmd (Desktop must be CLOSED — file locks): `vault-cli --vault-db … --vector-dir … --graph-db … consolidate --bge-model … --bge-tokenizer … --ort-lib … --phi4-model …\models\Phi-4-mini-instruct-Q4_K_M.gguf run` (set `$env:RUST_LOG="info,vault_consolidator=debug"`).

### Disk / env
- C: ~63 GB free (target/ ~80 GB; the day's 4B-eval download was deleted). Confirm before any cargo build/test/clippy + check disk ([[confirm-before-cargo-build-and-check-disk]]); only `cargo fmt` is free. Reranker on the Intel UHD Vulkan iGPU not used for ORT (CPU EP) — reads are CPU-bound.

---

## 🟦 NEXT SESSION OPENER (2026-06-01 evening) — **[HISTORICAL — superseded by the 🟦 2026-06-02 recall-first opener above. The synonym-gap over-abstention this opener queued is now RESOLVED via ADR-066 recall-first, dogfood-green.]** A5 + DEDUP SHIPPED & DOGFOOD-CONFIRMED on real Phi-4 (committed). NEXT: fix read over-abstention (synonym gap) + 3 rough edges. — READ THIS FIRST

> This session implemented + SHIPPED the A5 nearest-neighbor fix (ADR-065), ran the full §7 live dogfood on real Phi-4 (Claude Desktop seeded; Claude Code watched the MCP log + ran `consolidate`), and **committed the whole arc** this session. **A5 (the V0.2 ship-gate) + dedup PASS live.** The dogfood ALSO confirmed read-side over-abstention is not fully solved (synonym-gap case) — that is the headline next-session task. All older openers are HISTORICAL.

### ⏳ FIRST THING NEXT SESSION — verify CI
Commit `4f2cff6` was pushed to `main` and its CI run (`gh run list --workflow=ci.yml -L 1`) was **in_progress** at session close — CONFIRM it went `success` before staging anything new ([[broken-ci-is-regression-not-techdebt]]). Also note: a **scheduled** (cron) CI run on the *prior* commit `2302842` showed `failure` even though that commit's *push* CI was green — likely flaky/scheduled-only; check whether `4f2cff6`'s run carries it forward, and if so fix it same-session.

### ✅ SHIPPED + COMMITTED THIS SESSION (see `git log` on `main`)
- **A5 nearest-neighbor contradiction detection (ADR-065)** — Phase 2b computes per-fact top-K (=3) nearest cosine neighbors above a **0.70** floor, unions to candidate pairs, feeds the EXISTING Phi-4 pairwise judge + recency aggregator. Replaces K-means topic grouping (ADR-060's "topics co-locate the pair" premise was FALSE). Only candidate *generation* changed.
- **Floor 0.70 measured, not guessed** (`nn_contradiction_spike.rs`, real bge-small): real contradictions Vega/Atlas **0.905** + Tesla/Rivian **0.823** (mutual #1 neighbors); unrelated-noise ceiling **0.634**. 0.70 sits in the clean 0.634→0.823 gap.
- Also in this commit (prior-uncommitted arc, now all green + shipped): **dedup ADR-063**, **A5 pairwise ADR-062**, **Bug-1 recency** (`stale_by_recency`), **merge-resilience**, **read-side ADR-064** (`DOC_SUBJECT_FRAME "The user — "` subject framing) + **rerank-cap 8→20**.
- All 5 DoD gates green pre-commit: `test -p vault-consolidator` (99 unit + all integration) · `clippy --workspace --all-targets -D warnings` 0 · `build --workspace` 0 · `fmt --all --check` 0.

### 🎯 DOGFOOD VERDICT (real Phi-4, §7 runbook, 2026-06-01 evening)
- **B1 — A5 ship-gate ✅** `memory_read "what does the user drive now?"` → ONLY the Rivian (id …2859); Tesla NOT surfaced; `abstain:false`. Read path honors the invalidation.
- **B2 — reversible ✅** Tesla (id …1bc8) retained via `valid_until` (`superseded_by:null`), recoverable under `include_archived:true`.
- **B3 — no false retirement ✅ (core)** PRECJOB, cello, PRECFOOD all stayed active (`valid_until:null`, rank-1 in search). Consolidator precision held; the old "engineer vs hiking" false-contradiction did NOT recur. ⚠️ but see read over-abstention below.
- **B4 — dedup ✅** `"what is the user's dog?"` → Biscuit once (survivor …8350); the 2 copies `superseded_by` the survivor (recoverable).
- **Part A:** C5 (−32602) C6 (update→amber) C7 (unicode, via NL query) C8 (−32001) C9 (norm-determinism) C10 (stopword→empty, no −32603) C11 (`as_of`→`valid_from` 2024-01-15 confirmed in the read) A6 (abstain) I2 (hard-delete) — **all ✅**. C4 NOT run (see rough edges).
- **Consolidation log evidence:** Phase 2b `active=9, candidate_pairs=1` (only Tesla/Rivian cleared 0.70), `stale_count=1` (older Tesla); summary `clusters deduped: 2, memories deduped: 3` (DEDUPDOG ×3→1 +2, C9NORM ×2→1 +1), `clusters skipped: 0`, `memories_merged: 0` (dedup is deterministic — zero LLM merge calls).

### 🟦 NEXT SESSION — FIX THESE (priority order)

**1. READ OVER-ABSTENTION on synonym gaps — TOP PRIORITY (this is the product-thesis bug, "correct output every time").**
- **Symptom (live):** `memory_read "what food does the user like?"` → `abstain:true`, EMPTY — but the vault holds "PRECFOOD The user's favourite cuisine is Japanese" (fully active, `valid_until:null`, rank-1 in `memory_search`). Reword to "what is the user's favourite **cuisine**?" → surfaces cleanly. The miss is a **zero-token-overlap synonym leap** (food↔cuisine).
- **This is the SAME class as Bug-2 (A7) — NOT fully fixed.** ADR-064 (subject framing `"The user — "`) fixed the *subject-less* case (cello). The *synonym-gap* case survives it. So the read-relevance gate still over-abstains.
- **Correct attribution (don't mis-target the fix):** the LIVE MCP server runs WITH the Qwen3 reranker (ADR-059) — those reads take ~13s, the reranker ran. So the over-abstention is the **reranker scoring food→cuisine below its logit-0 floor**, NOT the ADR-057 cosine floor (that's only the no-reranker fallback, e.g. inside `consolidate`). Target the reranker gate.
- **The structural fix is already designed** — see [[project-reranker-conformal-not-absolute-threshold]] + the HISTORICAL 2026-06-01 opener's "controllable-algorithm" research: an absolute logit-0 floor is wrong; calibrate τ via **split-conformal** on a labelled fixture (knob α = miss-rate), pluggable `RelevanceGate` (Conformal now → learned Combiner later), capture labelled read features in prod. Start by adding food→cuisine to a `read_quality_eval` fixture and reproducing via `reranker_fun_diagnostic.rs` (edit `doc`/`queries`).
- **Decision needed from Shahbaz:** is read over-abstention now the priority arc (vs latency, vs other V0.2 items)? It directly breaks the core promise. Recommend yes.

**2. C4 content-ceiling backstop — OWED (I deferred it).** This run did NOT verify C4: Claude Desktop wrote only 1 of 3 probes and it was short (3787 chars, not ~5000) — correctly refusing to send a shortened payload. The 10K/50K exact-length probes are Claude Code's to write via the MCP path (no `vault-cli write` subcommand exists → spin a short-lived `vault-cli mcp serve` + send `memory_write` JSON-RPC, OR write a tiny harness). **Do on a CLEAN slate** (giant CAP_OK blobs pollute clustering/dedup — never seed them before an A5 run). Ceiling already ~≥26K-characterized in a prior dogfood; this is "confirm a known-good behavior," low priority.

**3. C7 single-token marker search short-circuit — rough edge.** `memory_search "C7UNI"` (one alphanumeric token) returns EMPTY in ~0 ms (BM25 leg finds no match → abstains before the semantic leg). The doc IS stored (a natural-language query finds it byte-identical incl. ﬁ ligature). Investigate the BM25 abstain / degenerate-guard path in the search pipeline; affects single-token marker queries (workaround: natural-language query). Reproduced twice live this session.

**4. K-means topic mislabeling (K1 not gate-ready) — rough edge, display-only.** On the tiny `testeval` boundary the labeler found ~2 clusters and mis-assigned 3 of 4: PRECJOB→`vehicle_transitions`, cello→`japanese_cuisine`, Biscuit→`vehicle_transitions` (only PRECFOOD→`japanese_cuisine` correct). It's coarse cluster *assignment*, not a random labeler. Does NOT affect read correctness (B1/B3/B4 returned correct facts despite junk topics). Folds into the deferred clustering rework (threshold connected-components / HDBSCAN). K1 (topics populate) can't gate until this improves.

### ADR-065 — contradiction candidate generation by nearest neighbor, not K-means topics (SHIPPED)
**Decision:** Phase 2b generates contradiction candidate pairs via per-fact top-K nearest cosine neighbors above a similarity floor (`phases::candidates::nearest_neighbor_candidate_pairs`), replacing the K-means topic grouping of ADR-060.
**Why:** ADR-060's premise — "a K-means topic co-locates the conflicting pair" — was proven FALSE in the §7 dogfood: K-means split Tesla→Rivian across groups, so the judge never saw it. Contradiction detection is a nearest-neighbor problem, not a partitioning one.
**Mechanics:** top-K = 3, floor = 0.70 (measured). Bounded ≤ N·K/2. Existing `judge_candidate_pairs` (Phi-4 pairwise + `shared_attribute` gate + recency `stale_by_recency`) + the whole-active-set mass-invalidate refusal UNCHANGED. The 0.92 merge gate untouched.
**Live-confirmed:** real Phi-4, `candidate_pairs=1`, older Tesla retired, B1 read returns only Rivian. **Supersedes** ADR-060's K-means premise; K-means stays in `topics.rs` for REPORT display only (see rough edge #4).

### Tech debt logged (NOT addressed — no drive-by refactor)
- **N-ary contradiction fallback now unreferenced by production.** `detect_contradiction` (dispatcher), `detect_contradiction_nary`, `detect_contradictions_pairwise` are off the production path (Phase 2b calls `judge_candidate_pairs` directly). Still `pub` + unit-tested (no dead-code warning). Evaluate removing them + `MAX_PAIRWISE_GROUP_SIZE` + the N-ary prompt/schema in a focused cleanup. Kept to avoid scope creep.
- **K-means topic cohesion** (rough edge #4) — display-only.
- **Phase 2b invalidations not counted in `contradictions_resolved`.** The summary's `contradictions_resolved`/`contradictions queued` counters track the legacy merge-gate ConflictReview path, NOT the Phase 2b NN invalidations (those log `stale_count` but aren't summed into the report counter). Cosmetic reporting gap; consider surfacing an `a5_invalidated` count in `ConsolidationReport`.

### How to get up to speed next session (fast)
- Re-read this opener + [[project-reranker-conformal-not-absolute-threshold]], [[bge-small-cannot-separate-relevant]], [[project-reranker-qwen3-solves-model-fit]], [[correctness-is-the-product]], [[kmeans-wrong-for-contradiction-use-nn]] (now SHIPPED), [[anchor-on-measured-not-projected]], [[reference-mcp-dogfood-log-is-ground-truth]], [[confirm-before-cargo-build-and-check-disk]], [[feedback-confirm-before-commit-push]].
- Live dogfood harness (works, reuse it): wipe vault stores at `C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\` (`vault.db` + `graph.duckdb` + empty `lance/` + `reports/`; keep `models/`). MCP server binary = `target\debug\vault-cli.exe` (Claude Desktop config at `%APPDATA%\Claude\claude_desktop_config.json`, boundaries `personal`+`testeval`, reranker wired). MCP log (ground truth, Desktop UI hides error codes) = `%APPDATA%\Roaming\Claude\logs\mcp-server-memory-vault.log` — byte-offset tail watcher pattern in this session's transcript pings on each new entry. Consolidate cmd: `vault-cli --vault-db … --vector-dir … --graph-db … consolidate --bge-model … --bge-tokenizer … --ort-lib … --phi4-model C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\models\Phi-4-mini-instruct-Q4_K_M.gguf run` (set `$env:RUST_LOG="info,vault_consolidator=debug"`). **Desktop must be CLOSED during consolidate** (DuckDB/SQLite file locks); reopen for B-verify.
- Reranker measurement instrument: `crates/vault-embedding/tests/reranker_fun_diagnostic.rs` (`#[ignore]`, edit `doc`/`queries`, run `--ignored --nocapture`).

### Disk / env
- C: ~125 GB free; `target/` ~workspace-built (heavy native deps: llama-cpp/Vulkan, lance, duckdb, aws-lc). Confirm before any cargo build/test/clippy + check disk ([[confirm-before-cargo-build-and-check-disk]]); only `cargo fmt` is free. Consolidation loads Phi-4 on the Intel UHD Vulkan GPU (~33/33 layers offloaded) — fine on this machine. Reads take ~3–13s (Qwen3 reranker on CPU) — latency arc is post-correctness.

---

## 🟥 NEXT SESSION OPENER (2026-06-01 late) — **[HISTORICAL — superseded by the 🟩 2026-06-01 evening opener above. A5 is now FIXED via ADR-065 nearest-neighbor candidates; the fix designed here was implemented + DoD-green.]** READ SIDE SOLID; A5 CONTRADICTION IS STRUCTURALLY BROKEN (K-means). Fix = nearest-neighbor candidate pairs.

> This session ran the full §7 clean-vault dogfood with the watcher streaming the MCP log live. It (a) confirmed the Bug-2 read fix, (b) found AND fixed a second read bug (rerank-cap) the dogfood exposed, (c) confirmed dedup, and (d) surfaced the headline problem: **A5 contradiction detection does not fire because K-means topic clustering doesn't co-locate the conflicting pair.** Root cause is definitively pinned (not Phi-4, not the aggregator) and the fix is designed + spiked. The 2026-06-01 (earlier) opener below is now HISTORICAL.

### TL;DR — where we are
- ✅ **Bug 2 (read over-abstention) — FIXED + live-confirmed.** ADR-064 read-side subject framing (`DOC_SUBJECT_FRAME = "The user — "` in `reranker.rs`). Root cause was subject-LESS stored facts, NOT the floor. Live A7 reads surface the cello after the rerank-cap fix below.
- ✅ **Rerank-cap bug — FIXED (found live this session).** `RERANK_CANDIDATE_CAP` 8 → `DEFAULT_MAX_CANDIDATES` (20) in `structured_read_pipeline.rs`. The live A7 read abstained on a populated vault because BGE-small ranked the cello BELOW the top-8, so the (correctly-framed) reranker never scored it. Now reranks the full retrieved pool. **Pinned by a NEW multi-fact regression test** (`vault-app/tests/read_no_keyword_overlap.rs::cello_surfaces_amid_distractors_and_still_abstains_on_no_signal`) — GREEN. The old 1-fact test couldn't catch this.
- ✅ **§7 Part A dogfood: 10/10 PASS** (C4 content-ceiling ≥26K + 50K→backstop, C5 −32602, C6 update→amber, C7 unicode byte-identical incl. ﬁ ligature, C8 −32001, C9 norm-determinism, C10 degenerate→empty, C11 settable as_of, A6 abstain, I2 hard-delete). Dedup (ADR-063) confirmed live (2 clusters / 3 memories: DEDUPDOG ×3 + C9NORM ×2).
- 🟥 **A5 (knowledge-update contradiction) — STRUCTURALLY BROKEN. THE V0.2 SHIP-GATE. This is the next arc.** See the full diagnosis + fix below.
- ⚠️ **Everything UNCOMMITTED.** Read-side wins are committable independently of the A5 fix.

### 🟥 A5 — THE ISSUE (definitively diagnosed this session)
**Symptom:** live consolidation on the seeded Tesla→Rivian vault → `contradictions: 0, archived: 0`. The stale Tesla is NOT retired; B1 (the ship-gate read "what does the user drive now?") would return BOTH cars.

**Root cause (pinned by a verbose-logging diagnostic re-run — ruled out everything else):**
- ❌ NOT Phi-4 — it correctly judged all 6 pairs it was handed (`contradiction=false` on genuinely-unrelated pairs).
- ❌ NOT the Bug-1 recency aggregator (`stale_by_recency`) — it's sound.
- ✅ **It's the K-means topic clustering.** The contradiction pass (`consolidator.rs` Phase 2b) groups facts via `discover_topics` (K-means) and only judges pairs WITHIN a topic. K-means does NOT reliably put the conflicting Tesla/Rivian pair in the same group — knowledge-update pairs aren't near-duplicates (cosine < 0.92), so K-means happily splits them. **The verbose run showed the Tesla/Rivian pair was NEVER judged** (0 log mentions; the 6 judged pairs were all other facts). It's also unstable: the REPORT and the contradiction pass — same run, same facts — grouped differently.
- This is the SAME "[[contradiction-gated-by-merge-threshold]]" failure recurring at the topic level. ADR-060's premise ("K-means topics co-locate the conflicting pair") is FALSE.

### ▶️ A5 — THE FIX (do this next session)
**Replace K-means grouping in contradiction detection with per-fact NEAREST-NEIGHBOR candidate pairs.** Contradiction detection is a similarity-search problem, not a partitioning problem — "for fact X, is there a more-recent fact about the same thing?"
1. In `consolidator.rs` Phase 2b: instead of `discover_topics` → per-topic groups, for each active fact compute its **top-K cosine neighbors** (reuse the vector store / BGE embeddings) and union into an unordered candidate-pair set (bounded ~N·K/2, with a similarity floor).
2. Feed those candidate pairs to the EXISTING `detect_pair_contradiction` (the Phi-4 pairwise judge — already works) and the EXISTING aggregator (`stale_by_recency` — already correct). Only the CANDIDATE-GENERATION step changes.
3. **Spike status: ✅ PASSED + VALIDATED (`vault-consolidator/tests/nn_contradiction_spike.rs`, real BGE).** The (Tesla, Rivian) pair K-means dropped is the **single strongest edge in the neighbor graph: cosine 0.823** (next-highest pair only 0.634), and they are **MUTUAL #1 nearest neighbors** (rivian = tesla's #1 of 8, tesla = rivian's #1 of 8). Candidate set bounded (19 of 36 all-pairs at top-K=3). The weak distractor pairs (0.43–0.63, BGE-small noise) Phi-4 already rejects correctly. **A similarity floor ~0.7 cleanly isolates the real contradiction.** → Direction confirmed; promote the spike's per-fact top-K logic into `consolidator.rs` Phase 2b (add a `≥~0.7` floor + cap K), keep the spike as executable documentation.
4. Bound cost: similarity floor + cap K (offline nightly run; O(N·K) Phi-4 calls).
5. **Cleanup:** remove the temporary diagnostic `tracing::info!("pairwise contradiction verdict" …)` line added to `phases/contradiction.rs` this session (it was for the diagnosis only).
6. K-means for the REPORT topic *display* (facts_by_topic) is a SEPARATE, lower-priority concern — it produced junk clusters (lumped normalization/job/dog with the cars) but it's display-only, not correctness. Defer; if improved, use threshold connected-components (the union-find we already have at 0.92, run lower) or HDBSCAN.

### ✅ Read-side wins — files (committable independently of A5)
- `crates/vault-embedding/src/reranker.rs` — `DOC_SUBJECT_FRAME` (ADR-064) + `format_prompt_with` + `testing`-gated `rerank_with_instruction` seam; `format_prompt` removed; pins `doc_subject_frame_is_pinned` + `rerank_frames_each_doc_with_the_subject`.
- `crates/vault-retrieval/src/structured_read_pipeline.rs` — `RERANK_CANDIDATE_CAP = DEFAULT_MAX_CANDIDATES` (the cap fix) + updated `reranker_caps_at_rerank_candidate_cap` test.
- `crates/vault-app/tests/read_no_keyword_overlap.rs` — NEW multi-fact regression (the cap-fix gate).
- `crates/vault-embedding/tests/reranker_fun_diagnostic.rs` — the 3 measurement instruments (conformal / subject-prefix / framing sweep) + serde_json dev-dep.
- `crates/vault-mcp/src/server.rs` — write-size observability log (`content_bytes`); `crates/vault-cli/src/main.rs` — EnvFilter adds `vault_mcp=info`.

### Rough edges logged (not blocking)
- **`memory_search` single-token short-circuit:** a marker-token query like `"C7UNI"` returns empty in ~0ms (BM25 leg finds no match → abstains before the semantic leg). The doc IS indexed (natural queries find it). Investigate the BM25 abstain/degenerate-guard path; affects single-token marker searches (workaround: natural-language queries).
- **K-means topic cohesion:** junk clusters (display REPORT). Folds into the A5 clustering rework.

### Commit state + disk
- `main` HEAD = `2302842`. ALL session work UNCOMMITTED.
- **Suggested commit split:** Commit A = read-side arc (Bug-2 ADR-064 + rerank-cap + regression + diagnostics + write-log) — green, self-contained, committable now. Commit B = the A5 nearest-neighbor rework (next arc). Plus the prior-uncommitted dedup (ADR-063) + A5 pairwise (ADR-062) + merge-resilience.
- **Per-action approval for every commit + push** ([[feedback-confirm-before-commit-push]]).
- **DISK:** did a full `cargo clean` at session end (target/ had ballooned to 136 GB across the day's rebuilds; disk hit 3 GB free). Next session's first build is a ~30–40 min from-scratch rebuild. Watch disk; clear `target/debug/incremental` for cheap regenerable space.

### Memories to load next session
[[contradiction-gated-by-merge-threshold]] · [[project-reranker-subjectless-facts-framing]] · [[bge-small-cannot-separate-relevant]] · [[correctness-is-the-product]] · [[no-sub-7b-models-for-synthesis]] · [[source-read-call-graph-upstream-of-empirical]] · [[reference-mcp-dogfood-log-is-ground-truth]] · [[dev-vault-is-throwaway-test-data]] · [[feedback-confirm-before-commit-push]] · [[confirm-before-cargo-build-and-check-disk]]

---

## 🟩🟩 NEXT SESSION OPENER (2026-06-01) — **[HISTORICAL — superseded by the 🟥 2026-06-01-late opener above. Bug 2 still fixed; but A5 turned out structurally broken in the live dogfood, and a rerank-cap bug was found + fixed.]** Bug 1 (A5 polarity) + Bug 2 (read over-abstention) BOTH FIXED. — READ THIS FIRST

> Prior session (2026-05-31) found two critical bugs in the clean-vault dogfood. **This session FIXED BOTH and proved them green.** Bug 1 (A5 polarity) fixed by recency-based stale selection. Bug 2 (read over-abstention) — we FALSIFIED the handoff's "reranker floor too high" hypothesis with a measurement, found the REAL cause (the reranker mis-scores subject-LESS facts), and fixed it read-side with a measured framing change (ADR-064). Read this whole block before touching anything. The 2026-05-31 opener further down is now HISTORICAL.
>
> **⚠️ The conformal-calibration plan in the OLD step-1 below is SUPERSEDED — do NOT pursue it for Bug 2.** The measurement proved floor 0 is correct (it separates the A7 set cleanly once facts carry a subject); the bug was phrasing, not the threshold. The conformal harness stays in the tree as a measurement instrument and is still the right tool *if* score-drift ever reappears, but it is NOT the Bug-2 fix. See the "✅ BUG 2 FIXED" block for what actually happened.

### TL;DR — where we are
- ✅ **Bug 1 (A5 contradiction POLARITY INVERSION) — FIXED.** The consolidator no longer trusts Phi-4's `stale` label; **code now picks the stale fact deterministically by recency (older `valid_from` = stale, "newest-wins")**, with a tie → abstain. Inversion is now structurally impossible. Pinned by a Tesla→Rivian regression test that reproduces the exact dogfood bug.
- ✅ **Two stale dedup tests fixed** (`merge_acceptance` + `properties` idempotency) — they assumed the near-identical "standup" pair would be LLM-*merged*, but dedup (ADR-063) now collapses it *before* merge, so they correctly assert `memories_deduped > 0`.
- ✅ **ALL GATES GREEN on Windows (2026-06-01):** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` — all pass (full from-scratch rebuild after a `cargo clean`).
- ✅ **Bug 2 (read OVER-ABSTENTION / A7) — FIXED + regression GREEN.** Root cause was NOT the floor. A measurement falsified that: on the labelled A7 fixture the reranker at floor 0 already separates relevant (all +0.57…+8.65) from guards (all −7.36…−1.38) cleanly. The live failure was specific to the cello fact — a subject-LESS fragment ("Plays the cello…") that the reranker reads as not-about-the-user and rejects even for near-literal queries. **Fix (ADR-064): read-side subject framing — prepend `"The user — "` to every candidate before the reranker scores it.** Measured winner of an A/B sweep (8/8 relevant clear floor incl. cello, 0 guard leaks, widest gap +1.69). `read_no_keyword_overlap` regression GREEN through the real BGE+reranker stack (cello surfaces for "for fun"; blood-type still abstains).
- ⚠️ **NOTHING is committed.** All Bug-1 + Bug-2 + dedup + A5 + test-fix work is in the working tree. Commit needs Shahbaz approval + a clean A5 re-dogfood (see Commit Plan below).

### ✅ DONE this session — details + files

**Bug 1 — A5 polarity inversion (the V0.2 ship-gate blocker).**
- Root cause CONFIRMED (not a comparison bug): `phases/contradiction.rs::detect_contradictions_pairwise` mapped the model's `stale` side label straight to an id (`StaleSide::A => a.id`). On the polluted Tesla/Rivian topic Phi-4 labelled the *newer* Rivian stale and the code obeyed → vault served the stale Tesla as current truth.
- Fix: new pure fn `stale_by_recency(a, b)` — older `valid_from` is stale; `None` on a tie. The aggregator calls it AFTER the contradiction + shared-attribute gates; the model's `stale` label no longer drives retirement. Tie (identical `valid_from`) → WARN + abstain (keep both — Shahbaz's locked decision: never confidently serve the wrong one).
- Files: `crates/vault-consolidator/src/phases/contradiction.rs` (fn + aggregator + doc comments + tests). Tests added/updated: `aggregator_retires_older_fact_even_when_model_mislabels_newer_as_stale` (THE regression), `aggregator_abstains_on_contradiction_with_tied_valid_from`, `aggregator_recency_keeps_only_the_newest_in_a_conflict_chain`, plus the existing aggregator tests re-pinned to explicit dates. Integration `tests/contradiction_resolution.rs` comments corrected; assertions unchanged + still green.

**Stale dedup test fixes** (same root: ADR-063 dedup runs BEFORE the LLM merge step, so the near-identical "standup" pair is deduped, not merged):
- `crates/vault-consolidator/tests/merge_acceptance.rs::summary_markdown_is_non_empty_and_contains_required_sections` → now asserts `memories_deduped > 0`.
- `crates/vault-consolidator/tests/properties.rs::consolidation_is_idempotent` → run 1 asserts `memories_deduped > 0`; run 2 also asserts `memories_deduped == 0` (idempotency on the dedup dimension). `no_memory_is_ever_lost` already passed under dedup (no change needed).

**Bug 2 — wiring fix (correct + necessary, KEEP — rode in earlier this session):**
- `crates/vault-app/src/application.rs`: the `StructuredReadPipeline` is now built from the **raw `hybrid`** retriever, NOT the BM25 `AbstainingRetriever`. The keyword abstain gate is now `memory_search`-only. The reranker owns read abstention.
- `crates/vault-retrieval/src/structured_read_pipeline.rs`: corrected the false "BM25 gate stays vestigial" comment.

### ✅ BUG 2 FIXED (2026-06-01, later) — read-side subject framing (ADR-064)

**What it was, really.** NOT a threshold problem. Three measurements (all via `reranker_fun_diagnostic.rs`, single model, `--ignored --nocapture`):
1. **Conformal calibration over the A7 fixture** → relevant logits all POSITIVE (+0.57…+8.65), guard logits all NEGATIVE (−7.36…−1.38). Clean +1.95 gap. **Floor 0 already separates the A7 set perfectly (6/6 recall, 0 leak).** This FALSIFIED the handoff's "reranker floor too high for prose facts" hypothesis — that table tested only ONE doc (the cello fact), an outlier.
2. **Subject-prefix diagnostic** → the cello fact's problem is the missing subject. ADD "The user" to the cello fact: for-fun −4.46→+1.75, music −5.21→+1.58 (both cross the floor). STRIP "The user" from known-good A7 facts: every one drops (Δ −1.3…−3.6). The reranker's instruction matches "a question about a USER to a personal fact"; a subject-less fragment doesn't read as about-the-user, so it's rejected even near-literally.
3. **A/B framing sweep** (full A7 + cello, under floor 0): **Variant A1 — frame the doc as `"The user — {fact}"` — WINS:** 8/8 relevant clear the floor (cello for-fun +3.23, music +2.08), 0 guard leaks (cello no-signal blood-type stays −6.86), widest gap +1.69. Beat `"The user: "` (fragile +0.18 cello/music), `"About the user: "` (broke 2 cases), and BOTH instruction-hint variants (Variant B) which actually **leaked a guard** — measuring saved us from shipping B.

**The fix (read-side, robust to uncontrolled stored prose, no new model, floor unchanged at 0):**
- `crates/vault-embedding/src/reranker.rs`: new `const DOC_SUBJECT_FRAME = "The user — "`; the production `RerankProvider::rerank` prepends it to every candidate before scoring. The instruction is now parameterised (`format_prompt_with`) and a `testing`-gated `rerank_with_instruction` seam lets the sweep measure variants without touching the production path. Old dead `format_prompt` removed; unit tests `doc_subject_frame_is_pinned` + `rerank_frames_each_doc_with_the_subject` pin the fix.
- `crates/vault-embedding/tests/reranker_fun_diagnostic.rs`: the 3 measurement instruments (conformal calibration, subject-prefix, framing sweep), all `#[ignore]`. Added `serde_json` dev-dep to parse the A7 fixture.
- **Regression GREEN:** `crates/vault-app/tests/read_no_keyword_overlap.rs::no_keyword_overlap_read_surfaces_fact_and_still_abstains_on_no_signal` passes through the REAL BGE + reranker stack (cello surfaces for "for fun"; blood-type abstains). DoD gates all green (fmt / clippy `--workspace --all-targets -D warnings` / build `--workspace` / `-p vault-embedding` + `-p vault-app` regression).

### 🟥 [HISTORICAL — see "✅ BUG 2 FIXED" above; this hypothesis was FALSIFIED] Bug 2 — THE PROBLEM, in full

**Symptom (live dogfood 2026-05-31):** answerable, loosely-phrased reads abstain. `"what hobby does the user have?"` returns the cello fact, but `"what does the user do for fun?"` (no shared keywords with "plays the cello in a community orchestra") returns `abstain: true`. A vault that says "I don't know" about facts it holds is broken for the agent-read workload ([[correctness-is-the-product]]).

**Handoff hypothesis (FALSIFIED this session):** "the BM25 abstain gate runs upstream of the reranker; remove it and the reranker will surface the fact." We removed it. The read STILL abstains.

**Actual root cause (measured 2026-06-01):** the production reranker (`Qwen3RerankerProvider`, v4 instruction, floor = **logit 0**) scores our prose facts BELOW the floor for these conversational questions. It RANKS correctly (relevant questions score least-negative, no-signal most-negative) but the **absolute cut-off (0) is too high** for conversational→prose-fact leaps — so everything abstains.

### 🔬 THE DECISIVE MEASUREMENT (re-run any time — see instrument below)

Production reranker scoring the doc `"Plays the cello in a community orchestra on Sunday afternoons."` (floor = logit 0; ≥0 = surfaces):

| Query | logit | result |
|---|---|---|
| "what does the user enjoy in their spare time?" | −2.17 | abstain |
| "what hobby does the user have?" | −2.57 | abstain |
| "what does the user do for fun?" | −4.46 | abstain |
| "what does the user do to relax?" | −4.68 | abstain |
| "what music does the user play?" | **−5.21** | abstain |
| "what is the user's blood type?" (no-signal) | −6.57 | abstain |

**Read this carefully:** even `"what music does the user play?"` → `"plays the cello"` (near-literal) scores −5.21. That is a RED FLAG that the 0.6B reranker may have a real *ceiling* for pure-meaning matching, not just an instruction-wording problem. BUT note the apparent contradiction: in the 2026-05-29 dogfood the reranker DID surface a hobby for a "fun"-ish query ("outdoors for fun on weekends" → hiking). Difference = lexical/semantic anchor + doc phrasing. **So we MEASURE, we don't assume.**

### 🧭 THE DECISION FRAMING (locked context: storage = uncontrolled LLM prose)

The agent saves whatever it wants — a full sentence, a paragraph, several facts mushed together. We do NOT control the wording. So **the fix must live on the READ side (robust to whatever prose got stored), not the write side.** Write-side canonical phrasing was considered and DROPPED (can't guarantee clean input; a paragraph can't be canonicalized to one tidy fact without loss).

### 🔬 RESEARCH DONE (2026-06-01) — the controllable-algorithm answer (2 parallel research threads, STRONG convergence)

Shahbaz's question: *"can we use an ALGORITHM we control, instead of swapping in another model?"* → **YES, and it's the right move.** Both threads converged on the same root cause + fix:

- **Root cause (textbook):** an ABSOLUTE, global threshold on a reranker/cosine score is *structurally* wrong — those scores are only meaningful **relative to each query** (Elastic et al.: "dense scores only make sense in the context of the specific query"). Our cosine-floor + reranker-floor failures and the whole "fix-one-break-another" history are the documented symptom. **The model RANKS fine; the fixed cut-off is the bug.**
- **THE FIX (controllable, deterministic, local, tiny-data, ONE knob): split-CONFORMAL calibration of the reranker threshold.** Don't *guess* the cut-off — *measure* it from our own labelled A7 examples: run the reranker on each (query, known-relevant fact), take a quantile of those scores as the threshold τ (replacing the guessed `RERANK_RELEVANCE_FLOOR = 0.0`). One plain-English knob **α = accepted miss-rate** (lower α = surface more eagerly; matches our locked recall>precision stance). ~20 lines of pure Rust, **computed OFFLINE → zero read-time cost** (decisive: reads are already ~12s), **works with a single candidate** (τ is external to the candidate set — unlike top1−top2 gap methods, which break on our exact one-memory cello case), no new deps. It is THE small-data abstention method — honest even at n=8–25 and it tightens as the fixture grows.
- **Alternative considered + rejected for us: per-query "irrelevant-anchor noise floor"** (each read, score the query against ~5–8 known-irrelevant facts; surface only candidates that clearly beat that floor). Equally controllable + per-query-adaptive, BUT it costs N extra reranker passes PER READ (~0.39s each, on top of an already-slow read). Conformal precomputes the same per-query awareness OFFLINE → conformal wins on our latency reality. Keep anchor-probe as a possible diagnostic / future feature, not the gate.
- **The DESTINATION (once data grows past ~150 labelled cases): a tiny learned COMBINER we fully own** — logistic regression over `[reranker_logit, bge_cosine, bm25, score-gap]`, interpretable weights, pure-Rust dot-product+sigmoid inference, retrainable. It OVERFITS at today's n=8–25 (don't start here). Build the feature-extraction plumbing NOW so the decision head can evolve conformal → combiner with no churn.
- **SCALE-READINESS — agreed with Shahbaz 2026-06-01 (build NOW, don't wait):** production fills FAST (one Claude Code session = lots of memories), and MORE stored memories means MORE distractors per read → the gate gets harder, sooner. So build for the graduation now. **Critical distinction:** the combiner (and conformal) train on LABELLED relevance judgments (query → which fact is relevant / which are guards / abstain), NOT on raw stored memories — so a fast-filling vault does NOT auto-produce training data. The readiness levers are therefore: (a) **make the relevance gate PLUGGABLE** (a `RelevanceGate` abstraction: `Conformal` now → `Combiner` later = a one-line swap, not a rewrite); (b) **CAPTURE labelled data in production** (log per-read features + the decision, ideally a lightweight "was this right?" signal) so "fills fast" becomes combiner FUEL; (c) flip to the combiner only when we have ~150+ REAL captured labels, trained + validated. Do NOT train/activate the combiner on the tiny hand-labelled fixture (overfits → confident-but-wrong, worse than honest conformal).
- **Rejected as the gate:** Platt/temperature scaling (monotonic global rescale — fixes offset but NOT per-query drift; weaker than conformal at equal data cost; keep only for a cosmetic `confidence` field). Industry reality check: LlamaIndex / Mem0 / Elastic all default to an absolute similarity cutoff — the exact brittle thing we proved fails — so conformal puts us *ahead* of the default, locally, with no LLM in the read path. Full sources in the session transcript; see [[count-vs-score-filters-in-retrieval]] + [[fix-one-break-another-signals-structural]] (this is the structural fix those memories pointed at).

### ▶️ WHAT TO DO NEXT SESSION (in order)

1. **FIRST EXPERIMENT — conformal-calibrate the reranker threshold (the controllable algorithm; ~1 day, no new deps).** Offline: for each A7 fixture case run the production reranker on (query, expected-surface fact), collect the logits; set τ = the `⌈(n+1)(1−α)⌉/n` quantile of `−logit` for α ∈ {0.05, 0.10, 0.20}. Replace `RERANK_RELEVANCE_FLOOR = 0.0` (`crates/vault-embedding/src/reranker.rs`) with the derived τ. **Validate leave-one-out on the fixture:** per held-out case, recall (expected fact clears τ) + guard-leakage (any must-exclude clears τ). Pick the lowest α (most recall-aggressive) with **zero guard-leakage**. Extend `reranker_fun_diagnostic.rs` (it already prints the logits you need) into the calibration harness.
2. **Build the feature-extraction plumbing** (`[reranker_logit, bge_cosine, bm25, score-gap]` per candidate) alongside step 1 — the shared substrate for both conformal and the future combiner; no rework later.
3. **(SCALE-READINESS) Make the relevance gate PLUGGABLE.** Introduce a small `RelevanceGate` abstraction (trait/enum) with `Conformal` as the active impl now and a `Combiner` slot for later. The point: graduating to the learned combiner becomes a ~one-line swap of the decision head, not a read-pipeline rewrite. Cheap to build now, expensive to retrofit later.
4. **(SCALE-READINESS) Capture labelled relevance data in production.** Production fills fast, but raw memories are NOT training labels — the combiner needs (query → relevant/guard/abstain) judgments. Add a lightweight capture path: log each read's features (`[reranker_logit, bge_cosine, bm25, score-gap]`) + the gate decision, with room for a "was this right?" signal. THIS is what turns the fast-filling vault into combiner FUEL. Keep it simple (a log/table); do NOT over-build a feedback UI for V0.2.
5. **Make the Bug-2 regression test green:** with the calibrated τ, `crates/vault-app/tests/read_no_keyword_overlap.rs` should now surface "for fun"→cello AND still abstain on no-signal "blood type" (A6 guard). Re-measure on `read_quality_eval.rs` (real A7 fixture) — MUST NOT regress A6 or introduce false-answers.
6. **ONLY IF conformal can't separate on the fixture** (good answers + guards still interleave even with a data-derived τ → the reranker genuinely can't tell them apart): THEN consider (a) a sharper "v5" reranker instruction (re-measure via the diagnostic against a subject-bearing doc too), or (b) a stronger local reranker (Apache/MIT, ONNX-able) via a model search — a bigger joint decision, surfaced with measured data, NOT a blind swap. ([[project-reranker-qwen3-solves-model-fit]], [[bge-small-cannot-separate-relevant]])
7. **Grow the A7 fixture + flip to the COMBINER when ready.** Keep adding query/fact/guard cases (conformal's honesty scales with it). Once we have **~150+ real, production-captured labels** (from step 4), train the tiny logistic combiner on REAL data, validate leave-one-query-out (recall + zero guard-leakage), and swap it in via the pluggable gate (step 3). Do NOT train the combiner on the tiny hand-labelled fixture — it overfits.

> NOTE: the Bug-2 regression test (`read_no_keyword_overlap.rs`) is currently RED by design (failing-first; `#[ignore]`d so it does NOT break the normal gates: `cargo test -p vault-app --test read_no_keyword_overlap -- --ignored`). It must go green before Bug 2 is "done."

### 🧰 Instruments left in the tree (re-measure fast)
- `crates/vault-embedding/tests/reranker_fun_diagnostic.rs` — **NEW**, `#[ignore]`. Prints the production-reranker logit for a doc vs a spread of queries + the floor. ~1 min. This is how you measure any reranker change. Edit the `doc`/`queries` to test new phrasings.
- `crates/vault-app/tests/read_no_keyword_overlap.rs` — **NEW**, `#[ignore]`. The Bug-2 end-to-end regression (BGE + reranker through the real read stack).
- `crates/vault-app/tests/read_quality_eval.rs` — the existing A7 calibration harness (real BGE; characterization, not a gate). Use for floor re-calibration.

### 🌳 Tree / commit state + COMMIT PLAN
- `main` HEAD = `2302842` (the two small fixes, CI green — verified this session).
- **Uncommitted (ALL gate-green now):** the prior dedup (ADR-063) + A5 pairwise (ADR-062) + merge-resilience work, PLUS this session's Bug-1 recency fix, the 2 stale-test fixes, the Bug-2 wiring change, **and the Bug-2 read-side framing fix (ADR-064) + its 3 `#[ignore]` measurement instruments + the green `read_no_keyword_overlap` regression.**
- **Recommended commit plan — SPLIT the arc (they're independent layers):**
  - **Commit A (ready after a clean A5 re-dogfood):** dedup (ADR-063) + A5 recency fix (Bug 1) + merge resilience + the 2 stale-test fixes. This is the consolidator / write-side arc; it's green and self-contained.
  - **Commit B (READY):** Bug 2 read-side framing fix (ADR-064 — `reranker.rs` `DOC_SUBJECT_FRAME` + `format_prompt_with`/`rerank_with_instruction` seam) + the `read_no_keyword_overlap` regression (GREEN) + the 3 diagnostics + the `serde_json` dev-dep + the Bug-2 wiring change (`application.rs` raw-hybrid + `structured_read_pipeline.rs` comment). This is the retrieval/read side; self-contained and green.
  - **Note:** the Bug-2 *wiring* change rides with Commit B (its natural home — same read-side arc). Earlier "ride with A?" open question resolved: B.
- **Before Commit A:** wipe the dev vault and re-run the §7 A5 dogfood (Tesla→Rivian: read "what does the user drive now?" must return ONLY Rivian; `include_archived` shows ONLY the older Tesla retired). The earlier A5 dogfood that found Bug 1 is now stale ([[dev-vault-is-throwaway-test-data]]).
- **Per-action approval still required for every commit + push** ([[feedback-confirm-before-commit-push]]).

### ⚠️ Environment note (disk)
`target/` filled the disk this session (down to 0.3 GB free) and a full-workspace test link failed (LNK1318) purely from lack of space. Did a full `cargo clean` (reclaimed ~147 GB). From-scratch rebuilds are ~30–40 min on this machine (heavy native deps: llama-cpp, lance, datafusion, duckdb, aws-lc). Watch disk; clear `target/debug/incremental` + leftover `%TEMP%\.tmp*` dirs for cheap regenerable space before nuking the whole cache.

### Memories to load next session
[[correctness-is-the-product]] · [[bge-small-cannot-separate-relevant]] · [[project-reranker-qwen3-solves-model-fit]] · [[no-sub-7b-models-for-synthesis]] · [[anchor-on-measured-not-projected]] · [[count-vs-score-filters-in-retrieval]] · [[fix-one-break-another-signals-structural]] · [[reference-mcp-dogfood-log-is-ground-truth]] · [[dev-vault-is-throwaway-test-data]] · [[feedback-confirm-before-commit-push]]

---

## 🟥🟥 NEXT SESSION OPENER (2026-05-31) — **[HISTORICAL — superseded by the 2026-06-01 opener above]** DOGFOOD DONE: dedup SHIPS, A5 + read BROKEN. Fix 2 critical bugs before any commit.

**Nothing is committed this session beyond the two small fixes already on `main` (`2302842`).** The dedup + A5 + merge-resilience work is DoD-gate-green but **UNCOMMITTED** in the working tree, held because the live clean-vault dogfood surfaced **two critical correctness bugs**. Do NOT commit until both are fixed and re-dogfooded.

### What this session did
- **Shipped + pushed (`2302842`):** (1) degenerate/all-stopword `memory_search` → graceful empty result (was `-32603`); (2) settable `as_of` param on `memory_write` → seeds `valid_from`. ⚠️ **At session open, confirm `2302842` CI went green** (`gh run list --workflow=ci.yml -L 1`) — it was in-flight when we moved on.
- **Built + DoD-green + dogfood-PROVEN (UNCOMMITTED): deterministic dedup (ADR-063).** `phases/dedup.rs` (two-axis near-identical gate **cos ≥ 0.93 AND containment ≥ 0.80**, golden-record survivor pick, all-pairs over-merge guard) + `StorageBackend::apply_dedup` (atomic supersede-losers + roll survivor aggregates, metadata-only/no re-embed, new `MemoryDeduped` audit event) + orchestrator dedup step **before** `decide_merge` + `clusters_deduped`/`memories_deduped`/`clusters_skipped` in `ConsolidationReport` + CLI. **Thresholds calibrated from real BGE on a hand-labeled pair set** (`tests/dedup_threshold_calibration.rs`, run 2026-05-31 — near-identical cos floor 0.962 vs next-class ceiling 0.883; containment floor 0.889 vs 0.556). Decision (locked with Shahbaz on the calibration data): ship **dedup-core only**; the LLM-classifier complementary-merge band is DEFERRED (the data showed nothing reaches the 0.92 cluster gate except near-identical, and contradictions are already handled by the A5 topic pass). 21 dedup unit tests + 4 `apply_dedup` storage tests green; `merge_resilience.rs` rewritten (constant-vector mock embedder → platform-independent, asserts the new skip counter); new `dedup_integration.rs` (real-BGE end-to-end, proves dedup fires with zero LLM calls).
- **Ran the §7 clean-vault dogfood:** wiped vault → Claude Desktop ran Part A + B-seed (with `as_of` on the A5 cars + a DEDUPDOG ×3 triplet) → Claude Code ran `vault-cli consolidate run` on real Phi-4 → Desktop ran B-verify. **§7 runbook updated** for dedup (B4), settable `as_of` (C11), stopword (C10), + a contamination guard (delete CAP_OK before consolidating).

### ✅ What WORKS (dogfood-confirmed)
- **Dedup (ADR-063): PASS.** DEDUPDOG ×3 → 1 canonical survivor + 2 superseded (reversible), **zero LLM calls, zero skips**; also collapsed the accidental C9NORM ×2 pair. Report: `clusters deduped: 2, memories deduped: 3, clusters skipped: 0`. **The structural overflow/skip class is gone.** This is the session's shippable win.
- **Part A contract surface: all PASS** — C4 (content ceiling), C5 (`-32602` malformed-id), C6 (update→amber), C7 (unicode ﬁ ligature byte-identical), C8 (`-32001` AccessDenied), C9 (normalization determinism), **C10 (all-stopword search → empty, no `-32603`)**, **C11 (settable `as_of` accepted live)**, A6 (blood-type → abstain true-negative), I2 (delete removes from retrieval incl. archived).
- **A5 precision at the DATA layer (B3): PASS** — PRECJOB / PRECHOBBY / PRECFOOD all stayed active (`valid_until: null`); no false-contradiction retirement of different-attribute facts (the "engineer vs hiking" false-positive class did NOT recur).

### 🟥 TWO CRITICAL BUGS TO FIX NEXT SESSION (priority order)

**BUG 1 — A5 contradiction POLARITY INVERSION (CRITICAL — V0.2 ship-gate blocker).**
The consolidator retired the **WRONG side** of the Tesla→Rivian knowledge update: it invalidated the **newer** Rivian fact (`valid_from 2026-05-01`, `valid_until` set at consolidation `13:28:50`) and kept the **older** Tesla fact (`2026-02-01`, still active, ranks #1 in a normal search). **The vault now confidently serves the STALE fact ("the user drives a Tesla") as current truth** — the exact failure the whole product thesis exists to prevent. `as_of` seeded `valid_from` correctly (verified in storage), so the INPUT was right; the **resolution step chose the wrong fact to retire.** It also false-flagged the C9NORM survivor (`stale_count=2`, group of 6).
- **Root-cause hypothesis:** the topic-contradiction pass lets **Phi-4-mini pick which id is stale** (`stale_memory_ids`); on a polluted 6-fact topic it picked the newer one + an unrelated extra. This is the over-flag precision risk flagged last session, now manifesting as an inversion.
- **Fix direction (structural, classifier-not-writer — same principle as dedup):** Phi-4 should only *DETECT* the contradiction; **code retires the fact with the OLDER `valid_from` deterministically** (newest-wins). This makes inversion structurally impossible regardless of how Phi-4 labels the ids. **First step:** source-read `crates/vault-consolidator/src/phases/contradiction.rs` + the topic-contradiction loop in `consolidator.rs` to confirm it's "LLM-picks-stale" (vs an actual comparison bug), then make retirement recency-deterministic, test-first.

**BUG 2 — read pipeline OVER-ABSTENTION / keyword-gating (CRITICAL — A7, the read-path correctness gap).**
Loosely-phrased but answerable reads abstain. Claude Desktop's clean diagnostic: `"what hobby does the user have?"` / "play" / "orchestra" → returns PRECHOBBY (`abstain:false`); but `"what does the user do for fun?"` (zero lexical overlap with "plays the cello in a community orchestra") → `abstain:true`. Same for `"where does the user work?"`, `"what does the user drive now?"`, etc. — **purely-semantic matches with no keyword anchor get floored.** A memory vault that says "I don't know" about facts it holds is broken for the agent-read workload ([[correctness-is-the-product]]).
- **Root-cause hypothesis:** `AbstainingRetriever` (`crates/vault-retrieval/src/strategies/abstain.rs`) gates on **top-1 BM25 (lexical) score < 1.0 → abstain**, and it runs UPSTREAM of the semantic channel + reranker. A no-lexical-overlap query → BM25 top-1 = 0 → abstains immediately; the reranker never gets to judge semantic relevance. (This is why "dog"→"dog" passed but "for fun"→cello did not.)
- **Fix direction (measured):** make the abstain decision **semantic/reranker-aware**, not BM25-keyword-gated — e.g. let the reranker be the relevance/abstain authority, or also consider the semantic channel's top score. **Use the `read_quality_eval` harness to measure** and MUST NOT regress the true-negatives we want (A6 blood-type abstain). Files: `abstain.rs` + the `StructuredReadPipeline` wiring in `vault-app`.

**(Secondary — feeds BUG 1) Topic clustering pollution.** K-means lumped 6 unrelated facts (the two cars + job + hobby + dog + C9NORM) into ONE mislabeled topic `"electric_vehicle_ownership"` (the dog fact landed in the car topic). The contradiction pass ran over this polluted group, amplifying BUG 1's mis-judgment. `crates/vault-consolidator/src/topics.rs`. Lower priority than 1 & 2 but related to BUG 1's root.
  - **Root cause is k-means itself:** it is *forced* to fill a fixed number of groups, so unrelated facts get shoved together rather than left apart.
  - **Candidate fix — switch to a density-based / no-forced-fit clustering algorithm:**
    - **HDBSCAN / DBSCAN** (density-based) — finds *natural* clusters and **leaves outliers ungrouped** (the dog would NOT be forced into the car topic). Strongest candidate.
    - Hierarchical/agglomerative + distance threshold — only groups items closer than a cutoff.
    - Affinity propagation — infers the cluster count itself (no guessing "k").
  - **Caution:** any clustering still runs on BGE-small embeddings, which we *measured* as unreliable at fine distinctions ([[bge-small-cannot-separate-relevant]]) — so calibrate/measure the new algorithm on a real labeled set (as we did for the dedup thresholds) before trusting it; a smarter algorithm on shaky inputs is not a guaranteed win.
  - **Note:** BUG 1's recency-deterministic fix should make A5 robust to topic pollution on its own; this topic-clustering upgrade is a quality improvement that also cleans up the REPORT's topic labels (V1.0+ polish, not a ship-gate).

### Tree / commit state
- `main` HEAD = `2302842` (the 2 small fixes, pushed).
- **Uncommitted (gate-green, HELD pending the 2 fixes):** dedup (ADR-063) + A5 pairwise (ADR-062) + merge resilience + this HANDOFF update + the updated §7 (`Memory Vault Tests.md`). Dedup is committable on its own merit but is entangled with the A5 work in `consolidator.rs`/`merge.rs`; **plan: commit dedup + the A5 fix together after a clean re-dogfood.**
- **Vault state:** `testeval` holds the post-dogfood state (Rivian WRONGLY invalidated, Tesla active, DEDUPDOG deduped to 1). Throwaway ([[dev-vault-is-throwaway-test-data]]) — **wipe before the next dogfood.** Single-writer: fully quit Claude Desktop before any `consolidate run` (DuckDB is single-process RW). Teardown markers: A5CAR ×2, PRECJOB/HOBBY/FOOD, DEDUPDOG ×3 (1 active + 2 superseded), C6COLOR, C7UNI, C9NORM ×2.

### Plan for next session (in order)
1. Confirm `2302842` CI is green.
2. **BUG 1 (A5 polarity):** source-read `contradiction.rs` → confirm root cause → make retirement recency-deterministic (test-first) → DoD gates.
3. **BUG 2 (read over-abstention):** confirm root cause in `abstain.rs` → de-keyword-gate (reranker-aware abstain), measured on `read_quality_eval`, guard A6 → DoD gates.
4. (Optional) topic-cohesion tightening (`topics.rs`).
5. Wipe vault → re-run §7 dogfood (esp. B1 A5 + the A7/B3 reads) → if clean, **commit the whole arc** (dedup + A5 fix + merge resilience) with Shahbaz approval, then CI green.

### Memories to load next session
[[correctness-is-the-product]] · [[bge-small-cannot-separate-relevant]] · [[no-sub-7b-models-for-synthesis]] · [[reference-mcp-dogfood-log-is-ground-truth]] · [[as-of-write-time-blocks-a5-temporal]] · [[byte-equality-probe-before-non-determinism-hunt]] · [[anchor-on-measured-not-projected]] · [[source-read-call-graph-upstream-of-empirical]]

---

## 🎯🎯 NEXT SESSION OPENER (2026-05-30 late) — **[HISTORICAL — merge-gap = dedup ADR-063, BUILT this session 2026-05-31]** — PRIORITY #1: merge consolidation gap (RESEARCH → implement)

**Do this first next session. Research the best solution BEFORE implementing — do not jump straight to code.**

### The problem (surfaced live 2026-05-30)
The consolidator's Phase-2 **merge** step can **skip** a near-duplicate cluster when the LLM merge response fails (e.g. truncated/oversized). This session we made the skip *safe* (resilience + token-budget fixes — see "Current in-flight state" below), so a skip no longer crashes the run and never loses or corrupts data. **But two real gaps remain about what happens to the skipped, still-un-consolidated data:**

1. **Persistent skips never resolve.** Clustering re-runs nightly, so *transient* merge failures self-heal next cycle. But a *structural* failure (content genuinely too large to merge, e.g. the CAP_OK 50K probes) overflows identically every run → those near-duplicates sit un-merged **forever**. Pure retry can't fix a structural overflow.
2. **Skips are log-only.** A skipped cluster emits a `WARN` but is **not counted or surfaced** in `ConsolidationReport` — so you can't see "N clusters never consolidate." Borderline silent.

### What to research (NOT yet decided — find the best approach first)
- **Deterministic dedup for near-identical clusters** (leading candidate): when members are near-identical (cosine ~1.0 — exactly the overflow case), **skip the LLM entirely** — keep the canonical member (longest / highest-confidence / newest) and `mark_superseded` the rest. No LLM call → nothing to truncate → the overflow class disappears. Reserve LLM-merge for genuinely *different-but-related* content. **Research:** where's the cosine cutoff between "deterministic dedup" and "needs LLM merge"? How do other memory/dedup systems split these? Risk of deterministic dedup discarding a better-phrased variant?
- **Report/queue tracking of skipped clusters** — surface a count + ids in `ConsolidationReport` (cheap; closes the silent-skip gap). Possibly queue persistent skips like `conflicts_for_user_review` does.
- **Dead-letter integration** — the CLI already has a `dead-letter` subsystem; could persistent merge-skips be dead-lettered for inspection/retry instead of only logged?

**Approach:** research the options (web + codebase), pick the best, write an ADR, then implement with failing-first tests. This is correctness-of-stored-state — treat it like the A5 work: measured, not guessed. See [[correctness-is-the-product]] and [[spike-playbook-for-unknowns]].

### ✅ RESEARCH DONE (2026-05-30 — 3 parallel agents: competitor scan + dedup-algorithm scan + codebase recon). Next session: write the ADR from this, then implement.

**The convergent answer (all three tracks agree): make the small LLM a CLASSIFIER, not a WRITER, and dedup near-identical content deterministically (no LLM at all).** This kills the truncation/overflow class at the root — a label + an id is ~10 tokens and cannot truncate, no matter how large the memories.

**Recommended design (4 parts):**
1. **Deterministic dedup for near-identical clusters (no LLM).** When members are near-identical — high cosine AND high lexical overlap — pick a **canonical survivor** and `mark_superseded` the rest. Survivorship rule (data-fusion "golden record" standard): **newest `valid_from`/`created_at` → longest/most-complete text → highest confidence → most-accessed**, and **union the metadata + sum `access_count`** onto the survivor (honors BRD §5.6 line 947). The survivor is an EXISTING member — **no new merged row written, no re-embed** (simpler + cheaper than today's `apply_merge`). This is the Zep/Graphiti + Cognee + Mem0g consensus: on dup/update they keep a canonical record + flip a supersede/`valid_until` flag, never re-emit merged prose.
2. **Two-axis routing (cosine is NOT enough — our own measured finding).** Add a cheap lexical axis (token **Jaccard / containment**) alongside cosine: `cos ≥ ~0.92 AND high lexical overlap` → deterministic pick-one (the common case, no LLM); `cos ~0.82–0.92 AND LOW lexical overlap` → genuinely complementary → the ONLY case that earns an LLM call; disagreement on a value → supersede / contradiction (the A5 path, already shipped).
3. **When the LLM IS needed (rare complementary merge): emit a structured delta/field, NEVER echo full text** — with a hard token cap + truncation-fallback (keep the longer original). Keeps the "LLM out of the heavy path" lock.
4. **Clustering: keep union-find connected-components at the high cosine gate** (we already do this), add a **centroid-linkage per-group check** to block transitive over-merge (A~B~C drift). Stay **batch-nightly**. Calibrate the 0.92 / near-identical thresholds on a **hand-labeled BGE-small memory-pair set** — do NOT trust literature numbers (our BGE cosine is measurably unreliable; [[bge-small-cannot-separate-relevant]]).

**Codebase fit (recon):**
- `Cluster` (`phases/cluster.rs:88`) carries **only member ids** — pairwise cosines are computed in `collect_edges_for_memory` then **discarded** (`cluster.rs:299-300`). To gate near-identical vs different, **extend `Cluster` to retain pairwise distances** (capture what's already computed) — cheapest slot.
- Deterministic-dedup path slots into the orchestrator loop **before `decide_merge`** (`consolidator.rs:236`): hydrate members → compute/read pairwise cosine + lexical overlap → if near-identical, pick canonical → `storage.mark_superseded(loser, winner)` for the rest (winner = existing id; no `write_memory`, no re-embed). Else fall through to the existing LLM `decide_merge`.
- Constraints to honor: supersede-not-delete (BRD §5.6 line 948), Σ access_count + max confidence (line 947), `mark_superseded` is metadata-only (ADR-046).
- **Phase 2b contradiction (the just-shipped A5 work) is fully independent** of the merge flow — this redesign does NOT touch `contradiction.rs`/`topics.rs`. Confirmed by recon.

**Key external references:** Zep/Graphiti 3-tier dedup (exact→MinHash/Jaccard→LLM, invalidate-not-rewrite) `arxiv 2501.13956`; Mem0 ADD/UPDATE/DELETE/NOOP classifier `arxiv 2504.19413`; data-fusion "golden record" survivorship (Bleiholder & Naumann, ACM CSUR); SemDeDup/SemHash cosine sweep bands; Letta proposed dup gate `cosine > 0.9` (next to our 0.92 — our gate is already industry-sane). Full citations in the session transcript / to be carried into the ADR.

### Current in-flight state (this session — code is GATE-GREEN but NOT committed)
- **A5 contradiction precision — pairwise rewrite (ADR-062, iter 1 + 2) is in the working tree.** Pairwise judging + few-shot prompt + **index-return (`stale: "a"/"b"/"neither"`, no UUID echo)** + **`shared_attribute` precision gate**. Earlier live run (pre-contamination): **0 false positives, 0 parse failures** vs the prior N-ary run. Unit + integration tests green.
- **Merge resilience + token-budget fixes** (which spawned this opener): Phase-2 loop logs-and-skips a failed cluster instead of aborting the whole run; merge `max_tokens` 256 → 1024. New test `tests/merge_resilience.rs`. **Live-proven 2026-05-30:** the CAP_OK giant cluster overflowed even 1024 tokens and **skipped gracefully — run completed (exit 0)** where it previously crashed (exit 1). The resilience fix WORKS.
- **All 4 DoD gates GREEN** (fmt / clippy `-D warnings` / build zero-warnings / test incl. `malformed_merge_response_does_not_abort_the_run`). **NOT committed, NOT CI-verified.** Commit needs Shahbaz per-action approval, gated on the clean A5 verify below.

### 🟥 NEXT-SESSION TASKS — in order (start fresh here)

**TASK 1 (close out this session): clean A5 end-to-end verify → then commit.**
The 2026-05-30 retest was **CONTAMINATED and is NOT a valid A5 verdict.** Cause: Part-A's 3 giant `CAP_OK` probes were still in the vault and K-means **mis-clustered them into the same topic as the Tesla/Rivian facts** (`topic "Tesla_Rivian_Transition"` held all 3 CAP_OK + Tesla + Rivian). The contradiction pass reported `stale_count=3, group_size=5` — muddied; **cannot confirm which 3 were retired** because `generate_reports` **includes invalidated rows** (logged tech-debt) so the REPORT can't show `valid_until`. The clean PREC facts (separate topic) **all survived — precision held there** (no employment-vs-hobby false positive), which is the one solid A5 signal from the run.
- **Do:** wipe vault ([[dev-vault-is-throwaway-test-data]]: delete `vault.db`/`lance/`/`graph.duckdb`/`reports/`, keep `models/`) → reopen Desktop → seed **ONLY** the 5 B-facts (Tesla, Rivian, PRECJOB, PRECHOBBY, PRECFOOD) — **NOT** Part A's CAP_OK/C6/C7/C9 → close Desktop → `vault-cli consolidate run` (real Phi-4) → reopen → B-verify: "what does the user drive now?" returns **only Rivian**; `include_archived` shows **only Tesla** retired (`valid_until` set); PREC all survive.
- **If clean-green → COMMIT** the in-flight work (A5 pairwise ADR-062 + merge resilience/token + ADR updates + `Memory Vault Tests.md` §7 runbook) in ONE commit (per [[admin-changes-ride-with-code]]), Shahbaz-approved, then **CI green** ([[ci-green-per-commit-vault-code]]). Suggested msg: `T0.3.x A5 pairwise precision (ADR-062) + merge resilience/token-budget fixes`.

**TASK 2 (Priority #1 project): merge consolidation gap — write ADR from the RESEARCH DONE block above, then implement** (deterministic dedup for near-identical clusters + classifier-not-writer + skip-tracking in the report). The "report includes invalidated rows" issue (which blocked TASK-1 verification) overlaps the report-tracking sub-item — fix together.

**TASK 3 (lower priority — after 1 & 2):**

### Lower-priority findings logged this session
- **A7 over-abstention** (read path / reranker): `memory_read "what does the user do for fun?"` abstained despite the cello hobby present. **May be collateral** from the polluted tiny vault (giant CAP_OK blobs crowding BGE top-K) — **re-test on the clean vault first**; if it persists, fix via the `read_quality_eval` harness (measured), NOT a blind reranker-floor nudge (risks regressing A6/false-answers). Separate from A5/merge.
- **`-32603` on all-stopword query**: `memory_search "the a is of and to"` returned an internal error instead of a graceful empty/abstain. Degenerate-query robustness; search path.
- **Settable `as_of` on `memory_write`** (carried from prior session — Step 2): add an optional `as_of` param → `NewMemory.valid_from` (vault-core already accepts `valid_from: Option<DateTime>`), falling back to write-time when absent. Write-only (update keeps ADR-028 date-preservation). Decision locked 2026-05-30 (agent sets the date, layered on write-time as the safety net). See [[as-of-write-time-blocks-a5-temporal]].

### Memories to load next session
[[correctness-is-the-product]] · [[no-sub-7b-models-for-synthesis]] · [[dev-vault-is-throwaway-test-data]] · [[reference-mcp-dogfood-log-is-ground-truth]] · [[spike-playbook-for-unknowns]] · [[anchor-on-measured-not-projected]]

---

> **⚠️ The opener below is from the PRIOR session (A5 SHIPPED). The "Phi-4 precision" item it names as "next" was DONE this session (pairwise rewrite, ADR-062 — see the in-flight block above). `settable as_of` (Step 2) is still open.**

## 🎯 NEXT SESSION OPENER (2026-05-30 — **A5 SHIPPED ✅** · next: Phi-4 precision + settable `as_of`) — READ THIS FIRST

**A5 (knowledge-update / contradiction detection) — the V0.2 ship-gate — is SHIPPED and LIVE-PROVEN on real Phi-4-mini.** Committed this session (hash in `git log`; ADR-060 + ADR-061). The "FIX A5" opener further below is now HISTORICAL.

### What shipped this session
1. **Topic-level contradiction detection (ADR-060 — the A5 fix).** Detection is decoupled from the 0.92 merge gate and now runs over the looser K-means **topic** grouping. New `crates/vault-consolidator/src/phases/contradiction.rs`: per topic of ≥2 facts, Phi-4 returns explicit `stale_memory_ids` (never a whole-group winner-sweep — a topic is a loose grouping, so a winner-takes-all verdict would wrongly retire compatible facts). Wired into `run_consolidation` after the merge pass; re-enumerates the post-merge active set, skips already-invalidated rows, groups via `discover_topics(.., llm=None)` (grouping only — no naming calls), invalidates exactly the model-named stale ids via the existing ADR-051 `invalidate()` path, with a **guard that refuses to retire an entire group** + ignores ids outside the group. Retirement is non-destructive (rows persist with `valid_until`; recoverable; visible in `include_archived` search).
2. **Clustering divergence robustness (ADR-061).** `find_candidate_clusters` panicked (`no entry found for key`, `cluster.rs:294`) on real data: LanceDB returns NN hits for superseded/invalidated memories whose vectors linger but which `list_memories` excludes, so `union_find` looked up a non-node. Fixed: drop NN edges to non-member ids (normal SQLite/Lance divergence hygiene, WARN-logged) + defensive `union_find` (unknown id → own root, never panics). Pinned by `union_find_treats_unknown_edge_endpoint_as_root`. **Caught by the live consolidation dogfood — a latent crash that would hit ANY real consolidation with superseded data, independent of A5.**
3. **macOS reranker CI fix.** The previous push `87d0b72` reddened macOS clippy: `reranker.rs`'s `#[ignore]`d real-model test had `ort_lib` cfg-branches for windows/linux only → `E0425` on macOS (`--all-targets` still *compiles* ignored tests). Added the macOS `.dylib` branch. Rides here so one CI cycle goes green. (Per [[broken-ci-is-regression-not-techdebt]] + [[cfg-gate-transitively-platform-only-items]].)

### Live dogfood — DECISIVE PASS (server-log-verified, real Phi-4-mini)
Ran `vault-cli consolidate run` on the live vault (real Phi-4, run `0a55c212…`), then the §7 [LIVE] batch from Claude Desktop, monitored via a server-log watcher. **Two independent reviews agree** (the §7 run + the advisory Claude's adversarial per-test review):
- **A5 ✅ ship-gate:** "where does the user work now?" → **only Atlas, no Vega**. Both Vega facts (two write-batches) `invalidate()`d; `health: ok` (REPORT_MISSING gone); `topic` populated. The exact session-open bug ("read returns both Vega + Atlas") is FIXED on real models.
- Full §7 green: **A4 ✅ A6 ✅ A7 ✅ C5 ✅** (clean `-32602`, log-confirmed — Claude Desktop's UI collapses it to "Tool execution failed", the log is ground truth) **C6 ✅ C7 ✅ C9 ✅ I2 ✅ K1 ✅ K2 ✅**. C8 not re-run (proven 2026-05-29; auth path untouched).
- Vega confirmed **invalidated-NOT-deleted** via `include_archived` search — ADR-051 bi-temporal semantics working; over-retirements are reversible.

### NEXT STEPS (next session, priority order)
1. **Step 1b — Phi-4 over-flagging precision (the one open item).** Real Phi-4-mini is too eager: it flagged contradictions in non-contradictory topics (the `personal` boundary's 3/3 topics; the `memory_ceiling_probes` topic of 3 distinct probes). On `testeval` the employment detections were *correct*. **NOT ship-blocking** (consolidator is manual-trigger only until the T0.2.6 scheduler; over-retirement is reversible). Try: tighten/few-shot `CONTRADICTION_SYSTEM_PROMPT`; add a confidence/abstain step; or judge **pairwise within a topic** instead of N-ary (N-ary dilutes the signal). Harden against false positives, then re-dogfood. See [[no-sub-7b-models-for-synthesis]] (Phi-4 is the merge-classifier floor; contradiction *classification* is closer to its strength than *synthesis*, but precision needs work).
2. **Step 2 — settable `as_of` on `memory_write`.** Independently confirmed needed by BOTH the §7 dogfood AND the advisory review: `as_of` = write-time; real dates live only in content; so A4/A5 temporal correctness rode entirely on the contradiction mechanism, not `as_of` ranking. Add an optional `as_of` param to `WriteToolParams` → `NewMemory.valid_from` (vault-core ALREADY accepts `valid_from: Option<DateTime>`); falls back to write-time when absent. **Write-only** (update keeps ADR-028 date-preservation). Decision locked 2026-05-30 (Shahbaz: agent sets the date, layered on write-time as the safety net — a forgotten date degrades to write-time, never breaks). See [[as-of-write-time-blocks-a5-temporal]].
3. **Tech-debt / hygiene (logged, not blocking):**
   - Topic-pass auto-invalidations are **tracing-only**, not counted in `ConsolidationReport.contradictions_resolved` (mirrors the existing clear_winner path; report says "0 queued" even when N were invalidated). Surface a count.
   - Ambiguous topic contradictions (model returns empty `stale_memory_ids`) are **not queued** to `conflicts_for_user_review` — V0.2 only auto-resolves the determinable ones. Wire queuing if dogfood shows ambiguous cases matter.
   - `generate_reports` includes invalidated rows in the REPORT (retriever hides them at read, so harmless — but noisy).
   - `testeval`/`personal` eval boundaries aren't isolated (both authorized) — blocks a clean §2 precision baseline + I3 isolation. Want a `testeval`-only MCP session.
   - Probe accumulation in the live vault needs teardown (advisory left: `C6MARKER` vermilion, `C7MARKER` unicode, two `C9MARKER` in testeval).
   - Topic labels imperfect (hiking landed under `career_transition`) — K-means/Phi-4 labeling noise; cosmetic, not a read-correctness issue.

### Memories to load next session
[[contradiction-gated-by-merge-threshold]] · [[as-of-write-time-blocks-a5-temporal]] · [[no-sub-7b-models-for-synthesis]] · [[correctness-is-the-product]] · [[reference-mcp-dogfood-log-is-ground-truth]] · [[feedback-review-prior-session-logs-before-fixing]]

---

### ADR-060 — Topic-level contradiction detection (A5 ship-gate, 2026-05-30)

**Status:** Accepted, shipped 2026-05-30. Closes the A5 ship-gate ([[contradiction-gated-by-merge-threshold]]).

**Context.** Contradiction detection only ran on Phase-1 merge clusters (cosine ≥ 0.92). A knowledge-update contradiction ("works at Vega" → "works at Atlas, having left Vega") is semantically related but sits BELOW 0.92, so the pair never clustered and Phi-4 never judged it — reads returned both stale + current truth. Reproduced live 2026-05-29 (`contradictions queued: 0`).

**Decision.** A new contradiction pass (`phases/contradiction.rs`) runs over the looser K-means **topic** grouping (`discover_topics`, which already co-locates the conflicting pair), SEPARATE from the 0.92 merge gate (which stays for near-duplicate *merging*). `detect_contradiction(group, llm)` returns a `ContradictionVerdict { stale_memory_ids, reasoning }` — **explicit stale ids, never a winner-takes-all sweep** (a topic is a loose grouping; a whole-group verdict would retire compatible facts). The orchestrator invalidates exactly those ids via the ADR-051 `invalidate()` API, with two safety nets: (a) ignore returned ids not in the group, (b) refuse to invalidate an ENTIRE group (≥1 fact must remain current). The judge prompt is conservative (same-subject/same-attribute incompatibility only) and is fed each fact's `as_of`. Failure semantics: per-topic LLM failure or a failed invalidate is logged-and-continued, not a run abort.

**Live-proven** end-to-end on real Phi-4-mini (see dogfood above). **Known limitation:** Phi-4-mini over-flags on some topics (Step 1b precision work); not ship-blocking (manual-trigger only; reversible). Pinned by `tests/contradiction_resolution.rs` (Vega→Atlas retires; co-topical-compatible survives; mass-invalidate refused) + `phases/contradiction.rs` unit tests.

### ADR-061 — Clustering robustness to vector-store / metadata divergence (2026-05-30)

**Status:** Accepted, shipped 2026-05-30. Bug fix surfaced by the A5 live consolidation dogfood.

**Context.** `find_candidate_clusters` panicked `no entry found for key` (`cluster.rs:294`) on the real `testeval` vault. Root cause: `mark_superseded`/`invalidate` update SQLite metadata only — the LanceDB vector lingers — so an NN search returns ids that `list_memories` (default filter) excludes. `union_find_components` then indexed a non-node id and panicked. This SQLite/Lance divergence is an EXPECTED steady-state after any merge/supersede/invalidate, so it crashed every real consolidation run with superseded data — independent of A5.

**Decision.** Two layers: (a) in `find_candidate_clusters`, build a `member_ids` set and **drop NN edges to non-member ids** (normal divergence hygiene, count WARN-logged); (b) make `union_find`'s `find()` **defensive** — an id with no parent entry is its own root (loop simply doesn't run), so the disjoint-set primitive can never panic on input shape. Pinned by `union_find_treats_unknown_edge_endpoint_as_root`. (Forward note: physically GC-ing superseded vectors from LanceDB is the real cure — deferred; this makes clustering robust regardless.)

### ADR-064 — Read-side subject framing for the reranker (Bug-2 over-abstention fix, 2026-06-01)

**Status:** Accepted, fix in working tree (UNCOMMITTED), regression green through the real BGE+reranker stack, all DoD gates green.

**Context.** The live clean-vault dogfood (2026-05-31) showed answerable reads abstaining: `"what does the user do for fun?"` returned `abstain:true` against a vault holding `"Plays the cello in a community orchestra on Sunday afternoons."`. The prior handoff hypothesised the cross-encoder reranker's floor (logit 0) was simply too high for conversational→prose-fact leaps and proposed split-conformal recalibration of the floor. **A measurement (conformal harness over the labelled A7 fixture) FALSIFIED that:** at floor 0 the reranker already separates the A7 relevant set (all logits +0.57…+8.65) from guards (all −7.36…−1.38) cleanly. The live failure was specific to the cello fact, whose distinguishing feature is the **missing subject** — a bare `"Plays the cello…"`. A subject-prefix diagnostic confirmed bidirectionally: adding `"The user"` lifts the cello above the floor (−4.46→+1.75); stripping it collapses known-good A7 facts. The reranker's task instruction matches *"a question about a **user** to a personal fact"*, so a subject-less fragment scores as not-about-the-user and is rejected even for near-literal queries. The agent stores **uncontrolled prose**, so subject-less facts are a permanent real-world input — the fix must be read-side, not write-side canonicalisation (which can't guarantee clean input; locked decision).

**Decision.** Prepend a fixed subject frame — `DOC_SUBJECT_FRAME = "The user — "` — to every candidate document in the production `RerankProvider::rerank` path before scoring. The **relevance floor stays at 0** (it was never the problem). The reranker instruction is parameterised via `format_prompt_with(instruct, query, doc)`; production passes `QWEN3_RERANKER_INSTRUCT` + framed docs, and a `#[cfg(feature="testing")]` `rerank_with_instruction` seam scores raw docs so the framing sweep can measure variants without mutating the production path. The frame was the **measured winner of an A/B sweep** over the full A7 set + the live cello case (`reranker_fun_diagnostic.rs::framing_variant_sweep`): `"The user — "` gave 8/8 relevant above floor (cello for-fun +3.23, music +2.08), 0 guard leaks (no-signal blood-type −6.86), widest gap +1.69 — beating `"The user: "` (fragile), `"About the user: "` (broke 2 cases), and BOTH instruction-hint variants (which leaked a guard). Pinned by `doc_subject_frame_is_pinned` + `rerank_frames_each_doc_with_the_subject` unit tests and the real-model `read_no_keyword_overlap` regression.

**Consequences / known limits.** (a) Double-subjecting facts that already say "The user…" is harmless — measured: those A7 facts stay comfortably above floor when framed (min relevant +1.19). (b) The frame is a single global string; a future re-sweep (e.g. after a reranker model swap) must re-break the pin consciously. (c) Conformal calibration is NOT used as the gate — it stays in the tree as a measurement instrument and remains the right tool if per-query score-drift ever reappears, but floor 0 + framing is the V0.2 fix. (d) The earlier ADR-057 cosine `RELEVANCE_COSINE_FLOOR` path is unaffected (reranker remains the relevance authority when wired).

---

> **⚠️ The "FIX A5" opener below is HISTORICAL — A5 shipped 2026-05-30 (see the block above).**

## 🎯 NEXT SESSION OPENER (2026-05-29 — **FIX A5: contradiction detection**) — HISTORICAL

**Start here. The cross-encoder reranker arc SHIPPED this session (read-path correctness — see "Reranker arc — SHIPPED" just below). The next correctness battle, and the V0.2 ship-gate, is A5.**

### The problem (exact — live-confirmed 2026-05-29)
**A5 = knowledge-update / contradiction detection.** When a newer fact contradicts an older one on the same subject (canonical case: "the user works at Vega Bridgeworks" → later "the user works at Atlas Structures, having left Vega"), the consolidator must: **detect the contradiction → `invalidate()` the stale fact → reads return ONLY the current truth.**

**It does not.** A live `vault-cli consolidate run` on `testeval` (2026-05-29, both Vega + Atlas facts seeded) returned **`contradictions queued: 0`** — the contradiction was never detected. (Merges work — it correctly dedup-merged an identical pair — so the consolidator runs end-to-end; it just never sees the contradiction.)

**Root cause #1 — the clustering gate (primary blocker).** Phase-1 clustering (`crates/vault-consolidator/src/phases/cluster.rs`) groups memories by cosine **≥ 0.92** (union-find transitive closure). Contradictory facts are semantically *related* but their pairwise cosine sits **below 0.92**, so Vega and Atlas **never land in the same cluster** → Phi-4's judge (`phases/merge.rs::decide_merge`) never receives them as a pair → 0 contradictions. This is [[contradiction-gated-by-merge-threshold]], now reproduced LIVE (not just spike).

**Root cause #2 — `as_of` is write-time, not fact-time.** `memory_write`/`memory_update` capture `as_of` = the write timestamp; the real-world date in the content ("As of 2026-04-01…") is NOT parsed into `valid_from`. So even once facts cluster, "which is current?" can't be decided by recency. **Open product decision for Shahbaz (do NOT fold into spec silently): (a) add a writer-settable `as_of` field to the write/update tools, or (b) extract dates from content.** See [[as-of-write-time-blocks-a5-temporal]].

### Fix direction (parked, ready for plan iteration 1)
**Decouple contradiction detection from the 0.92 merge gate — run it at the K-means TOPIC-CLUSTER level.** K-means topic discovery (`crates/vault-consolidator/src/topics.rs`, shipped Batch A) already co-locates Vega + Atlas in one topic (both "employment"), even though their pairwise cosine is < 0.92. So: within each K-means topic, run a contradiction pass (Phi-4 judges the set) SEPARATE from the 0.92 near-duplicate MERGE clustering. The 0.92 gate stays for merging; contradiction detection moves to the looser topic grouping. First task next session = plan iteration 1 on this.

### What's already built (don't re-discover)
- Phase 1 clustering (cosine 0.92, union-find) — `phases/cluster.rs`.
- Phase 2 Phi-4 `decide_merge` → `MergeOutcome::{Merge, KeepSeparate, Contradiction}` — `phases/merge.rs`.
- `invalidate()` bi-temporal API (ADR-051) — consumed on a `clear_winner`.
- K-means topic discovery — `topics.rs` (already co-locates the contradiction pair — the lever).
- REPORT generation + read pipeline — shipped; REPORTs now written live (K1/K2 ✅: `personal.report.json` + `testeval.report.json` on disk under the vault data dir).
- The **reranker (read-path)** is DONE + live-proven (A6/A7) — it does NOT touch A5 (separate engine; don't conflate).

### Memories to load
[[contradiction-gated-by-merge-threshold]] · [[as-of-write-time-blocks-a5-temporal]] · [[project-reranker-qwen3-solves-model-fit]] · [[correctness-is-the-product]] · [[no-sub-7b-models-for-synthesis]] · [[parallel-agent-pairs-for-strategic-decisions]]

---

## ✅ Reranker arc — SHIPPED this session (2026-05-29)

The read-path model-fit problem ([[bge-small-cannot-separate-relevant]]) is **fixed and live-verified.** A cross-encoder reranker (**Qwen3-Reranker-0.6B**, seq-cls, Apache-2.0) re-scores BGE's top-8 retrieved candidates and gates on its own relevance score, **replacing the ADR-057 cosine-0.66 floor**.

**How we got here:** comprehensive model search → `reranker_spike.rs` bake-off (BGE / ms-marco / gte-modernbert / Qwen3) on a hardened A7 fixture incl. topically-adjacent "wrong-attribute" traps. Only Qwen3-Reranker + a strict-yes/no instruction separated cleanly (0 false-answers, 8/8 recall, 8/8 ranking at a logit-0 cutoff). gte collapsed on the hard cases; the others failed. Full detail: [[project-reranker-qwen3-solves-model-fit]].

**Live dogfood (§7, Claude Desktop, server-log-verified):** **A6** abstains on no-signal ("blood type"); **A7** no longer over-abstains — a loose paraphrase ("outdoors for fun on weekends") surfaced the hiking fact. The exact over-abstention BGE couldn't handle is fixed. C4–C9 + I2 also passed.

**Code (this commit):**
- `crates/vault-embedding/src/reranker.rs` — `RerankProvider` trait + `Qwen3RerankerProvider` (batched, left-padded, f16-aware via ort `half` feature, `spawn_blocking`); v4 instruction + logit-0 floor as calibrated consts.
- `crates/vault-embedding/src/ort_init.rs` — shared process-global ORT init (extracted from `bge_small.rs` so embedder + reranker don't double-init).
- `integrity.rs` — SHA-256 pins for the reranker model + tokenizer; `Cargo.toml` — `half` promoted to a real dep + ort `half` feature.
- `crates/vault-retrieval/src/structured_read_pipeline.rs` — `with_reranker` builder + `apply_reranker` (rerank top-8 → filter ≥ floor → re-sort → abstain if none pass); reranker takes precedence over the cosine gate (cosine retained as no-reranker fallback). 4 MockReranker tests.
- `crates/vault-app` (config + application) + `vault-cli` (`--rerank-model`/`--rerank-tokenizer` args, env fallbacks) + `vault-tauri`/integration/harness construction sites.
- Spike + hardened fixture (`reranker_spike.rs`, `read_quality_eval.json`) + read-quality harness ride here per [[commit-only-with-tested-fix]].

### ADR-059 — Cross-encoder reranker as the read relevance gate (supersedes ADR-057's cosine floor)

**Status:** Accepted, 2026-05-29. Supersedes ADR-057's deterministic cosine-0.66 floor for read relevance (ADR-057 itself foresaw this: "deferred to a non-LLM cross-encoder reranker at V1.0+" — pulled into V0.2 because the over-abstention was a live dogfood blocker).

**Decision.** `StructuredReadPipeline`, when a reranker is wired (`Application` opens `Qwen3RerankerProvider` iff `rerank_model_path` + `rerank_tokenizer_path` are configured), reranks the top **8** retrieved candidates with Qwen3-Reranker-0.6B (seq-cls, f16) under the **v4 strict-yes/no instruction**, keeps those scoring **≥ logit 0** (sigmoid 0.5), re-sorts by reranker score, abstains if none pass. The cosine `with_relevance_gate` path is retained as the no-reranker fallback (graceful degradation for deployments without the ~1.2 GB model).

**Calibration.** `reranker_spike` hardened A7 set, v4 instruction: every real answer scored > 0, every guard (incl. adjacent-attribute traps) < 0, ~3-logit margin. Top-8 = recall-first (Shahbaz's call).

**Latency (known, deferred).** ~12–13s/read on CPU (Intel iGPU box) for top-8; cold-first ~36s; sub-second on GPU. int8 + GPU are the fast-follow (int8 local-quant from the f16 export hit a tooling wall — needs a clean fp32 optimum re-export). Per [[project-correctness-before-latency]]: correctness proven first, latency next.

**Bundled:** the shared-ORT-init extraction is required (not drive-by) so the embedder + reranker share one `ort::init`.

> **⚠️ The opener below ("meaning-similarity calibration") is HISTORICAL — superseded by the A5 opener above.**

## 🎯 NEXT SESSION OPENER (2026-05-29 close — meaning-similarity calibration) — HISTORICAL

**This supersedes the older "Commit 8 close" opener further down (now historical).**

### Where we are
- ✅ **Commit 10 shipped + CI GREEN** (`474c367`): ADR-058 wired per-boundary REPORT generation into the consolidation run. `vault-cli consolidate run` now writes `<vault_root>/reports/<boundary>.report.json`; live-verified — Claude Desktop reads return `health: ok` + topics. That work is DONE and committed.
- 🔬 **Active workstream: "meaning-similarity calibration."** Two live-dogfood findings (2026-05-29) share ONE root cause:
  - **A7 — read over-abstention:** `memory_read` says "I don't know" to questions it HAS answers to (5/6 of realistic queries in the fixture).
  - **A5 — contradiction not retired (ship-gate):** the Vega→Atlas contradiction never clusters (cosine < 0.92), so the consolidator never detects it. See [[contradiction-gated-by-merge-threshold]].

### Tests already run this session (all measured, not guessed)
1. **Read-quality baseline harness** (`crates/vault-app/tests/read_quality_eval.rs`, `#[ignore]`, real BGE) against the 8-case A7 fixture (`crates/vault-retrieval/tests/fixtures/read_quality_eval.json`): BGE-small top-1 cosine — real-answer cosines `0.461–0.751`, guard cosines `0.525, 0.593` → **interleave, NOT separable by any floor** (a correct answer at 0.461 scores BELOW an unrelated guard at 0.593).
2. **BGE query-instruction prefix** (model-card s2p usage, query only): measured in the same harness → **did not help** (slightly *lowered* real answers, e.g. Lisbon 0.461→0.431; still not separable).
3. **Cross-encoder re-ranker spike** (`crates/vault-embedding/tests/reranker_spike.rs`, `#[ignore]`, real ONNX) with `ms-marco-MiniLM-L-6-v2`: **integration VALIDATED** (reproduces the model card's reference example to 3 decimals: relevant 8.846 / irrelevant −11.246) — but on OUR data it **also does not separate** (real −10.8..+5.7, guards −11.1..−6.3; only literal-overlap case scored positive). It *did* rank the right fact #1 in 5/6 cases though.

### THE CORE FINDING
**Our data shape is out-of-distribution for off-the-shelf, web-search-trained models.** Ours = *conversational questions* ("is the user bothered by bright screens?") matched to *short first-person personal facts* ("…finds light themes straining"). BGE-small (bi-encoder cosine) and MS-MARCO MiniLM (cross-encoder) both fail to separate relevant from irrelevant by an absolute threshold on this shape — confirmed with hard numbers, not hypothesis. It is **not a threshold-tuning problem and not a "rerankers don't work" problem — it's a model-fit problem.** Full detail + the ruled-out cheap levers (K-means, threshold, prefix) in [[bge-small-cannot-separate-relevant]].

### FIRST THING NEXT SESSION
**Run a comprehensive web search to identify the RIGHT model for our specific requirement** before downloading/wiring anything else. Requirements to search against:
- Fits our shape: **conversational / question → short personal-fact relevance** (asymmetric, QA-style or instruction-tuned retrieval/rerank — NOT pure MS-MARCO web search). Candidates to investigate: `bge-reranker-base` / `bge-reranker-v2-m3`, newer instruction rerankers (mxbai-rerank, jina-reranker-v2), Qwen3-Embedding/-Reranker, etc.
- **Local-runnable** (on-device for Local mode — small/base size, ONNX-able to reuse our `ort` stack). Per [[three-mode-deployment]].
- **🔑 LICENSE: must be MIT or Apache-2.0 (commercial use explicitly allowed).** This is a hard gate — verify each candidate's license before shortlisting. Reject non-commercial / research-only / restrictive licenses outright.
- Then test the shortlist against the **already-validated spike instrument** — swap the model path in `reranker_spike.rs` (or add an embedder to `read_quality_eval.rs`), re-run, read the separability + the abstention confusion matrix in ~10s each. If a model separates (real answers clear, guards stay below, FALSE-ANSWER = 0), THAT is the fix → wire it in, then commit harness + fixture + spike + fix together.
- If NO off-the-shelf model separates: fall back to (a) fine-tune a small reranker on our data shape, or (b) rethink abstain — note relative ranking already works 5/6, so abstain may not need an absolute-score threshold at all.

### Uncommitted working tree (rides with the eventual fix — do NOT commit alone, per [[commit-only-with-tested-fix]])
- `crates/vault-app/tests/read_quality_eval.rs` (harness, real-BGE, `#[ignore]`)
- `crates/vault-retrieval/tests/fixtures/read_quality_eval.json` (8-case A7 fixture, from Claude Desktop)
- `crates/vault-embedding/tests/reranker_spike.rs` (validated re-ranker instrument)
- `crates/vault-embedding/test-fixtures/ms-marco-minilm-l6-v2/` (downloaded model — keep or delete depending on whether MiniLM stays a candidate)
- `Memory Vault Tests.md` is COMMITTED (rode with Commit 10).

### Parked / secondary items (don't lose, but not the main thread)
- **A5 fix direction:** contradiction detection at the **topic-cluster** level (K-means already co-locates Vega+Atlas), not behind the 0.92 merge gate. Same model-fit root — a better model may also lift clustering.
- **§0 spec amendment** (`Memory Vault Tests.md`): change `test.eval` → a **dot-free authorized eval boundary** (dots are `-32602`-rejected before auth). Unblocks I3 + clean eval isolation. One-line MCP config edit + Claude Desktop restart.
- **Claude Desktop [STORAGE]-tier hand-offs:** C4 50K/100K byte-intact sweep (note: >100KB clean-rejection already unit-tested at `vault-core/src/memory.rs:182`); I2 storage-half (row+embedding+cascade gone, no orphan).
- **Minor:** `generate_reports` includes invalidated rows in the REPORT (retriever hides them at read, so harmless) — `vault-consolidator`; Unicode NFC/NFKC normalization decision (dedup correctness); `memory_delete` `{deleted}` no-op indistinguishable (dogfood #8).

### Memories to load next session
[[bge-small-cannot-separate-relevant]] · [[contradiction-gated-by-merge-threshold]] · [[commit-only-with-tested-fix]] · [[correctness-is-the-product]] · [[architectural-lock-llm-out-of-read-path]] · [[three-mode-deployment]]

---

## 🆕 Current state

**Live arc:** [[locked-next-arc-t03x]] (amended 2026-05-26) — T0.3.x consolidator-driven structured-fact read pipeline + founder-dogfood. Phase C (write-time decision loop) DEFERRED to V1.0+. Four-step sequence:

1. ✅ **MCP `memory.write` description hardening** — shipped at `93d1410` (2026-05-25). Canonical-save contract in tool description + `vault-app::normalization` server-side helper + JSON-RPC wire-level pin test.
2. ✅ **Consolidator → REPORT (structured per-boundary state)** — shipped at **T0.3.x Batch A** (2026-05-26). K-means topic discovery (`crates/vault-consolidator/src/topics.rs`) + per-boundary REPORT artifact with atomic write (`crates/vault-consolidator/src/report.rs`) + `invalidate()` auto-resolution in the Phase 2 contradiction branch when the LLM surfaces a `clear_winner`. Phi-4-mini placeholder fallback when LLM unavailable → `TOPIC_NAMES_UNAVAILABLE` health-warning surfaces at read time (Batch B). ADR-053 rides here.
3. ✅ **Read returns structured facts — NO LLM at read** — shipped at **T0.3.x Batch B Commit 6** (`99052f2`, 2026-05-26). Qwen-7B's 86s synthesis replaced with `StructuredReadPipeline`: retrieve top-K (existing BGE + Tantivy + RRF + abstain) → filter by relevance + relevance-threshold abstain → structure into JSON facts → return via MCP. The calling agent (Claude / GPT / Codex / Kimi) composes its own response from the structured facts. Read latency: ~500ms total. ADR-052 + ADR-054 + ADR-053 Amendment 1 all shipped here.
4. ✅ **Wire consolidator into runtime + manual trigger** — runtime wiring shipped at **T0.3.x Batch A** (`f0cc158`). `Application::run_consolidation_with_safety` wraps `Consolidator::run_consolidation` in a cross-process lockfile (RAII guard at `crates/vault-app/src/consolidator_lock.rs`) + 30-min hard timeout + tracing span with `run_id`. CLI entrypoint: `vault-cli consolidate run --bge-model ... --bge-tokenizer ... --ort-lib ... --phi4-model ...` with `VAULT_*_PATH` env-var fallbacks. ⏳ Founder-dogfood via Claude Desktop's MCP lands at **Commit 8** (final Batch B commit) after Commit 7's Contract-4-retirement cleanup ships.

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

**Last CI run:** `99052f2` (T0.3.x Batch B Commit 6 — structured-fact read pipeline) — **GREEN matrix-wide** (ubuntu / macos / windows × build+test + clippy + fmt; weekly real-model smoke correctly skipped). Previous run: `f0cc158` (Batch A) also GREEN. The 3-commit CI-green chain since `08901bf` (T0.2.3 close) holds: 08901bf → f0cc158 → 99052f2 all matrix-clean.

**Working tree at this update:** clean post-`99052f2` push. The "Commit 6 shipped, opening Commit 7" rewrite below (this HANDOFF.md edit) is the only admin ride-along expected to accompany the Commit 7 code commit per [[admin-changes-ride-with-code]]. The full "Commit 6 shipped" rewrite did not bundle with `99052f2` itself; rolling it forward with Commit 7 closes that gap.

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
| ~~**Append-only delta log**~~ | ~~A → C~~ | ~~Track writes that landed since last consolidation run; read pipeline merges with REPORT at query time~~ | **RETIRED 2026-05-27** — falsified by Commit 6's retriever-primary architecture. Retriever queries SQLite/Lance directly; today's writes are visible the moment SQLite commits. See ADR-054 Amendment 2. |
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

- **Adopt now**: K-means topic discovery + structured filter/pack code (delta log RETIRED 2026-05-27 — see ADR-054 Amendment 2)
- **Keep using**: HNSW + cascading writes + hashing + CoW-via-SQLite/Lance + Phi-4-mini at night + BGE embedder + Tantivy/RRF/abstain
- **Park for sync (V0.2.9-13)**: Cuckoo filters
- **Park for V1.0+ Managed**: per-tenant sharding (we get it naturally), the consensus/replication stack (likely use managed DB, don't roll our own)
- **Don't reach for**: Bloom, Z-order, quad tree, skip list, external sorting

The architectural lock **simplified** the menu rather than complicated it. The vault needs brilliant plumbing (filter + structure + pack), not exotic structures.

---

## 🎯 Next-session opener — Commit 8 close: live MCP connection + dogfood fixes (2026-05-28)

Read this whole block before any new work. **The detailed Step 1–4 plan further down (Commits 7 + 8) is now HISTORICAL** — Commit 7 shipped and Commit 8 grew during live dogfood. This block is the current truth.

### Where we are (2026-05-28)

- **Commit 7 shipped** at `f6293c6` (2026-05-27, CI green) — Contract 4 retirement + ADR-054 Amendment 2.
- **🎉 Memory Vault connected LIVE to Claude Desktop** via `vault-cli mcp serve` over MCP stdio. Full handshake, all 5 tools enumerated, real write→read round-trips. First time the product ran end-to-end against a real external agent.
- **Commit 8 is in the working tree, NOT yet committed.** Local DoD gates ALL GREEN (fmt / check --all-targets / clippy -D warnings / test). `vault-cli` binary rebuilt. **Live re-verification of the 3 dogfood fixes was the next step when the session ended.**

### What Commit 8 contains (working tree, uncommitted)

**(a) MCP entrypoint** (drafted before this session): `vault-cli mcp serve` subcommand binds rmcp stdio via `Application::start_with_mcp`; new public `ApplicationHandle::wait()`; `phi4_model: Option<PathBuf>`; `--boundary` repeatable (default `["personal"]`). **ADR-055.** Files: `vault-cli/src/main.rs`, `vault-app/src/application.rs`.

**(b) Two correctness fixes from connecting** (this session):
- **Tool-name rename `memory.X` → `memory_X`** (dots → underscores). Claude Desktop's MCP client rejects tool names containing dots (regex `^[a-zA-Z0-9_-]{1,64}$`); the server connected but no tool was usable until renamed. Touched all 5 `#[tool]` decorators + every doc/test/audit reference in `vault-mcp` + cross-crate doc comments. **vault-storage audit event-type taxonomy (`memory.create`/`memory.read`/… in `vault-storage/src/audit.rs` + migration 0001) deliberately LEFT dotted** — separate persisted-string concept, not an MCP tool name; renaming would break audit-log parsing of existing rows.
- **Tracing → stderr** (`vault-cli/src/main.rs::init_tracing` now `.with_writer(std::io::stderr)`). MCP stdio reserves stdout for JSON-RPC; tracing on stdout corrupted the channel (the original symptom: Claude Desktop "Unexpected token" parse errors).

**(c) Three dogfood-surfaced fixes** (this session — **ADR-056** below):
- **#0 Keyword-index maintenance on write** — a fresh write was invisible to search/read until the next server restart (in-RAM BM25 index bulk-loaded at boot, never updated on write — documented Phase-1 gap at `application.rs:234`). Fix: `VaultAdapter` holds the shared `Arc<KeywordIndex>` and upserts on write/update, deletes on delete (best-effort + WARN; SQLite is source of truth).
- **#3 Delete idempotency** — deleting a missing id returned `-32602 not found`, contradicting the tool's documented "idempotent" contract. Fix: `handle_delete` returns `Ok(())` when `lookup_boundary` finds nothing.
- **#7 Content cap** — canonical-save normalizer rejected content > 2000 chars, contradicting both vault-core's real 100 KB cap and the consolidator's paragraph-scale fixtures. Fix: removed the redundant 2000-char reject; vault-core's `MAX_MEMORY_CONTENT_BYTES` (100 KB) is the sole length gate, embedder truncates at 512 tokens (store-whole / embed-truncate).

### Files modified in the Commit 8 working tree

- **vault-app:** `src/adapter.rs` (keyword-index field + `maintain_keyword_index_upsert`/`_delete` helpers + 3 call sites + 2 regression tests + fixture wiring), `src/application.rs` (`ApplicationHandle::wait` + passes `keyword_index.clone()`), `src/normalization.rs` (#7 cap removal), `tests/integration_smoke.rs` (rename)
- **vault-cli:** `src/main.rs` (mcp serve subcommand + stderr tracing)
- **vault-mcp:** `Cargo.toml`, `examples/macro_spike.rs`, `src/adapter.rs`, `src/audit.rs`, `src/lib.rs`, `src/server.rs` (rename + #3 handle_delete + #7 description), `tests/{common/mock_adapter.rs, common/mod.rs, error_mapping.rs, initialize_smoke.rs, tool_invoke.rs, trust_boundary.rs}`
- **doc-only (rename ripple):** `vault-core/src/memory.rs`, `vault-consolidator/src/report.rs`, `vault-retrieval/Cargo.toml` + `src/retriever.rs` + `src/structured_read_pipeline.rs`, `vault-storage/src/audit.rs` (taxonomy strings unchanged — only the MCP-tool-name doc line)
- **HANDOFF.md** (this update)

### Testing stage — DONE: all 3 fixes live-verified (2026-05-28)

- ✅ Local DoD gates all green on the full working tree. 4 new tests pass: `write_makes_memory_searchable_in_keyword_index_without_restart`, `delete_removes_memory_from_keyword_index`, `tool_delete_missing_id_is_idempotent_success`, `accepts_long_content_above_former_2000_cap`.
- ✅ `vault-cli` binary rebuilt with all fixes (`target/debug/vault-cli.exe`).
- ✅ **Live-verified in Claude Desktop, confirmed against the server log (ground truth):**
  - **#0** wrote `019e6e01…` then `memory_read` 6s later in the SAME chat (no restart) → returned it, `abstain:false`. Read-after-write works.
  - **#7** ~2000+ char write accepted + read back intact (was rejected before).
  - **#3** missing-id delete → `{"deleted":…}` success, `isError:false`.
- **Ground-truth note for future sessions:** the MCP log at `%APPDATA%\Claude\logs\mcp-server-memory-vault.log` shows actual `tools/call` requests + server responses. Trust it over Claude Desktop's prose — its UI collapses JSON-RPC errors to a generic "Tool execution failed", and it can echo stale tool descriptions from earlier in a conversation / its loaded project context.
- **Rebuild gotcha:** Claude Desktop holds the `.exe` lock as a running MCP child. To rebuild `vault-cli` you must fully quit Claude Desktop first; `Get-Process vault-cli` then `Stop-Process -Force` if it lingers. Clear the MCP log (`> $LOG`) before each fresh live test.

### Next steps — pick up here (verification PASSED, nothing blocking)

1. **Commit + push Commit 8** (NOT yet done — needs Shahbaz's explicit per-action approval per [[confirm-before-commit-push]]). Admin (ADR-056 + this HANDOFF) rides with the code commit per [[admin-changes-ride-with-code]]. Suggested message: `T0.3.x Batch B Commit 8: MCP serve entrypoint + tool-name rename + dogfood fixes (ADR-055, ADR-056)`. Then CI green check before anything else per [[ci-green-per-commit-vault-code]].
2. **Consolidation dogfood (highest-value next test).** Run `vault-cli consolidate run` on the `personal` boundary, then re-read. This is the single move that unlocks the entire untested surface: K-means topic discovery, Phi-4 topic labels, the REPORT-present read path (`topic` is `null` on every fact today because consolidation has never run), and it clears the `degraded`/`REPORT_MISSING` health so we can see the `ok` path. Needs Phi-4 GGUF (already on disk per pre-dogfood gating).
3. **Read-precision characterization (the agent-read differentiator — own session).** Live reads on the tiny ~5-memory `personal` vault return ALL memories regardless of query relevance — expected small-N behavior, NOT a regression (retriever core is validated at SCALE=10K, 9/9 quality, Phase 5). But the `StructuredReadPipeline` relevance filter has never been characterized on a realistic vault. Load the ~100-memory fixture, run varied queries, measure signal-vs-noise. Note: `memory_read` exposes no caller-side score threshold (only `memory_search` does) — the read pipeline's internal filter is the only relevance gate. Ties to [[correctness-is-the-product]].

### Deferred (NOT in Commit 8 — decide later)

- **#1 Opaque errors in Claude Desktop** — NOT our bug. The server emits correct structured JSON-RPC errors (verified in the log: `-32001`/`-32602` + messages); Claude Desktop's UI collapses them to "Tool execution failed." Optional future enhancement: surface actionable errors as `isError` results so agents can react — trades against ADR-024 no-info-leak. ADR-level.
- **#4 Boundary names with dots** — `project.memory-vault` / `work.acme` are rejected by `Boundary::new`, yet the `memory_write` boundary-field description still lists them as examples. Decide: allow dots, or fix the examples + error message.
- **#2 REPORT_MISSING severity** — young vault with no REPORT surfaces `health.status: degraded`, which is noisy/cry-wolf. Consider info-level instead.
- **#6 Write-success echoes only `{id}`** — no content/byte echo, limits client-side round-trip verification. Minor.
- **#8 Delete no-op indistinguishable from real delete** (dogfood 2026-05-28, consequence of the ADR-056 #3 idempotency fix) — `memory_delete` returns `{"deleted": id}` whether a memory existed or not, so an agent narrating its actions could tell the user "done, forgotten that" when nothing was there. The cascade already computes a `deleted: bool` (`cascading.rs`); surface it as an `existed: false` flag or outcome enum. Pre-beta polish (delete is irreversible + agents narrate), not ship-blocking.
- **Content-limit contract: already fixed in Commit 8.** The `memory_write` content-field description now says "no hard length cap (~100 KB), only the first ~2000 chars feed the embedding" — vault-core's `MAX_MEMORY_CONTENT_BYTES` (100 KB) is the real ceiling and rejects past it with a clean `InvalidInput` (already unit-tested). If Claude Desktop still shows a "hard 2000" limit, it's reading a stale cached schema, not the live binary.
- Optional: remove dead Qwen-7B Rust code (`Qwen25_14BProvider`) + `AppConfig.qwen_model_path` (Commit 8's original optional scope).

### ADR-056 — Dogfood-surfaced correctness fixes (Commit 8, 2026-05-28)

**Status:** Accepted, T0.3.x Batch B Commit 8 (2026-05-28). Surfaced by the first live Claude Desktop MCP connection + founder dogfood.

**(a) Keyword-index maintained inline on write/update/delete.** The in-RAM Tantivy BM25 index (`vault-retrieval::KeywordIndex`) is bulk-loaded from SQLite at `Application::new` but was never updated on subsequent writes — a documented Phase-1 gap. Because the `AbstainingRetriever` gates on the BM25 channel, a memory written after boot was invisible to both `memory_read` and `memory_search` until the next restart re-ran the bulk-load. Decision: `VaultAdapter` holds the same `Arc<KeywordIndex>` the retriever's keyword channel queries and maintains it inline — `upsert` after `write`/`update`, `delete` after `delete`. Best-effort with a loud WARN: the durable SQLite write is the commit point; the in-RAM index rebuilds from SQLite on every restart, so an index hiccup self-heals. Read-after-write contract is now **a write is searchable the instant the call returns** (BM25 inline; the async Lance vector leg lags <1s but RRF fusion carries the result). Pinned by `write_makes_memory_searchable_in_keyword_index_without_restart` + `delete_removes_memory_from_keyword_index`.

**(b) Delete is idempotent on missing ids.** `handle_delete` previously returned `VaultError::NotFound` (`-32602`) when `lookup_boundary` found no memory — contradicting the `memory_delete` tool description's documented idempotency contract. Decision: return `Ok(())` for a missing memory. Nothing exists to auth-gate and returning success leaks nothing an attacker couldn't already infer from the prior not-found-vs-access-denied split. Pinned by `tool_delete_missing_id_is_idempotent_success`.

**(c) Content-length cap is store-whole / embed-truncate.** The canonical-save normalizer rejected content > 2000 chars — a "sanity cap" contradicting both vault-core's real `MAX_MEMORY_CONTENT_BYTES` (100 KB) and the consolidator's fixtures (paragraph-scale memories up to ~2.4 KB designed to exercise embedding truncation). Decision: removed the normalizer's 2000-char reject. vault-core's 100 KB cap is the single length gate; the embedder truncates at its 512-token window. Long memories are stored whole; only the embedding is truncated. Pinned by `accepts_long_content_above_former_2000_cap`. Confirmed with Shahbaz 2026-05-28.

---

### ADR-057 — Deterministic cosine relevance gate for `memory_read` (Commit 9, 2026-05-28)

**Status:** Accepted, T0.3.x Batch B Commit 9 (2026-05-28). Surfaced by the §7 dogfood **A6 failure** (`memory_read` returned the whole boundary, never abstained on no-signal); mechanism chosen by the parallel agent pair (4/4 convergence) + measured calibration.

**Context.** `memory_read` only abstained on literally-empty retrieval, so a no-signal query (e.g. "what is the user's blood type") returned the entire boundary — a confident answer from nothing. Root cause: **ADR-052 removed the LLM from the read path but never reassigned its relevance-judgment job.** `abstain.rs`'s BM25-top-1 gate was deliberately built to catch only gibberish (its own module doc: "the LLM is the only correct gate"). The canonical "The user…" format makes the subject token corpus-wide, so no keyword/RRF-derived floor can separate signal from noise — the agent pair rejected an RRF-floor and elbow/gap detection on exactly these grounds; **cosine is the only channel that's an absolute semantic-relatedness measure, immune to the shared-subject-token degeneracy.**

**Decision.** A deterministic **raw-BGE-cosine relevance gate** in `StructuredReadPipeline` (**Approach P** — placed in the read path that has the bug; `memory_search` stays a raw-retrieval primitive and the `AbstainingRetriever` / `abstain_tests` suite is left untouched). Signal = **semantic top-1 cosine**; floor = **0.66**; abstain when below. Wired via `with_relevance_gate()` (mirrors the existing `with_clock` builder, gate opt-in so the other pipeline tests are unaffected); `score_threshold` stays an agent override, never the gate.

**Calibration** (`abstain_channel_diagnostic`, n=5 no-signal probes — 1 clean-distant + 4 near-topic adversarial). On the **top-1** column: no-signal ≤ 0.642, the four must-proceed contradictions ≥ 0.696 → **0.66** sits in that gap (slight recall bias toward proceeding).

**Over-abstain amendment (2026-05-28, live dogfood — supersedes the original top-3-mean choice).** The agent pair + fixture measurement first chose **top-3 mean** (wider fixture gap: 0.070 vs top-1's 0.054). Live dogfood on the sparse personal vault **falsified that for the real-world case**: a real query whose answer was present (the zafflang memory) **over-abstained**, because top-3 mean diluted the single strong match (~0.72) with two unrelated fillers (~0.45) below the floor — while raw `memory_search` found the memory instantly. Switched to **top-1** (cannot be diluted by fillers). Principle, per Claude Desktop: for a memory vault, **recall > precision — hiding a real memory is the worst failure**, worse than occasionally returning a marginal one. Re-validated live + server-log confirmed: blood-type abstains (`abstain=true`), zafflang proceeds (`abstain=false`, memory returned). Pinned by `top1_proceeds_on_single_strong_match_amid_weak_fillers`.

**Scope (load-bearing — what V0.2 fixes and what it knowingly does NOT):** V0.2 = deterministic raw-cosine floor for **no-signal abstention only**. Topical-noise discrimination (the Q21 class — top-1 **0.717**, above two must-proceed contradictions) is **structurally impossible for a cosine-top-1 floor — measurement-confirmed, not hedged** — and is deferred to a **non-LLM cross-encoder reranker at V1.0+**. This ADR fixes *confident-answer-from-nothing*; it does NOT fix *confident-answer-from-topically-adjacent-but-wrong*. (This is Shahbaz's verbatim scope line — the pivot back to top-1 made it literally accurate again.)

**Vestigial BM25 gate:** the BM25 abstain gate in `AbstainingRetriever` is left vestigial; superseded in effect by the pipeline cosine gate; formal retirement deferred to the per-candidate-precision / carry-cosine-through work (see Tech-debt — read-relevance follow-up).

**Pinned by** (`structured_read_pipeline.rs::tests`): `relevance_floor_and_top_k_are_pinned` (floor 0.66 + K=1), `no_signal_query_abstains_when_top_k_mean_below_floor` (A6), `genuine_content_proceeds_when_top_k_mean_above_floor` (no over-abstain), `contradiction_band_proceeds_at_lowest_measured_cosine` (0.696 proceeds), `top1_proceeds_on_single_strong_match_amid_weak_fillers` (anti-dilution — the over-abstain regression), `gate_disabled_by_default_does_not_abstain_on_low_cosine` (opt-in semantics).

**Also in Commit 9 — JoinHandle clean-disconnect fix (Codex dogfood, not its own ADR):** `vault-cli mcp serve` panicked "JoinHandle polled after completion" on clean stdin close — `wait()`'s `select!` drove `&mut server_handle` to completion on EOF, then `shutdown()` re-awaited it. Fixed with an `is_finished()` guard before the re-awaits in `ApplicationHandle::shutdown`; pinned by `wait_does_not_panic_when_server_handle_completes_first`.

---

### ADR-058 — Wire per-boundary REPORT generation into the consolidation run (Commit 10, 2026-05-29)

**Status:** Accepted, T0.3.x Batch B Commit 10 (2026-05-29). Surfaced by the **FIRST live consolidation dogfood** (`vault-cli consolidate run` against the live vault — 9 memories, exit 0, 0 merges).

**Context.** Batch A (`f0cc158`) shipped `topics.rs` (`discover_topics`, K-means) + `report.rs` (`generate_report` + `write_report_atomic`), and Commit 6 (`99052f2`) shipped the read-side `report_io.rs` + `StructuredReadPipeline` that serves from the REPORT. All three producer functions were unit-tested in isolation and exported from the crate — **but nothing in the run path ever called them.** `Consolidator::run_consolidation` built only the run-audit `ConsolidationReport` (+ `summary_markdown`); `Application::run_consolidation_with_safety` ran it, logged, and returned. Consequence, proven by the first live run: **no `reports/` dir or `<boundary>.report.json` was ever written**, so `memory_read` surfaced `REPORT_MISSING` / `health: degraded` on every read and `topic` was null on every fact — permanently, no matter how often consolidation ran. The "Consolidator → REPORT" link of [[locked-next-arc-t03x]] had no body. The unit tests passed because they exercised the producer functions directly; **no test exercised the run → disk path**, which is exactly why the gap shipped silently.

**Decision.** Add `Consolidator::generate_reports(run_id) -> VaultResult<Vec<Report>>`: re-enumerate active (non-superseded) memories, group by boundary, `discover_topics` (Phi-4 labels via the consolidator's own LLM handle) → `generate_report` per boundary. `Application::run_consolidation_with_safety` calls it immediately AFTER `run_consolidation` (under the same 30-min hard-timeout — both phases call the LLM + re-embed), then persists each REPORT via `write_report_atomic(&self.vault_root)`.

**Layering:** the consolidator BUILDS the reports (it owns storage + embedder + LLM); the app layer WRITES them (it owns `vault_root`). The consolidator stays filesystem-agnostic — mirrors how the cross-process lockfile already lives in the app layer. No change to `run_consolidation`'s audit return → zero ripple to its existing tests.

**Failure semantics:** a single REPORT write failure is logged-and-continued (WARN), NOT a run abort — mirrors the contradiction-invalidate philosophy; the merge work already committed durably, the next run retries, and a missing REPORT degrades to `REPORT_MISSING` at read (the correct signal).

**Pinned by** (CI every cycle, mocks — `vault-consolidator/tests/report_generation.rs`): `generate_reports_produces_topical_report_per_boundary` (one topical REPORT per boundary; every seeded fact present exactly once, none invented) + `generated_report_round_trips_to_disk_at_expected_path` (producer → `<vault_root>/reports/<boundary>.report.json` → JSON round-trip at the exact path `FilesystemReportLoader` reads). The full `run_consolidation_with_safety` → file-on-disk path is proven by the live dogfood re-run; a CI-level test of that path is blocked by `Application::new` loading the real Phi-4 GGUF — see Tech-debt below.

**Tech-debt (logged, not addressed here):** no CI-level test exercises `Application::run_consolidation_with_safety`'s REPORT-write because `Application::new` constructs the `Consolidator` internally from a real Phi-4 path (no injection seam). A mock-consolidator injection seam on `Application` would let this run in CI with mocks; until then the cron-gated real-Phi-4 path + live dogfood cover it.

---

> **⚠️ The Step 1–4 plan below (Commits 7 + 8) is HISTORICAL** — superseded by the block above. Kept for the design-reasoning trail only.

### Step 1 — Sanity check working tree + CI

```powershell
git status --short
gh run list --workflow=ci.yml -L 1
```

**Expected working tree:** only this HANDOFF.md (the Commits-7+8 opener rewrite — admin ride-along that bundles with Commit 7 per [[admin-changes-ride-with-code]]). If anything else is uncommitted, investigate before proceeding.

**Expected CI:** the latest run is for `99052f2` (T0.3.x Batch B Commit 6) and should show `success`. If it shows `failure` or anything unexpected, STOP — read `gh run view <run-id> --log-failed` and triage before any Commit 7 code per [[ci-green-per-commit-vault-code]].

Per [[gh-run-watch-exit-not-equal-run-status]] — if `gh run watch` errors, that's network/rate-limit transient, NOT a CI failure. Verify actual run status via `gh run list` before alarming.

### Step 2 — Confirm Commit 7 scope, no plan re-litigation

**Plan iteration 3 is locked** (2026-05-26). 4 of 5 Contracts shipped; Contract 4 retired 2026-05-27:
- Contract 1: REPORT artifact shape + storage (ADR-053, ✅ shipped at Batch A `f0cc158`)
- Contract 2: MCP `memory.read` response with `health` object (ADR-054, ✅ shipped at Commit 6 `99052f2`; **amended by ADR-054 Amendment 2 at Commit 7** to drop `DELTA_LOG_UNAVAILABLE` → 6 codes)
- Contract 3: Consolidator behavior — K-means + Phi-4 labels + contradiction `clear_winner` (✅ shipped at Batch A `f0cc158`)
- Contract 4: Same-day delta log (❌ **retired 2026-05-27** — falsified by Commit 6's shipped architecture; see ADR-054 Amendment 2 below)
- Contract 5: Read pipeline (deterministic filter+pack, no LLM) (✅ shipped at Commit 6 `99052f2`)

**Re-confirm briefly with Shahbaz:** "Contract 4 retirement still holds; Commit 7 is the ~30-line cleanup commit (drop `WarningCode::DeltaLogUnavailable` + obsolete pin test + tool-description update + ADR-054 Amendment 2 in HANDOFF). Then Commit 8 dogfood closes Batch B." This prevents silent drift back to "we should still build delta log".

**Why Contract 4 retired (one-line summary):** Contract 4 assumed REPORT was the read pipeline's candidate pool, so a delta layer was needed to keep today's writes visible. Commit 6 shipped a retriever-primary architecture where REPORT is enrichment only (`crates/vault-retrieval/src/structured_read_pipeline.rs:470` retrieves from the whole vault directly). Today's writes are visible to the retriever the moment SQLite commits them. The "make new memories visible" job has no body.

**Do NOT re-litigate further.** If a future recon surfaces another falsifying finding, surface as a plan amendment with falsified-by evidence per [[retract-with-falsified-by-when-prior-iteration-wrong]].

### Step 3 — Code sequence (2 commits closing Batch B)

**Commit 7 — Retire Contract 4 + ADR-054 Amendment 2** (~half a day):
- **Drop `WarningCode::DeltaLogUnavailable`** from the `WarningCode` enum in `crates/vault-retrieval/src/structured_read_pipeline.rs`. Compiler will surface any remaining references (none expected — Commit 6's emission path never lit up).
- **Remove the obsolete pin test** `commit_6_never_emits_delta_log_unavailable_warning` (`structured_read_pipeline.rs::tests`). No replacement test needed; the warning no longer exists.
- **Update the MCP tool description** at `crates/vault-mcp/src/server.rs::tool_read` IF it enumerates the 7 warning codes (verify via grep at kickoff). Either drop `DELTA_LOG_UNAVAILABLE` from the list or rewrite the count from 7 to 6.
- **Update HANDOFF cross-refs** that say "7 warning codes" → "6 warning codes" (ADR-054 body + cross-link summary + opener metadata).
- **ADR-054 Amendment 2** lands in HANDOFF (rides with this commit, no separate doc): drops `DELTA_LOG_UNAVAILABLE` from the locked-codes set, cites the falsified-by evidence (Commit 6 architecture), retires Plan Iteration 3 Contract 4. Already drafted below ADR-054 base text.
- **Local DoD gates** before commit: fmt → check → clippy → test → fmt --check → `git status --short`. Per [[run-cargo-gates-in-background]] all in background; per [[no-parallel-cargo-invocations]] strictly serial.

**Commit 8 — MCP entrypoint + founder dogfood** (~2-2.5 days):

**Scope expanded 2026-05-27** after dogfood-prep recon surfaced that no MCP stdio entrypoint binary existed: `crates/vault-mcp` was library-only, `crates/vault-tauri` deliberately omits MCP per ADR-034 ("V0.1 vault-tauri is UI-only — no MCP server bound inside the Tauri process"), and `crates/vault-cli` had no `mcp` subcommand. ADR-034 forward-pointed to a "V0.2 alpha-distribution subcommand-split design"; this commit lands it.

Code shape (already drafted 2026-05-27, sitting in the working tree):
- `crates/vault-app/src/application.rs` — new public `ApplicationHandle::wait()` method: selects on `server_handle` (stdio EOF) and `signal_handle` (SIGINT path), then calls `shutdown()` for graceful cleanup. ~40 LoC including doc.
- `crates/vault-cli/src/main.rs` — new `Command::Mcp { ..., command: McpAction }` variant + `McpAction::Serve` + `dispatch_mcp` + `run_mcp_serve` + 3 CLI parser tests. `phi4_model` refactored to `Option<PathBuf>` on `build_application` (Phi-4 is needed only for the consolidate path; MCP read path is fully deterministic per ADR-052). `--boundary` flag repeatable, defaults to `["personal"]`.
- **ADR-055** (`vault-cli mcp` subcommand-split design) rides with this commit. Documents the rejected alternatives (standalone `vault-mcp` binary, modifying `vault-tauri`).
- ~329 net LoC across the two files (vault-app +42, vault-cli +287). Close to the 250-LoC pre-write estimate.

Dogfood phase (after CI green):
- End-to-end check from Claude Desktop via MCP stdio: register `vault-cli mcp serve …` in `%APPDATA%\Claude\claude_desktop_config.json`'s `mcpServers` block, write a few memories from Claude Desktop, run `vault-cli consolidate run` in a separate terminal, read memories back, verify the structured-fact shape arrives cleanly and Claude composes a coherent answer.
- Tighten any rough edges surfaced during dogfood. Possible items: error-message clarity, REPORT staleness threshold tuning, MCP tool description final polish.
- If Qwen-7B Rust code (`Qwen25_14BProvider` in `vault-llm`) is fully unused, remove it here. `AppConfig.qwen_model_path` (currently `#[allow(dead_code)]` per ADR-052) also removed.

Pre-dogfood gating: BGE model + tokenizer + ONNX Runtime DLL need to live on disk first — run `scripts/setup-dev-env.ps1` (Windows) which downloads them into `crates/vault-embedding/test-fixtures/bge-small-en-v1.5/` (~150 MB, idempotent, SHA-256-verified per `MODEL_PROVENANCE.md`). Phi-4-mini GGUF already present at `%APPDATA%\com.shahbaz242630.memory-vault\models\Phi-4-mini-instruct-Q4_K_M.gguf`.

**Note on cadence:** Commit 7 (retirement) is deterministic ~30-LoC + HANDOFF — ships cleanly on its own CI cycle. Commit 8 (dogfood) is exploratory and may surface fix-forward needs. Two separate commit+push cycles, per [[ci-green-per-commit-vault-code]].

### Step 4 — Remaining tech-debt (still not in Commits 7 + 8 scope)

The four open items in the Tech-debt section below are NOT small:
- Entity-extraction-at-consolidation + GraphStore relationship-rewrite — multi-week scope
- `VaultError::Storage(String)` → structured variants — ~30 call sites + new ADR
- `pending_sync` sweep + migration 0003 — ~80 LoC + schema migration + tests (ship-gated with V0.2 sync, not the consolidator arc)
- Lance Cosine NaN community filing — LOW priority

### Frozen vs open going into Commits 7 + 8

**Frozen (do not re-litigate):**
- 🔒 **Architectural lock 2026-05-26**: LLM out of read; agent composes; vault returns structured facts. Phi-4 stays at consolidation; Qwen-7B fired from read path. See [[architectural-lock-llm-out-of-read-path]] memory.
- [[locked-next-arc-t03x]] — four-step sequence; Steps 1-3 + runtime-wiring of Step 4 shipped; Step 4 founder dogfood lands at Commit 8.
- Phase C (write-time decision loop) DEFERRED to V1.0+
- **Plan iteration 3** (2026-05-26): 5 Contracts + failure semantics locked. 4 of 5 shipped; **Contract 4 (same-day delta log) retired 2026-05-27** — falsified by Commit 6's retriever-primary architecture; see ADR-054 Amendment 2.
- **Batch A deliverables** (commit `f0cc158`): `consolidator_lock.rs` RAII guard + 30-min hard timeout + `run_id` tracing span + `Application::run_consolidation_with_safety` + `vault-cli consolidate run` subcommand + K-means topic discovery (`topics.rs`) + per-boundary REPORT artifact (`report.rs`) + `MergeOutcome::Contradiction.clear_winner` auto-invalidate wiring.
- **Batch B Commit 6 deliverables** (commit `99052f2`): `structured_read_pipeline.rs` (~870 lines, 21 tests) + `report_io.rs` (~280 lines, 5 tests) + `read_pipeline.rs` deleted + 13 Qwen spike examples deleted + `Application::new` step 9 rewired + VaultAdapter + Adapter trait return type updated + `tool_read` description rewrite. 193 tests pass workspace-wide.
- **ADR-052** (Qwen retirement from read path) — shipped at Commit 6.
- **ADR-053** (REPORT shape + storage + lifecycle) — shipped at Batch A. **Amendment 1** (additive `topic_names_unavailable: bool` with `#[serde(default)]`) shipped at Commit 6.
- **ADR-054** (MCP read response health-warning contract — staleness thresholds + aggregate-status rule) — shipped at Commit 6 with 7 codes; **Amendment 2 at Commit 7 (2026-05-27) drops `DELTA_LOG_UNAVAILABLE` → 6 locked codes**.
- ADR-051 (bi-temporal `invalidate()` API contract) — still load-bearing, consumed by Batch A merge orchestrator.
- MCP `memory.write` + `memory.update` + `memory.delete` canonical-save contract (tool descriptions + field docs + server-side `normalize_for_canonical_save`)
- ADR-044 / 045 / 046 / 047 (consolidator surface) — still load-bearing
- ADR-048 / 049 — formally **superseded-in-effect by ADR-052** as of Commit 6 ship; kept in archive for the t023-t027b empirical anchors that informed the supersession.
- Consolidator inventory above (canonical — do NOT re-discover; update in lockstep if new code lands)
- Technique map above (do NOT re-debate Bloom vs Cuckoo, Z-order, quad-tree, etc. — settled)
- BRD v1.4 (correctness-is-the-product thesis + three-mode deployment)

**Open (Commits 7 + 8 + tech-debt):**
- Commit 7: retire Contract 4 (drop `WarningCode::DeltaLogUnavailable` + obsolete pin test + MCP tool-description update + HANDOFF cross-ref count updates) + ADR-054 Amendment 2 in HANDOFF
- Commit 8: **MCP entrypoint** (new `vault-cli mcp serve` subcommand per ADR-055 — closes ADR-034's forward-pointer) + founder dogfood + polish + (optional) Qwen-7B Rust code removal + (optional) `AppConfig.qwen_model_path` field removal
- The four multi-session tech-debt items in the Tech-debt section
- Eventual: scheduling (T0.2.6), Phase 4 decay (T0.2.4), checkpoint+rollback (T0.2.5) — sequenced after Batch B closes

### Files to read first in next session

1. **This block** — current state + architectural lock + consolidator inventory + technique map + this opener
2. **Project memories** — [[architectural-lock-llm-out-of-read-path]] + [[locked-next-arc-t03x]] + [[correctness-is-the-product]] + [[mcp-descriptions-cross-platform-lever]] + [[managed-mode-per-user-vault]]
3. **CI status** — `gh run list --workflow=ci.yml -L 1` (confirm `99052f2` shows `success`)
4. **Commit 7 surgery target — the warning enum** — `crates/vault-retrieval/src/structured_read_pipeline.rs::WarningCode`. Drop the `DeltaLogUnavailable` variant; recompile to surface any callers.
5. **Commit 7 surgery target — the obsolete pin test** — `crates/vault-retrieval/src/structured_read_pipeline.rs::tests::commit_6_never_emits_delta_log_unavailable_warning`. Remove it entirely.
6. **Commit 7 cross-ref site** — `crates/vault-mcp/src/server.rs::tool_read`. Grep for `DELTA_LOG_UNAVAILABLE` or "7 warning codes" or similar; update if found.
7. **Commit 7 falsification anchor (read for grounding only, do NOT modify)** — `crates/vault-retrieval/src/structured_read_pipeline.rs::read` lines 460-490. This is the retriever-primary read path that falsified Contract 4's "REPORT-bound candidate pool" assumption. Cited verbatim in ADR-054 Amendment 2.
8. **Commit 8 dogfood entry points** — `crates/vault-cli/src/main.rs` (`vault-cli consolidate run` subcommand) + `crates/vault-mcp/src/server.rs` (MCP stdio surface for Claude Desktop).

### Three sentences to open next session with

If you're me opening cold: confirm CI green on `99052f2` first, then re-anchor with Shahbaz "Contract 4 retirement still holds; Commit 7 is the ~30-line cleanup (drop `WarningCode::DeltaLogUnavailable` + obsolete pin test + MCP description update + ADR-054 Amendment 2). Then Commit 8 dogfood closes Batch B." Read `crates/vault-retrieval/src/structured_read_pipeline.rs` to confirm the `DeltaLogUnavailable` variant + obsolete test are still the only delta-log surface to remove. Then proceed with Commit 7 (deterministic, CI-cycles cleanly) → Commit 8 (founder dogfood, may surface fix-forward) as two separate commit+push cycles per [[ci-green-per-commit-vault-code]].

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

**Status:** Accepted, shipped at Commit 6 (`99052f2`, 2026-05-26).

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

**Status:** Accepted, shipped at Commit 6 (`99052f2`, 2026-05-26). **Supersedes ADR-048 + ADR-049 in effect** (the LLM read-time pipeline they ship is retired; the model lock they document becomes archival).

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

**Status:** Accepted, shipped at Commit 6 (`99052f2`, 2026-05-26). Locks the locked-next-arc Plan Iteration 3 Contract 2 surface.

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

### ADR-054 Amendment 2 — drop `DELTA_LOG_UNAVAILABLE`, retire Plan Iteration 3 Contract 4 (Commit 7, 2026-05-27)

**Status:** Drafted 2026-05-27, lands at Commit 7. Rides with the Commit 7 code commit (cleanup of `WarningCode::DeltaLogUnavailable` + obsolete pin test).

**Context.** ADR-054's base text (above) locks 7 warning codes including `DELTA_LOG_UNAVAILABLE`, which is reserved for a same-day delta-log layer scoped under Plan Iteration 3 Contract 4. The Commit 6 pin test `commit_6_never_emits_delta_log_unavailable_warning` is the forward-looking pin: Commit 6 ships the code as a reserved variant; Commit 7 was meant to light up its real emission path when delta-log reads fail.

**Falsifying recon (2026-05-27 session-open).** Source-read of the as-shipped `crates/vault-retrieval/src/structured_read_pipeline.rs::read` (Commit 6, `99052f2`) shows:

- **Line 470** — `let candidates = self.retriever.retrieve(retrieval_query).await?` — retrieval runs against the **whole vault** via the existing `Retriever` (BGE + Tantivy + RRF + abstain). It queries SQLite + Lance directly. No REPORT-based candidate filter.
- **Lines 449-458** — REPORT is loaded only to build a `topic_lookup: HashMap<MemoryId, String>`. REPORT is **enrichment-only**, not a candidate pool.
- **Lines 477-487** — memories not in REPORT still get returned to the agent — just with `topic: None`.

Combined with the cascading-write path (`crates/vault-storage/src/cascading.rs::write_memory` at line 343 → `cascading_write` at line 728): SQLite + Lance commits happen synchronously before `Ack` returns. The retriever sees newly-written memories immediately. **Today's writes are visible without a delta-log layer.**

Contract 4 was conceived under an architecture where REPORT was the candidate pool. Commit 6 shipped a retriever-primary architecture where REPORT is enrichment only. The architecture changed during the locked-next-arc work; Contract 4's mechanism wasn't re-litigated against the actual shipped shape. Per [[fix-one-break-another-signals-structural]] + [[retract-with-falsified-by-when-prior-iteration-wrong]] the right move is honest retirement, not building infrastructure with no job.

**Decision.**

1. **Drop `WarningCode::DeltaLogUnavailable`** from the `WarningCode` enum in `structured_read_pipeline.rs`. The locked-codes set drops from 7 → 6: `REPORT_MISSING` / `REPORT_STALE_INFO` / `REPORT_STALE_WARN` / `REPORT_STALE_CRITICAL` / `TOPIC_NAMES_UNAVAILABLE` / `CLOCK_SKEW_DETECTED`.
2. **Remove the obsolete pin test** `commit_6_never_emits_delta_log_unavailable_warning` from `structured_read_pipeline.rs::tests`. No replacement test; the variant no longer exists.
3. **Update the MCP tool description** at `vault-mcp/src/server.rs::tool_read` IF it enumerates the 7 codes (verify via grep at kickoff).
4. **Retire Plan Iteration 3 Contract 4** entirely. No delta-log table. No schema migration 0003 for delta log (migration 0003 stays reserved for the `pending_sync` payload tech-debt entry).
5. **The ADR-054 base text above stays unchanged for archival** — Amendment 2 is the locked surface from Commit 7 forward. Future readers see "Status: Accepted, shipped at Commit 6" + this Amendment 2's "Status: Drafted 2026-05-27" together and understand the supersession chain.

**What stays load-bearing in ADR-054 base.**

- Six of seven warning codes (all except `DELTA_LOG_UNAVAILABLE`)
- Staleness threshold constants (`STALE_INFO_THRESHOLD` = 24h, `STALE_WARN_THRESHOLD` = 72h, `STALE_CRITICAL_THRESHOLD` = 7d)
- Aggregate-status rule (Critical / Degraded / Ok three-tier)
- Emission ordering (schema-guard → clock-skew → staleness tier → topic-names-unavailable, per-boundary in input order)
- Response shape (`boundary` / `query` / `relevant_facts` / `abstain` / `health` with `status` + `warnings`)

**Rejected alternatives.**

- **Keep `DeltaLogUnavailable` as a Reserved variant with `#[allow(dead_code)]`** — bleeds a never-emitted enum variant into the public API surface. The CLAUDE.md no-backwards-compat principle applies: delete the code.
- **Repurpose `DELTA_LOG_UNAVAILABLE` for a different signal** (e.g., "post-REPORT memories present, topic clustering pending") — different semantics under the same name is worse than a clean rename. Future ADR can add a new code with a precise name if the use case surfaces.
- **Build delta_log anyway as forward-compat for V0.2 cross-device sync (V0.2.9-13)** — violates CLAUDE.md "Don't design for hypothetical future requirements." When the sync arc lands, its data-shape needs may differ from what Contract 4 envisioned anyway. Build at sync time with sync-anchored evidence.

**Pin tests (after the obsolete one is removed).**

- 6 warning-code tests in `structured_read_pipeline.rs::tests` (one per remaining code's trigger + severity)
- 4 aggregate-status tests (one per branch of the three-tier rule)
- 4 boundary-field semantics tests
- 15 → 14 total covering ADR-054 surface

**Pin-test count update.** ADR-054 base text said "Pinned by 15 unit tests in `crates/vault-retrieval/src/structured_read_pipeline.rs`." Post-Amendment-2 the count is **14** (one test removed, no replacement added). The base-text count stays as-written for archival fidelity; this Amendment is the authoritative current count.

**Cross-refs.** ADR-054 base text above · `crates/vault-retrieval/src/structured_read_pipeline.rs::read` lines 460-490 (falsifying anchor) · `crates/vault-storage/src/cascading.rs::cascading_write` line 728 (commit-before-Ack proof) · [[architectural-lock-llm-out-of-read-path]] (the architecture lock that drove Commit 6's retriever-primary shape) · [[retract-with-falsified-by-when-prior-iteration-wrong]] (the discipline this Amendment honors).

---

## ADR-055 — `vault-cli mcp serve` subcommand-split design (Commit 8 of locked-next-arc, 2026-05-27)

**Status:** Drafted 2026-05-27, lands at Commit 8 of locked-next-arc Batch B (this commit). Closes ADR-034's forward-pointer to "V0.2 alpha-distribution / subcommand-split design".

**Context.** ADR-034 (T0.1.11 Phase 5b) deliberately kept the V0.1 `vault-tauri` UI-only — *"V0.1 vault-tauri is UI-only — no MCP server bound inside the Tauri process. `start_with_mcp` would call rmcp's `ServiceExt::serve(server, stdio()).await` which blocks on JSON-RPC `initialize` from a peer that doesn't exist when launched as a Tauri UI app, hanging Tauri's setup() hook indefinitely."* The pointer-forward said *"AI-client MCP integration deferred to V0.2 alpha-distribution task (subcommand-split design per ADR-034 cross-link)."*

Commit 8 dogfood-prep recon (2026-05-27) confirmed the gap was still open: `crates/vault-mcp` is a library crate (no `main.rs`), `crates/vault-tauri/src/main.rs:272-283` still reflects ADR-034's deferral, `crates/vault-cli/src/main.rs` (1,336 lines pre-Commit-8) had subcommands for dead-letter / divergence-check / consolidate but no `mcp` subcommand. So before Claude Desktop could stdio-talk to the vault, the MCP entrypoint binary had to be built. ADR-055 documents that build.

**Decision.** New `vault-cli mcp serve` subcommand (Subcommand-split path α), NOT a standalone `vault-mcp` binary (path β) and NOT modifying `vault-tauri` (path γ).

Concrete shape (locked at this commit):

```text
vault-cli mcp serve
  --bge-model <path>           # required (or VAULT_BGE_MODEL_PATH env)
  --bge-tokenizer <path>       # required (or VAULT_BGE_TOKENIZER_PATH env)
  --ort-lib <path>             # required (or VAULT_ORT_LIB_PATH env)
  [--phi4-model <path>]        # optional (Option<PathBuf>) — read pipeline is
                               #   fully deterministic per ADR-052; Phi-4 only
                               #   needed if this process is ALSO to host
                               #   consolidation runs (uncommon)
  [--boundary <name>]...       # repeatable; defaults to ["personal"]
  --vault-db <path>            # inherited from the top-level Cli struct
  --vector-dir <path>
  --graph-db <path>
```

**Rejected alternatives.**

- **Path β — Standalone `vault-mcp` binary** (give `crates/vault-mcp/` a `src/main.rs`). Rejected because: (1) duplicates the keychain-bootstrap + `AppConfig` construction logic already in `vault-cli::build_application`; (2) ships a second Rust binary that has to be packaged in alpha distribution + registered for the user; (3) `vault-cli` is the existing operator-tools entrypoint — `mcp serve` fits its surface naturally. Path α reuses the entire vault-cli skeleton.
- **Path γ — Modify `vault-tauri` to optionally host MCP** (config flag, runtime branch). Rejected because: (1) reintroduces the rmcp-blocking issue ADR-034 explicitly designed around; (2) couples UI lifecycle to MCP transport lifecycle; (3) for V0.2 alpha the UI app and the MCP daemon should be separable — a user running Claude Desktop's MCP integration may not want the Tauri UI open at all.
- **Single-binary monolith with auto-detect mode (UI / MCP / CLI)** (e.g. argv[0]-based dispatching). Rejected as over-engineering for V0.2 alpha. The subcommand split is mechanical, discoverable (`vault-cli --help` lists `mcp` alongside `consolidate`), and trivially extensible (e.g. a future `vault-cli mcp status` subcommand for diagnostics).

**Implementation surface (rides with this commit).**

| Surface | Change |
|---|---|
| `crates/vault-app/src/application.rs::ApplicationHandle` | **NEW** `pub async fn wait(self) -> VaultResult<()>` — selects on `server_handle` + `signal_handle`, then calls `shutdown()` for graceful cleanup. Doc-pinned: worker is NOT in the select (worker exits only when the shutdown signal flips, which happens AFTER one of the other two tasks resolves). |
| `crates/vault-cli/src/main.rs::Command` | **NEW** `Mcp { bge_model, bge_tokenizer, ort_lib, phi4_model: Option<PathBuf>, boundary: Vec<String>, action: McpAction }` variant. |
| `crates/vault-cli/src/main.rs::McpAction` | **NEW** enum with single `Serve` variant. (Forward-compat: future subcommands like `vault-cli mcp status` can land as sibling variants without breaking the parser surface.) |
| `crates/vault-cli/src/main.rs::dispatch_mcp` | **NEW** async fn. Parses `--boundary` strings into typed `Boundary` values BEFORE touching keychain / opening backend (cheap failure surface). |
| `crates/vault-cli/src/main.rs::run_mcp_serve` | **NEW** async fn. Calls `Application::start_with_mcp` → `handle.wait()`. Eprintln-stderr announces "ready" + "clean shutdown" for operator visibility (stdout is reserved for the MCP JSON-RPC traffic). |
| `crates/vault-cli/src/main.rs::build_application` | **REFACTORED** `phi4_model: PathBuf` → `phi4_model: Option<PathBuf>`. Backward-compatible for the consolidate caller which now passes `Some(phi4_model)`. `Application::new` already handles `phi4_model_path: None` gracefully (logs WARN, leaves consolidator unwired; `run_consolidation_with_safety` returns `ConsolidatorUnconfigured` if called). |
| `crates/vault-cli/src/main.rs::tests` | **NEW** 3 CLI parser tests: defaults case (no phi4, no boundary), full case (phi4 supplied + 3 boundaries), rejection case (missing --bge-model). |

**Boundary auth gating.**

The MCP server's tool-call layer (already in `crates/vault-mcp/src/server.rs`) refuses calls that touch boundaries outside the `authorized_boundaries: Vec<Boundary>` passed to `Application::start_with_mcp` per BRD §11.4.3. The CLI's `--boundary` flag is the operator's "what does Claude Desktop see today" gate. Default `["personal"]` matches the single-default convention; supplying multiple boundaries (`--boundary personal --boundary work`) is an explicit operator action.

**Why `phi4_model` is `Option`.**

The MCP read path is fully deterministic per ADR-052 (LLM out of read; `StructuredReadPipeline` filter-and-pack). Loading Phi-4-mini at MCP-server startup would waste ~30s of cold-start time + ~4 GB resident memory for zero functional benefit. The consolidation path (`vault-cli consolidate run`) does require Phi-4 — and runs as a separate process in the typical alpha deployment. If the operator wants the MCP-server process to ALSO be able to invoke consolidation (e.g. via a future MCP `vault.consolidate` tool), they can supply `--phi4-model`; today that path is unused but the type plumbing already accommodates it.

**Pin tests.**

3 CLI parser tests in `crates/vault-cli/src/main.rs::tests`:
- `cli_parses_mcp_serve_with_default_boundary` — defaults case
- `cli_parses_mcp_serve_with_multiple_boundaries_and_phi4` — full-opt case + repeatable-flag ordering
- `cli_rejects_mcp_serve_with_missing_bge_model` — required-arg enforcement

Protocol-level coverage (rmcp JSON-RPC handshake, tools/list, tool dispatch) is already pinned by `crates/vault-mcp/tests/initialize_smoke.rs` — no new integration test needed at the vault-cli layer because the dispatcher is a thin shim over `Application::start_with_mcp` which is already covered.

**Forward-compat.**

- A future `vault-cli mcp status` subcommand (diagnostics — show wired boundaries, REPORT staleness, recent retry-queue depth) drops in as a sibling `McpAction` variant.
- If V1.0 Managed PAYG needs a non-stdio transport (HTTP/SSE for cloud deployment), a future `McpAction::Serve` flag like `--transport http --listen 0.0.0.0:8443` can extend without breaking the stdio default. rmcp already supports HTTP per its crate docs; the swap point is the `transport::stdio()` call inside `Application::start_with_mcp`.
- If a future commit needs a dedicated `vault-mcp` binary (e.g. minimized distribution surface for embedded deployments), the same `Application::start_with_mcp` + `handle.wait()` pattern works — no API redesign needed.

**Cross-refs.** ADR-034 (T0.1.11 Phase 5b — V0.1 UI-only vault-tauri; this ADR closes the forward-pointer) · ADR-052 (Qwen out of read path → MCP server doesn't need Phi-4) · `crates/vault-app/src/application.rs::Application::start_with_mcp` (the underlying API consumed) · `crates/vault-app/src/application.rs::ApplicationHandle::wait` (the new public method this commit adds) · `crates/vault-mcp/src/server.rs` (the StdioServer construction site) · `crates/vault-mcp/tests/initialize_smoke.rs` (protocol-level pin) · BRD §11.4.3 (authorized_boundaries surface) · [[locked-next-arc-t03x]] Commit 8.

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

### T0.3.x — read-relevance: per-candidate cosine filter + carry-cosine-through-fusion + retire vestigial BM25 gate

**Surfaced + logged:** ADR-057 (Commit 9, 2026-05-28).

**The gap.** ADR-057's cosine gate closes no-signal abstention but leaves three coupled items for one follow-up:
1. **Vestigial BM25 gate** — `AbstainingRetriever`'s BM25-top-1 gate is superseded in effect for the read path by the pipeline cosine gate but still runs. Harmless (near-no-op) but should be formally retired so a future reader isn't confused by two gates.
2. **No per-candidate relevance filtering** — the gate is all-or-nothing abstain on the top-3-mean; it does NOT drop individual off-topic candidates below the floor from a non-abstaining response (the "real query still returns lots" precision side of the A6 finding).
3. **Double-embed on the proceed path** — the pipeline runs its own semantic probe AND the inner hybrid re-embeds the same query (~+50-150ms). Accepted under correctness-before-latency for V0.2.

**The fix (one follow-up).** Carry the raw semantic cosine through `HybridRetriever` fusion onto `RetrievedMemory` (today `hybrid.rs:221-247` discards it for the RRF score). Then the pipeline filters per-candidate on the carried cosine (abstain = filtered-empty), which (a) removes the separate probe + double-embed, (b) enables per-candidate precision filtering, and (c) lets the BM25 gate be formally retired. Sequenced after the cosine-floor ship + live A6 validation. **Distinct from** the V1.0+ cross-encoder reranker (ADR-057 scope), which addresses topical-noise discrimination, not no-signal.

**Affected files (forward-pointer):** `crates/vault-retrieval/src/strategies/hybrid.rs:221-247` (RRF discards cosine), `crates/vault-retrieval/src/structured_read_pipeline.rs` (gate + future carry-cosine consumer), `crates/vault-retrieval/src/strategies/abstain.rs` (BM25 gate to retire).

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
- **ADR-052** — Qwen-7B retirement from read path (shipped at Commit 6, `99052f2`, 2026-05-26). Formally supersedes ADR-048 + ADR-049 in effect: replaces the V0.2-era Qwen-7B single-call synthesis pipeline (mean 86s, p99 119.7s on Vulkan iGPU) with the deterministic `StructuredReadPipeline` (~500ms total). Delivers ~170× local-mode speedup, ~50× BYOK cost cut, ~10× Managed PAYG margin. Phi-4-mini stays at nightly consolidation. ADR-051 + ADR-053 + ADR-044/045/046/047 unchanged. Full ADR text above this section.
- **ADR-053** — Per-boundary REPORT artifact shape + storage + lifecycle (shipped at T0.3.x Batch A, `f0cc158`, 2026-05-26). Locks the structured JSON shape (`schema_version` + `boundary` + `generated_at` + `consolidator_run_id` + `facts_by_topic` keyed by topic label), storage path `<vault_root>/reports/<boundary>.report.json`, atomic `.tmp + fsync + rename` write protocol, and latest-only versioning. Consumed by the Batch B Commit 6 structured-fact read pipeline. **Amendment 1 shipped at Commit 6 (`99052f2`, 2026-05-26)** adds `topic_names_unavailable: bool` (additive, `#[serde(default)]`) so the read pipeline can surface ADR-054's `TOPIC_NAMES_UNAVAILABLE` warning. Full ADR text + Amendment 1 above this section.
- **ADR-054** — MCP `memory.read` response health-warning contract (shipped at Commit 6, `99052f2`, 2026-05-26). Locks the structured-fact response shape (`boundary` / `query` / `relevant_facts` / `abstain` / `health`) with staleness threshold constants + aggregate-status rule + per-boundary deterministic emission ordering. **Amendment 2 at Commit 7 (drafted 2026-05-27) drops `DELTA_LOG_UNAVAILABLE`** → 6 locked codes (`REPORT_MISSING` / `REPORT_STALE_INFO` / `REPORT_STALE_WARN` / `REPORT_STALE_CRITICAL` / `TOPIC_NAMES_UNAVAILABLE` / `CLOCK_SKEW_DETECTED`); retires Plan Iteration 3 Contract 4 (same-day delta log) as falsified by Commit 6's retriever-primary architecture. Pinned by 14 unit tests in `crates/vault-retrieval/src/structured_read_pipeline.rs` (was 15; one obsolete pin removed at Commit 7). Full ADR text + Amendment 2 above this section.
- **ADR-055** — `vault-cli mcp serve` subcommand-split design (Commit 8 of locked-next-arc, 2026-05-27). Closes ADR-034's V0.1 forward-pointer to "V0.2 alpha-distribution subcommand-split design". New `Mcp { ..., action: McpAction::Serve }` variant in `vault-cli`; `phi4_model: Option<PathBuf>` (read path is fully deterministic per ADR-052); repeatable `--boundary` flag defaulting to `["personal"]`; new public `ApplicationHandle::wait()` method in `vault-app` for graceful select-then-shutdown lifecycle. Rejected: standalone `vault-mcp` binary (duplicates keychain bootstrap), modifying `vault-tauri` (reintroduces ADR-034's rmcp-blocking issue), argv[0]-based monolith (over-engineering). Pinned by 3 CLI parser tests in `crates/vault-cli/src/main.rs`; protocol-level coverage already in `crates/vault-mcp/tests/initialize_smoke.rs`. Full ADR text above this section.

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
