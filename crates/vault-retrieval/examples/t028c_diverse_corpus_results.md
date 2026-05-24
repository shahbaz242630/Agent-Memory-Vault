# T0.2.7 Phase 1 — t028c diverse-corpus diagnostic

**Diagnostic question.** Does the t028b iteration-3 quality collapse at scale {1K, 10K} reproduce when the corpus is genuinely DIVERSE (template + vocabulary combinatorial distractors) instead of paraphrase-decorated copies of the 100-memory base?

**Decision tree.** If 10K-diverse quality holds at 4/4 contradictions + 2/2 hard-negatives → the t028b collapse was synthetic-stress-only and V0.2 ships without retrieval-side fixes (add a synthetic-near-dup regression test as a CI canary). If 10K-diverse quality also degrades → real RAG-at-scale problem confirmed, proceed to Phase B (MMR + value-aware guard, then value-grouping if insufficient).

**Run started:** 2026-05-18 06:36:20 UTC
**Host OS:** windows

## Cross-scale summary

| Scale | Build (s) | Embed (s) | Upsert (s) | Recall@10 | Recall@20 | Search p50 (µs) | Search p99 (µs) | Contradictions | Hard-negatives | LLM mean (s) |
|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---:|
| 100 | 0.15 | 1.3 | 0.42 | 1.000 | 1.000 | 6238 | 13787 | n/a | n/a | n/a |
| 1000 | 0.16 | 12.6 | 2.62 | 1.000 | 1.000 | 20296 | 30599 | 2/4 | 1/2 | 112.3 |
| 10000 | 1.63 | 415.5 | 15.06 | 1.000 | 1.000 | 294661 | 337260 | 2/4 | 1/2 | 120.9 |

## Scale = 100

### Index + retrieval

| Metric | Value |
|---|---:|
| Index build (s) | 0.15 |
| Embed total (s) | 1.3 |
| Upsert total (s) | 0.42 |
| Mean recall@10 | 1.000 |
| Mean recall@20 | 1.000 |
| Search p50 (µs) | 6238 |
| Search p99 (µs) | 13787 |
| Search mean (µs) | 6707 |

### Per-query recall

| Query | Recall@10 | Recall@20 |
|---|---:|---:|
| Q11 | 1.000 | 1.000 |
| Q13 | 1.000 | 1.000 |
| Q17 | 1.000 | 1.000 |
| Q19 | 1.000 | 1.000 |
| Q21 | 1.000 | 1.000 |
| Q22 | 1.000 | 1.000 |
| Q25 | 1.000 | 1.000 |
| Q26 | 1.000 | 1.000 |

## Scale = 1000

### Index + retrieval

| Metric | Value |
|---|---:|
| Index build (s) | 0.16 |
| Embed total (s) | 12.6 |
| Upsert total (s) | 2.62 |
| Mean recall@10 | 1.000 |
| Mean recall@20 | 1.000 |
| Search p50 (µs) | 20296 |
| Search p99 (µs) | 30599 |
| Search mean (µs) | 20996 |

### Per-query recall

| Query | Recall@10 | Recall@20 |
|---|---:|---:|
| Q11 | 1.000 | 1.000 |
| Q13 | 1.000 | 1.000 |
| Q17 | 1.000 | 1.000 |
| Q19 | 1.000 | 1.000 |
| Q21 | 1.000 | 1.000 |
| Q22 | 1.000 | 1.000 |
| Q25 | 1.000 | 1.000 |
| Q26 | 1.000 | 1.000 |

### LLM stage (Qwen-7B Q4_K_M, ADR-049)

**Contradictions surfaced:** 2/4  ·  **Hard-negatives rejected:** 1/2

**LLM latency:** p50 = 121.8s · p99 = 152.4s · mean = 112.3s

| Query | Verdict | Detail | Latency (s) |
|---|---|---|---:|
| Q11 | contradiction PASS | flagged=1 · 'Q1 2027'=true 'Q2 2027'=true | 152.4 |
| Q13 | contradiction PASS | flagged=1 · '89'=true '109'=true | 114.5 |
| Q17 | observational | contradictions_flagged.len()=1 | 130.8 |
| Q19 | observational | contradictions_flagged.len()=1 | 120.5 |
| Q21 | hard-negative FAIL | vault_has_no_relevant_content=false | 123.3 |
| Q22 | hard-negative PASS | vault_has_no_relevant_content=true | 57.0 |
| Q25 | contradiction FAIL | flagged=0 · 'Q1 2027'=false 'Q2 2027'=false | 77.9 |
| Q26 | contradiction FAIL | flagged=0 · '89'=true '109'=true | 121.8 |

