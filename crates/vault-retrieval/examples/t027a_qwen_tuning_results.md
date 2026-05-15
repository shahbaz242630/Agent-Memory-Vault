# T0.2.3 t027a — Qwen-7B Knob-Tuning Spike Results

**Run started:** 2026-05-15 07:04:56 UTC  
**Model:** Qwen2.5-7B-Instruct Q4_K_M  
**Hardware:** i7-13620H (10P / 16T, AVX2)  
**Baseline target (from t026):** mean 187s · p99 224s  
**Hard ceiling:** 120s per query  
**Queries:** Q19 (multi-cluster narrative) + Q26 (oblique Comcast contradiction)  
**Fixture:** 100 memories, 4.1s insertion

## Summary table

| Config | Description | Q19 (s) | Q26 (s) | Mean (s) | Q26 quality |
|---|---|---|---|---|---|
| baseline | framework defaults (n_threads=4, KV f16, n_batch=n_ctx) | 214.3 | 205.7 | 210.0 | **PASS** |
| t8 | n_threads=8, n_threads_batch=8 (KV f16 default) | 171.7 | 145.0 | 158.4 | **PASS** |
| t10 | n_threads=10 (= P-core count on i7-13620H) | 142.0 | 137.2 | 139.6 | **PASS** |
| t12 | n_threads=12 (mid-ground with hyperthreading) | 132.4 | 136.0 | 134.2 | **PASS** |
| t12_kv_q8 | winning threads (12) + KV K/V both Q8_0 (compose) | 191.9 | 168.2 | 180.0 | **PASS** |

## Per-config detail

### baseline — framework defaults (n_threads=4, KV f16, n_batch=n_ctx)

**Tuning:**

```text
TuningConfig {
    n_threads: None,
    n_threads_batch: None,
    n_batch: None,
    type_k: None,
    type_v: None,
}
```

#### Q19

- latency: 214.3s
- contradictions_flagged: 2 · vault_has_no_relevant_content: false

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. This decision was made to ensure at least 8 weeks of beta data, align with customer procurement cycles, and take advantage of the industry conference circuit. The beta is scheduled to open on December 5, with a three-wave rollout. However, there is a recent shift to Q2 2027 due to the analytics module not being ready by the initial plan, the on-call rotation not being able to handle a Q1 launch alongside a new-platform migration, and the saturated analyst-briefing calendar. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the Q2 2027 GA date. The beta data from the extended period will significantly strengthen the GA narrative. Contradictions_flagged: The initial GA target was Q1 2027, but it has been shifted to Q2 2027 due to the latest beta-readiness assessment.
  ```

#### Q26

- latency: 205.7s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **Q26 quality assert: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89 due to the expiration of the loyalty discount. This is a significant increase, and you should consider either restarting the loyalty negotiation or switching to a cheaper fiber competitor. Additionally, the review noted that the current streaming services (Hulu, Netflix, Disney+, and HBO Max) cost $54 per month, but usage patterns show that Hulu was only used 3 times in the last 6 months, suggesting it should be cancelled. The goal is to reduce fixed subscription spending by $150 per month. The total monthly fixed expenses, excluding rent and groceries, are around $1,150, which is higher than expected. You should re-evaluate your subscriptions and consider reducing costs.
  ```

---

### t8 — n_threads=8, n_threads_batch=8 (KV f16 default)

**Tuning:**

```text
TuningConfig {
    n_threads: Some(
        8,
    ),
    n_threads_batch: Some(
        8,
    ),
    n_batch: None,
    type_k: None,
    type_v: None,
}
```

#### Q19

- latency: 171.7s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. The beta release is scheduled for December 5, with a three-wave rollout. The GA date was moved from Q1 to Q2 2027 due to the analytics module not being ready, the on-call rotation conflict with a new platform migration, and a saturated analyst-briefing calendar. The beta data will be extended by 6 weeks to ensure a higher quality launch. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the new GA date. The beta launch is on track, with the first 50 design-partner customers launching on December 5, followed by 500 paid waitlist customers on December 12, and self-serve signups on December 19. The marketing site copy and pricing page updates are scheduled for December 1 and 3, respectively. Contradictions_flagged: The GA date was initially set for Q1 2027 but was moved to Q2 2027 due to the latest beta-readiness assessment, which contradicts the initial plan.
  ```

#### Q26

- latency: 145.0s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **Q26 quality assert: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89, due to the expiration of the loyalty discount. This is a significant increase, and the household is considering switching to a cheaper fiber competitor or downgrading the service. Additionally, the review noted that the streaming services, including Hulu, are being overcharged, with Hulu usage being minimal. The household plans to cancel Hulu and HBO Max, aiming for a $150 reduction in fixed subscription spending. The total monthly fixed expenses, excluding rent and groceries, are around $1,150, with the goal to reduce this by $150.
  ```

---

### t10 — n_threads=10 (= P-core count on i7-13620H)

**Tuning:**

