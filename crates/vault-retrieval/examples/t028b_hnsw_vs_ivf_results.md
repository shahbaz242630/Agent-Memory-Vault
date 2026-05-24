# T0.2.7 Phase 1 — t028b HNSW + LLM at scale (iteration 3)

**Iteration 3** — scales=[100, 1000, 10000], HNSW only (IVF blocked by V0.2 sealed-envelope per-file-granularity lock — confirmed in iteration 2). LLM stage (Qwen-7B Q4_K_M, ADR-049) runs at scales >= 1000. 8-query t026 gauntlet, lancedb 0.27.2 defaults.

**Run started:** 2026-05-17 18:31:05 UTC
**Host OS:** windows

Fixture content shape matches t026 realism-rewrite, not synthetic short.

## Cross-scale summary

| Scale | Build (s) | Embed (s) | Upsert (s) | Recall@10 | Recall@20 | Search p50 (µs) | Search p99 (µs) | Contradictions | Hard-negatives | LLM mean (s) |
|---:|---:|---:|---:|---:|---:|---:|---:|---|---|---:|
| 100 | 0.09 | 1.7 | 0.15 | 1.000 | 1.000 | 4301 | 7302 | n/a | n/a | n/a |
| 1000 | 0.08 | 18.8 | 0.35 | 1.000 | 1.000 | 15956 | 22054 | 1/4 | 2/2 | 103.0 |
| 10000 | 0.61 | 186.6 | 4.05 | 1.000 | 1.000 | 83753 | 103786 | 0/4 | 1/2 | 70.6 |

## Scale = 100

### Index + retrieval

| Metric | Value |
|---|---:|
| Index build (s) | 0.09 |
| Embed total (s) | 1.7 |
| Upsert total (s) | 0.15 |
| Mean recall@10 | 1.000 |
| Mean recall@20 | 1.000 |
| Search p50 (µs) | 4301 |
| Search p99 (µs) | 7302 |
| Search mean (µs) | 4520 |

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
| Index build (s) | 0.08 |
| Embed total (s) | 18.8 |
| Upsert total (s) | 0.35 |
| Mean recall@10 | 1.000 |
| Mean recall@20 | 1.000 |
| Search p50 (µs) | 15956 |
| Search p99 (µs) | 22054 |
| Search mean (µs) | 16013 |

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

**Contradictions surfaced:** 1/4  ·  **Hard-negatives rejected:** 2/2

**LLM latency:** p50 = 99.3s · p99 = 270.3s · mean = 103.0s

| Query | Verdict | Detail | Latency (s) |
|---|---|---|---:|
| Q11 | contradiction PASS | flagged=1 · 'Q1 2027'=true 'Q2 2027'=true | 270.3 |
| Q13 | contradiction FAIL | flagged=0 · '89'=true '109'=true | 149.1 |
| Q17 | observational | contradictions_flagged.len()=0 | 49.3 |
| Q19 | observational | contradictions_flagged.len()=0 | 26.4 |
| Q21 | hard-negative PASS | vault_has_no_relevant_content=true | 42.7 |
| Q22 | hard-negative PASS | vault_has_no_relevant_content=true | 56.7 |
| Q25 | contradiction FAIL | flagged=1 · 'Q1 2027'=true 'Q2 2027'=false | 130.4 |
| Q26 | contradiction FAIL | flagged=0 · '89'=true '109'=false | 99.3 |

## Scale = 10000

### Index + retrieval

| Metric | Value |
|---|---:|
| Index build (s) | 0.61 |
| Embed total (s) | 186.6 |
| Upsert total (s) | 4.05 |
| Mean recall@10 | 1.000 |
| Mean recall@20 | 1.000 |
| Search p50 (µs) | 83753 |
| Search p99 (µs) | 103786 |
| Search mean (µs) | 83787 |

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

**Contradictions surfaced:** 0/4  ·  **Hard-negatives rejected:** 1/2

**LLM latency:** p50 = 67.0s · p99 = 154.6s · mean = 70.6s

| Query | Verdict | Detail | Latency (s) |
|---|---|---|---:|
| Q11 | contradiction FAIL | flagged=0 · 'Q1 2027'=true 'Q2 2027'=false | 154.6 |
| Q13 | contradiction FAIL | flagged=0 · '89'=true '109'=false | 154.3 |
| Q17 | observational | contradictions_flagged.len()=0 | 27.5 |
| Q19 | observational | contradictions_flagged.len()=0 | 26.4 |
| Q21 | hard-negative PASS | vault_has_no_relevant_content=true | 28.4 |
| Q22 | hard-negative FAIL | vault_has_no_relevant_content=false | 67.0 |
| Q25 | contradiction FAIL | flagged=0 · 'Q1 2027'=false 'Q2 2027'=false | 25.8 |
| Q26 | contradiction FAIL | flagged=0 · '89'=true '109'=false | 80.8 |

## Methodology

- 100-memory base fixture from `crates/vault-consolidator/tests/fixtures/merge_acceptance_100.json`
- 8-query gauntlet from `crates/vault-retrieval/test-fixtures/merge_acceptance_100_queries.json` (subset `Q11`, `Q13`, `Q17`, `Q19`, `Q21`, `Q22`, `Q25`, `Q26`)
- Scale-up via session-prefix variation: `[session-{j:03}] {original_content}` — see `generate_scaled_corpus` in the spike source. **Limitation:** variations cluster around base centroids; stresses near-duplicate handling, not corpus diversity.
- BGE-small-en-v1.5 ONNX provider for embedding
- Sealed `LanceVectorStore` per scale (fresh tempdir). HNSW index built via `IvfHnswSqIndexBuilder::default()`.
- Bulk inserts via `LanceVectorStore::bulk_upsert` in chunks of 500 (production-candidate batch API; ~730× faster than single-row at scale 10K).
- `16` search-latency reps × 8 queries = 128 samples per scale
- Brute-force ground truth: dot product on BGE's L2-normalized vectors, top-20 per query (recomputed at each scale)
- Recall@K = |retrieved∩ground_truth| ÷ K, computed per query then averaged
- LLM stage uses production `ReadPipeline` (ADR-048) + `SemanticRetriever`. Same Qwen-7B (`Qwen2.5-7B-Instruct-Q4_K_M.gguf`) + locked V0.2 `TuningConfig` (n_threads=12, n_threads_batch=12, n_gpu_layers=99) as `read_pipeline_acceptance.rs`.
- LLM stage runs only at scale >= 1000 (scale=100 LLM coverage already landed via t026 + `read_pipeline_acceptance`).

## Open items

- If recall stays at 1.000 across all scales (degenerate due to near-duplicate clustering), iteration 4+ would build a richer noise corpus (template + vocabulary combinations) to genuinely diversify embeddings.
- IVF re-eval blocked until V0.2.x adds streaming-multipart support to SealedObjectStore. Not on the V0.2 critical path.
- Production-index decision: HNSW locked for V0.2 (architectural compatibility). See ADR-050.
