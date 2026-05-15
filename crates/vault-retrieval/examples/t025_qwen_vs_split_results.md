# T0.2.3 Read-Time Architecture Spike — Pipeline A vs Pipeline B Results

**Run started:** 2026-05-14 15:05:33 UTC  
**Host OS:** windows  
**Pipeline A:** Phi-4 V-A relevance gate → Phi-4 stage-2.5 pairwise contradiction detection → Qwen2.5-14B synthesis with pre-flagged contradictions  
**Pipeline B:** Qwen2.5-14B standalone (single call does filter + contradiction-flag + synthesize)  
**Latency budget:** 2-min hard ceiling per partner-locked product framing.  
**Fixture:** 100 memories, 8 target queries from existing t023 26-query set  
**Setup time:** 7.3s for memory insertion

> **Discipline note.** Measurement only. Architectural decision (which pipeline to ship, or whether either does) is partner conversation based on this data + manual review of the verbatim synthesis outputs below.

---

## Latency comparison

| Pipeline | min | p50 | p99 | max | mean |
|---|---|---|---|---|---|
| A end-to-end | 301.2s | 417.5s | 655.6s | 655.6s | 415.4s |
| B end-to-end | 270.8s | 412.4s | 554.2s | 554.2s | 406.4s |

2-min (120s) ceiling: pipelines must clear this on every query, not just average.

## Structural assertion results (contradiction queries only)

| Query | Pipeline A struct | Pipeline B struct |
|---|---|---|
| Q11 | FAIL | **PASS** |
| Q13 | FAIL | **PASS** |
| Q25 | **PASS** | **PASS** |
| Q26 | FAIL | FAIL |

---

## Per-query detail

### Q11 — "When did we decide GA launch?"

**Shape:** decision-history · **Notes:** ga-launch-quarter CONTRADICTION PAIR (Q1 2027 vs Q2 2027, both long-form). Load-bearing for read-time re-rank architecture viability.

**Expected memories (2):** `mem-w-deadline-007`, `mem-w-deadline-008`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (6):** `mem-w-deadline-007`, `mem-w-deadline-009`, `mem-w-deadline-010`, `mem-w-deadline-012`, `mem-w-tech-001`, `mem-w-tech-003`
- **Stage 2 reasoning:** These candidates directly address the query about the GA launch date. Mem-w-deadline-007 discusses the planned GA launch timing and the reasoning behind it. Mem-w-deadline-009, 010, and 012 provide specific dates for the GA launch, beta release, and security review, respectively. Mem-w-tech-001 and 003 discuss decisions related to the GA launch, such as the choice of Postgres as the primary data store and the architecture decision record for the database spike.
- **Stage 2 latency:** 122.6s
- **Stage 2.5 pairs checked (0):** (no pairs above cosine 0.85 threshold)
- **Stage 2.5 total latency:** 0.0s
- **Total Pipeline A latency:** 400.5s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 159 · **latency:** 277.9s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false
- **structural assertion:** FAIL — contradictions_flagged.len()=0 · contains 'Q1 2027'=true AND 'Q2 2027'=false

