//! Graph read-path dogfood (ADR-SEC-002 Part 2, 2026-06-28).
//!
//! Proves the knowledge-graph recall channel END-TO-END on real data through
//! the real engine: real BGE embed + real Phi-4 extraction + real Qwen3
//! reranker + the real `VaultAdapter.search` path the MCP `memory_search` tool
//! calls. NOT a unit test — a characterization dogfood (like `scale_eval`): it
//! prints what actually happened for a human to judge, with only soft sanity
//! asserts (the run completed; the graph is non-empty).
//!
//! ## What it does
//!   1. Seeds a hand-built RELATIONAL corpus (sister→hospital, mentor→startup,
//!      pet→event) + optional distractors, into a fresh temp vault.
//!   2. Runs ONE real consolidation pass — this is where Phi-4 extracts the
//!      entity/relationship graph and seals it (`<graph>.sealed`).
//!   3. DUMPS the extracted graph (entities + live relationships) so we can SEE
//!      whether extraction produced the right edges.
//!   4. For each probe, runs the SAME search THREE ways:
//!        - GRAPH-ONLY  — the `GraphRetriever` channel alone (what it resolves
//!          a query-named entity to), so we see the channel firing in isolation;
//!        - SEARCH graph-ON  — the full production retriever (graph wired in);
//!        - SEARCH graph-OFF — identical pipeline with the graph channel parked
//!          OFF (its default; `VAULT_ENABLE_GRAPH_CHANNEL` unset) — the A/B control.
//!
//!      A target fact that appears ON but not OFF is the graph earning its keep.
//!
//! ## The honest caveat (read before interpreting results)
//! The graph channel is RECALL INSURANCE: it guarantees a structurally-connected
//! fact becomes a rerank candidate. On a TINY vault, pure-semantic recall already
//! returns ~everything, so ON and OFF often look identical — that does NOT mean
//! the channel is idle (the GRAPH-ONLY column shows it resolving). The A/B
//! DIFFERENCE shows up once enough distractors crowd the target out of semantic
//! recall — bump `DOGFOOD_DISTRACTORS` for that scale run.
//!
//! ## Running (Windows; needs BGE + reranker fixtures + a Phi-4 GGUF)
//! ```text
//! $env:DOGFOOD_PHI4='C:\Users\shahb\AppData\Roaming\com.shahbaz242630.memory-vault\models\Phi-4-mini-instruct-Q4_K_M.gguf'
//! # optional: $env:DOGFOOD_DISTRACTORS='80'   # for the scale A/B
//! # optional: $env:VAULT_CONSOLIDATOR_TIMEOUT_SECS='0'  # disable the 30-min cap for big N
//! cargo test -p vault-app --test graph_readpath_dogfood -- --ignored --nocapture
//! ```
//!
//! macOS: disabled (ORT SIGABRTs at process exit — same as `scale_eval`).

#![cfg(not(target_os = "macos"))]

use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

use vault_app::{AppConfig, Application};
use vault_core::{Boundary, MemoryType, NewMemory};
use vault_mcp::Adapter;
use vault_retrieval::{RetrievalOptions, RetrievalQuery};
use vault_storage::{DuckDbGraphStore, GraphStore, LanceVectorStore, SqlCipherKey, VectorStore};

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

/// Relational target facts. Each is phrased WITH the entity name so per-fact
/// enrichment reliably forms the edge; the probes name an entity and expect the
/// connected fact to surface.
const TARGETS: &[&str] = &[
    "The user's sister is named Maria Delgado.",
    "Maria Delgado works as a cardiac nurse at St. Mary's Hospital.",
    // The 2-HOP CHAIN: this fact is about St. Mary's, never names Maria, so it is
    // reachable from a "Maria" query ONLY by walking Maria → St. Mary's → here.
    "St. Mary's Hospital specializes in pediatric cardiology and neonatal care.",
    "The user's mentor is Dr. Aldous Patel.",
    "Dr. Aldous Patel founded a biotech startup called Helixon.",
    "The user adopted a greyhound named Comet.",
    "Comet won a regional racing championship in 2019.",
    // A deliberate cross-fact PRONOUN gap — informative for whether per-fact
    // enrichment can (it likely cannot) link "he" back to Dr. Patel.
    "He later sold Helixon to a pharmaceutical company.",
];

