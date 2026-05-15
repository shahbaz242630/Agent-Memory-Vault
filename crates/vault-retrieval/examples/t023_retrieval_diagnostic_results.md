# T0.2.3 Retrieval-Diagnostic Spike — Results

**Run started:** 2026-05-14 10:43:11 UTC  
**Host OS:** windows  
**Embedding model:** bge-small-en-v1.5 (384-dim, BgeSmallProvider)  
**Retriever:** SemanticRetriever with `RetrievalOptions::default()` (no score threshold, `include_archived = false`)  
**Storage:** sealed LanceVectorStore + MetadataStore against a tempdir (single-process, single-run)  
**Fixture:** 100 memories from `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json`  
**Queries:** 26 from `crates/vault-retrieval/test-fixtures/merge_acceptance_100_queries.json`  
**Setup wall time:** 2.6s memory insertion (26 ms/memory, includes BGE embed + LanceDB upsert + SQLite write)  
**Query wall time:** 0.37s for 26 queries (14 ms/query at max_results=20)  

> **Discipline note.** This document reports measurements only. Architectural conclusions (scenario A / B / C, read-time re-rank viability, embedding-layer overhaul) live in a separate conversation with Shahbaz once the data is in. No ADR amendments, no plan changes, no threshold tuning, no code changes are derived in this file.

---

## Aggregate by query shape

| Group | n queries | avg recall@5 | avg recall@10 | avg recall@20 | avg MRR |
|---|---|---|---|---|---|
| `catch-me-up` | 5 | 0.90 | 0.95 | 1.00 | 0.900 |
| `decision-history` | 5 | 1.00 | 1.00 | 1.00 | 1.000 |
| `hard-negative` | 2 | — | — | — | — |
| `oblique-agent-reformulation` | 2 | 0.75 | 1.00 | 1.00 | 1.000 |
| `specific-fact` | 5 | 1.00 | 1.00 | 1.00 | 1.000 |
| `topic-synthesis` | 5 | 0.81 | 1.00 | 1.00 | 1.000 |
| `verbose-agent-reformulation` | 2 | 1.00 | 1.00 | 1.00 | 1.000 |

## Aggregate by content-length tier

This is the load-bearing breakdown for the architectural question. If recall is healthy on `short-only` but degraded on `mixed-length` and `long-form-dominant`, the same length-variance pattern that broke pairwise clustering is also present in query-anchored retrieval (Scenario B/C). If recall is uniform across tiers, the query anchor compensates for what pairwise cosine couldn't (Scenario A).

| Group | n queries | avg recall@5 | avg recall@10 | avg recall@20 | avg MRR |
|---|---|---|---|---|---|
| `hard-negative` | 2 | — | — | — | — |
| `long-form-dominant` | 7 | 0.93 | 1.00 | 1.00 | 0.929 |
| `mixed-length` | 3 | 1.00 | 1.00 | 1.00 | 1.000 |
| `multi-cluster` | 4 | 0.76 | 1.00 | 1.00 | 1.000 |
| `short-only` | 10 | 0.95 | 0.97 | 1.00 | 1.000 |

## Contradiction-surfacing verdict (Q11, Q13)

**The single most product-critical output of this spike.** Read-time re-rank with Phi-4 can only flag a contradiction to the agent if BGE retrieval surfaces BOTH halves of the conflicting pair in the top-K. If retrieval drops one, Phi-4 has nothing to compare against.

| Query | Pair | Verdict | recall@20 | top-20 hits / expected | first-hit rank |
|---|---|---|---|---|---|
| Q11 | GA launch Q1 vs Q2 | **PASS** | 1.00 | 2 / 2 | 1 |
| Q13 | Comcast $89 vs $109 | **PASS** | 1.00 | 2 / 2 | 1 |

**Verdict legend.** PASS = both contradiction-pair memories appeared in top-20. PARTIAL = exactly one in top-20 (read-time re-rank cannot surface the conflict). FAIL = neither in top-20 (retrieval is blind to the contradiction).

## Hard-negative inspection (Q21, Q22)

Are BGE scores absolute (low score → no good match) or relative (top-K always returned even if scores are bad)? Low top-1 scores on hard-negatives → score-threshold gating would let the retriever say "I don't know" cleanly. High top-1 scores → false positives are a precision problem we'd need to handle elsewhere (read-time re-rank can filter, or absolute thresholds must be applied at the retriever).

### Q21 — "What did we decide about the Kubernetes migration?"

- Top-1 score: **0.7169**  
- Results with score ≥ 0.5: 20  
- Results with score ≥ 0.7: 2  

Top-5 returned (all false positives — vault has no matching memories):

| Rank | Score | Fixture ID | Content snippet |
|---|---|---|---|
| 1 | 0.7169 | mem-w-tech-005 | Database migrations are Bob's responsibility from now on |
| 2 | 0.7049 | mem-w-tech-009 | Moving CI from CircleCI to GitHub Actions by end of Q1. Thre… |
| 3 | 0.6823 | mem-w-tech-004 | Bob owns the database migration scripts going forward |
| 4 | 0.6721 | mem-w-tech-010 | Adopting protobuf for service-to-service messages across the… |
| 5 | 0.6697 | mem-w-tech-003 | Architecture decision record from the database spike: Postgr… |

### Q22 — "What's my dental insurance policy number?"

- Top-1 score: **0.6833**  
- Results with score ≥ 0.5: 20  
- Results with score ≥ 0.7: 0  

Top-5 returned (all false positives — vault has no matching memories):

| Rank | Score | Fixture ID | Content snippet |
|---|---|---|---|
| 1 | 0.6833 | mem-p-health-009 | Dental cleanings every 6 months booked through end of 2026. … |
| 2 | 0.6085 | mem-p-finance-010 | Auto insurance renewal is in May — should shop around this c… |
| 3 | 0.5980 | mem-p-health-004 | Annual physical writeup from this morning's appointment, cap… |
| 4 | 0.5807 | mem-p-health-010 | Eye exam reminder for September with the local optometrist |
| 5 | 0.5805 | mem-p-health-001 | BP 132/85 at last visit |

---

## Per-query detail (all 22 queries)

### Q01 — catch-me-up / short-only

**Query:** "What did we decide about the engineering standup?"

**Notes:** standup-10am-change cluster (4 short memories)