```
The decision for GA launch was made during the product leadership review, with the current plan set for Q1 2027. The public launch announcement is targeted for late February, with the press push scheduled for early March. This timeline allows for at least 8 weeks of beta data collection, starting from December 5, to gather performance and reliability metrics. The Q1 quarter-close timing aligns with customer procurement cycles, and the industry conference circuit in early March provides an opportunity to announce and demo the product at multiple conferences. The risk of Sev-1 issues requiring schema migrations could potentially delay the GA by 4-6 weeks, but mitigations such as load testing, real-customer rollouts in waves, and on-call rotations are in place. Stakeholder alignment is confirmed, with CEO, CRO, CMO, and VP-eng all signing off on the Q1 2027 GA date at the leadership offsite last month. Sales and marketing teams are briefed and preparing their pipelines and pre-launch communications accordingly.
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 432.0s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 91 · **latency:** 432.0s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains 'Q1 2027'=true AND 'Q2 2027'=true

```
The decision on GA launch timing was discussed in two separate meetings, leading to contradictory plans. In the product leadership review (mem-w-deadline-007), the plan was set for Q1 2027, with a public launch announcement in late February and press push in early March. However, in a subsequent product strategy session (mem-w-deadline-008), the GA was moved to Q2 2027 due to beta-readiness concerns and operational constraints. Both plans cite stakeholder alignment with CEO, CRO, CMO, and VP-eng signing off on their respective dates. Sales and marketing teams are adjusting their plans accordingly.
```

**contradictions_flagged:**

```json
[
  {
    "current_position_if_determinable": "",
    "memory_ids": [
      "mem-w-deadline-007",
      "mem-w-deadline-008"
    ],
    "positions": [
      "Q1 2027 GA with late February public announcement and early March press push",
      "Q2 2027 GA with April 14 target launch date"
    ]
  }
]
```

---

### Q13 — "What's the Comcast bill?"

**Shape:** decision-history · **Notes:** comcast-monthly-bill CONTRADICTION PAIR ($89 vs $109, both long-form). Second product-strategic gate.

**Expected memories (2):** `mem-p-finance-006`, `mem-p-finance-007`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (7):** `mem-p-finance-006`, `mem-p-finance-007`, `mem-p-finance-010`, `mem-w-deadline-009`, `mem-w-deadline-006`, `mem-p-finance-012`, `mem-w-deadline-007`
- **Stage 2 reasoning:** The query asks for the Comcast bill. The relevant candidates directly address this query: mem-p-finance-006 and mem-p-finance-007 provide details on the Comcast bill amount and the agent's thoughts on renegotiating or switching services. mem-p-finance-010 discusses auto insurance renewal, which is not related to the Comcast bill. mem-w-deadline-009, mem-w-deadline-006, and mem-w-deadline-007 contain deadlines for various projects and tasks, which are not related to the Comcast bill. mem-p-finance-012 provides information on the tax filing extension, which is not related to the Comcast bill.
- **Stage 2 latency:** 124.5s
- **Stage 2.5 pairs checked (1):** 
  - [mem-p-finance-006 ↔ mem-p-finance-007] cos=0.854 · contradicts=false · field="" · 27.7s
- **Stage 2.5 total latency:** 27.7s
- **Total Pipeline A latency:** 384.9s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 91 · **latency:** 232.7s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false
- **structural assertion:** FAIL — contradictions_flagged.len()=0 · contains '89'=true AND '109'=true

```
The current Comcast bill for internet plus cable is $109 per month, up from the previous rate of $89 per month after the loyalty discount expired. The original rate was negotiated in spring when a switch to a fiber competitor was considered, offering a 12-month loyalty discount. The higher bill was noticed during a monthly statement review, prompting a plan to renegotiate or switch to the competitor offering 1 Gbps for $70 per month. Other subscriptions like Hulu and HBO Max are being evaluated for cancellation to reduce fixed subscription spending.
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 412.4s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 71 · **latency:** 412.4s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains '89'=true AND '109'=true

```
The Comcast bill for internet plus cable is currently 109 dollars per month (mem-p-finance-007). This is up from the previous rate of 89 dollars per month, which was locked in after a spring renegotiation when a loyalty discount expired (mem-p-finance-006). The plan is to call Comcast retention next week to either restart the loyalty negotiation or evaluate switching to a fiber competitor offering 1 Gbps for 70 dollars per month (mem-p-finance-007).
```

**contradictions_flagged:**

```json
[
  {
    "current_position_if_determinable": "",
    "memory_ids": [
      "mem-p-finance-006",
      "mem-p-finance-007"
    ],
    "positions": [
      "Comcast bill is 89 dollars per month (mem-p-finance-006)",
      "Comcast bill is 109 dollars per month (mem-p-finance-007)"
    ]
  }
]
```

---

### Q25 — "Help me update the product roadmap doc with the latest milestone dates."

**Shape:** oblique-agent-reformulation · **Notes:** Oblique reformulation of Q11. NO exact nouns from the GA contradiction memories ('GA', 'launch', 'Q1', 'Q2' all absent). Tests whether semantic generalization ('milestone dates', 'roadmap') still surfaces the contradiction pair. Expected set narrow on purpose — measures whether the contradiction-surfacing verdict survives oblique phrasing.

