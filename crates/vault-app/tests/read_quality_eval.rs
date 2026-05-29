//! A7 read-quality calibration harness (meaning-similarity calibration workstream, 2026-05-29).
//!
//! Drives the `read_quality_eval.json` fixture through the **real** production
//! read path (`Adapter::read` → `StructuredReadPipeline` with the ADR-057
//! relevance gate wired, real BGE embeddings) and prints a scorecard:
//! per-case pass/fail + abstention confusion matrix + the **raw BGE top-1
//! cosine** the gate keys on, plus the separability between real-answer
//! cosines and the true-negative guard cosines. This is the *baseline
//! measurement instrument* for the A7 over-abstention / recall-cliff finding
//! — NOT a gating test yet. We read the numbers, design the fix against them,
//! and add hard threshold gates in a later commit once targets are chosen.
//!
//! It also reports the cosine distribution **with the BGE query instruction
//! prefix applied to the query only** (model-card s2p usage), so we can see
//! whether using the embedder correctly (a free, no-new-model change) lifts
//! real-answer separability before we consider a re-ranker / stronger embedder.
//!
//! Cosines are computed directly here (embed query + each seed via BGE, take
//! the top-1) because `Adapter::read` only exposes abstain/facts, not the gate's
//! internal cosine. BGE is deterministic, so this matches the gate's value.
//!
//! ## Why in-process (no MCP / no Claude Desktop)
//!
//! `Adapter::read` is the same entry point the MCP `memory_read` tool calls,
//! so running it directly reproduces the live behaviour (including the
//! relevance gate) deterministically and without the dotted-boundary /
//! authorization dance. Each case is seeded into its own dot-free boundary
//! (`eval0`, `eval1`, …) so cases don't cross-contaminate; the fixture's
//! advisory `boundary: "testeval"` field is overridden here.
//!
//! ## Running
//!
//! ```text
//! cargo test -p vault-app --test read_quality_eval -- --ignored --nocapture
//! ```
//!
//! Requires the bge-small-en-v1.5 fixtures (run scripts/setup-dev-env.{sh,ps1}).
//! No Phi-4 needed — the read path is fully deterministic per ADR-052.
//!
//! ## macOS deferral (ADR-033)
//!
//! Disabled on macOS via the `#![cfg(...)]` below — BGE transitively loads ORT
//! which SIGABRTs at process exit on macOS. Linux + Windows cover the path.

#![cfg(not(target_os = "macos"))]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use tempfile::TempDir;

use vault_app::{AppConfig, Application};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider};
use vault_mcp::Adapter;
use vault_retrieval::ReadQuery;
use vault_storage::SqlCipherKey;

// ---------------------------------------------------------------------------
// Fixture path resolution (minimal duplication of integration_smoke.rs helpers
// — test files don't share modules, and these are 3 trivial fns).
// ---------------------------------------------------------------------------

const BGE_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/bge-small-en-v1.5";

/// BGE-small-en-v1.5 query instruction for s2p retrieval (per the model card).
/// Applied to the QUERY ONLY; passages get no prefix. v1.5 notes omitting it is
/// only a "slight degradation", so this may help modestly — the harness measures
/// whether it actually lifts query→answer separation on our fixture.
const BGE_QUERY_PREFIX: &str = "Represent this sentence for searching relevant passages: ";

fn bge_fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(BGE_FIXTURE_REL);
    p.push(name);
    assert!(
        p.exists(),
        "missing bge fixture {p:?} — run scripts/setup-dev-env.(sh|ps1) first"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib() -> PathBuf {
    bge_fixture("onnxruntime.dll")
}
#[cfg(target_os = "linux")]
fn ort_lib() -> PathBuf {
    bge_fixture("libonnxruntime.so")
}

fn eval_fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../vault-retrieval/tests/fixtures/read_quality_eval.json");
    p
}

/// Cosine similarity of two vectors. BGE outputs are L2-normalised so this is
/// effectively a dot product, but we normalise defensively.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na * nb)
    }
}

/// min / max of a slice of f32 (NaN if empty).
fn min_max(v: &[f32]) -> (f32, f32) {
    let min = v.iter().copied().fold(f32::INFINITY, f32::min);
    let max = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if v.is_empty() {
        (f32::NAN, f32::NAN)
    } else {
        (min, max)
    }
}