**Expected memories:** 4  
**Top-20 hits / expected:** 4 / 4  
**Recall:** @5=0.50 (2/4) · @10=0.75 (3/4) · @20=1.00 (4/4)  
**Precision:** @5=0.40 · @10=0.30 · @20=0.20  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.6779 | ✓ | mem-w-standup-001 | Engineering standup moved from 9am to 10am starting next wee… |
| 2 | 0.6652 |  | mem-w-standup-011 | Retrospective writeup on why we changed the standup process … |
| 3 | 0.6473 |  | mem-w-standup-010 | Standup attendance is now optional for engineers on the IC p… |
| 4 | 0.6380 | ✓ | mem-w-standup-004 | Engineering daily standup is now at 10am instead of 9am |
| 5 | 0.6308 |  | mem-w-standup-009 | Standup is video-on per new culture norm from leadership |
| 6 | 0.6132 |  | mem-w-standup-005 | Skip standup on Friday this week — most of team is at confer… |
| 7 | 0.6077 |  | mem-w-standup-008 | Standup notes go in Notion page eng-standups-2026 from now o… |
| 8 | 0.5979 |  | mem-w-hr-006 | Friday is Mike's final day on the engineering team |
| 9 | 0.5908 |  | mem-w-standup-006 | Engineering standup format changed from free-form go-around … |
| 10 | 0.5873 | ✓ | mem-w-standup-002 | Eng standup time changed: now 10am, was 9am |

### Q02 — catch-me-up / short-only

**Query:** "What's the plan around the alpha launch?"

**Notes:** alpha-launch-nov-15 cluster (3 short memories)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8362 | ✓ | mem-w-deadline-002 | We're launching alpha on Nov 15 |
| 2 | 0.8310 | ✓ | mem-w-deadline-001 | Alpha launch target is November 15 |
| 3 | 0.7336 | ✓ | mem-w-deadline-003 | Alpha release date locked for November 15 |
| 4 | 0.7168 |  | mem-w-deadline-007 | Discussed GA launch timing in the product leadership review … |
| 5 | 0.7002 |  | mem-w-deadline-005 | Beta release date is locked at December 5 — discussed in the… |
| 6 | 0.6939 |  | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 7 | 0.6678 |  | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |
| 8 | 0.6632 |  | mem-w-deadline-008 | Reviewed GA timing in product strategy session this morning … |
| 9 | 0.6052 |  | mem-w-tech-008 | Standardize on Node 20 LTS for the platform team |
| 10 | 0.6015 |  | mem-w-tech-006 | Use feature flags via LaunchDarkly for the new auth flow |

### Q03 — catch-me-up / short-only

**Query:** "Catch me up on Mike's situation."

**Notes:** mike-last-day cluster (3 short memories)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7247 | ✓ | mem-w-hr-005 | Mike is leaving on Friday — last day on the team |
| 2 | 0.7178 | ✓ | mem-w-hr-004 | Mike's last day at the company is this Friday |
| 3 | 0.6994 | ✓ | mem-w-hr-006 | Friday is Mike's final day on the engineering team |
| 4 | 0.6030 |  | mem-p-health-004 | Annual physical writeup from this morning's appointment, cap… |
| 5 | 0.6005 |  | mem-w-hr-009 | Team headcount approved for two new hires in Q2. Both are se… |
| 6 | 0.5857 |  | mem-p-finance-007 | Reviewed the monthly statement breakdown for the household a… |
| 7 | 0.5807 |  | mem-w-standup-006 | Engineering standup format changed from free-form go-around … |
| 8 | 0.5807 |  | mem-p-family-006 | Dad's retirement party scheduled for August 16 at the lake h… |
| 9 | 0.5783 |  | mem-w-standup-010 | Standup attendance is now optional for engineers on the IC p… |
| 10 | 0.5777 |  | mem-w-hr-011 | Annual offsite scheduled for the week of June 12 |

### Q04 — catch-me-up / long-form-dominant

**Query:** "What's going on with the beta launch?"

**Notes:** beta-launch-dec-5 cluster (3 paragraph-to-long-form memories)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 0.500  
**First-hit rank:** 2

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7507 |  | mem-w-deadline-002 | We're launching alpha on Nov 15 |
| 2 | 0.7398 | ✓ | mem-w-deadline-005 | Beta release date is locked at December 5 — discussed in the… |
| 3 | 0.7194 | ✓ | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 4 | 0.7180 |  | mem-w-deadline-001 | Alpha launch target is November 15 |
| 5 | 0.7097 | ✓ | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |
| 6 | 0.6919 |  | mem-w-deadline-003 | Alpha release date locked for November 15 |
| 7 | 0.6847 |  | mem-w-deadline-007 | Discussed GA launch timing in the product leadership review … |
| 8 | 0.6533 |  | mem-w-deadline-008 | Reviewed GA timing in product strategy session this morning … |
| 9 | 0.6051 |  | mem-w-deadline-009 | Code freeze starts October 30 |
| 10 | 0.5988 |  | mem-w-tech-006 | Use feature flags via LaunchDarkly for the new auth flow |

### Q05 — catch-me-up / short-only

**Query:** "Where are we with Sarah's promotion?"

**Notes:** sarah-promotion cluster (3 short memories)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7726 | ✓ | mem-w-hr-003 | Sarah is now a senior engineer per the latest promotion cycl… |
| 2 | 0.7679 | ✓ | mem-w-hr-002 | Sarah's promotion to senior eng announced this cycle |
| 3 | 0.7040 | ✓ | mem-w-hr-001 | Sarah promoted to senior engineer effective Monday |
| 4 | 0.5808 |  | mem-w-deadline-005 | Beta release date is locked at December 5 — discussed in the… |
| 5 | 0.5744 |  | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 6 | 0.5700 |  | mem-w-hr-009 | Team headcount approved for two new hires in Q2. Both are se… |
| 7 | 0.5695 |  | mem-w-standup-010 | Standup attendance is now optional for engineers on the IC p… |
| 8 | 0.5591 |  | mem-w-deadline-002 | We're launching alpha on Nov 15 |
| 9 | 0.5544 |  | mem-w-hr-011 | Annual offsite scheduled for the week of June 12 |
| 10 | 0.5540 |  | mem-w-standup-003 | Standup pushed to 10am from 9am |

### Q06 — specific-fact / short-only

**Query:** "When is rent due?"

**Notes:** rent-due-1st cluster (3 short memories)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8641 | ✓ | mem-p-finance-001 | Rent is due on the 1st of every month |
| 2 | 0.8043 | ✓ | mem-p-finance-003 | Rent due date is always the 1st of the month |
| 3 | 0.7713 | ✓ | mem-p-finance-002 | Pay rent by the 1st of each month |
| 4 | 0.6704 |  | mem-p-finance-009 | Annual HOA fee is 850 dollars, due in February. Covers share… |
| 5 | 0.6422 |  | mem-w-deadline-009 | Code freeze starts October 30 |
| 6 | 0.5976 |  | mem-w-hr-011 | Annual offsite scheduled for the week of June 12 |
| 7 | 0.5949 |  | mem-w-deadline-011 | RFC for billing-v2 review deadline is October 21. Doc covers… |
| 8 | 0.5906 |  | mem-w-standup-001 | Engineering standup moved from 9am to 10am starting next wee… |
| 9 | 0.5881 |  | mem-p-family-010 | Sister's new address: 412 Maple Ave, Madison WI, effective t… |
| 10 | 0.5838 |  | mem-w-standup-002 | Eng standup time changed: now 10am, was 9am |

### Q07 — specific-fact / short-only

**Query:** "When is mom's birthday?"