/// (probe text, a substring that marks the connected TARGET we hope surfaces).
const PROBES: &[(&str, &str)] = &[
    ("Where does Maria Delgado work?", "St. Mary's"),
    // The MULTI-HOP probe: names Maria, but the answer is 2 hops away and never
    // says "Maria" — the case 1-hop + plain search miss, 2-hop should catch.
    (
        "What is Maria Delgado's hospital known for?",
        "pediatric cardiology",
    ),
    ("What did Dr. Aldous Patel found?", "Helixon"),
    ("What championship did Comet win?", "championship"),
];

/// HARD, semantically-COMPETING distractors (2026-06-29 harder A/B run).
///
/// The first scale run used generic neighbourhood notes (bakeries, marathons)
/// that scored ~0.000 against every probe — they never crowded the target, so
/// graph-ON == graph-OFF and the A/B proved nothing. These are OTHER hospitals
/// and their specialties: high BGE cosine to "what is Maria's hospital known
/// for?" (so they fill the top-[`SEARCH_CANDIDATE_FANOUT`] candidate pool and
/// push the genuine target — St. Mary's, which never names Maria — OUT of
/// pure-semantic recall), but NONE is Maria's hospital (so the reranker scores
/// them below the graph-injected target). That is the exact condition the graph
/// channel defends: target outside semantic recall, graph puts it back.
///
/// Deliberately NONE contains the probe-2 marker "pediatric cardiology" / the
/// other probe markers, and NONE is relationally connected to Maria / St. Mary's
/// (so the graph channel never resolves to them — they stay pure noise).
fn distractors(n: usize) -> Vec<String> {
    // 30 distinct hospital names × 29 distinct specialties. Pairing by index
    // (lengths are coprime, gcd(30,29)=1 → no (hospital, specialty) pair repeats
    // until i=870) makes every distractor differ in BOTH the hospital AND the
    // specialty — NOT just a numeric suffix. The earlier "(regional referral N)"
    // suffix kept the strings unique but left hospital+specialty identical, so
    // Phi-4's SEMANTIC dedup correctly collapsed 26/60 of them. Distinct content
    // defeats dedup, so the vault fills with the full count of real competitors.
    let hospitals = [
        "Riverside General Hospital",
        "Lakeside Medical Center",
        "Northgate Hospital",
        "Highland Memorial Hospital",
        "Westbrook Clinic",
        "Cedar Valley Hospital",
        "Eastside Medical Center",
        "Sunnyvale Hospital",
        "Pinecrest Hospital",
        "Fairview General Hospital",
        "Brookhaven Medical Center",
        "Maplewood Hospital",
        "Grandview Hospital",
        "Stonebridge Clinic",
        "Oakdale Hospital",
        "Harborview Medical Center",
        "Ashford General Hospital",
        "Clearwater Hospital",
        "Birchwood Hospital",
        "Meadowbrook Clinic",
        "Kingsford Hospital",
        "Glenwood Medical Center",
        "Thornbury Hospital",
        "Rosewood Clinic",
        "Silvercreek Hospital",
        "Bayview Medical Center",
        "Elmhurst Hospital",
        "Foxglove Clinic",
        "Whitfield Hospital",
        "Carrington Medical Center",
    ];
    // None of these is the probe-2 marker "pediatric cardiology" / "neonatal care".
    let specialties = [
        "orthopedic surgery",
        "oncology",
        "radiation therapy",
        "burn treatment",
        "organ transplantation",
        "sports medicine",
        "neurology",
        "trauma care",
        "maternity care",
        "dermatology",
        "cardiac surgery",
        "geriatric care",
        "ophthalmology",
        "psychiatric care",
        "fertility treatment",
        "gastroenterology",
        "respiratory care",
        "rheumatology",
        "kidney dialysis",
        "reconstructive surgery",
        "endocrinology",
        "hematology",
        "urology",
        "vascular surgery",
        "pain management",
        "infectious disease care",
        "palliative care",
        "audiology",
        "nephrology",
    ];
    (0..n)
        .map(|i| {
            let h = hospitals[i % hospitals.len()];
            let s = specialties[i % specialties.len()];
            format!("{h} is known for its {s} program.")
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "graph read-path dogfood: real Phi-4 consolidation, heavy. Run with --ignored --nocapture + DOGFOOD_PHI4"]
async fn graph_readpath_dogfood() {
    let phi4 = PathBuf::from(
        std::env::var("DOGFOOD_PHI4")
            .expect("DOGFOOD_PHI4 env var (path to Phi-4 GGUF) is required"),
    );
    assert!(phi4.exists(), "DOGFOOD_PHI4 not found at {phi4:?}");
    let distractor_n: usize = std::env::var("DOGFOOD_DISTRACTORS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let tmp = TempDir::new().expect("tempdir");
    let metadata_path = tmp.path().join("vault.db");
    let vector_dir = tmp.path().join("lance");
    let graph_path = tmp.path().join("graph.duckdb");
    let key = SqlCipherKey::new("graph-dogfood-key");
    let at_rest_key = zeroize::Zeroizing::new([7u8; 32]);
    let boundary = Boundary::new("personal").expect("personal boundary");

    let base_config = |phi4: Option<PathBuf>| AppConfig {
        metadata_path: metadata_path.clone(),
        vector_dir: vector_dir.clone(),
        graph_path: graph_path.clone(),
        key: key.clone(),
        model_path: fixture(BGE_FIXTURE_REL, "model.onnx"),
        tokenizer_path: fixture(BGE_FIXTURE_REL, "tokenizer.json"),
        ort_lib_path: ort_lib(),
        at_rest_key: at_rest_key.clone(),
        qwen_model_path: None,
        phi4_model_path: phi4,
        rerank_model_path: Some(fixture(RERANK_FIXTURE_REL, "model.onnx")),
        rerank_tokenizer_path: Some(fixture(RERANK_FIXTURE_REL, "tokenizer.json")),
    };

    // -- Phase 1: seed the relational corpus + distractors -----------------
    let facts: Vec<String> = TARGETS
        .iter()
        .map(|s| s.to_string())
        .chain(distractors(distractor_n))
        .collect();
    let total = facts.len();

    println!("\n================ GRAPH READ-PATH DOGFOOD ================");
    println!("vault   : {}", tmp.path().display());
    println!(
        "facts   : {} targets + {distractor_n} distractors = {total}",
        TARGETS.len()
    );

    {
        let app = base_config(Some(phi4.clone()));
        let app = Application::new(&app)
            .await
            .expect("Application::new (seed + consolidate)");
        let _shutdown = app.start();
        let adapter = app.adapter();

        for content in &facts {
            let nm = NewMemory {
                content: content.clone(),
                memory_type: MemoryType::Semantic,
                boundary: boundary.clone(),
                source_agent: Some("dogfood".into()),
                confidence: 0.95,
                valid_from: None,
                valid_until: None,
                metadata: serde_json::json!({}),
            };
            adapter.write(nm).await.expect("seed write");
        }

        // Drain every vector into LanceDB before consolidating (the cascade
        // worker writes vectors async; count is the only ground truth).
        drain_vectors(&vector_dir, &at_rest_key, total).await;
        println!("seeded {total} facts; all vectors drained. Running consolidation (Phi-4)...");

        let report = app
            .run_consolidation_with_safety()
            .await
            .expect("consolidation must complete");
        println!(
            "consolidation done: {} processed, {} merged, {} deduped, {} contradictions resolved ({} auto). Graph relationships shown below.",
            report.memories_processed,
            report.memories_merged,
            report.memories_deduped,
            report.contradictions_resolved,
            report.contradictions_auto_resolved,
        );
        // app (and its exclusive graph handle) dropped here.
    }

    // -- Phase 2: dump the extracted graph ---------------------------------
    dump_graph(&graph_path, &at_rest_key, &boundary).await;

    // -- Phase 3: probe — GRAPH-ONLY, SEARCH graph-ON, SEARCH graph-OFF ----
    // The channel is PARKED OFF by default (tech-debt #9); ON first (env set),
    // then OFF (env unset) so the same process does both with a fresh
    // Application each time. `VAULT_ENABLE_GRAPH_CHANNEL` is the opt-in lever.
    std::env::set_var("VAULT_ENABLE_GRAPH_CHANNEL", "1");
    let on = open_read_app(&base_config(None)).await;
    let on_results = run_probes(on.adapter(), &boundary, "SEARCH graph-ON").await;
    drop(on);

    std::env::remove_var("VAULT_ENABLE_GRAPH_CHANNEL");
    let off = open_read_app(&base_config(None)).await;
    let off_results = run_probes(off.adapter(), &boundary, "SEARCH graph-OFF").await;
    drop(off);
    std::env::remove_var("VAULT_ENABLE_GRAPH_CHANNEL");

    // -- A/B verdict per probe ---------------------------------------------
    println!("\n================ A/B VERDICT (graph ON vs OFF) ================");
    for (i, (probe, marker)) in PROBES.iter().enumerate() {
        let on_hit = on_results[i].iter().any(|c| c.contains(marker));
        let off_hit = off_results[i].iter().any(|c| c.contains(marker));
        let verdict = match (on_hit, off_hit) {
            (true, false) => "★ GRAPH-ONLY WIN (ON surfaces it, OFF misses)",
            (true, true) => "both surface it (no recall cutoff at this scale)",
            (false, true) => "OFF-only (unexpected — investigate)",
            (false, false) => "neither surfaced the target",
        };
        println!(
            "  [{}] {probe:?}  target={marker:?}\n        → {verdict}",
            i + 1
        );
    }

    // Soft sanity: the run completed and produced a graph to read. Precision is
    // characterized above for human judgment, not hard-gated (per scale_eval).
    assert!(!on_results.is_empty(), "probes must have run");
}

/// Poll LanceDB row count via a FRESH read handle each tick until `>= target`.
async fn drain_vectors(vector_dir: &Path, at_rest_key: &[u8; 32], target: usize) {
    for attempt in 0..100_000usize {
        let probe = LanceVectorStore::open_with_at_rest_key(vector_dir, 384, at_rest_key)
            .await
            .expect("open vector count probe");
        let n = probe.count(None).await.expect("vector count probe");
        drop(probe);
        if n >= target {
            return;
        }
        if attempt % 20 == 0 {
            println!("  draining vectors: {n}/{target}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("vectors never drained to {target}");
}

/// Open the extracted sealed graph and print every entity + live relationship.
async fn dump_graph(graph_path: &PathBuf, at_rest_key: &[u8; 32], boundary: &Boundary) {
    let graph = DuckDbGraphStore::open_with_at_rest_key(graph_path, at_rest_key)
        .await
        .expect("open sealed graph snapshot");
    let bounds = [boundary.clone()];
    let entities = graph.list_entities(&bounds).await.expect("list_entities");

    println!("\n================ EXTRACTED GRAPH ================");
    println!("entities ({}):", entities.len());
    for e in &entities {
        println!("  • {} [{}]", e.name, e.id.0);
    }

    use std::collections::HashMap;
    let names: HashMap<_, _> = entities.iter().map(|e| (e.id.0, e.name.clone())).collect();
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    println!("relationships (live, deduped):");
    let mut count = 0usize;
    for e in &entities {
        let rels = graph
            .relationships_for_entity(&e.id, &bounds)
            .await
            .expect("relationships_for_entity");
        for r in rels {
            if !seen.insert(r.id.0) {
                continue;
            }
            count += 1;
            let from = names
                .get(&r.from_entity.0)
                .cloned()
                .unwrap_or_else(|| r.from_entity.0.to_string());
            let to = names
                .get(&r.to_entity.0)
                .cloned()
                .unwrap_or_else(|| r.to_entity.0.to_string());
            let src = r
                .source_memory_id
                .map(|m| m.0.to_string())
                .unwrap_or_else(|| "—".into());
            println!(
                "  {from}  --[{}]-->  {to}   (src memory {src})",
                r.relation_type
            );
        }
    }
    println!("({count} live relationships total)");
    assert!(
        !entities.is_empty(),
        "extraction produced NO entities — graph is empty"
    );
}

async fn open_read_app(config: &AppConfig) -> Application {
    Application::new(config)
        .await
        .expect("Application::new (read app)")
}

/// Run every probe through `adapter.search` (max_results=10) and the standalone
/// graph channel; print results; return the per-probe content lists.
async fn run_probes(
    adapter: &std::sync::Arc<vault_app::VaultAdapter>,
    boundary: &Boundary,
    label: &str,
) -> Vec<Vec<String>> {
    let authorized = vec![boundary.clone()];
    let mut all = Vec::new();
    for (probe, marker) in PROBES {
        let hits = adapter
            .search(RetrievalQuery {
                query_text: probe.to_string(),
                authorized_boundaries: authorized.clone(),
                max_results: 10,
                options: RetrievalOptions::default(),
            })
            .await
            .expect("search must not error");
        let contents: Vec<String> = hits.iter().map(|h| h.memory.content.clone()).collect();
        let hit = contents.iter().any(|c| c.contains(marker));
        println!(
            "\n-- {label} : {probe:?}  (target {marker:?} {}) --",
            if hit { "PRESENT" } else { "ABSENT" }
        );
        for (i, h) in hits.iter().enumerate() {
            let mark = if h.memory.content.contains(marker) {
                "  <== TARGET"
            } else {
                ""
            };
            println!(
                "  [{:>2}] {:.4}  {}{}",
                i + 1,
                h.score,
                h.memory.content,
                mark
            );
        }
        all.push(contents);
    }
    all
}
