# T0.2.3 Qwen2.5-7B Standalone Spike — Results

**Run started:** 2026-05-14 17:32:09 UTC  
**Model:** Qwen2.5-7B-Instruct Q4_K_M (4.36 GB GGUF, Apache 2.0)  
**Pipeline:** Single Qwen-7B call — filter + flag contradictions + synthesize  
**Latency ceiling:** 2 min hard cap per partner product framing  
**Fixture:** 100 memories from merge_acceptance_100.json, 8 target queries  
**Setup:** 3.9s memory insertion

## Latency summary

| min | p50 | p99 | max | mean |
|---|---|---|---|---|
| 156.1s | 187.0s | 224.5s | 224.5s | 186.9s |

**2-min ceiling check:** p99 = 224.5s — **OVER budget**

## Structural assertion (contradiction queries)

| Query | Verdict | Detail |
|---|---|---|
| Q11 | **PASS** | contradictions_flagged.len()=1 · contains 'Q1 2027'=true AND 'Q2 2027'=true |
| Q13 | **PASS** | contradictions_flagged.len()=1 · contains '89'=true AND '109'=true |
| Q25 | **PASS** | contradictions_flagged.len()=1 · contains 'Q1 2027'=true AND 'Q2 2027'=true |
| Q26 | **PASS** | contradictions_flagged.len()=1 · contains '89'=true AND '109'=true |

## Per-query detail

### Q11 — "When did we decide GA launch?"

**Shape:** decision-history · **Notes:** ga-launch-quarter CONTRADICTION PAIR (Q1 2027 vs Q2 2027, both long-form). Load-bearing for read-time re-rank architecture viability.

- **word_count:** 100 · **latency:** 204.8s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains 'Q1 2027'=true AND 'Q2 2027'=true

**synthesis_markdown:**