**Notes:** mom-birthday-march-12 cluster (3 short memories)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8658 | ✓ | mem-p-family-001 | Mom's birthday is March 12 |
| 2 | 0.8218 | ✓ | mem-p-family-002 | Mom's birthday: March 12 every year |
| 3 | 0.7720 | ✓ | mem-p-family-003 | Mom turns another year older on March 12 |
| 4 | 0.5850 |  | mem-p-family-004 | Our wedding anniversary is October 20 |
| 5 | 0.5755 |  | mem-p-family-006 | Dad's retirement party scheduled for August 16 at the lake h… |
| 6 | 0.5655 |  | mem-p-family-005 | Wedding anniversary: October 20 each year |
| 7 | 0.5553 |  | mem-w-hr-004 | Mike's last day at the company is this Friday |
| 8 | 0.5394 |  | mem-p-family-007 | Niece graduates high school in June this year |
| 9 | 0.5382 |  | mem-w-hr-002 | Sarah's promotion to senior eng announced this cycle |
| 10 | 0.5364 |  | mem-w-hr-011 | Annual offsite scheduled for the week of June 12 |

### Q08 — specific-fact / short-only

**Query:** "How much is the Netflix subscription?"

**Notes:** netflix-monthly cluster (2 short memories)

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8987 | ✓ | mem-p-finance-004 | Netflix subscription charges 15.99 dollars monthly |
| 2 | 0.8225 | ✓ | mem-p-finance-005 | Netflix monthly fee: 15.99 dollars on the credit card |
| 3 | 0.7338 |  | mem-p-finance-006 | Annual budget review yesterday — captured the household mont… |
| 4 | 0.6973 |  | mem-p-finance-007 | Reviewed the monthly statement breakdown for the household a… |
| 5 | 0.5719 |  | mem-p-finance-002 | Pay rent by the 1st of each month |
| 6 | 0.5653 |  | mem-p-finance-012 | Emergency fund target: 6 months of household expenses, which… |
| 7 | 0.5447 |  | mem-p-finance-009 | Annual HOA fee is 850 dollars, due in February. Covers share… |
| 8 | 0.5297 |  | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 9 | 0.5203 |  | mem-p-hobby-003 | Reflection on the Spanish learning project at the 4-month ma… |
| 10 | 0.5145 |  | mem-p-finance-001 | Rent is due on the 1st of every month |

### Q09 — specific-fact / short-only

**Query:** "When is our wedding anniversary?"

**Notes:** anniversary-oct-20 cluster (2 short memories)

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8398 | ✓ | mem-p-family-004 | Our wedding anniversary is October 20 |
| 2 | 0.8035 | ✓ | mem-p-family-005 | Wedding anniversary: October 20 each year |
| 3 | 0.6208 |  | mem-p-family-008 | Cousin's wedding in Boston the second weekend of September. … |
| 4 | 0.6155 |  | mem-p-family-002 | Mom's birthday: March 12 every year |
| 5 | 0.6103 |  | mem-p-health-007 | Annual physical scheduled for April 18 |
| 6 | 0.6038 |  | mem-p-family-003 | Mom turns another year older on March 12 |
| 7 | 0.5862 |  | mem-p-family-001 | Mom's birthday is March 12 |
| 8 | 0.5858 |  | mem-p-family-006 | Dad's retirement party scheduled for August 16 at the lake h… |
| 9 | 0.5780 |  | mem-p-family-009 | Family reunion every two years — next one is 2027 in Colorad… |
| 10 | 0.5713 |  | mem-w-hr-011 | Annual offsite scheduled for the week of June 12 |

### Q10 — specific-fact / short-only

**Query:** "What's the daily vitamin I take?"

**Notes:** vitamin-d-daily cluster (2 short memories)

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8104 | ✓ | mem-p-health-006 | Daily vitamin D supplement in the morning routine |
| 2 | 0.8070 | ✓ | mem-p-health-005 | Take vitamin D supplement every morning |
| 3 | 0.6876 |  | mem-p-health-004 | Annual physical writeup from this morning's appointment, cap… |
| 4 | 0.6479 |  | mem-p-health-011 | Iron levels came back low (65, should be 80-plus) at the las… |
| 5 | 0.6366 |  | mem-p-health-003 | Quick health summary from the past three months written for … |
| 6 | 0.6157 |  | mem-p-health-002 | Doctor's visit yesterday went well overall — BP reading was … |
| 7 | 0.5932 |  | mem-p-health-008 | Nutritionist consult yesterday — switching to whole grain br… |
| 8 | 0.5671 |  | mem-p-health-001 | BP 132/85 at last visit |
| 9 | 0.5379 |  | mem-p-health-013 | Sleep target: 7 hours minimum on weeknights |
| 10 | 0.5298 |  | mem-p-hobby-005 | Weekly 5k run on Saturdays as the exercise routine |

### Q11 — decision-history / long-form-dominant

**Query:** "When did we decide GA launch?"

**Notes:** ga-launch-quarter CONTRADICTION PAIR (Q1 2027 vs Q2 2027, both long-form). Load-bearing for read-time re-rank architecture viability.

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7569 | ✓ | mem-w-deadline-007 | Discussed GA launch timing in the product leadership review … |
| 2 | 0.7299 | ✓ | mem-w-deadline-008 | Reviewed GA timing in product strategy session this morning … |
| 3 | 0.7054 |  | mem-w-deadline-002 | We're launching alpha on Nov 15 |
| 4 | 0.6937 |  | mem-w-deadline-001 | Alpha launch target is November 15 |
| 5 | 0.6455 |  | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 6 | 0.6330 |  | mem-w-deadline-005 | Beta release date is locked at December 5 — discussed in the… |
| 7 | 0.6201 |  | mem-w-deadline-003 | Alpha release date locked for November 15 |
| 8 | 0.5806 |  | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |
| 9 | 0.5720 |  | mem-w-tech-006 | Use feature flags via LaunchDarkly for the new auth flow |
| 10 | 0.5686 |  | mem-w-deadline-009 | Code freeze starts October 30 |

### Q12 — decision-history / mixed-length

**Query:** "What did we pick for the primary data store?"

**Notes:** use-postgres cluster (short + paragraph + long-form same fact)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8620 | ✓ | mem-w-tech-001 | Decided on Postgres for the primary data store |
| 2 | 0.7524 | ✓ | mem-w-tech-003 | Architecture decision record from the database spike: Postgr… |
| 3 | 0.7427 | ✓ | mem-w-tech-002 | Selected Postgres as the primary data store for the platform… |
| 4 | 0.6598 |  | mem-w-tech-005 | Database migrations are Bob's responsibility from now on |
| 5 | 0.6490 |  | mem-w-tech-004 | Bob owns the database migration scripts going forward |
| 6 | 0.6179 |  | mem-w-tech-012 | Cache layer uses Redis 7 cluster mode in production. Decisio… |
| 7 | 0.6117 |  | mem-w-tech-011 | Architecture decision log for the new authentication service… |
| 8 | 0.5915 |  | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |
| 9 | 0.5814 |  | mem-w-tech-009 | Moving CI from CircleCI to GitHub Actions by end of Q1. Thre… |
| 10 | 0.5804 |  | mem-w-tech-010 | Adopting protobuf for service-to-service messages across the… |

