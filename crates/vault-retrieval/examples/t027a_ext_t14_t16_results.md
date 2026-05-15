# T0.2.3 t027a extension — n_threads = 14 + 16 sanity check

**Run started:** 2026-05-15 08:02:38 UTC  
**Model:** Qwen2.5-7B-Instruct Q4_K_M  
**Hardware:** i7-13620H (10P / 16T, AVX2)  
**Reference (t027a):** t10 = 139.6s mean, t12 = 134.2s mean (winner so far)  
**Hard ceiling:** 120s per query  
**Fixture:** 100 memories, 3.9s insertion

## Summary table (with t027a reference)

| Config | Description | Q19 (s) | Q26 (s) | Mean (s) | Q26 quality |
|---|---|---|---|---|---|
| t10 (ref) | n_threads=10 (from t027a) | 142.0 | 137.2 | 139.6 | **PASS** |
| t12 (ref) | n_threads=12 (from t027a) | 132.4 | 136.0 | 134.2 | **PASS** |
| t14 | n_threads=14, n_threads_batch=14 (above P-core count, into HT) | 171.7 | 149.5 | 160.6 | **PASS** |
| t16 | n_threads=16 (= full logical-thread count) | 161.8 | 140.7 | 151.2 | **PASS** |

## Per-config detail

### t14 — n_threads=14, n_threads_batch=14 (above P-core count, into HT)

#### Q19

- latency: 171.7s
- contradictions_flagged: 2 · vault_has_no_relevant_content: false

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. This decision was made to ensure at least 8 weeks of beta data, align with customer procurement cycles, and take advantage of the industry conference circuit. The beta is scheduled to open on December 5, with a three-wave rollout. However, there is a recent shift to Q2 2027 due to the analytics module not being ready until late February, the on-call rotation not being able to handle a Q1 launch alongside a new-platform migration, and a saturated analyst-briefing calendar. The GA is now targeted for April 14, with the beta still opening on December 5. The engineering team is on track with the beta launch, and the on-call rotation is set for launch week. Contradictions flagged: the initial GA target was Q1 2027, but it has been shifted to Q2 2027 due to new factors.
  ```

#### Q26

- latency: 149.5s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **Q26 quality assert: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89 due to the expiration of the loyalty discount. This is a significant increase, and the household should consider switching to a cheaper competitor or renegotiating the contract. Additionally, the review noted that the household could save $54 per month by canceling Hulu and HBO Max, as these services were infrequently used. The total fixed subscription spending could be reduced by $150 per month with these changes. The household should also re-evaluate the Comcast package to see if a lower tier would suffice. Contradictions_flagged: The initial monthly cost of Comcast was reported as $89 in [mem-p-finance-006] but increased to $109 in [mem-p-finance-007].
  ```

---

### t16 — n_threads=16 (= full logical-thread count)

#### Q19

- latency: 161.8s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false

  synthesis_markdown:

  ```
  The launch timeline for the product is currently set for Q1 2027, with a GA launch in late February, and a public launch announcement in early March. This decision was based on several factors, including the need for 8 weeks of beta data, alignment with customer procurement cycles, and the industry conference circuit. The beta launch is scheduled for December 5, with a three-wave rollout. However, there is a recent shift to Q2 2027 due to the analytics module not being ready by the initial Q1 date. The GA is now planned for April 14, with an additional 6 weeks of beta data to strengthen the launch narrative. The beta launch is on track, with the engineering team completing feature work by November 22 and a two-week stabilization period before the December 5 cutover. Risks include potential Sev-1 issues that could delay the GA by 4-6 weeks, but mitigations are in place. Stakeholders, including the CEO, CRO, CMO, and VP-eng, have signed off on the new Q2 2027 GA date.
  ```

#### Q26

- latency: 140.7s
- contradictions_flagged: 1 · vault_has_no_relevant_content: false
- **Q26 quality assert: PASS — contradictions_flagged.len()=1 · '89'=true AND '109'=true**

  synthesis_markdown:

  ```
  During the recent monthly budget review, Comcast's monthly cost increased to $109, up from the previous $89, due to the expiration of the loyalty discount. This is a significant increase, and the household should consider switching to a cheaper competitor or renegotiating the contract. Additionally, the streaming services, which total $54 per month, are being re-evaluated, with plans to cancel Hulu and HBO Max due to low usage. The goal is to reduce fixed subscription spending by $150 per month. The electricity bill averages $145 per month, with seasonal fluctuations, and the Comcast package is being re-evaluated for potential downgrading to reduce costs.
  ```

---

## Decision

Data only — partner reviews before promoting.
