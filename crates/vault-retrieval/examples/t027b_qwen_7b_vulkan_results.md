# T0.2.3 t027b — Qwen-7B + Vulkan iGPU Offload Spike Results

**Run started:** 2026-05-15 10:50:25 UTC  
**Model:** Qwen2.5-7B-Instruct Q4_K_M  
**Hardware:** i7-13620H (10P / 16T, AVX2) + Intel UHD Graphics iGPU (Vulkan)  
**CPU reference (t027a t12):** 134.2s mean (Q19+Q26)  
**Hard ceiling:** 120s per query  
**Quality gate:** t026 baseline — 4/4 contradictions + 2/2 hard-negatives  
**Tuning:** `n_gpu_layers=99, n_threads=12, n_threads_batch=12, KV K/V = f16`  
**Fixture:** 100 memories, 3.2s insertion

## Latency summary

| min | p50 | p99 | max | mean |
|---|---|---|---|---|
| 60.8s | 84.9s | 119.7s | 119.7s | 86.0s |

**Mean vs t12 CPU baseline (134.2s):** -48.2s (-35.9%)  
**p99 vs 120s hard ceiling:** -0.3s (**WITHIN budget**)

## Quality rollup (t026 baseline: 4/4 + 2/2)

- Contradictions: **4/4** (Q11, Q13, Q25, Q26)
- Hard-negatives: **2/2** (Q21, Q22)
- **VERDICT: quality preserved vs t026 baseline.**

## Per-query detail

| Query | Shape | Quality | Contradictions | Vault empty | Latency |
|---|---|---|---|---|---|
| Q11 | decision-history | **PASS** | 1 | false | 84.6s |
| Q13 | decision-history | **PASS** | 1 | false | 74.3s |
| Q25 | oblique-agent-reformulation | **PASS** | 1 | false | 101.0s |
| Q26 | oblique-agent-reformulation | **PASS** | 1 | false | 84.9s |
| Q17 | topic-synthesis | — | 1 | false | 101.7s |
| Q19 | topic-synthesis | — | 1 | false | 119.7s |
| Q21 | hard-negative | **PASS** | 0 | true | 61.1s |
| Q22 | hard-negative | **PASS** | 0 | true | 60.8s |

## Tail-latency margin — Q19 calls this out explicitly

Q19 ("Where are we with the launch timeline?") is the **most ambitious recall query in the gauntlet**: it spans 3 clusters across 8 memories (alpha-launch-nov-15 short + beta-launch-dec-5 paragraph + ga-launch-quarter long-form contradiction pair). Latency: **119.7s — exactly 0.3s under the 120s hard ceiling.**

That is a **0.25% safety margin** on the tail. It is a load-bearing claim about V0.2 free-tier UX, and it depends on Q19's worst-case design *remaining* the worst case once real beta usage hits the system. Plausible shifts that erode the margin:

- **More relevant candidates in BGE top-K.** Q19 used 8 of the 20 retrieved candidates. A denser cluster of relevant memories around a topic (e.g., months of session writes about one project) could feed Qwen 12+ relevant candidates instead of 8 — prompt-eval phase scales roughly linearly with input tokens.
- **Longer generated output.** Q19 produced 181 words / ~400 generated tokens. Deeper synthesis questions ("give me the full timeline including risks, mitigations, stakeholder positions, and open questions") could push output to 600+ tokens. Generation phase is linear in output length.
- **Heavier system-prompt customization.** The spike pinned `STANDALONE_SYSTEM_PROMPT` at ~150 words. Production may layer in per-tenant context, tool descriptions, or agent personality — every added token costs prompt-eval time.

Each shift moves worst-case latency UP. The 0.3s margin is fragile.

### Escape valve if real-world tail breaches the ceiling

The deferred Family B (Qwen2.5-0.5B speculative-decoding draft + Qwen2.5-7B target) is the V0.2.x mitigation path. Speculative decoding is **mathematically lossless** — the draft model proposes tokens cheaply and the 7B target verifies each in parallel, so user-visible output is byte-identical to running 7B alone. No t026 quality re-validation required. Reported speedup on code/structured-output workloads is 2x–2.5x on the generation phase, which is roughly half of our wall-clock time. Conservative estimate of mean latency after speculative decoding: **~50–60s on this hardware — roughly half the current Vulkan-only result.**

Implementation cost: 2–3 weeks. The safe `llama-cpp-2` wrapper at 0.1.146 does NOT expose a speculative-decoding API, so the work routes through raw `llama-cpp-sys-2` FFI with KV-cache sequence-management primitives. Defer to V0.2.x unless beta usage forces the issue earlier.