### Q13 — decision-history / long-form-dominant

**Query:** "What's the Comcast bill?"

**Notes:** comcast-monthly-bill CONTRADICTION PAIR ($89 vs $109, both long-form). Second product-strategic gate.

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7286 | ✓ | mem-p-finance-006 | Annual budget review yesterday — captured the household mont… |
| 2 | 0.7178 | ✓ | mem-p-finance-007 | Reviewed the monthly statement breakdown for the household a… |
| 3 | 0.6309 |  | mem-p-finance-005 | Netflix monthly fee: 15.99 dollars on the credit card |
| 4 | 0.6231 |  | mem-p-finance-004 | Netflix subscription charges 15.99 dollars monthly |
| 5 | 0.6137 |  | mem-p-finance-001 | Rent is due on the 1st of every month |
| 6 | 0.6032 |  | mem-p-finance-002 | Pay rent by the 1st of each month |
| 7 | 0.5926 |  | mem-p-finance-009 | Annual HOA fee is 850 dollars, due in February. Covers share… |
| 8 | 0.5825 |  | mem-w-deadline-011 | RFC for billing-v2 review deadline is October 21. Doc covers… |
| 9 | 0.5606 |  | mem-p-finance-010 | Auto insurance renewal is in May — should shop around this c… |
| 10 | 0.5603 |  | mem-p-finance-003 | Rent due date is always the 1st of the month |

### Q14 — decision-history / short-only

**Query:** "Who owns the database migrations?"

**Notes:** bob-migrations-owner cluster (control case: short-only decision-history)

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8487 | ✓ | mem-w-tech-004 | Bob owns the database migration scripts going forward |
| 2 | 0.8288 | ✓ | mem-w-tech-005 | Database migrations are Bob's responsibility from now on |
| 3 | 0.6653 |  | mem-w-tech-001 | Decided on Postgres for the primary data store |
| 4 | 0.6359 |  | mem-w-tech-003 | Architecture decision record from the database spike: Postgr… |
| 5 | 0.6283 |  | mem-w-tech-009 | Moving CI from CircleCI to GitHub Actions by end of Q1. Thre… |
| 6 | 0.5991 |  | mem-w-tech-011 | Architecture decision log for the new authentication service… |
| 7 | 0.5845 |  | mem-w-tech-010 | Adopting protobuf for service-to-service messages across the… |
| 8 | 0.5825 |  | mem-w-tech-002 | Selected Postgres as the primary data store for the platform… |
| 9 | 0.5823 |  | mem-w-hr-009 | Team headcount approved for two new hires in Q2. Both are se… |
| 10 | 0.5743 |  | mem-w-tech-008 | Standardize on Node 20 LTS for the platform team |

### Q15 — decision-history / mixed-length

**Query:** "What was my latest blood pressure reading?"

**Notes:** bp-reading-132-85 cluster (short + paragraph + paragraph + long-form)

**Expected memories:** 4  
**Top-20 hits / expected:** 4 / 4  
**Recall:** @5=1.00 (4/4) · @10=1.00 (4/4) · @20=1.00 (4/4)  
**Precision:** @5=0.80 · @10=0.40 · @20=0.20  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7567 | ✓ | mem-p-health-002 | Doctor's visit yesterday went well overall — BP reading was … |
| 2 | 0.7304 | ✓ | mem-p-health-001 | BP 132/85 at last visit |
| 3 | 0.7260 | ✓ | mem-p-health-003 | Quick health summary from the past three months written for … |
| 4 | 0.7174 | ✓ | mem-p-health-004 | Annual physical writeup from this morning's appointment, cap… |
| 5 | 0.6039 |  | mem-p-hobby-006 | Reading goal: 24 books this year, 2 per month average. Track… |
| 6 | 0.6018 |  | mem-p-health-011 | Iron levels came back low (65, should be 80-plus) at the las… |
| 7 | 0.5861 |  | mem-w-hr-008 | Compensation cycle outcomes communicated next Tuesday |
| 8 | 0.5803 |  | mem-w-hr-007 | Quarterly performance reviews are due by end of next month. … |
| 9 | 0.5778 |  | mem-p-finance-010 | Auto insurance renewal is in May — should shop around this c… |
| 10 | 0.5731 |  | mem-w-standup-006 | Engineering standup format changed from free-form go-around … |

### Q16 — topic-synthesis / mixed-length

**Query:** "What's my Spanish learning plan?"

**Notes:** learn-spanish cluster (short + paragraph + long-form). Same factual content at varying lengths.

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8257 | ✓ | mem-p-hobby-001 | Want to learn Spanish this year |
| 2 | 0.7155 | ✓ | mem-p-hobby-002 | Spanish learning is on the plan for this year — committing t… |
| 3 | 0.6932 | ✓ | mem-p-hobby-003 | Reflection on the Spanish learning project at the 4-month ma… |
| 4 | 0.5817 |  | mem-p-hobby-012 | Practicing guitar 20 minutes a day on weeknights — small dai… |
| 5 | 0.5701 |  | mem-w-standup-008 | Standup notes go in Notion page eng-standups-2026 from now o… |
| 6 | 0.5199 |  | mem-p-health-012 | Yoga twice a week is the new exercise target for spring. Sch… |
| 7 | 0.5085 |  | mem-p-hobby-013 | Photography session journal from the Saturday at the botanic… |
| 8 | 0.5073 |  | mem-p-hobby-006 | Reading goal: 24 books this year, 2 per month average. Track… |
| 9 | 0.4948 |  | mem-p-hobby-005 | Weekly 5k run on Saturdays as the exercise routine |
| 10 | 0.4939 |  | mem-w-deadline-011 | RFC for billing-v2 review deadline is October 21. Doc covers… |

### Q17 — topic-synthesis / multi-cluster

**Query:** "What's my exercise routine looking like?"

**Notes:** weekly-5k-run cluster + yoga twice-weekly (cross-cluster topic synthesis)

**Expected memories:** 3  
**Top-20 hits / expected:** 3 / 3  
**Recall:** @5=1.00 (3/3) · @10=1.00 (3/3) · @20=1.00 (3/3)  
**Precision:** @5=0.60 · @10=0.30 · @20=0.15  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7190 | ✓ | mem-p-hobby-005 | Weekly 5k run on Saturdays as the exercise routine |
| 2 | 0.6867 |  | mem-p-health-004 | Annual physical writeup from this morning's appointment, cap… |
| 3 | 0.6841 | ✓ | mem-p-hobby-004 | Running a 5k every weekend as the new exercise habit |
| 4 | 0.6745 |  | mem-p-health-003 | Quick health summary from the past three months written for … |
| 5 | 0.6449 | ✓ | mem-p-health-012 | Yoga twice a week is the new exercise target for spring. Sch… |
| 6 | 0.6190 |  | mem-p-health-002 | Doctor's visit yesterday went well overall — BP reading was … |
| 7 | 0.5929 |  | mem-p-health-007 | Annual physical scheduled for April 18 |
| 8 | 0.5884 |  | mem-p-hobby-003 | Reflection on the Spanish learning project at the 4-month ma… |
| 9 | 0.5846 |  | mem-p-health-006 | Daily vitamin D supplement in the morning routine |
| 10 | 0.5754 |  | mem-w-standup-006 | Engineering standup format changed from free-form go-around … |