```
The GA launch date has been adjusted from Q1 2027 to Q2 2027 based on the latest beta-readiness assessment. The primary factors driving this change are the completion of the analytics module, the on-call rotation constraints, and the saturated analyst-briefing calendar. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the new Q2 2027 GA date. The beta launch is scheduled for December 5, with a pre-launch comms plan starting January 10. The marketing calendar has been rebuilt with April 14 as the target launch date. Contradictions flagged: mem-w-deadline-007 (Q1 2027 GA) vs mem-w-deadline-008 (Q2 2027 GA).
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

### Q13 — "What's the Comcast bill?"

**Shape:** decision-history · **Notes:** comcast-monthly-bill CONTRADICTION PAIR ($89 vs $109, both long-form). Second product-strategic gate.

- **word_count:** 102 · **latency:** 176.2s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains '89'=true AND '109'=true

**synthesis_markdown:**

```
The Comcast bill is currently 109 dollars per month, up from the previous 89 dollars after the loyalty discount expired. The package includes 800 Mbps internet and basic cable channels, which are rarely watched. The goal is to renegotiate the rate back down and potentially switch to a fiber competitor offering 1 Gbps for 70 dollars per month. The monthly fixed expenses, excluding rent and groceries, are about 1,150 dollars, with plans to reduce streaming subscriptions to lower this amount. The previous year's rate was 89 dollars per month, but it has increased to 109 dollars per month, which is a contradiction.
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
      "Comcast bill was 89 dollars per month last year (mem-p-finance-006)",
      "Comcast bill is now 109 dollars per month (mem-p-finance-007)"
    ]
  }
]
```

---

### Q25 — "Help me update the product roadmap doc with the latest milestone dates."

**Shape:** oblique-agent-reformulation · **Notes:** Oblique reformulation of Q11. NO exact nouns from the GA contradiction memories ('GA', 'launch', 'Q1', 'Q2' all absent). Tests whether semantic generalization ('milestone dates', 'roadmap') still surfaces the contradiction pair. Expected set narrow on purpose — measures whether the contradiction-surfacing verdict survives oblique phrasing.

- **word_count:** 128 · **latency:** 224.5s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains 'Q1 2027'=true AND 'Q2 2027'=true

**synthesis_markdown:**

```
The product roadmap for the GA launch has been updated to Q2 2027, based on the latest beta-readiness assessment. The GA is scheduled for late February 2027, with the beta opening on December 5, 2026. The engineering team is on track with feature work complete by November 22, 2026, and stabilization and bug-bashing planned for the week before the beta launch. The on-call rotation for the launch week is set, and the beta data will be used to strengthen the GA narrative. The marketing and sales teams have been briefed and are aligning their plans with the new GA date. Contradictions_flagged: The GA date was initially set for Q1 2027 but was moved to Q2 2027 due to beta-readiness and other factors, as detailed in mem-w-deadline-008 and mem-w-deadline-007.
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
      "GA in Q1 2027",
      "GA moved to Q2 2027"
    ]
  }
]
```

---

### Q26 — "Doing the monthly budget review — anything I should flag about household services costing more than expected?"

**Shape:** oblique-agent-reformulation · **Notes:** Oblique reformulation of Q13. NO 'Comcast' noun, no dollar amounts mentioned. Tests whether 'household services costing more than expected' semantically surfaces the Comcast contradiction pair (one paragraph says $89, the other says $109 — the price change IS what the agent is asking to flag).

- **word_count:** 124 · **latency:** 189.8s · **contradictions_flagged.len():** 1 · **vault_has_no_relevant_content:** false
- **structural assertion:** PASS — contradictions_flagged.len()=1 · contains '89'=true AND '109'=true

**synthesis_markdown:**

```
During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89 due to the expiration of the loyalty discount. This is a significant increase, and you should consider either restarting the loyalty negotiation or switching to a cheaper fiber competitor. Additionally, the review noted that the current streaming services (Hulu, Netflix, Disney+, and HBO Max) cost $54 per month, but usage patterns show that Hulu was only used 3 times in the last 6 months, suggesting it should be cancelled. The goal is to reduce fixed subscription spending by $150 per month. The total monthly fixed expenses, excluding rent and groceries, are around $1,150, which is higher than expected. You should re-evaluate your subscriptions and consider reducing costs.
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
      "Comcast cost is $89 per month (mem-p-finance-006)",
      "Comcast cost is $109 per month (mem-p-finance-007)"
    ]
  }
]
```

---

### Q17 — "What's my exercise routine looking like?"

**Shape:** topic-synthesis · **Notes:** weekly-5k-run cluster + yoga twice-weekly (cross-cluster topic synthesis)

- **word_count:** 119 · **latency:** 179.3s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** false

**synthesis_markdown:**

```
Your exercise routine includes running a 5k on most weekends and doing yoga twice a week. The doctor recommends increasing your exercise to 4 days per week and suggests adding a sleep tracker. You are also planning to try indoor rock climbing this fall, starting with an intro class. Your current routine involves running 2-3 times a week and yoga on Tuesday evening and Saturday morning. The doctor is monitoring your blood pressure, which is slightly elevated but within the watch zone. Iron levels are low, and you are focusing on dietary changes to improve them. Sleep quality is average, and you aim for at least 7 hours of sleep on weeknights. No contradictions were found among the memories.
```

---

### Q19 — "Where are we with the launch timeline?"

**Shape:** topic-synthesis · **Notes:** Spans 3 clusters across 8 memories: alpha-launch-nov-15 (short) + beta-launch-dec-5 (paragraph) + ga-launch-quarter contradiction (long-form). Most ambitious recall query.

- **word_count:** 159 · **latency:** 187.0s · **contradictions_flagged.len():** 2 · **vault_has_no_relevant_content:** false

**synthesis_markdown:**

```
The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. This decision was made to ensure at least 8 weeks of beta data, align with customer procurement cycles, and take advantage of the industry conference circuit. The beta is scheduled to open on December 5, with a three-wave rollout. However, there is a recent shift to Q2 2027 due to the analytics module not being ready by the initial plan, the on-call rotation not being able to handle a Q1 launch alongside a new-platform migration, and the saturated analyst-briefing calendar. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the Q2 2027 GA date. The beta data from the extended period will significantly strengthen the GA narrative. Contradictions_flagged: The initial GA target was Q1 2027, but it has been shifted to Q2 2027 due to the latest beta-readiness assessment.
```

**contradictions_flagged:**

```json
[
  {
    "current_position_if_determinable": "",
    "memory_ids": [
      "mem-w-deadline-001",
      "mem-w-deadline-002",
      "mem-w-deadline-003"
    ],
    "positions": [
      "Q1 2027",
      "Q1 2027",
      "November 15"
    ]
  },
  {
    "current_position_if_determinable": "",
    "memory_ids": [
      "mem-w-deadline-007",
      "mem-w-deadline-008"
    ],
    "positions": [
      "Q1 2027",
      "Q2 2027"
    ]
  }
]
```

---

### Q21 — "What did we decide about the Kubernetes migration?"

**Shape:** hard-negative · **Notes:** Vault has zero Kubernetes memories. Precision test: do retrieval scores correctly stay low (relative scoring) or does BGE return top-K of unrelated memories with high scores (absolute scoring)?

- **word_count:** 41 · **latency:** 177.5s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** true

**synthesis_markdown:**

```
There is no direct mention of a decision regarding the Kubernetes migration in the provided memories. The closest related topic is the CI/CD migration from CircleCI to GitHub Actions, which was decided in Q1. No contradictions were found among the memories.
```

---

### Q22 — "What's my dental insurance policy number?"

**Shape:** hard-negative · **Notes:** Vault has one dental-cleaning memory (mem-p-health-009) but no insurance policy info. Precision test: does BGE return health-009 as a high-score false positive?

- **word_count:** 54 · **latency:** 156.1s · **contradictions_flagged.len():** 0 · **vault_has_no_relevant_content:** true

**synthesis_markdown:**

```
There is no direct mention of your dental insurance policy number in any of the provided memories. The closest relevant memory is [mem-p-health-009], which discusses dental cleanings and mentions a night guard that is partially covered by the insurance plan. However, the policy number is not provided. No contradictions were found among the memories.
```

---

## Architectural decision — DEFERRED

Data only.