```text
TuningConfig {
    n_threads: Some(
        10,
    ),
    n_threads_batch: Some(
        10,
    ),
    n_batch: None,
    type_k: None,
    type_v: None,
}
```

#### Q19

- latency: 142.0s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. This decision was made to ensure at least 8 weeks of beta data, align with customer procurement cycles, and take advantage of the industry conference circuit. The beta is scheduled to open on December 5, with a three-wave rollout. However, there is a recent shift to Q2 2027 due to the analytics module not being ready, the on-call rotation conflict with a new platform migration, and a saturated analyst-briefing calendar. The GA is now targeted for April 14, with the beta still set for December 5. The engineering team is on track for the beta release, with the code freeze starting on October 30. Contradictions flagged: the GA launch date has been shifted from late February to April 14, and the beta launch date remains December 5 as originally planned.
  ```

#### Q26

- latency: 137.2s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **Q26 quality assert: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89, due to the expiration of the loyalty discount. This is a significant increase, and the household should consider switching to a competitor or renegotiating the contract. Additionally, the review noted that the current streaming services, including Hulu, are being overpriced based on actual usage. Hulu was only used three times in the last six months, and it is recommended to cancel this subscription. The goal is to reduce fixed subscription spending by $150 per month. The electricity bill averages $145 per month, with seasonal fluctuations, and the cell phone bill is $75 per month on the family plan with your partner.
  ```

---

### t12 — n_threads=12 (mid-ground with hyperthreading)

**Tuning:**

```text
TuningConfig {
    n_threads: Some(
        12,
    ),
    n_threads_batch: Some(
        12,
    ),
    n_batch: None,
    type_k: None,
    type_v: None,
}
```

#### Q19

- latency: 132.4s
- contradictions_flagged: 0 · vault_has_no_relevant_content: false

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. This decision was made to ensure at least 8 weeks of beta data, align with customer procurement cycles, and take advantage of the industry conference circuit. The beta is scheduled to open on December 5, with a three-wave rollout. However, there is a recent shift to Q2 2027 due to the analytics module not being ready until late February, the on-call rotation not being able to handle a Q1 launch alongside a new-platform migration, and a saturated analyst-briefing calendar. Stakeholders have re-aligned, and the sales and marketing teams are updating their plans accordingly. The beta data will be crucial for the GA narrative, and the GA could slip by 4-6 weeks if there are Sev-1 issues. The code freeze starts on October 30, and the CI/CD pipeline will be migrated from CircleCI to GitHub Actions by the end of Q1. No contradictions were found among the relevant memories.
  ```

#### Q26

- latency: 136.0s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **Q26 quality assert: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89 due to the expiration of the loyalty discount. This is a significant increase, and the household should consider switching to a competitor or renegotiating the contract. Additionally, the streaming services, which total $54 per month, are being re-evaluated. Hulu was only used 3 times in the last 6 months, and it is recommended to cancel this subscription. The goal is to reduce fixed subscription spending by $150 per month. The electricity bill averages $145 per month, with seasonal fluctuations, and the cell phone bill is $75 per month on the family plan with your partner.
  ```

---

### t12_kv_q8 — winning threads (12) + KV K/V both Q8_0 (compose)

**Tuning:**

```text
TuningConfig {
    n_threads: Some(
        12,
    ),
    n_threads_batch: Some(
        12,
    ),
    n_batch: None,
    type_k: Some(
        Q8_0,
    ),
    type_v: Some(
        Q8_0,
    ),
}
```

#### Q19

- latency: 191.9s
- contradictions_flagged: 2 · vault_has_no_relevant_content: false

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. The beta release is scheduled for December 5, with a three-wave rollout. The GA date was moved from Q1 to Q2 2027 due to the analytics module not being ready, the on-call rotation not being able to handle two simultaneous crises, and a saturated analyst-briefing calendar. The beta data will be extended by 6 weeks to ensure a higher quality launch. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the new GA date. The beta data will be crucial for the GA narrative, and the marketing pre-launch comms plan is being rebuilt with Apr 14 as the target launch date. The beta data will be extended by 6 weeks to ensure a higher quality launch. Contradictions_flagged: The GA date was initially set for Q1 2027 but was moved to Q2 2027 due to the analytics module not being ready, the on-call rotation not being able to handle two simultaneous crises, and a saturated analyst-briefing calendar.
  ```

#### Q26

- latency: 168.2s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **Q26 quality assert: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89 due to the expiration of the loyalty discount. This is a significant increase, and the household should consider switching to a cheaper fiber competitor or downgrading the service. Additionally, the review noted that the streaming services, including Hulu, are being overcharged. Hulu was only used 3 times in the last 6 months, and it is recommended to cancel this subscription. The goal is to reduce fixed subscription spending by $150 per month. The electricity bill averages $145 per month, with seasonal fluctuations, and the cell phone bill is $75 per month on the family plan with your partner.
  ```

---

## Decision

Data only — partner reviews before promoting.