fn join_cos(v: &[f32]) -> String {
    let mut sorted = v.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    sorted
        .iter()
        .map(|c| format!("{c:.3}"))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Fixture model (parsed from serde_json::Value; only the fields the harness
// consumes — schema_version / boundary / _* annotations / as_of are ignored).
// ---------------------------------------------------------------------------

struct EvalCase {
    id: String,
    ability: String,
    seed_memories: Vec<SeedMemory>,
    query: String,
    expect: Expect,
}

struct SeedMemory {
    content: String,
    source_agent: Option<String>,
    confidence: f32,
}

struct Expect {
    must_surface: Vec<String>,
    must_exclude: Vec<String>,
    must_rank_top_k: Option<RankReq>,
    abstain: bool,
}

struct RankReq {
    id: String,
    k: usize,
}

fn parse_fixture(v: &Value) -> Vec<EvalCase> {
    v.get("cases")
        .and_then(Value::as_array)
        .expect("fixture must have a `cases` array")
        .iter()
        .map(parse_case)
        .collect()
}

fn parse_case(c: &Value) -> EvalCase {
    EvalCase {
        id: c["id"].as_str().expect("case.id").to_string(),
        ability: c["ability"].as_str().unwrap_or("unknown").to_string(),
        query: c["query"].as_str().expect("case.query").to_string(),
        seed_memories: c
            .get("seed_memories")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(parse_seed).collect())
            .unwrap_or_default(),
        expect: parse_expect(&c["expect"]),
    }
}

fn parse_seed(s: &Value) -> SeedMemory {
    SeedMemory {
        content: s["content"].as_str().expect("seed.content").to_string(),
        source_agent: s
            .get("source_agent")
            .and_then(Value::as_str)
            .map(String::from),
        confidence: s
            .get("confidence")
            .and_then(Value::as_f64)
            .map(|f| f as f32)
            .unwrap_or(0.9),
    }
}

fn parse_expect(e: &Value) -> Expect {
    let str_vec = |key: &str| -> Vec<String> {
        e.get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    let must_rank_top_k = e.get("must_rank_top_k").and_then(|r| {
        Some(RankReq {
            id: r.get("id")?.as_str()?.to_string(),
            k: r.get("k")?.as_u64()? as usize,
        })
    });
    Expect {
        must_surface: str_vec("must_surface"),
        must_exclude: str_vec("must_exclude"),
        must_rank_top_k,
        abstain: e.get("abstain").and_then(Value::as_bool).unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real-BGE calibration harness; run with --ignored --nocapture to read the scorecard"]
async fn read_quality_eval_baseline() {
    // ---- build one real read stack over a temp vault (no Phi-4) ----
    let tmp = TempDir::new().expect("tempdir");
    let config = AppConfig {
        metadata_path: tmp.path().join("vault.db"),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: SqlCipherKey::new("read-quality-eval-key"),
        model_path: bge_fixture("model.onnx"),
        tokenizer_path: bge_fixture("tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        qwen_model_path: None,
        phi4_model_path: None,
        rerank_model_path: None,
        rerank_tokenizer_path: None,
    };
    let app = Application::new(&config)
        .await
        .expect("Application::new must compose the read stack");
    let _shutdown = app.start(); // spawn the cascading retry worker
    let adapter = app.adapter();

    // Separate BGE handle to read out the raw top-1 cosine the gate keys on
    // (Adapter::read does not expose it; BGE is deterministic so this matches).
    let probe = BgeSmallProvider::open(
        &bge_fixture("model.onnx"),
        &bge_fixture("tokenizer.json"),
        &ort_lib(),
    )
    .expect("open BGE probe embedder");

    // ---- load + parse fixture ----
    let bytes = std::fs::read(eval_fixture_path()).expect("read read_quality_eval.json");
    let value: Value = serde_json::from_slice(&bytes).expect("parse read_quality_eval.json");
    let cases = parse_fixture(&value);

    // ---- seed each case into its own boundary; record "caseid#i" -> uuid ----
    let mut id_map: BTreeMap<String, String> = BTreeMap::new();
    for (n, case) in cases.iter().enumerate() {
        let boundary = Boundary::new(format!("eval{n}")).expect("eval boundary valid");
        for (i, sm) in case.seed_memories.iter().enumerate() {
            let nm = NewMemory {
                content: sm.content.clone(),
                memory_type: MemoryType::Semantic,
                boundary: boundary.clone(),
                source_agent: sm.source_agent.clone(),
                confidence: sm.confidence,
                valid_from: None,
                valid_until: None,
                metadata: serde_json::json!({}),
            };
            let id = adapter.write(nm).await.expect("seed write");
            id_map.insert(format!("{}#{}", case.id, i), id.to_string());
        }
    }

    // ---- wait for the cascade worker to drain writes into LanceDB ----
    // Semantic retrieval (and the ADR-057 relevance probe) query the vector
    // store; the BM25 leg is inline, but the gate keys on BGE cosine.
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;

    // ---- run each case through the real read path + score ----
    let mut correct_abstain = 0usize; // expected abstain, got abstain
    let mut correct_answer = 0usize; // expected answer, got answer
    let mut false_abstain = 0usize; // expected answer, got abstain  (THE CLIFF)
    let mut false_answer = 0usize; // expected abstain, got answer
    let mut surface_pass = 0usize;
    let mut surface_total = 0usize;
    let mut rank_pass = 0usize;
    let mut rank_total = 0usize;
    let mut exclude_pass = 0usize;
    let mut exclude_total = 0usize;
    // top-1 cosines split by ground truth, raw + with the BGE query prefix.
    let mut real_cos: Vec<f32> = Vec::new();
    let mut guard_cos: Vec<f32> = Vec::new();
    let mut real_cos_pfx: Vec<f32> = Vec::new();
    let mut guard_cos_pfx: Vec<f32> = Vec::new();

    println!("\n================ READ-QUALITY EVAL (A7) — BASELINE ================\n");

    for (n, case) in cases.iter().enumerate() {
        let boundary = Boundary::new(format!("eval{n}")).expect("eval boundary valid");

        // raw top-1 cosine + with-prefix top-1 cosine (prefix on QUERY only).
        let q_emb = probe.embed(&case.query).await.expect("embed query");
        let q_emb_pfx = probe
            .embed(&format!("{BGE_QUERY_PREFIX}{}", case.query))
            .await
            .expect("embed prefixed query");
        let mut top1_cos = 0.0f32;
        let mut top1_cos_pfx = 0.0f32;
        for sm in &case.seed_memories {
            let s_emb = probe.embed(&sm.content).await.expect("embed seed");
            top1_cos = top1_cos.max(cosine(&q_emb, &s_emb));
            top1_cos_pfx = top1_cos_pfx.max(cosine(&q_emb_pfx, &s_emb));
        }
        if case.expect.abstain {
            guard_cos.push(top1_cos);
            guard_cos_pfx.push(top1_cos_pfx);
        } else {
            real_cos.push(top1_cos);
            real_cos_pfx.push(top1_cos_pfx);
        }

        let resp = adapter
            .read(ReadQuery {
                query_text: case.query.clone(),
                authorized_boundaries: vec![boundary],
            })
            .await
            .expect("read must not error");

        let returned: Vec<&str> = resp
            .relevant_facts
            .iter()
            .map(|f| f.memory_id.as_str())
            .collect();

        // resolve a "caseid#i" reference to the uuid it was written as
        let resolve = |key: &str| -> Option<String> { id_map.get(key).cloned() };

        // abstention confusion matrix
        match (case.expect.abstain, resp.abstain) {
            (true, true) => correct_abstain += 1,
            (false, false) => correct_answer += 1,
            (false, true) => false_abstain += 1,
            (true, false) => false_answer += 1,
        }

        // must_surface
        let mut surfaced_ok = true;
        for key in &case.expect.must_surface {
            surface_total += 1;
            let present = resolve(key).is_some_and(|id| returned.iter().any(|r| *r == id));
            if present {
                surface_pass += 1;
            } else {
                surfaced_ok = false;
            }
        }

        // must_exclude
        let mut excluded_ok = true;
        for key in &case.expect.must_exclude {
            exclude_total += 1;
            let absent = match resolve(key) {
                Some(id) => !returned.iter().any(|r| *r == id),
                None => true, // unresolved reference → treat as not-leaked
            };
            if absent {
                exclude_pass += 1;
            } else {
                excluded_ok = false;
            }
        }

        // must_rank_top_k
        let mut rank_ok = true;
        let mut rank_note = String::new();
        if let Some(req) = &case.expect.must_rank_top_k {
            rank_total += 1;
            match resolve(&req.id) {
                Some(id) => match returned.iter().position(|r| *r == id) {
                    Some(p) if p < req.k => {
                        rank_pass += 1;
                        rank_note = format!("rank {} <= top-{}", p + 1, req.k);
                    }
                    Some(p) => {
                        rank_ok = false;
                        rank_note = format!("rank {} > top-{} (DISPLACED)", p + 1, req.k);
                    }
                    None => {
                        rank_ok = false;
                        rank_note = format!("not returned (need top-{})", req.k);
                    }
                },
                None => {
                    rank_ok = false;
                    rank_note = "rank target id unresolved".to_string();
                }
            }
        }

        let abstain_ok = case.expect.abstain == resp.abstain;
        let pass = abstain_ok && surfaced_ok && excluded_ok && rank_ok;
        let extra = if !surfaced_ok {
            "  | MISSING a must_surface fact"
        } else if !excluded_ok {
            "  | leaked a must_exclude fact"
        } else {
            ""
        };
        let rank_col = if case.expect.must_rank_top_k.is_some() {
            format!("  | {rank_note}")
        } else {
            String::new()
        };
        println!(
            "[{}] {} ({})\n    query: {:?}\n    cos raw={:.3} / prefixed={:.3} (floor 0.66)  | expect abstain={} got abstain={}  | returned {} fact(s){}{}",
            if pass { "PASS" } else { "FAIL" },
            case.id,
            case.ability,
            case.query,
            top1_cos,
            top1_cos_pfx,
            case.expect.abstain,
            resp.abstain,
            returned.len(),
            rank_col,
            extra,
        );
    }

    // ---- aggregate scorecard ----
    let (real_min, _) = min_max(&real_cos);
    let (_, guard_max) = min_max(&guard_cos);
    let (real_min_pfx, _) = min_max(&real_cos_pfx);
    let (_, guard_max_pfx) = min_max(&guard_cos_pfx);

    println!("\n---------------- AGGREGATE ----------------");
    println!("abstention confusion matrix (current production gate, raw cosine):");
    println!("  correct-answer  (expect answer, got answer) : {correct_answer}");
    println!("  correct-abstain (expect abstain, got abstain): {correct_abstain}");
    println!("  FALSE-ABSTAIN   (expect answer, got abstain) : {false_abstain}   <-- the A7 cliff");
    println!("  FALSE-ANSWER    (expect abstain, got answer) : {false_answer}   <-- over-correction guard");
    println!("must_surface  : {surface_pass}/{surface_total} facts surfaced");
    println!("must_rank_top : {rank_pass}/{rank_total} rank checks met");
    println!("must_exclude  : {exclude_pass}/{exclude_total} exclusions held");

    println!("\ntop-1 BGE cosine — RAW (no query prefix; what production does today):");
    println!("  real-answer (should proceed): {}", join_cos(&real_cos));
    println!("  guard       (should abstain): {}", join_cos(&guard_cos));
    println!(
        "  min(real)={real_min:.3}  max(guard)={guard_max:.3}  separable by one floor? {}",
        real_min > guard_max
    );

    println!("\ntop-1 BGE cosine — WITH query prefix (\"Represent this sentence…\", query only):");
    println!(
        "  real-answer (should proceed): {}",
        join_cos(&real_cos_pfx)
    );
    println!(
        "  guard       (should abstain): {}",
        join_cos(&guard_cos_pfx)
    );
    println!(
        "  min(real)={real_min_pfx:.3}  max(guard)={guard_max_pfx:.3}  separable by one floor? {}",
        real_min_pfx > guard_max_pfx
    );
    println!("===========================================\n");

    // Characterization harness — assert only that every case ran + scored.
    // Threshold gates come in a later commit once the fix targets are chosen.
    let scored = correct_answer + correct_abstain + false_abstain + false_answer;
    assert_eq!(
        scored,
        cases.len(),
        "every fixture case must be scored exactly once"
    );
}