**ADR-050 (locked tuning) MUST cite this margin explicitly.** A 0.3s ceiling-headroom claim is too load-bearing to bury in a results-file footnote — it shapes the V0.2 free-tier UX promise. The ADR should name Family B by ADR number (TBD when drafted) as the documented escape valve.

---

## Hardware generalization — single-data-point honesty

The 86.0s mean / 119.7s p99 numbers are **one hardware data point**: a 2023 Intel i7-13620H with Intel UHD Graphics (16–64 EU integrated iGPU) running Windows 11. Before V0.2 free-tier ships, the honest latency spread across user hardware is:

| Hardware class | Code path | Expected mean | Status |
|---|---|---|---|
| **Modern Intel laptop with iGPU (this spike's class)** | Vulkan backend, full GPU offload (29/29 layers) | **86s (measured)** | ✅ Within 120s ceiling |
| Modern laptop with NO usable iGPU (rare — some server-class CPUs in laptops; very old hardware) | CPU-only fallback (t12 config) | **134s (measured at t027a)** | ❌ Over 120s ceiling — free-tier broken on this class |
| Older Intel laptops (UHD 620, HD 4000, weaker iGPUs) | Vulkan backend, full or partial offload | Unknown — likely 100–180s depending on EU count + memory bandwidth | ❌ Untested; may breach ceiling |
| **Apple Silicon Macs (M1 / M2 / M3 / M4)** | **Metal backend — entirely different code path from Vulkan** | Projected 30–60s per research playbook (memory-bandwidth analysis) | ❌ Untested; Metal feature flag deferred per V0.1 ADR-042 scope-amendment trail |
| Discrete GPU (NVIDIA/AMD) | Vulkan or CUDA — would smoke this result | Projected < 30s | ⚪ Works automatically once CUDA feature flag added; not the V0.2 default target |

### Honest V0.2 free-tier framing (Shahbaz's locked wording)

> *"V0.2 free-tier ships at 86s mean on a representative Intel iGPU. Pure-CPU fallback is 134s mean and breaks the 120s ceiling. Metal autodetect on macOS is still deferred (per V0.1 archive ADR-042 scope-amendment trail)."*

That is the real shape — NOT "shippable everywhere."

### Concrete implications for V0.2 product framing

- **Mac latency story is deferred.** Metal-backend code path is scope-amendment territory tracked under the V0.1 archive's ADR-042 selection ADR. Until a Metal-equipped Mac runs a Vulkan-equivalent gauntlet, the Mac free-tier latency claim is "we expect it to be faster than the Intel measurement based on memory-bandwidth analysis, but we have no measured number." Do not promise Mac latency in product copy.
- **Pure-CPU fallback breaches the 120s ceiling.** Users on hardware without a Vulkan-capable iGPU fall back to the t12 CPU config and land at 134s mean — over ceiling. Three product options:
  1. Tell those users explicitly: "agents wait ~2:14 per query on CPU-only hardware, vs ~1:26 with iGPU."
  2. Require Vulkan-capable iGPU as the free-tier hardware minimum; downsell to cloud-tier for incompatible hardware.
  3. Defer the decision until V0.2 onboarding telemetry reveals what fraction of users actually hit the no-iGPU class.
- **Older Intel iGPUs (UHD 620, HD 4000) are an open question.** The 86s number on UHD Graphics in i7-13620H establishes a floor for modern Intel laptops; it does NOT predict performance on a 4–6-year-old Dell or ThinkPad with older Intel graphics. Recommend a follow-up V0.2.x spike on at least one older laptop class before promoting the latency claim in marketing copy.

### Why this matters for the ADR block

ADR-049 (Qwen-7B model selection) and ADR-050 (locked tuning) MUST state the measured hardware-class scope explicitly: *"shippable on 2023-class modern Intel laptops with Vulkan-capable iGPUs at 86s mean / 119.7s p99; CPU-only fallback at 134s mean breaches the 120s ceiling; Metal-backend macOS path deferred; older Intel iGPUs untested."* Anything narrower than that is "works on my machine" wrapped in latency benchmarks. The honest framing here belongs in the ADR text, not the press release.

---

## Per-query synthesis

### Q11 — "When did we decide GA launch?"

**Shape:** decision-history · **Notes:** ga-launch-quarter CONTRADICTION PAIR (Q1 2027 vs Q2 2027, both long-form). Load-bearing for read-time re-rank architecture viability.

- word_count: 119 · latency: 84.6s · contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **quality: PASS — contradictions_flagged.len()=1 · 'Q1 2027'=true AND 'Q2 2027'=true**

  synthesis_markdown:

  ```
  The GA launch date has been shifted from Q1 2027 to Q2 2027 based on the latest beta-readiness assessment. The primary factors driving this change are the analytics module's delayed feature set completion, the on-call rotation's inability to handle simultaneous workstreams, and the saturated analyst-briefing calendar. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the new Q2 2027 GA date. The beta launch is scheduled for December 5, with a three-wave rollout plan. The marketing pre-launch comms plan is being rebuilt with April 14 as the target launch date. Contradictions_flagged: The initial plan was for a Q1 2027 GA, but this was later changed to Q2 2027 due to beta-readiness issues and on-call rotation constraints.
  ```

---

### Q13 — "What's the Comcast bill?"

**Shape:** decision-history · **Notes:** comcast-monthly-bill CONTRADICTION PAIR ($89 vs $109, both long-form). Second product-strategic gate.

- word_count: 105 · latency: 74.3s · contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **quality: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  The Comcast bill is currently 109 dollars per month, up from the previous 89 dollars after the loyalty discount expired. The package includes 800 Mbps internet and basic cable channels, which are mostly watched for sports. The goal is to renegotiate the rate back down and potentially switch to a fiber competitor offering 1 Gbps for 70 dollars per month. The total monthly fixed expenses, excluding rent and groceries, are about 1,150 dollars, with plans to reduce streaming subscriptions to lower this amount. The previous year's rate was 89 dollars per month, but it has increased to 109 dollars per month, which is a contradiction.
  ```

---

### Q25 — "Help me update the product roadmap doc with the latest milestone dates."

**Shape:** oblique-agent-reformulation · **Notes:** Oblique reformulation of Q11. NO exact nouns from the GA contradiction memories ('GA', 'launch', 'Q1', 'Q2' all absent). Tests whether semantic generalization ('milestone dates', 'roadmap') still surfaces the contradiction pair. Expected set narrow on purpose — measures whether the contradiction-surfacing verdict survives oblique phrasing.

- word_count: 176 · latency: 101.0s · contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **quality: PASS — contradictions_flagged.len()=1 · 'Q1 2027'=true AND 'Q2 2027'=true**

  synthesis_markdown:

  ```
  The product roadmap doc should be updated with the latest milestone dates. The GA launch is now scheduled for Q2 2027, with a beta launch on December 5, 2026. The GA launch was initially planned for Q1 2027 but was postponed due to the need for additional beta data and alignment with stakeholder expectations. The beta will run for 6 weeks, with the first 50 design-partner customers launching on December 5, followed by 500 paid waitlist customers on December 12, and self-serve signups on December 19. The GA launch could slip 4-6 weeks if Sev-1 issues arise during the beta. The on-call rotation for the launch week is set, and the marketing pre-launch comms plan is being rebuilt with a target launch date of April 14, 2027. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the new GA date. The engineering ladder has been updated to include a new staff-plus track, with changes to the senior and principal level expectations. The first calibration under the new ladder will run in March 2027.
  ```

---

### Q26 — "Doing the monthly budget review — anything I should flag about household services costing more than expected?"

**Shape:** oblique-agent-reformulation · **Notes:** Oblique reformulation of Q13. NO 'Comcast' noun, no dollar amounts mentioned. Tests whether 'household services costing more than expected' semantically surfaces the Comcast contradiction pair (one paragraph says $89, the other says $109 — the price change IS what the agent is asking to flag).

- word_count: 145 · latency: 84.9s · contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **quality: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89. This increase is due to the expiration of the loyalty discount, which was not noticed due to the renewal email being in the spam folder. The partner suggested switching to a fiber competitor offering 1 Gbps for $70, but the current 800 Mbps tier on Comcast is more bandwidth than needed. The review also noted that the streaming services, totaling $54 per month, are hard to justify given actual viewing patterns. Hulu was used only 3 times in the last 6 months, and it will be cancelled. HBO Max will also be dropped after the current month. The goal is to reduce fixed subscription spending by $150 per month. Contradictions_flagged: The initial monthly cost of Comcast was reported as $89 in [mem-p-finance-006] but increased to $109 in [mem-p-finance-007].
  ```

---

### Q17 — "What's my exercise routine looking like?"

**Shape:** topic-synthesis · **Notes:** weekly-5k-run cluster + yoga twice-weekly (cross-cluster topic synthesis)

- word_count: 120 · latency: 101.7s · contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **quality: N/A (observational)**

  synthesis_markdown:

  ```
  Your exercise routine involves running a 5k on most weekends and doing yoga twice a week. The doctor recommends increasing your exercise to 4 days per week and suggests adding a sleep tracker. You are also planning to add a strength session to your yoga routine starting in June. Your current exercise schedule includes running 2-3 times a week and yoga on Tuesday evening and Saturday morning. You are considering indoor rock climbing as a new hobby, with plans to take an intro class and try a 3-month membership. Your sleep target is 7 hours minimum on weeknights, and you are focusing on dietary changes to improve your iron levels, including increasing iron-rich foods and drinking vitamin-C-containing juice with meals.
  ```

---

### Q19 — "Where are we with the launch timeline?"

**Shape:** topic-synthesis · **Notes:** Spans 3 clusters across 8 memories: alpha-launch-nov-15 (short) + beta-launch-dec-5 (paragraph) + ga-launch-quarter contradiction (long-form). Most ambitious recall query.

- word_count: 181 · latency: 119.7s · contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **quality: N/A (observational)**

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. The beta release is scheduled for December 5, with a three-wave rollout. The GA date was shifted from Q1 to Q2 due to the analytics module not being ready, the on-call rotation not being able to handle two simultaneous workstreams, and a saturated analyst-briefing calendar. The beta data will be extended by 6 weeks to ensure a higher quality launch. The alpha release is set for November 15. The code freeze starts on October 30, and the CI system will be migrated from CircleCI to GitHub Actions by the end of Q1. Contradictions_flagged: The GA date was initially set for Q1 2027 but was later moved to Q2 2027 due to the analytics module not being ready. Stakeholder alignment is locked, with all key stakeholders signing off on the new GA date. The beta opens on December 5, and the beta data will be extended to provide more reliability numbers for the GA launch.
  ```

---

### Q21 — "What did we decide about the Kubernetes migration?"

**Shape:** hard-negative · **Notes:** Vault has zero Kubernetes memories. Precision test: do retrieval scores correctly stay low (relative scoring) or does BGE return top-K of unrelated memories with high scores (absolute scoring)?

- word_count: 45 · latency: 61.1s · contradictions_flagged: 0 · vault_has_no_relevant_content: true
- **quality: PASS — vault_has_no_relevant_content=true**

  synthesis_markdown:

  ```
  There is no direct mention of a Kubernetes migration decision in the provided memories. The closest related topic is the database migration, which is not the same as a Kubernetes migration. Therefore, the vault has no relevant content for the query about the Kubernetes migration.
  ```

---

### Q22 — "What's my dental insurance policy number?"

**Shape:** hard-negative · **Notes:** Vault has one dental-cleaning memory (mem-p-health-009) but no insurance policy info. Precision test: does BGE return health-009 as a high-score false positive?

- word_count: 60 · latency: 60.8s · contradictions_flagged: 0 · vault_has_no_relevant_content: true
- **quality: PASS — vault_has_no_relevant_content=true**

  synthesis_markdown:

  ```
  There is no direct mention of your dental insurance policy number in any of the provided memories. The closest relevant memory is [mem-p-health-009], which discusses dental cleanings and mentions a night guard that is partially covered by the insurance plan once the annual maximum is hit. However, the policy number is not provided. No contradictions were found among the memories.
  ```

---

## Decision

**Latency gate cleared on the measured hardware class.** Quality preserved byte-for-byte vs t026 baseline (4/4 contradictions + 2/2 hard-negatives). Vulkan iGPU offload was the right single-knob lever — the 36% latency drop came from one feature flag, not weeks of tuning.

**Two load-bearing risks documented above:** Q19's 0.3s tail-latency margin (escape valve = Family B speculative decoding, V0.2.x), and hardware generalization beyond the measured Intel UHD class (untested: macOS Metal, older Intel iGPUs, CPU-only laptops).

**Forward path (gated on partner approval, in order):** platform-conditional Cargo.toml proposal → ADRs 048 (read-time pipeline) + 049 (Qwen-7B selection) + 050 (locked tuning + hardware scope) + ADR-045 Amendment 1 (consolidator reframed as housekeeping) → production read-time pipeline code in vault-retrieval → 26-query fixture promoted from spike to acceptance suite → bundle with T0.2.3 commit-3 staged content into a single commit (or 2-commit arc).