**Expected memories (2):** `mem-w-deadline-007`, `mem-w-deadline-008`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (9):** `mem-w-deadline-007`, `mem-w-deadline-003`, `mem-w-deadline-001`, `mem-w-deadline-008`, `mem-w-deadline-006`, `mem-w-deadline-011`, `mem-w-deadline-004`, `mem-w-deadline-002`, `mem-w-deadline-005`
- **Stage 2 reasoning:** These candidates directly address the query about updating the product roadmap doc with the latest milestone dates. They contain specific information about the planned dates for the product's GA launch, beta release, and other related milestones.
- **Stage 2 latency:** 160.5s
- **Stage 2.5 pairs checked (4):** 
  - [mem-w-deadline-007 ↔ mem-w-deadline-008] cos=0.904 · contradicts=true · field="GA launch timing" · 37.9s
  - [mem-w-deadline-003 ↔ mem-w-deadline-001] cos=0.854 · contradicts=false · field="None" · 14.1s
  - [mem-w-deadline-003 ↔ mem-w-deadline-002] cos=0.857 · contradicts=false · field="None" · 14.2s
  - [mem-w-deadline-001 ↔ mem-w-deadline-002] cos=0.929 · contradicts=false · field="None" · 14.3s
- **Stage 2.5 total latency:** 80.6s
- **Total Pipeline A latency:** 655.6s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 150 · **latency:** 414.4s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains 'Q1 2027'=true AND 'Q2 2027'=true

```
The product roadmap for the upcoming release includes several key milestones. The alpha release is locked for November 15, 2023, per [mem-w-deadline-003] and [mem-w-deadline-002]. The beta release is scheduled for December 5, 2023, with a three-wave rollout plan: first 50 design-partner customers on December 5, expanding to 500 paid waitlist customers on December 12, and opening self-serve signups on December 19, as discussed in [mem-w-deadline-004] and [mem-w-deadline-006]. The GA launch timing has been a point of contention, with initial plans set for Q1 2027, targeting late February for the public launch announcement and early March for the press push, per [mem-w-deadline-007]. However, a recent product strategy session moved the GA to Q2 2027 due to the analytics module's incomplete V1 feature set, the on-call rotation's inability to handle simultaneous workstreams, and the saturated analyst-briefing calendar for Q1, as detailed in [mem-w-deadline-008]. Contradictions_flagged: [mem-w-deadline-007] and [mem-w-deadline-008] disagree on GA launch timing.
```

**contradictions_flagged:**

```json
[
  {
    "current_position_if_determinable": "",
    "memory_ids": [
      "mem-w-deadline-007",
      "mem-w-deadline-008"
    ],
    "positions": [
      "Q1 2027 GA, targeting late February for the public launch announcement and early March for the press push",
      "moving GA to Q2 2027 based on the latest beta-readiness assessment"
    ]
  }
]
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 554.2s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 154 · **latency:** 554.2s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains 'Q1 2027'=true AND 'Q2 2027'=true

```
The product roadmap for the upcoming milestones includes the following key dates: Beta launch on December 5, 2026, with three waves of customer rollout. The initial wave of 50 design-partner customers will start on December 5, followed by 500 paid waitlist customers on December 12, and self-serve signups will open on December 19. The General Availability (GA) launch is currently planned for Q1 2027, with a public launch announcement in late February and a press push in early March. However, there is a contradiction regarding the GA date. [mem-w-deadline-007] states Q1 2027 GA, while [mem-w-deadline-008] moves GA to Q2 2027 based on beta-readiness assessments. The Q2 2027 GA plan is supported by the CEO, CRO, CMO, and VP-eng, with a target launch date of April 14. Beta opens on December 5 as previously planned, providing an additional 6 weeks of beta data to strengthen the GA narrative. Contradictions_flagged: [mem-w-deadline-007] vs [mem-w-deadline-008] on GA date.
```

**contradictions_flagged:**

```json
[
  {
    "current_position_if_determinable": "",
    "memory_ids": [
      "mem-w-deadline-007",
      "mem-w-deadline-008"
    ],
    "positions": [
      "Q1 2027 GA",
      "Q2 2027 GA"
    ]
  }
]
```

---

### Q26 — "Doing the monthly budget review — anything I should flag about household services costing more than expected?"

**Shape:** oblique-agent-reformulation · **Notes:** Oblique reformulation of Q13. NO 'Comcast' noun, no dollar amounts mentioned. Tests whether 'household services costing more than expected' semantically surfaces the Comcast contradiction pair (one paragraph says $89, the other says $109 — the price change IS what the agent is asking to flag).

**Expected memories (2):** `mem-p-finance-006`, `mem-p-finance-007`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (4):** `mem-p-finance-007`, `mem-p-finance-010`, `mem-p-finance-011`, `mem-p-health-007`
- **Stage 2 reasoning:** mem-p-finance-007: Relevant because it discusses the monthly budget review and household services costing more than expected, which directly relates to the user's query about flagging issues with household services costs. mem-p-finance-010: Relevant as it provides specific details about Comcast's internet plus cable cost, which is part of the user's concern about household services costing more than expected. mem-p-finance-011: Relevant because it mentions the tax filing extension, which could impact the user's household budget. mem-p-health-007: Relevant because it discusses the annual physical scheduled for April 18, which is a personal health matter and not directly related to the user's query about household services costs. However, it is included as it is part of the user's personal memory vault.
- **Stage 2 latency:** 139.2s
- **Stage 2.5 pairs checked (0):** (no pairs above cosine 0.85 threshold)
- **Stage 2.5 total latency:** 0.0s
- **Total Pipeline A latency:** 301.2s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 109 · **latency:** 162.0s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false
- **structural assertion:** FAIL — contradictions_flagged.len()=0 · contains '89'=false AND '109'=true

```
During the monthly budget review, it was noted that the household services costs exceeded expectations, primarily due to an increase in the Comcast internet and cable bill from 109 dollars per month, up from a lower rate due to an expired loyalty discount. The plan is to renegotiate with Comcast or switch to a fiber competitor offering 1 Gbps for 70 dollars per month. Additionally, two streaming services totaling 54 dollars per month will be canceled to reduce fixed subscription spending by targeting a 150 dollars per month reduction. The budget will be revisited in 90 days to confirm these changes. No contradictions were found among the reviewed memories.
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 384.1s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 110 · **latency:** 384.1s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false
- **structural assertion:** FAIL — contradictions_flagged.len()=0 · contains '89'=true AND '109'=true

