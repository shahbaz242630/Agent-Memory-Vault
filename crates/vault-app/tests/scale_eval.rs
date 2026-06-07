//! Scale correctness harness — "prove correctness at scale" arc (T0.3.x, 2026-06-04).
//!
//! The correctness core (recall-first read ADR-066 + search ADR-067, the -2.5
//! no-signal floor, honest abstention) is proven STRUCTURALLY on a ~12-fact toy
//! vault. This harness asks the next question: **does it still hold when the
//! right answer must beat ~80 unrelated distractor facts?**
//!
//! It seeds the `scale_eval.json` planted facts (synonym-gap recall targets +
//! their near-miss must-excludes + genuine no-signal questions) into a shared
//! pool, pads to `scale` (100) with the deterministic t029 distractor generator,
//! then runs the REAL production read + search paths per query and prints a
//! scorecard:
//!
//! - **Read abstention confusion matrix** — false-abstain (the A7 cliff) vs
//!   false-answer (over-correction). This is the headline number.
//! - **Read recall + rank** — did the target surface, and where did it rank
//!   relative to its near-miss distractor.
//! - **Search recall@k** — recall-first hybrid (no abstain gate), top-5 / top-20.
//! - **Near-miss leak diagnostic** — recall-first read returns candidates and
//!   trusts the agent for precision, so a returned near-miss is NOT a hard fail
//!   here; we report it (and its rank vs the target) as a precision signal.
//!
//! Characterization only — NO hard pass/fail gate yet. We read these numbers,
//! calibrate the -2.5 floor at scale, THEN pin gates (per the "measure first,
//! anchor on measured not projected" discipline). Reading the exact reranker
//! margins (to tune the floor) is a deliberate pass-2 follow-up: this pass uses
//! only what the production read/search responses expose, so it measures the
//! floor's *behaviour* at scale, not its internal scores.
//!
//! ## Why in-process (no MCP / no Claude Desktop)
//!
//! `adapter.read` / `adapter.search` are the same entry points the MCP
//! `memory_read` / `memory_search` tools call, so running them directly
//! reproduces live behaviour (reranker + the -2.5 floor) deterministically.
//!
//! ## Running
//!
//! ```text
//! cargo test -p vault-app --test scale_eval -- --ignored --nocapture
//! ```
//!
//! Needs the bge-small-en-v1.5 AND qwen3-reranker-0.6b-seq-cls fixtures
//! (run scripts/setup-dev-env.{sh,ps1}). The reranker is load-bearing: it is
//! the read relevance authority (ADR-059) and holds the -2.5 no-signal floor
//! (ADR-066) we are here to stress at scale. No Phi-4 needed (read is
//! deterministic per ADR-052).
//!
//! ## macOS deferral (ADR-033)
//!
//! Disabled on macOS — BGE/reranker transitively load ORT which SIGABRTs at
//! process exit on macOS. Linux + Windows cover the path.

#![cfg(not(target_os = "macos"))]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

use vault_app::{AppConfig, Application};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider};
use vault_mcp::Adapter;
use vault_retrieval::structured_read_pipeline::DEFAULT_MAX_CANDIDATES;
use vault_retrieval::{ReadQuery, RetrievalOptions, RetrievalQuery};
use vault_storage::SqlCipherKey;

// ---------------------------------------------------------------------------
// Fixture path resolution (mirrors read_no_keyword_overlap.rs — test files
// don't share modules; these are 3 trivial fns).
// ---------------------------------------------------------------------------

const BGE_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/bge-small-en-v1.5";
const RERANK_FIXTURE_REL: &str = "../vault-embedding/test-fixtures/qwen3-reranker-0.6b-seq-cls";

fn fixture(rel: &str, name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push(rel);
    p.push(name);
    assert!(
        p.exists(),
        "missing fixture {p:?} — run scripts/setup-dev-env.(sh|ps1) first"
    );
    p
}

#[cfg(target_os = "windows")]
fn ort_lib() -> PathBuf {
    fixture(BGE_FIXTURE_REL, "onnxruntime.dll")
}
#[cfg(target_os = "linux")]
fn ort_lib() -> PathBuf {
    fixture(BGE_FIXTURE_REL, "libonnxruntime.so")
}

fn scale_fixture_path() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/scale_eval.json");
    p
}

// ---------------------------------------------------------------------------
// Fixture model (parsed from serde_json::Value; only the fields the harness
// consumes — _* annotations are ignored).
// ---------------------------------------------------------------------------