## Scale = 10000

### Index + retrieval

| Metric | Value |
|---|---:|
| Index build (s) | 1.63 |
| Embed total (s) | 415.5 |
| Upsert total (s) | 15.06 |
| Mean recall@10 | 1.000 |
| Mean recall@20 | 1.000 |
| Search p50 (µs) | 294661 |
| Search p99 (µs) | 337260 |
| Search mean (µs) | 284703 |

### Per-query recall

| Query | Recall@10 | Recall@20 |
|---|---:|---:|
| Q11 | 1.000 | 1.000 |
| Q13 | 1.000 | 1.000 |
| Q17 | 1.000 | 1.000 |
| Q19 | 1.000 | 1.000 |
| Q21 | 1.000 | 1.000 |
| Q22 | 1.000 | 1.000 |
| Q25 | 1.000 | 1.000 |
| Q26 | 1.000 | 1.000 |

### LLM stage (Qwen-7B Q4_K_M, ADR-049)

**Contradictions surfaced:** 2/4  ·  **Hard-negatives rejected:** 1/2

**LLM latency:** p50 = 145.1s · p99 = 214.3s · mean = 120.9s

| Query | Verdict | Detail | Latency (s) |
|---|---|---|---:|
| Q11 | contradiction PASS | flagged=1 · 'Q1 2027'=true 'Q2 2027'=true | 145.1 |
| Q13 | contradiction PASS | flagged=1 · '89'=true '109'=true | 147.6 |
| Q17 | observational | contradictions_flagged.len()=1 | 214.3 |
| Q19 | observational | contradictions_flagged.len()=1 | 152.6 |
| Q21 | hard-negative FAIL | vault_has_no_relevant_content=false | 88.9 |
| Q22 | hard-negative PASS | vault_has_no_relevant_content=true | 36.8 |
| Q25 | contradiction FAIL | flagged=0 · 'Q1 2027'=false 'Q2 2027'=false | 77.0 |
| Q26 | contradiction FAIL | flagged=1 · '89'=true '109'=false | 104.9 |

## Methodology

- 100-memory base fixture from `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json` (preserves contradiction ground truth for Q11/Q13/Q25/Q26).
- Diverse distractors generated via template + vocabulary combinatorial generator with `SplitMix64` PRNG (seed=`0x7028C_DEADBEEF`). 10 distractor clusters (5 work + 5 personal), chosen NOT to collide with any gauntlet query content. See module doc-comment for full rationale.
- Length-tier mix matches t026 realism rewrite: 56% short / 30% paragraph / 11% long-form / 3% truncation. Boundary split 50/50 work/personal.
- Vocabulary deliberately excludes `"89"`, `"109"`, `"Q1 2027"`, `"Q2 2027"`, `"Kubernetes"`, `"dental"`, `"insurance"` substrings to avoid gauntlet-test collision. Money figures span 200-9999 only.
- BGE-small-en-v1.5 ONNX provider for embedding.
- Sealed `LanceVectorStore` per scale (fresh tempdir). HNSW index built via `IvfHnswSqIndexBuilder::default()`. Bulk inserts via `bulk_upsert` in chunks of 500.
- `16` search-latency reps × 8 queries = 128 samples per scale.
- Brute-force ground truth: dot product on BGE's L2-normalized vectors, top-20 per query (recomputed at each scale).
- LLM stage uses production `ReadPipeline` (ADR-048) + `SemanticRetriever`. Same Qwen-7B + locked V0.2 `TuningConfig` (n_threads=12, n_threads_batch=12, n_gpu_layers=99) as `read_pipeline_acceptance.rs`.
- LLM stage runs at scale >= 1000 (scale=100 covered by t026 + `read_pipeline_acceptance`).

## Cross-reference

- `t028b_hnsw_vs_ivf_results.md` — the iteration 3 paraphrase-corpus run that triggered this diagnostic.
- ADR-048 — Read-time pipeline architecture (V0.2 read contract).
- ADR-049 — Qwen-7B model lock.
- ADR-050 — V0.2 production index lock (HNSW), pending Phase A/B outcome documented here.