```
During the monthly budget review, it was noted that household services cost more than expected. Specifically, the Comcast bill for internet plus cable increased from $89 to $109 per month due to the expiration of a loyalty discount (mem-p-finance-007). This increase, along with a re-evaluation of streaming services, prompted a plan to reduce fixed subscription spending by $150 per month. The audit revealed that Hulu was used only three times in six months, leading to its cancellation, and HBO Max will also be dropped after the current month. The goal is to renegotiate the Comcast bill or switch to a fiber competitor offering 1 Gbps for $70 per month (mem-p-finance-007).
```

---

### Q17 — "What's my exercise routine looking like?"

**Shape:** topic-synthesis · **Notes:** weekly-5k-run cluster + yoga twice-weekly (cross-cluster topic synthesis)

**Expected memories (3):** `mem-p-hobby-004`, `mem-p-hobby-005`, `mem-p-health-012`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (10):** `mem-p-hobby-005`, `mem-p-health-004`, `mem-p-hobby-004`, `mem-p-health-002`, `mem-p-health-006`, `mem-p-hobby-008`, `mem-p-health-008`, `mem-p-hobby-002`, `mem-p-health-011`, `mem-p-health-013`
- **Stage 2 reasoning:** The query asks for the user's exercise routine. The relevant candidates are those that directly address the user's exercise habits or plans. These include weekly 5k runs (mem-p-hobby-005), running a 5k every weekend (mem-p-hobby-004), wanting to try indoor rock climbing (mem-p-hobby-008), practicing guitar 20 minutes a day (mem-p-hobby-002), and taking vitamin D supplement every morning (mem-p-health-001). The other candidates are not directly related to the user's exercise routine.
- **Stage 2 latency:** 147.9s
- **Stage 2.5 pairs checked (1):** 
  - [mem-p-hobby-005 ↔ mem-p-hobby-004] cos=0.895 · contradicts=false · field="None" · 12.4s
- **Stage 2.5 total latency:** 12.4s
- **Total Pipeline A latency:** 417.5s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 96 · **latency:** 257.2s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false