struct PlantedFact {
    id: String,
    boundary: String,
    content: String,
    source_agent: Option<String>,
    confidence: f32,
}

struct EvalQuery {
    id: String,
    kind: String,
    query_text: String,
    must_surface: Vec<String>,
    must_exclude: Vec<String>,
    abstain: bool,
}

fn str_vec(v: &Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_planted(v: &Value) -> Vec<PlantedFact> {
    v.get("planted")
        .and_then(Value::as_array)
        .expect("fixture must have a `planted` array")
        .iter()
        .map(|p| PlantedFact {
            id: p["id"].as_str().expect("planted.id").to_string(),
            boundary: p["boundary"]
                .as_str()
                .expect("planted.boundary")
                .to_string(),
            content: p["content"].as_str().expect("planted.content").to_string(),
            source_agent: p
                .get("source_agent")
                .and_then(Value::as_str)
                .map(String::from),
            confidence: p
                .get("confidence")
                .and_then(Value::as_f64)
                .map(|f| f as f32)
                .unwrap_or(0.9),
        })
        .collect()
}

fn parse_queries(v: &Value) -> Vec<EvalQuery> {
    v.get("queries")
        .and_then(Value::as_array)
        .expect("fixture must have a `queries` array")
        .iter()
        .map(|q| {
            let e = &q["expect"];
            EvalQuery {
                id: q["id"].as_str().expect("query.id").to_string(),
                kind: q
                    .get("_kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                query_text: q["query_text"]
                    .as_str()
                    .expect("query.query_text")
                    .to_string(),
                must_surface: str_vec(e, "must_surface"),
                must_exclude: str_vec(e, "must_exclude"),
                abstain: e.get("abstain").and_then(Value::as_bool).unwrap_or(false),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Distractor generator — ported verbatim-in-spirit from
// `crates/vault-retrieval/examples/t029_scale_1000_retrieval_diagnostic.rs`.
// Duplicated rather than shared via a feature-flagged module to avoid a
// CI-matrix change for one extra consumer (rule of three: extract if a third
// consumer appears). Deterministic (SplitMix64, fixed seed) so every run is
// byte-identical.
// ---------------------------------------------------------------------------

const DISTRACTOR_SEED: u64 = 0x5CA1_EACD_EED0;

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn pick<'a, T>(&mut self, slice: &'a [T]) -> &'a T {
        &slice[(self.next_u64() as usize) % slice.len()]
    }
}

const DISTRACTOR_BOUNDARIES: &[&str] = &["work", "personal", "tools"];

const TOPICS: &[&str] = &[
    "office plant care schedule",
    "cafeteria menu rotation",
    "fire drill calendar",
    "lobby art swap",
    "supply closet inventory",
    "parking permit refresh",
    "team birthday lunch coordination",
    "office library book donation",
    "stationery reorder cycle",
    "coffee machine cleaning rota",
    "wellness room booking process",
    "guest visitor badge handoff",
    "elevator maintenance window",
    "rooftop garden volunteer rota",
    "company swag inventory check",
];

const PEOPLE: &[&str] = &[
    "Olivia", "Priya", "Marcus", "Diego", "Sarah", "Jenna", "Tom", "Aisha", "Marco", "Lena",
    "Kenji", "Riya", "Felix", "Maya", "Sam",
];

const DAYS: &[&str] = &[
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "this morning",
    "yesterday afternoon",
];

const MONTHS: &[&str] = &[
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const CITIES: &[&str] = &[
    "Austin",
    "Denver",
    "Portland",
    "Seattle",
    "Boston",
    "Chicago",
    "Atlanta",
    "Raleigh",
    "Boise",
    "Salt Lake City",
];

const VENDORS: &[&str] = &[
    "Acme Travel",
    "BlueSky Bookings",
    "Mercury Logistics",
    "Northwind Catering",
    "Lighthouse Print",
    "Vertex Furniture",
    "Cascade Cleaning",
    "Summit Coffee Supply",
];

const AMOUNTS: &[&str] = &[
    "$1,200/mo",
    "$3,400/year",
    "$450 one-time",
    "$78/seat",
    "$6,500 annual",
    "$220/mo",
    "$15K total",
    "$320/quarter",
];

const ACTIONS: &[&str] = &[
    "posted the signup sheet",
    "refilled the supply bins",
    "wiped down the meeting room whiteboards",
    "stocked the snack pantry",
    "swapped the lobby flowers",
    "labelled the storage crates",
    "watered the office plants",
    "ordered new lanyards for visitors",
];

const DISTRACTOR_TEMPLATES: &[&str] = &[
    "{topic} note: {person} led the chat on {day}; {action}.",
    "Recap of {topic} held {day} — {person} shared the quarterly facilities update.",
    "{topic} session in {city} planned for {month}; {person} coordinating logistics.",
    "Booked {vendor} for the {topic} event in {city}, rate approximately {amount}.",
    "{person} updated the office bulletin board with the {month} {topic} schedule.",
    "{person} is moving the {topic} agenda to {day} so the team can prep beforehand.",
    "Travel arrangement for {person} to {city} in {month} via {vendor}; {action}.",
    "{topic} signup sheet posted by {person} on {day}; first {action}.",
    "Team event in {city} on {day} — {person} {action} after the wrap-up.",
    "Office news: {topic} confirmed for {month}; coordinator is {person}.",
];

fn render_template(template: &str, rng: &mut SplitMix64) -> String {
    let mut out = template.to_string();
    let replacements: &[(&str, &[&str])] = &[
        ("{topic}", TOPICS),
        ("{person}", PEOPLE),
        ("{day}", DAYS),
        ("{month}", MONTHS),
        ("{city}", CITIES),
        ("{vendor}", VENDORS),
        ("{amount}", AMOUNTS),
        ("{action}", ACTIONS),
    ];
    for (placeholder, pool) in replacements {
        while out.contains(*placeholder) {
            let pick = rng.pick(pool);
            out = out.replacen(*placeholder, pick, 1);
        }
    }
    out
}

struct Distractor {
    boundary: &'static str,
    content: String,
}

fn generate_distractors(count: usize, seed: u64) -> Vec<Distractor> {
    let mut rng = SplitMix64::new(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let template = rng.pick(DISTRACTOR_TEMPLATES);
        let boundary = *rng.pick(DISTRACTOR_BOUNDARIES);
        let content = render_template(template, &mut rng);
        out.push(Distractor { boundary, content });
    }
    out
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real BGE + Qwen3-Reranker over a ~100-fact vault (slow); run with --ignored --nocapture"]
async fn scale_correctness_eval() {
    // ---- build one real read+search stack over a temp vault ----
    let tmp = TempDir::new().expect("tempdir");
    let config = AppConfig {
        metadata_path: tmp.path().join("vault.db"),
        vector_dir: tmp.path().join("lance"),
        graph_path: tmp.path().join("graph.duckdb"),
        key: SqlCipherKey::new("scale-eval-key"),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: zeroize::Zeroizing::new([0u8; 32]),
        qwen_model_path: None,
        phi4_model_path: None,
        // Load-bearing: the reranker is the read relevance authority (ADR-059)
        // and holds the -2.5 no-signal floor (ADR-066) this harness stresses.
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };
    let app = Application::new(&config)
        .await
        .expect("Application::new must compose the read stack (BGE + reranker)");
    let _shutdown = app.start(); // spawn the cascading retry worker
    let adapter = app.adapter();

    // ---- load + parse fixture ----
    let bytes = std::fs::read(scale_fixture_path()).expect("read scale_eval.json");
    let value: Value = serde_json::from_slice(&bytes).expect("parse scale_eval.json");
    let planted = parse_planted(&value);
    let queries = parse_queries(&value);
    // Scale is the fixture default, overridable via `SCALE_EVAL_N` so the same
    // harness runs the 100 → 1k → 10k → 100k ladder without mutating the
    // committed fixture.
    let scale = std::env::var("SCALE_EVAL_N")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or_else(|| value.get("scale").and_then(Value::as_u64).unwrap_or(100) as usize);
    let authorized: Vec<Boundary> = value
        .get("authorized_boundaries")
        .and_then(Value::as_array)
        .expect("authorized_boundaries")
        .iter()
        .filter_map(Value::as_str)
        .map(|b| Boundary::new(b).expect("authorized boundary valid"))
        .collect();

    // ---- seed planted facts; record planted-id -> uuid ----
    let mut id_map: BTreeMap<String, String> = BTreeMap::new();
    for pf in &planted {
        let nm = NewMemory {
            content: pf.content.clone(),
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(pf.boundary.as_str()).expect("planted boundary valid"),
            source_agent: pf.source_agent.clone(),
            confidence: pf.confidence,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        let uuid = adapter
            .write(nm)
            .await
            .expect("seed planted write")
            .to_string();
        id_map.insert(pf.id.clone(), uuid);
    }

    // ---- pad to `scale` with deterministic distractors ----
    let distractor_count = scale.saturating_sub(planted.len());
    for d in generate_distractors(distractor_count, DISTRACTOR_SEED) {
        let nm = NewMemory {
            content: d.content,
            memory_type: MemoryType::Semantic,
            boundary: Boundary::new(d.boundary).expect("distractor boundary valid"),
            source_agent: Some("scale-distractor".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        };
        adapter.write(nm).await.expect("seed distractor write");
    }
    let total = planted.len() + distractor_count;
    println!(
        "\n================ SCALE CORRECTNESS EVAL ================\nseeded {} facts ({} planted + {} distractors), scale target {}\n",
        total,
        planted.len(),
        distractor_count,
        scale
    );

    // ---- readiness poll: wait until a distinctive planted fact is searchable ----
    // The cascade worker drains writes into LanceDB + the keyword index async;
    // poll search() for the Rivian fact rather than guessing a sleep duration.
    let rivian_uuid = id_map
        .get("drive-rivian")
        .cloned()
        .expect("drive-rivian planted");
    // Cascade drains async; the larger the seed, the longer the drain. Scale the
    // poll window with `scale` (2s ticks): ~120s floor, +2s per 10 facts. Breaks
    // early as soon as the planted Rivian fact is searchable.
    let max_attempts = (scale / 10).max(60);
    let mut ready = false;
    for attempt in 0..max_attempts {
        let hits = adapter
            .search(RetrievalQuery {
                query_text: "Rivian R1T".into(),
                authorized_boundaries: authorized.clone(),
                max_results: 5,
                options: RetrievalOptions::default(),
            })
            .await
            .expect("readiness search must not error");
        if hits
            .iter()
            .any(|h| h.memory.id.0.to_string() == rivian_uuid)
        {
            println!("vault ready after {}s (cascade drained)\n", attempt * 2);
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    assert!(
        ready,
        "cascade did not drain the planted Rivian fact into the searchable index within {}s",
        max_attempts * 2
    );

    // ---- per-query scoring ----
    // Read abstention confusion matrix.
    let mut correct_answer = 0usize; // expect answer, got answer
    let mut correct_abstain = 0usize; // expect abstain, got abstain
    let mut false_abstain = 0usize; // expect answer, got abstain  (THE CLIFF)
    let mut false_answer = 0usize; // expect abstain, got answer   (over-correction)
                                   // Recall + rank accounting (recall queries only).
    let mut read_surface_pass = 0usize;
    let mut read_surface_total = 0usize;
    let mut read_target_top1 = 0usize;
    let mut search_top5 = 0usize;
    let mut search_top20 = 0usize;
    let mut near_miss_leaks = 0usize;

    println!("---------------- PER-QUERY ----------------");

    for q in &queries {
        // Resolve a planted-id reference to the uuid it was written as.
        let resolve = |key: &str| -> Option<String> { id_map.get(key).cloned() };

        // ── READ ──────────────────────────────────────────────────────────
        let resp = adapter
            .read(ReadQuery {
                query_text: q.query_text.clone(),
                authorized_boundaries: authorized.clone(),
            })
            .await
            .expect("read must not error");
        let read_ids: Vec<&str> = resp
            .relevant_facts
            .iter()
            .map(|f| f.memory_id.as_str())
            .collect();

        match (q.abstain, resp.abstain) {
            (false, false) => correct_answer += 1,
            (true, true) => correct_abstain += 1,
            (false, true) => false_abstain += 1,
            (true, false) => false_answer += 1,
        }

        // ── SEARCH ────────────────────────────────────────────────────────
        let hits = adapter
            .search(RetrievalQuery {
                query_text: q.query_text.clone(),
                authorized_boundaries: authorized.clone(),
                max_results: 20,
                options: RetrievalOptions::default(),
            })
            .await
            .expect("search must not error");
        let search_ids: Vec<String> = hits.iter().map(|h| h.memory.id.0.to_string()).collect();

        // Rank of a uuid in the read response / search response (1-based).
        let read_rank = |uuid: &str| read_ids.iter().position(|r| *r == uuid).map(|p| p + 1);
        let search_rank = |uuid: &str| search_ids.iter().position(|r| r == uuid).map(|p| p + 1);

        // ── recall-query scoring ──
        let mut detail = String::new();
        for key in &q.must_surface {
            read_surface_total += 1;
            if let Some(uuid) = resolve(key) {
                let rr = read_rank(&uuid);
                let sr = search_rank(&uuid);
                if rr.is_some() {
                    read_surface_pass += 1;
                }
                if rr == Some(1) {
                    read_target_top1 += 1;
                }
                if let Some(r) = sr {
                    if r <= 5 {
                        search_top5 += 1;
                    }
                    if r <= 20 {
                        search_top20 += 1;
                    }
                }
                detail.push_str(&format!(
                    "\n    target {key}: read_rank={} search_rank={}",
                    rr.map(|r| r.to_string())
                        .unwrap_or_else(|| "MISSING".into()),
                    sr.map(|r| r.to_string())
                        .unwrap_or_else(|| "MISSING".into()),
                ));
            } else {
                detail.push_str(&format!("\n    target {key}: UNRESOLVED"));
            }
        }
        for key in &q.must_exclude {
            if let Some(uuid) = resolve(key) {
                if let Some(r) = read_rank(&uuid) {
                    near_miss_leaks += 1;
                    detail.push_str(&format!(
                        "\n    near-miss {key} LEAKED into read at rank {r} (recall-first: agent's call, not a hard fail)"
                    ));
                }
            }
        }

        let verdict = match (q.abstain, resp.abstain) {
            (false, false) => "answer ✅",
            (true, true) => "abstain ✅",
            (false, true) => "FALSE-ABSTAIN ❌ (cliff)",
            (true, false) => "FALSE-ANSWER ❌ (over-correction)",
        };
        println!(
            "[{}] ({}) {:?}\n    expect abstain={} got abstain={} → {} | read returned {} fact(s), search returned {}{}",
            q.id,
            q.kind,
            q.query_text,
            q.abstain,
            resp.abstain,
            verdict,
            resp.relevant_facts.len(),
            hits.len(),
            detail,
        );
        // Print the EXACT facts the vault hands the agent — this is the agent's
        // input for the live "what does Claude say?" test (Decision A, the
        // salary->job / cat->dog no-answer cases especially).
        for (i, f) in resp.relevant_facts.iter().enumerate() {
            println!("        fact[{i}]: {:?}", f.fact);
        }
    }

    // ---- aggregate scorecard ----
    println!("\n---------------- SCORECARD ----------------");
    println!("READ abstention confusion matrix:");
    println!("  correct-answer  (expect answer, got answer) : {correct_answer}");
    println!("  correct-abstain (expect abstain, got abstain): {correct_abstain}");
    println!("  FALSE-ABSTAIN   (expect answer, got abstain) : {false_abstain}   <-- the A7 cliff");
    println!("  FALSE-ANSWER    (expect abstain, got answer) : {false_answer}   <-- over-correction (the -2.5 floor's job at scale)");
    println!("READ recall  : {read_surface_pass}/{read_surface_total} targets surfaced; {read_target_top1}/{read_surface_total} at read rank 1");
    println!("SEARCH recall: {search_top5}/{read_surface_total} @top-5 ; {search_top20}/{read_surface_total} @top-20");
    println!("near-miss leaks into read: {near_miss_leaks} (diagnostic, not a gate)");
    println!("===========================================\n");

    // Characterization assert — every query scored exactly once. Threshold
    // gates come in a later commit once we have calibrated the -2.5 floor at
    // scale against this scorecard.
    let scored = correct_answer + correct_abstain + false_abstain + false_answer;
    assert_eq!(
        scored,
        queries.len(),
        "every fixture query must be scored exactly once"
    );
}

// ---------------------------------------------------------------------------
// Fast diagnostic — subject-frame retrieval-depth probe.
//
// The full scorecard (2026-06-04) showed the subject-LESS cello fact
// ("Plays the cello…") falling out of BOTH read and search top-20 at scale=100:
// BGE-small embeds the raw text and ranks it below the DEFAULT_MAX_CANDIDATES=20
// candidate cap, so the reranker (the relevance authority, which DOES apply
// DOC_SUBJECT_FRAME) never sees it. This probe isolates the question that picks
// the fix: where does each recall target rank in the BGE pool RAW vs with the
// "The user — " subject frame applied to the DOCUMENT embedding? If framing
// lifts the deep targets above the cap, the fix is to frame the retrieval
// embedding (not just the reranker's). Pure cosine — no reranker, no DB, no
// cascade — so it runs in a couple of minutes, de-risking the expensive
// scorecard re-run.
//
// cargo test -p vault-app --test scale_eval subject_frame_depth_probe -- --ignored --nocapture
// ---------------------------------------------------------------------------

/// Cosine similarity. BGE outputs are L2-normalised; we normalise defensively.
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "real BGE subject-frame retrieval-depth probe; run with --ignored --nocapture"]
async fn subject_frame_depth_probe() {
    const FRAME: &str = "The user — ";

    let probe = BgeSmallProvider::open(
        &fixture(BGE_FIXTURE_REL, "model.onnx"),
        &fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        &ort_lib(),
    )
    .expect("open BGE probe");

    let bytes = std::fs::read(scale_fixture_path()).expect("read scale_eval.json");
    let value: Value = serde_json::from_slice(&bytes).expect("parse scale_eval.json");
    let planted = parse_planted(&value);
    let queries = parse_queries(&value);
    let scale = value.get("scale").and_then(Value::as_u64).unwrap_or(100) as usize;

    // Pool = planted contents + generated distractors (same as the harness).
    let mut pool: Vec<String> = planted.iter().map(|p| p.content.clone()).collect();
    let distractor_count = scale.saturating_sub(planted.len());
    for d in generate_distractors(distractor_count, DISTRACTOR_SEED) {
        pool.push(d.content);
    }

    // Embed the whole pool RAW and FRAMED once.
    let mut raw_emb: Vec<Vec<f32>> = Vec::with_capacity(pool.len());
    let mut framed_emb: Vec<Vec<f32>> = Vec::with_capacity(pool.len());
    for c in &pool {
        raw_emb.push(probe.embed(c).await.expect("embed raw doc"));
        framed_emb.push(
            probe
                .embed(&format!("{FRAME}{c}"))
                .await
                .expect("embed framed doc"),
        );
    }

    // Rank a specific content string in a pool-embedding set against a query
    // embedding; return (1-based rank, cosine).
    let rank_of = |q_emb: &[f32], pool_emb: &[Vec<f32>], content: &str| -> Option<(usize, f32)> {
        let idx = pool.iter().position(|p| p == content)?;
        let mut scored: Vec<(usize, f32)> = pool_emb
            .iter()
            .enumerate()
            .map(|(i, e)| (i, cosine(q_emb, e)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
            .iter()
            .position(|(i, _)| *i == idx)
            .map(|rk| (rk + 1, scored[rk].1))
    };

    println!("\n===== SUBJECT-FRAME RETRIEVAL-DEPTH PROBE =====");
    println!(
        "pool={} facts ; candidate cap today = DEFAULT_MAX_CANDIDATES = {}",
        pool.len(),
        DEFAULT_MAX_CANDIDATES
    );
    println!("(lower rank = better; a target ranked > {DEFAULT_MAX_CANDIDATES} never reaches the reranker)\n");

    let mut raw_in_cap = 0usize;
    let mut framed_in_cap = 0usize;
    let mut total = 0usize;
    for q in &queries {
        for key in &q.must_surface {
            let Some(content) = planted.iter().find(|p| p.id == *key).map(|p| &p.content) else {
                continue;
            };
            total += 1;
            let q_emb = probe.embed(&q.query_text).await.expect("embed query");
            let raw = rank_of(&q_emb, &raw_emb, content);
            let framed = rank_of(&q_emb, &framed_emb, content);
            let in_cap = |r: &Option<(usize, f32)>| {
                r.map(|(rk, _)| rk <= DEFAULT_MAX_CANDIDATES)
                    .unwrap_or(false)
            };
            if in_cap(&raw) {
                raw_in_cap += 1;
            }
            if in_cap(&framed) {
                framed_in_cap += 1;
            }
            let fmt = |r: Option<(usize, f32)>| {
                r.map(|(rk, c)| format!("rank {rk} (cos {c:.3})"))
                    .unwrap_or_else(|| "not found".into())
            };
            println!(
                "[{}] {key}\n    RAW   : {}\n    FRAMED: {}",
                q.id,
                fmt(raw),
                fmt(framed)
            );
        }
    }

    println!("\n---------------- SUMMARY ----------------");
    println!("targets within the top-{DEFAULT_MAX_CANDIDATES} candidate cap:");
    println!("  RAW    embedding: {raw_in_cap}/{total}");
    println!("  FRAMED embedding: {framed_in_cap}/{total}   <-- if higher, framing the retrieval embedding is the fix");
    println!("=========================================\n");

    assert_eq!(total, 10, "expected 10 recall targets in the fixture");
}