### Q18 — topic-synthesis / multi-cluster

**Query:** "What's my health status overall?"

**Notes:** BP cluster (4 memories, mixed-length) + vitamin-d cluster (2 short) + iron levels (paragraph). Expanded per Shahbaz redline 2026-05-14.

**Expected memories:** 7  
**Top-20 hits / expected:** 7 / 7  
**Recall:** @5=0.71 (5/7) · @10=1.00 (7/7) · @20=1.00 (7/7)  
**Precision:** @5=1.00 · @10=0.70 · @20=0.35  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7270 | ✓ | mem-p-health-004 | Annual physical writeup from this morning's appointment, cap… |
| 2 | 0.6921 | ✓ | mem-p-health-003 | Quick health summary from the past three months written for … |
| 3 | 0.6661 | ✓ | mem-p-health-002 | Doctor's visit yesterday went well overall — BP reading was … |
| 4 | 0.6424 | ✓ | mem-p-health-001 | BP 132/85 at last visit |
| 5 | 0.5846 | ✓ | mem-p-health-006 | Daily vitamin D supplement in the morning routine |
| 6 | 0.5595 | ✓ | mem-p-health-005 | Take vitamin D supplement every morning |
| 7 | 0.5566 |  | mem-p-health-008 | Nutritionist consult yesterday — switching to whole grain br… |
| 8 | 0.5493 | ✓ | mem-p-health-011 | Iron levels came back low (65, should be 80-plus) at the las… |
| 9 | 0.5457 |  | mem-p-health-007 | Annual physical scheduled for April 18 |
| 10 | 0.5377 |  | mem-p-health-010 | Eye exam reminder for September with the local optometrist |

### Q19 — topic-synthesis / multi-cluster

**Query:** "Where are we with the launch timeline?"

**Notes:** Spans 3 clusters across 8 memories: alpha-launch-nov-15 (short) + beta-launch-dec-5 (paragraph) + ga-launch-quarter contradiction (long-form). Most ambitious recall query.

**Expected memories:** 8  
**Top-20 hits / expected:** 8 / 8  
**Recall:** @5=0.62 (5/8) · @10=1.00 (8/8) · @20=1.00 (8/8)  
**Precision:** @5=1.00 · @10=0.80 · @20=0.40  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7880 | ✓ | mem-w-deadline-002 | We're launching alpha on Nov 15 |
| 2 | 0.7747 | ✓ | mem-w-deadline-001 | Alpha launch target is November 15 |
| 3 | 0.7408 | ✓ | mem-w-deadline-007 | Discussed GA launch timing in the product leadership review … |
| 4 | 0.7317 | ✓ | mem-w-deadline-005 | Beta release date is locked at December 5 — discussed in the… |
| 5 | 0.7042 | ✓ | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 6 | 0.6875 | ✓ | mem-w-deadline-008 | Reviewed GA timing in product strategy session this morning … |
| 7 | 0.6708 | ✓ | mem-w-deadline-003 | Alpha release date locked for November 15 |
| 8 | 0.6332 | ✓ | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |
| 9 | 0.6231 |  | mem-w-tech-006 | Use feature flags via LaunchDarkly for the new auth flow |
| 10 | 0.6190 |  | mem-w-hr-009 | Team headcount approved for two new hires in Q2. Both are se… |

### Q20 — topic-synthesis / multi-cluster

**Query:** "What are my recurring monthly bills?"

**Notes:** Rent cluster (3 short) + Netflix cluster (2 short) + Comcast contradiction (2 long-form). Tightened per Shahbaz redline 2026-05-14 (was 'subscription bill breakdown' with 4 IDs; now broader cross-cluster synthesis test).

**Expected memories:** 7  
**Top-20 hits / expected:** 7 / 7  
**Recall:** @5=0.71 (5/7) · @10=1.00 (7/7) · @20=1.00 (7/7)  
**Precision:** @5=1.00 · @10=0.70 · @20=0.35  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7580 | ✓ | mem-p-finance-002 | Pay rent by the 1st of each month |
| 2 | 0.7288 | ✓ | mem-p-finance-001 | Rent is due on the 1st of every month |
| 3 | 0.6977 | ✓ | mem-p-finance-006 | Annual budget review yesterday — captured the household mont… |
| 4 | 0.6931 | ✓ | mem-p-finance-003 | Rent due date is always the 1st of the month |
| 5 | 0.6652 | ✓ | mem-p-finance-007 | Reviewed the monthly statement breakdown for the household a… |
| 6 | 0.6612 |  | mem-p-finance-010 | Auto insurance renewal is in May — should shop around this c… |
| 7 | 0.6609 |  | mem-p-finance-012 | Emergency fund target: 6 months of household expenses, which… |
| 8 | 0.6400 | ✓ | mem-p-finance-005 | Netflix monthly fee: 15.99 dollars on the credit card |
| 9 | 0.6321 | ✓ | mem-p-finance-004 | Netflix subscription charges 15.99 dollars monthly |
| 10 | 0.6292 |  | mem-p-finance-009 | Annual HOA fee is 850 dollars, due in February. Covers share… |

### Q21 — hard-negative / hard-negative

**Query:** "What did we decide about the Kubernetes migration?"

**Notes:** Vault has zero Kubernetes memories. Precision test: do retrieval scores correctly stay low (relative scoring) or does BGE return top-K of unrelated memories with high scores (absolute scoring)?

**Expected memories:** 0  
**Top-20 hits / expected:** 0 / 0  
**Recall:** @5=— (—) · @10=— (—) · @20=— (—)  
**Precision:** @5=0.00 · @10=0.00 · @20=0.00  
**MRR:** 0.000  
**First-hit rank:** — (no expected hit in top-20)

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7169 |  | mem-w-tech-005 | Database migrations are Bob's responsibility from now on |
| 2 | 0.7049 |  | mem-w-tech-009 | Moving CI from CircleCI to GitHub Actions by end of Q1. Thre… |
| 3 | 0.6823 |  | mem-w-tech-004 | Bob owns the database migration scripts going forward |
| 4 | 0.6721 |  | mem-w-tech-010 | Adopting protobuf for service-to-service messages across the… |
| 5 | 0.6697 |  | mem-w-tech-003 | Architecture decision record from the database spike: Postgr… |
| 6 | 0.6650 |  | mem-w-tech-001 | Decided on Postgres for the primary data store |
| 7 | 0.6631 |  | mem-w-tech-011 | Architecture decision log for the new authentication service… |
| 8 | 0.6569 |  | mem-w-tech-012 | Cache layer uses Redis 7 cluster mode in production. Decisio… |
| 9 | 0.6480 |  | mem-w-tech-008 | Standardize on Node 20 LTS for the platform team |
| 10 | 0.6382 |  | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |

### Q22 — hard-negative / hard-negative

**Query:** "What's my dental insurance policy number?"

**Notes:** Vault has one dental-cleaning memory (mem-p-health-009) but no insurance policy info. Precision test: does BGE return health-009 as a high-score false positive?

**Expected memories:** 0  
**Top-20 hits / expected:** 0 / 0  
**Recall:** @5=— (—) · @10=— (—) · @20=— (—)  
**Precision:** @5=0.00 · @10=0.00 · @20=0.00  
**MRR:** 0.000  
**First-hit rank:** — (no expected hit in top-20)

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.6833 |  | mem-p-health-009 | Dental cleanings every 6 months booked through end of 2026. … |
| 2 | 0.6085 |  | mem-p-finance-010 | Auto insurance renewal is in May — should shop around this c… |
| 3 | 0.5980 |  | mem-p-health-004 | Annual physical writeup from this morning's appointment, cap… |
| 4 | 0.5807 |  | mem-p-health-010 | Eye exam reminder for September with the local optometrist |
| 5 | 0.5805 |  | mem-p-health-001 | BP 132/85 at last visit |
| 6 | 0.5764 |  | mem-w-standup-008 | Standup notes go in Notion page eng-standups-2026 from now o… |
| 7 | 0.5568 |  | mem-p-health-002 | Doctor's visit yesterday went well overall — BP reading was … |
| 8 | 0.5501 |  | mem-w-standup-006 | Engineering standup format changed from free-form go-around … |
| 9 | 0.5491 |  | mem-w-hr-008 | Compensation cycle outcomes communicated next Tuesday |
| 10 | 0.5432 |  | mem-w-hr-009 | Team headcount approved for two new hires in Q2. Both are se… |

### Q23 — verbose-agent-reformulation / long-form-dominant

**Query:** "Picking up the launch timeline work — what's the latest on GA timing? Were there any schedule revisions I should know about?"

**Notes:** Verbose reformulation of Q11 (GA launch contradiction pair). Same expected_memory_ids as Q11. Tests whether the contradiction-surfacing verdict survives when an agent catches up with multi-clause phrasing rather than a lexically-direct single-question. The GA noun is still present, but wrapped in catch-up context.

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.8374 | ✓ | mem-w-deadline-007 | Discussed GA launch timing in the product leadership review … |
| 2 | 0.8117 | ✓ | mem-w-deadline-008 | Reviewed GA timing in product strategy session this morning … |
| 3 | 0.7481 |  | mem-w-deadline-005 | Beta release date is locked at December 5 — discussed in the… |
| 4 | 0.7234 |  | mem-w-deadline-001 | Alpha launch target is November 15 |
| 5 | 0.7182 |  | mem-w-deadline-002 | We're launching alpha on Nov 15 |
| 6 | 0.7023 |  | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |
| 7 | 0.6964 |  | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 8 | 0.6854 |  | mem-w-standup-010 | Standup attendance is now optional for engineers on the IC p… |
| 9 | 0.6803 |  | mem-w-standup-011 | Retrospective writeup on why we changed the standup process … |
| 10 | 0.6799 |  | mem-w-deadline-003 | Alpha release date locked for November 15 |

### Q24 — verbose-agent-reformulation / long-form-dominant

**Query:** "Going through the recurring bills — what's the current Comcast charge, and has it changed recently?"

**Notes:** Verbose reformulation of Q13 (Comcast bill contradiction pair). Same expected_memory_ids as Q13. The 'Comcast' noun is still present, wrapped in multi-clause budget-review phrasing.

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7780 | ✓ | mem-p-finance-007 | Reviewed the monthly statement breakdown for the household a… |
| 2 | 0.7647 | ✓ | mem-p-finance-006 | Annual budget review yesterday — captured the household mont… |
| 3 | 0.6781 |  | mem-p-finance-002 | Pay rent by the 1st of each month |
| 4 | 0.6780 |  | mem-p-finance-005 | Netflix monthly fee: 15.99 dollars on the credit card |
| 5 | 0.6771 |  | mem-p-finance-004 | Netflix subscription charges 15.99 dollars monthly |
| 6 | 0.6717 |  | mem-p-finance-001 | Rent is due on the 1st of every month |
| 7 | 0.6647 |  | mem-p-finance-010 | Auto insurance renewal is in May — should shop around this c… |
| 8 | 0.6646 |  | mem-w-deadline-011 | RFC for billing-v2 review deadline is October 21. Doc covers… |
| 9 | 0.6510 |  | mem-p-finance-009 | Annual HOA fee is 850 dollars, due in February. Covers share… |
| 10 | 0.6341 |  | mem-p-finance-012 | Emergency fund target: 6 months of household expenses, which… |

### Q25 — oblique-agent-reformulation / long-form-dominant

**Query:** "Help me update the product roadmap doc with the latest milestone dates."