```
The current exercise routine involves running a 5k every Saturday, as per [mem-p-hobby-005] and [mem-p-hobby-004]. However, the annual physical writeup from [mem-p-health-004] and the follow-up from [mem-p-health-002] indicate that the doctor recommends increasing exercise to 4 days per week to help manage slightly elevated blood pressure. Additionally, there is a plan to try indoor rock climbing this fall at a new gym, as mentioned in [mem-p-hobby-008]. The doctor's advice and the new hobby plan aim to improve overall health and manage lifestyle-driven health metrics like blood pressure and iron levels, as discussed in [mem-p-health-004] and [mem-p-health-011].
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 403.1s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 76 · **latency:** 403.1s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false

```
Your current exercise routine includes running a 5k 2-3 times per week and yoga twice a week. The doctor's visit ([mem-p-health-004]) recommended bumping exercise to 4 days per week to help manage blood pressure. The new plan ([mem-p-health-012]) is to maintain the running schedule and add yoga twice a week, with a goal to evaluate whether to keep this cadence or shift to once-weekly yoga plus a second weekly strength session by the end of June.
```

---

### Q19 — "Where are we with the launch timeline?"

**Shape:** topic-synthesis · **Notes:** Spans 3 clusters across 8 memories: alpha-launch-nov-15 (short) + beta-launch-dec-5 (paragraph) + ga-launch-quarter contradiction (long-form). Most ambitious recall query.

**Expected memories (8):** `mem-w-deadline-001`, `mem-w-deadline-002`, `mem-w-deadline-003`, `mem-w-deadline-004`, `mem-w-deadline-005`, `mem-w-deadline-006`, `mem-w-deadline-007`, `mem-w-deadline-008`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (10):** `mem-w-deadline-002`, `mem-w-deadline-001`, `mem-w-deadline-008`, `mem-w-deadline-004`, `mem-w-deadline-003`, `mem-w-deadline-006`, `mem-w-standup-010`, `mem-w-standup-006`, `mem-w-standup-011`, `mem-w-standup-008`
- **Stage 2 reasoning:** These candidates directly address the launch timeline query. They provide specific dates, reasons for changes, and planned actions related to the product's launch. The other candidates discuss topics like tech stack, hiring, and standup format, which are not directly related to the launch timeline.
- **Stage 2 latency:** 107.4s
- **Stage 2.5 pairs checked (3):** 
  - [mem-w-deadline-002 ↔ mem-w-deadline-001] cos=0.929 · contradicts=false · field="None" · 10.8s
  - [mem-w-deadline-002 ↔ mem-w-deadline-003] cos=0.857 · contradicts=false · field="None" · 10.0s
  - [mem-w-deadline-001 ↔ mem-w-deadline-003] cos=0.854 · contradicts=false · field="None" · 11.8s
- **Stage 2.5 total latency:** 32.7s
- **Total Pipeline A latency:** 434.1s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 142 · **latency:** 294.1s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false

```
The launch timeline is progressing with key dates set. Alpha is scheduled to launch on November 15, 2026, per [mem-w-deadline-002] and [mem-w-deadline-003]. Beta will open on December 5, 2026, as previously planned, with three waves of customer rollouts: first 50 design-partner customers on December 5, expanding to 500 paid waitlist customers on December 12, and opening self-serve signups on December 19, per [mem-w-deadline-004] and [mem-w-deadline-006]. General Availability (GA) has been moved to Q2 2027 due to several factors: the analytics module's V1 feature set won't be complete until late February, the on-call rotation can't support a Q1 launch alongside the new-platform migration, and the analyst-briefing calendar is saturated in Q1. Stakeholders including the CEO, CRO, CMO, and VP-eng have signed off on this decision. Marketing's pre-launch communications plan is being rebuilt with April 14, 2027, as the target launch date, per [mem-w-deadline-008].
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 435.3s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 115 · **latency:** 435.3s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false

```
The launch timeline is as follows: Alpha launch is scheduled for November 15 ([mem-w-deadline-002], [mem-w-deadline-001], [mem-w-deadline-003]). Beta release is locked for December 5, with a three-wave rollout plan ([mem-w-deadline-005], [mem-w-deadline-007], [mem-w-deadline-004], [mem-w-deadline-006]). General Availability (GA) was initially planned for Q1 2027, targeting late February for the public launch announcement and early March for the press push ([mem-w-deadline-007]). However, the GA timing was recently revised to Q2 2027 due to several factors: the analytics module's incomplete V1 feature set, the on-call rotation's inability to handle a Q1 launch alongside the new-platform migration, and the saturated analyst-briefing calendar for Q1 ([mem-w-deadline-008]). The new GA target is April 14, with beta opening as planned on December 5 ([mem-w-deadline-008]).
```

**contradictions_flagged:**

```json
[
  {
    "current_position_if_determinable": "",
    "memory_ids": [
      "mem-w-deadline-007",
      "mem-w-deadline-008"
    ],
    "positions": [
      "GA initially planned for Q1 2027, targeting late February for the public launch announcement and early March for the press push.",
      "GA timing revised to Q2 2027 based on the latest beta-readiness assessment, with a new target of April 14."
    ]
  }
]
```

---

### Q21 — "What did we decide about the Kubernetes migration?"

**Shape:** hard-negative · **Notes:** Vault has zero Kubernetes memories. Precision test: do retrieval scores correctly stay low (relative scoring) or does BGE return top-K of unrelated memories with high scores (absolute scoring)?

**Expected memories (0):** `(empty — hard-negative)`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (4):** `mem-w-tech-002`, `mem-w-deadline-007`, `mem-w-deadline-002`, `mem-w-deadline-011`
- **Stage 2 reasoning:** The query asks about the decision made regarding the Kubernetes migration. The relevant candidates are those that directly address decisions about Kubernetes or related infrastructure, such as the choice of Postgres as the primary data store for the platform, which impacts how services are deployed and managed in Kubernetes. The other candidates discuss different topics like hiring freezes, standup processes, and GA launch timing, which are not directly related to Kubernetes migration.
- **Stage 2 latency:** 144.9s
- **Stage 2.5 pairs checked (0):** (no pairs above cosine 0.85 threshold)
- **Stage 2.5 total latency:** 0.0s
- **Total Pipeline A latency:** 305.8s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 44 · **latency:** 160.8s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** true

```
There are no relevant memories about the Kubernetes migration in the vault. The filtered memories discuss decisions related to database selection, launch timelines, and billing system updates, but do not mention any Kubernetes migration plans or decisions. No contradictions were flagged among the memories.
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 359.6s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 29 · **latency:** 359.6s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** true

```
None of the provided memories directly address the Kubernetes migration. The candidates discuss various technical and operational decisions but do not mention Kubernetes or any migration related to it.
```

---

### Q22 — "What's my dental insurance policy number?"

**Shape:** hard-negative · **Notes:** Vault has one dental-cleaning memory (mem-p-health-009) but no insurance policy info. Precision test: does BGE return health-009 as a high-score false positive?

**Expected memories (0):** `(empty — hard-negative)`

#### Pipeline A — Phi-4 split + Qwen synthesis

- **Stage 2 filtered (14):** `mem-p-health-009`, `mem-p-finance-010`, `mem-p-health-004`, `mem-p-health-010`, `mem-p-health-001`, `mem-w-standup-008`, `mem-w-deadline-009`, `mem-w-deadline-011`, `mem-w-deadline-004`, `mem-w-deadline-006`, `mem-w-deadline-007`, `mem-p-finance-002`, `mem-p-finance-012`, `mem-p-finance-007`
- **Stage 2 reasoning:** The query is asking for the dental insurance policy number. None of the candidates contain this information. However, the candidates do contain other relevant information about dental insurance, such as the dentist's name, the insurance plan, and the cost of a night guard. These candidates are relevant to the query because they contain information related to dental insurance.
- **Stage 2 latency:** 133.2s
- **Stage 2.5 pairs checked (0):** (no pairs above cosine 0.85 threshold)
- **Stage 2.5 total latency:** 0.0s
- **Total Pipeline A latency:** 423.9s (budget 120s: **OVER**)

**Synthesis output (Qwen, with pre-flagged contradictions):**

- **word_count:** 37 · **latency:** 290.6s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** true

```
No relevant memories exist that provide the dental insurance policy number. The filtered memories discuss dental cleanings, annual physicals, eye exams, and various financial and work-related tasks, but none of them mention the dental insurance policy number.
```

#### Pipeline B — Qwen standalone

- **Total Pipeline B latency:** 270.8s (budget 120s: **OVER**)

**Synthesis output (Qwen single-call):**

- **word_count:** 29 · **latency:** 270.8s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** true

```
None of the provided memories contain information about the dental insurance policy number. The memories discuss various health and financial topics but do not mention dental insurance policy details.
```

---

## Architectural decision — DEFERRED

Data only. The architecture call (which pipeline ships, or whether either does at acceptable quality + latency) is partner conversation.