**Notes:** Oblique reformulation of Q11. NO exact nouns from the GA contradiction memories ('GA', 'launch', 'Q1', 'Q2' all absent). Tests whether semantic generalization ('milestone dates', 'roadmap') still surfaces the contradiction pair. Expected set narrow on purpose — measures whether the contradiction-surfacing verdict survives oblique phrasing.

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=0.50 (1/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.20 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.7011 | ✓ | mem-w-deadline-007 | Discussed GA launch timing in the product leadership review … |
| 2 | 0.6988 |  | mem-w-deadline-004 | Beta launch scheduled for December 5 per the product team's … |
| 3 | 0.6935 |  | mem-w-deadline-005 | Beta release date is locked at December 5 — discussed in the… |
| 4 | 0.6787 |  | mem-w-deadline-002 | We're launching alpha on Nov 15 |
| 5 | 0.6783 |  | mem-w-deadline-006 | Confirmed beta opens December 5 in engineering all-hands tod… |
| 6 | 0.6759 | ✓ | mem-w-deadline-008 | Reviewed GA timing in product strategy session this morning … |
| 7 | 0.6704 |  | mem-w-deadline-001 | Alpha launch target is November 15 |
| 8 | 0.6668 |  | mem-w-standup-008 | Standup notes go in Notion page eng-standups-2026 from now o… |
| 9 | 0.6619 |  | mem-w-tech-008 | Standardize on Node 20 LTS for the platform team |
| 10 | 0.6590 |  | mem-w-standup-011 | Retrospective writeup on why we changed the standup process … |

### Q26 — oblique-agent-reformulation / long-form-dominant

**Query:** "Doing the monthly budget review — anything I should flag about household services costing more than expected?"

**Notes:** Oblique reformulation of Q13. NO 'Comcast' noun, no dollar amounts mentioned. Tests whether 'household services costing more than expected' semantically surfaces the Comcast contradiction pair (one paragraph says $89, the other says $109 — the price change IS what the agent is asking to flag).

**Expected memories:** 2  
**Top-20 hits / expected:** 2 / 2  
**Recall:** @5=1.00 (2/2) · @10=1.00 (2/2) · @20=1.00 (2/2)  
**Precision:** @5=0.40 · @10=0.20 · @20=0.10  
**MRR:** 1.000  
**First-hit rank:** 1

Top-10 returned:

| Rank | Score | Expected? | Fixture ID | Snippet |
|---|---|---|---|---|
| 1 | 0.6962 | ✓ | mem-p-finance-006 | Annual budget review yesterday — captured the household mont… |
| 2 | 0.6850 |  | mem-p-finance-002 | Pay rent by the 1st of each month |
| 3 | 0.6741 | ✓ | mem-p-finance-007 | Reviewed the monthly statement breakdown for the household a… |
| 4 | 0.6692 |  | mem-p-finance-012 | Emergency fund target: 6 months of household expenses, which… |
| 5 | 0.6681 |  | mem-p-finance-001 | Rent is due on the 1st of every month |
| 6 | 0.6490 |  | mem-p-finance-009 | Annual HOA fee is 850 dollars, due in February. Covers share… |
| 7 | 0.6297 |  | mem-w-deadline-011 | RFC for billing-v2 review deadline is October 21. Doc covers… |
| 8 | 0.6243 |  | mem-p-finance-010 | Auto insurance renewal is in May — should shop around this c… |
| 9 | 0.6168 |  | mem-p-finance-005 | Netflix monthly fee: 15.99 dollars on the credit card |
| 10 | 0.6109 |  | mem-p-finance-004 | Netflix subscription charges 15.99 dollars monthly |

---

## Follow-up 1 — Hard-negative score-distribution analysis

**Question.** Q21's top-1 hard-negative score (0.7169) lands inside the score band where legitimate matches live (0.71–0.77 for in-cluster hits). Does a `score_threshold` cleanly separate "the vault has this" from "the vault doesn't"? Or do the bands overlap, meaning threshold gating cannot distinguish them and read-time re-rank has to absorb that uncertainty?

### Top-1 score distribution

| Statistic | Real queries (n=20, Q01–Q20) | Hard-negatives (n=2, Q21–Q22) |
|---|---|---|
| Min | **0.6779** (Q01 standup) | 0.6833 (Q22 dental) |
| Max | 0.8987 (Q08 Netflix) | **0.7169** (Q21 Kubernetes) |
| Mean | 0.7806 | 0.7001 |
| Median | 0.7803 | 0.7001 |

### Top-3 score distribution

| Statistic | Real queries (n=60, Q01–Q20 × top-3) | Hard-negatives (n=6, Q21–Q22 × top-3) |
|---|---|---|
| Min | 0.6208 (Q09 rank-3) | 0.5980 (Q22 rank-3) |
| Max | 0.8987 (Q08 rank-1) | 0.7169 (Q21 rank-1) |
| Mean | 0.7499 | 0.6657 |

### Band-floor vs FP-ceiling — the load-bearing comparison

| Metric | Value | Source |
|---|---|---|
| Band floor (lowest legitimate top-1) | **0.6779** | Q01 — "What did we decide about the engineering standup?" → top-1 was `mem-w-standup-001` (real hit) |
| FP ceiling (highest hard-negative top-1) | **0.7169** | Q21 — "What did we decide about the Kubernetes migration?" → top-1 was `mem-w-tech-005` "Database migrations are Bob's responsibility" (false positive via lexical bridge "migration") |
| Gap | **−0.039** (FP ceiling is **above** band floor) | Bands **overlap** |

**Reading the overlap concretely.** Sort all 22 top-1 scores ascending:

```
0.6779  Q01  legit
0.6833  Q22  hard-negative   ← sits ABOVE Q01 legit
0.7169  Q21  hard-negative   ← sits ABOVE 4 legit queries (Q01, Q17, Q03, Q18, also above Q22)
0.7190  Q17  legit
0.7247  Q03  legit
0.7270  Q18  legit
0.7286  Q13  legit
0.7507  Q04  legit (note: rank-1 was a cross-cluster false positive, but score still in legit band)
0.7567  Q15  legit
0.7569  Q11  legit
...
0.8987  Q08  legit
```

No single threshold separates the two populations:
- Threshold = 0.68 → blocks Q01 (legit), passes Q22 (HN) and Q21 (HN). Wrong direction.
- Threshold = 0.72 → blocks Q01, Q22, Q21, Q17. Blocks 2 HN ✓, blocks 2 legit ✗.
- Threshold = 0.73 → blocks Q01, Q22, Q21, Q17, Q03, Q18, Q13. Blocks 2 HN, blocks 5 legit.

### Finding

**Bands overlap by 0.039 cosine.** A simple `score_threshold` on `RetrievalQuery::options` cannot distinguish "the vault has relevant info" from "the vault doesn't" — at least not with the current bge-small embedding space and this fixture's 100-memory denominator.

**The lexical-bridge mechanism.** Q21 hard-negative's top-1 (0.7169) is high because "Kubernetes migration" semantically bridges to "database migration" via the word "migration." The embedding correctly identifies the only memory in the vault that talks about *migrations* — it just happens to be the wrong KIND of migration. That's a precision failure, not a relevance failure. Whatever filters the hard-negative case has to be smart enough to read the candidate's content and rule "this is database migration, not Kubernetes migration" — which is content-aware semantic comparison, not absolute score thresholding.

**Two implications for the architecture decision (deferred to partner conversation).**

1. The "I don't have this" mechanism — if needed at all in V0.2 — would have to live in the re-rank stage with Phi-4 reading candidate content, not in a `RetrievalQuery::options.score_threshold` setting at the retriever layer.
2. Without read-time re-rank, agents would always see 20 plausibly-scored results regardless of whether the vault actually has the requested information. Agent UX has to assume the top-K may include thematic false positives.

---

## Follow-up 2 — Verbose-agent and oblique-agent reformulations (Q23–Q26)

**Question.** Q11 ("When did we decide GA launch?") and Q13 ("What's the Comcast bill?") are lexically clean — they hit the exact nouns. Real agents catching up on work don't always phrase queries that cleanly. Does the contradiction-surfacing verdict (Q11 + Q13 PASS at ranks 1+2) survive when an agent uses multi-clause verbose phrasing, or oblique phrasing that omits the disputed nouns entirely?

### Q23 — verbose reformulation of Q11

**Query:** "Picking up the launch timeline work — what's the latest on GA timing? Were there any schedule revisions I should know about?"

| Metric | Q11 (lexical-direct) | Q23 (verbose-agent) | Δ |
|---|---|---|---|
| recall@5 | 1.00 (2/2) | 1.00 (2/2) | — |
| recall@10 | 1.00 | 1.00 | — |
| recall@20 | 1.00 | 1.00 | — |
| Pair ranks | 1 + 2 | 1 + 2 | adjacent |
| Pair scores | 0.7569 + 0.7299 | **0.8374 + 0.8117** | **+0.08 each** |
| Verdict | PASS | **PASS** | maintained |

**Notable:** Q23's scores on the contradiction pair are *higher* than Q11's. The verbose catch-up phrasing semantically aligns better with the verbose contradiction paragraphs (which themselves use catch-up language like "schedule revisions", "moving GA to Q2 2027 based on the latest beta-readiness assessment", "stakeholders re-aligned"). Adding context to the query helped, did not hurt.

### Q24 — verbose reformulation of Q13

**Query:** "Going through the recurring bills — what's the current Comcast charge, and has it changed recently?"

| Metric | Q13 (lexical-direct) | Q24 (verbose-agent) | Δ |
|---|---|---|---|
| recall@5 | 1.00 (2/2) | 1.00 (2/2) | — |
| recall@10 | 1.00 | 1.00 | — |
| recall@20 | 1.00 | 1.00 | — |
| Pair ranks | 1 + 2 | 1 + 2 | adjacent |
| Pair scores | 0.7286 + 0.7178 | **0.7780 + 0.7647** | **+0.05 each** |
| Verdict | PASS | **PASS** | maintained |

Same pattern as Q23 — verbose phrasing boosted the legitimate-match scores.

### Q25 — oblique reformulation of Q11 (no exact nouns)

**Query:** "Help me update the product roadmap doc with the latest milestone dates."

**Critical:** the query contains NONE of the disputed nouns from the GA contradiction (no "GA", "launch", "Q1", "Q2", "2027"). It relies on semantic generalization: "milestone dates" + "product roadmap" should surface launch-date memories.

| Metric | Q11 (lexical-direct) | Q25 (oblique-agent) | Δ |
|---|---|---|---|
| recall@5 | 1.00 (2/2) | **0.50 (1/2)** | −0.50 |
| recall@10 | 1.00 | 1.00 (2/2) | — |
| recall@20 | 1.00 | 1.00 | — |
| Pair ranks | 1 + 2 (adjacent) | **1 + 6** (split by 4 ranks of beta-launch noise) | not-adjacent |
| Pair scores | 0.7569 + 0.7299 | 0.7011 + 0.6759 | −0.05 / −0.05 |
| Verdict | PASS | **PASS at K=10, PARTIAL at K=5** | degraded |

**Reading the split.** Ranks 2–5 in Q25 are beta-launch and alpha-launch memories ("Beta launch scheduled for December 5", "We're launching alpha on Nov 15") — also legitimate milestone-date content per the query intent, just not the *contradiction* pair. The second half of the GA contradiction (`mem-w-deadline-008`) lands at rank 6, score 0.6759.

**What this means for read-time re-rank.** An agent looking only at top-5 from Q25 would see ONE half of the contradiction (Q1 2027 GA) but not the Q2 2027 revision. Phi-4 read-time inspection would need to look at top-10 (or wider) to catch both halves.

### Q26 — oblique reformulation of Q13 (no exact nouns)

**Query:** "Doing the monthly budget review — anything I should flag about household services costing more than expected?"

**Critical:** the query contains no "Comcast", no dollar amounts. Relies on "household services costing more than expected" → the Comcast price change.

| Metric | Q13 (lexical-direct) | Q26 (oblique-agent) | Δ |
|---|---|---|---|
| recall@5 | 1.00 (2/2) | 1.00 (2/2) | — |
| recall@10 | 1.00 | 1.00 | — |
| recall@20 | 1.00 | 1.00 | — |
| Pair ranks | 1 + 2 (adjacent) | **1 + 3** (one rent memory at rank 2) | nearly-adjacent |
| Pair scores | 0.7286 + 0.7178 | 0.6962 + 0.6741 | −0.03 / −0.04 |
| Verdict | PASS | **PASS** | maintained at K=5 |

**Reading the near-adjacent surfacing.** Rank 2 is `mem-p-finance-002` ("Pay rent by the 1st of each month") — a thematically-related but not-contradiction-relevant interloper. Both halves of the Comcast contradiction are still in top-5 (ranks 1 and 3), so an agent inspecting top-5 sees both. Cleaner outcome than Q25.

### Aggregate verdict on reformulations

| Verdict at K | verbose-agent (Q23+Q24, n=2) | oblique-agent (Q25+Q26, n=2) |
|---|---|---|
| Both pair members in top-5 | **2/2 PASS** | **1/2 PASS** (Q26 yes, Q25 only one half) |
| Both pair members in top-10 | **2/2 PASS** | **2/2 PASS** |
| Both pair members in top-20 | **2/2 PASS** | **2/2 PASS** |
| Pair adjacent at ranks 1+2 | **2/2 yes** | **0/2** (Q25 split 1+6, Q26 split 1+3) |

### Score-band comparison across phrasings

Top-1 scores for the contradiction pair across all four phrasings of each pair:

| Phrasing | Q11/Q23/Q25 (GA launch) | Q13/Q24/Q26 (Comcast bill) |
|---|---|---|
| Lexical-direct | 0.7569 / 0.7299 | 0.7286 / 0.7178 |
| Verbose-agent | 0.8374 / 0.8117 | 0.7780 / 0.7647 |
| Oblique-agent | 0.7011 / 0.6759 | 0.6962 / 0.6741 |

The verbose-agent phrasing produces the *highest* scores; oblique produces the lowest. All three phrasings still surface both pair members within top-10.

### Findings

1. **Contradiction-surfacing survives verbose phrasing without degradation.** Q23, Q24 maintain PASS at K=5 with both pair members at ranks 1+2 adjacent. Score band actually *improves* over the lexical-direct originals.
2. **Contradiction-surfacing survives oblique phrasing at K=10, with caveats.** Q25 + Q26 both PASS at K=20 and K=10, but Q25 splits the pair across 5 ranks of thematic noise (the second half at rank 6). An agent that only looks at top-5 of an oblique query loses one half of the contradiction.
3. **The oblique-Q25 score band overlaps with the hard-negative band.** Q25's contradiction-pair scores (0.7011 + 0.6759) sit between Q21's hard-negative top-1 (0.7169) and Q22's hard-negative top-1 (0.6833). Score-threshold gating from Follow-up 1 would also fail here — the legitimate but oblique-phrased match scores below a hard-negative false positive. Confirms F1's negative finding.

---

## Architectural decision — DEFERRED

This document is data, not decision. The architectural call (scenario A read-time re-rank / scenario B both-surfaces / scenario C embedding-layer overhaul) happens in a separate conversation with Shahbaz once these numbers are read. When that decision is made, the relevant ADR (e.g. ADR-048) will land alongside the production code that implements it, and this spike's artefacts will ride with that commit per `feedback_spike_examples_bundle_with_consumer_code.md`.
