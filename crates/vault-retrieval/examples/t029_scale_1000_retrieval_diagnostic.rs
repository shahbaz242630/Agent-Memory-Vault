//! T0.2.7 Phase 5 Step 2 diagnostic — answer "is the expected contradiction
//! pair actually in retrieval scope at SCALE=1000 on the bulk_upsert path?"
//!
//! Surfaced 2026-05-22 when SCALE=1000 came back 7/9 (Q25 contradiction +
//! S2 short-long both FAILed with `flagged=1, prose=false, structured=false`)
//! after passing 9/9 at SCALE=100 on the same bulk_upsert path. Two
//! candidate hypotheses:
//!
//! 1. **Retrieval issue.** Bulk_upsert changed LanceDB fragment layout
//!    in a way that perturbs top-K ordering vs the previous (per-row)
//!    9/9 run. The expected contradiction-pair memories don't make it
//!    into the top-20 surfaced to the LLM at SCALE=1000.
//!
//! 2. **LLM noise.** Vulkan parallel reductions or model-side variance
//!    crossed a behavior threshold at scale; retrieval is fine but
//!    Qwen output the wrong literals.
//!
//! This spike isolates hypothesis 1: **skip the LLM entirely, run only
//! retrieval against the same SCALE=1000 corpus + bulk_upsert path**, and
//! print the top-20 candidate set for every gauntlet query. Manual
//! inspection of Q25 + S2 reveals whether the expected literal-bearing
//! memories are in retrieval scope.
//!
//! # Diagnostic verdict
//!
//! For each query the spike prints whether both expected literal
//! substrings (per `structural_substrings`) are found among the top-20
//! retrieved memories' content. Two channels checked independently:
//!
//! - **Both literals present in top-20 content** → retrieval is correct;
//!   if the test FAILed, it's hypothesis 2 (LLM noise).
//! - **At least one literal missing from top-20 content** → retrieval
//!   itself dropped the contradiction pair; hypothesis 1 confirmed.
//!
//! # Running
//!
//! ```text
//! cargo run -p vault-retrieval --example t029_scale_1000_retrieval_diagnostic --release
//! ```
//!
//! Expected wall ~10-15 min (no LLM): ~30s release relink + ~5 min BGE
//! embedding generation for 1000 memories + ~5s bulk_upsert + ~2 min
//! Tantivy index + 9 × ~1s retrieval calls.

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, ensure, Context, Result};
use serde::Deserialize;

use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_retrieval::{
    AbstainingRetriever, HybridRetriever, KeywordIndex, KeywordRetriever, RetrievalOptions,
    RetrievalQuery, RetrievedMemory, Retriever, SemanticRetriever,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];
const DISTRACTOR_SEED: u64 = 0x5CA1_EACD_EED0;
const SCALE: usize = 1000;
const TOP_K: usize = 20;
const SNIPPET_CHARS: usize = 140;

// ── Same query IDs + structural substrings as the scale acceptance test ──

const PRODUCTION_QUERY_IDS: &[&str] = &["Q11", "Q13", "Q25", "Q26", "Q21", "Q22", "S1", "S2", "S3"];
const SHORT_LONG_QUERY_IDS: &[&str] = &["S1", "S2", "S3"];
const HARD_NEG_QUERY_IDS: &[&str] = &["Q21", "Q22"];

fn structural_substrings(query_id: &str) -> Option<(&'static str, &'static str)> {
    match query_id {
        "Q11" | "Q25" => Some(("Q1 2027", "Q2 2027")),
        "Q13" | "Q26" => Some(("89", "109")),
        "S1" => Some(("$89", "$145")),
        "S2" => Some(("Q1 2028", "Q3 2028")),
        "S3" => Some(("$2,500", "$4,200")),
        _ => None,
    }
}

// ── Short-long pairs (verbatim port from the scale-acceptance test) ──────

struct ShortLongPair {
    query_id: &'static str,
    boundary: &'static str,
    short_content: &'static str,
    long_content: &'static str,
    query_text: &'static str,
}

const SHORT_LONG_PAIRS: &[ShortLongPair] = &[
    ShortLongPair {
        query_id: "S1",
        boundary: "work",
        short_content: "Ergonomic mouse approved at $89/unit in the Q1 hardware budget.",
        long_content: "Hardware procurement reconciliation note from finance: \
            the actual purchase of the ergonomic mouse came in at $145/unit \
            instead of the approved $89/unit. The $56-per-unit overage \
            multiplied across the 24-seat order produces a $1,344 variance \
            against the Q1 hardware budget line item. Finance is asking us \
            to absorb the variance out of the Q2 contingency reserve rather \
            than re-open the Q1 budget for a single line item revision.",
        query_text:
            "What was the approved budget for our ergonomic mouse versus what we actually paid?",
    },
    ShortLongPair {
        query_id: "S2",
        boundary: "work",
        short_content: "PostgreSQL upgrade target Q1 2028.",
        long_content: "Long-form retrospective from the database platform \
            sync this morning. Headline outcome: the PostgreSQL upgrade is \
            pushed to Q3 2028 instead of the originally-circulated earlier \
            target. The driver for the push was the discovery during \
            integration testing that two of our older internal services \
            depend on a deprecated extension that has no direct replacement \
            in the newer major version; the platform team needs an extra \
            two quarters to either rewrite those services or vendor a \
            compatibility shim.",
        query_text: "When are we doing the PostgreSQL upgrade?",
    },
    ShortLongPair {
        query_id: "S3",
        boundary: "work",
        short_content: "Office Wi-Fi vendor budget: $2,500/mo.",
        long_content: "Facilities-operations rollup for the second half of \
            the year covering the workplace-tech vendor consolidation we \
            kicked off in March. Headline change relevant to the IT budget: \
            office Wi-Fi vendor renewed at $4,200 per month after a \
            negotiation cycle that lasted six weeks.",
        query_text: "What's our office Wi-Fi monthly cost?",
    },
];

// ── Distractor generation (verbatim port from scale-acceptance test) ─────

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

struct DistractorEntry {
    boundary: &'static str,
    content: String,
}

fn generate_distractors(count: usize, seed: u64) -> Vec<DistractorEntry> {
    let mut rng = SplitMix64::new(seed);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let template = rng.pick(DISTRACTOR_TEMPLATES);
        let boundary = *rng.pick(DISTRACTOR_BOUNDARIES);
        let content = render_template(template, &mut rng);
        out.push(DistractorEntry { boundary, content });
    }
    out
}

// ── Base fixture loader + query types (shared shape) ─────────────────────

#[derive(Debug, Clone, Deserialize)]
struct MemoryFixtureEntry {
    id: String,
    boundary: String,
    #[allow(dead_code)]
    topic_label: String,
    content: String,
    #[allow(dead_code)]
    ground_truth: GroundTruth,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GroundTruth {
    outcome: String,
    cluster: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct QuerySet {
    queries: Vec<QueryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct QueryEntry {
    id: String,
    #[allow(dead_code)]
    shape: String,
    #[allow(dead_code)]
    length_tier: String,
    query_text: String,
    authorized_boundaries: Vec<String>,
    #[allow(dead_code)]
    expected_memory_ids: Vec<String>,
    #[allow(dead_code)]
    notes: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let started = chrono::Utc::now();
    println!(
        "T0.2.7 t029 retrieval-only diagnostic — SCALE={SCALE} — started {}",
        started.format("%Y-%m-%d %H:%M:%S UTC")
    );

    // ── Stores + embedder ───────────────────────────────────────────────
    let dir = tempfile::tempdir()?;
    let key = SqlCipherKey::new("t029-passphrase");
    let metadata = MetadataStore::open(dir.path().join("metadata.db"), key).await?;
    let metadata = Arc::new(metadata);
    let vectors_raw = LanceVectorStore::open_with_at_rest_key(
        &dir.path().join("vectors"),
        EMBEDDING_DIM,
        &TEST_AT_REST_KEY,
    )
    .await?;
    let vectors: Arc<dyn VectorStore> = Arc::new(vectors_raw);

    println!("Opening BgeSmallProvider...");
    let bge = open_bge_provider()?;

    // ── Load base fixture + query fixture ────────────────────────────────
    let base_path = repo_root()?
        .join("crates")
        .join("vault-consolidator")
        .join("tests")
        .join("fixtures")
        .join("merge_acceptance_100.json");
    let mut base_fixture: Vec<MemoryFixtureEntry> =
        serde_json::from_slice(&std::fs::read(&base_path)?)?;
    base_fixture.sort_by(|a, b| a.id.cmp(&b.id));
    let base_count = base_fixture.len();
    let pair_member_count = SHORT_LONG_PAIRS.len() * 2;
    let min_total = base_count + pair_member_count;
    let distractor_count = SCALE.saturating_sub(min_total);
    let total_target = min_total + distractor_count;
    println!(
        "Corpus plan: base={base_count} + short-long pair members={pair_member_count} + \
         distractors={distractor_count} → total={total_target}"
    );

    let query_fixture_path = vault_retrieval_root()
        .join("test-fixtures")
        .join("merge_acceptance_100_queries.json");
    let query_set: QuerySet = serde_json::from_slice(&std::fs::read(&query_fixture_path)?)?;
    let mut production_queries: Vec<QueryEntry> = Vec::with_capacity(PRODUCTION_QUERY_IDS.len());
    for wanted in PRODUCTION_QUERY_IDS {
        if SHORT_LONG_QUERY_IDS.contains(wanted) {
            let pair = SHORT_LONG_PAIRS
                .iter()
                .find(|p| &p.query_id == wanted)
                .with_context(|| format!("short-long pair {wanted} missing"))?;
            production_queries.push(QueryEntry {
                id: pair.query_id.to_string(),
                shape: "short-long".to_string(),
                length_tier: "mixed".to_string(),
                query_text: pair.query_text.to_string(),
                authorized_boundaries: vec![pair.boundary.to_string()],
                expected_memory_ids: Vec::new(),
                notes: String::new(),
            });
        } else {
            let q = query_set
                .queries
                .iter()
                .find(|q| q.id == *wanted)
                .cloned()
                .with_context(|| format!("target {wanted} missing from query fixture"))?;
            production_queries.push(q);
        }
    }

    // ── Insertion via bulk_upsert (matches the failed scale-acceptance run) ─
    println!("Computing embeddings + metadata for {total_target} memories...");
    let embed_t0 = Instant::now();
    let mut all_memories: Vec<Memory> = Vec::with_capacity(total_target);
    let mut vector_rows = Vec::with_capacity(total_target);

    for entry in &base_fixture {
        let memory = build_memory(&entry.boundary, &entry.content)?;
        let embedding = bge.embed(&entry.content).await?;
        metadata.create_memory(&memory).await?;
        vector_rows.push((memory.id, embedding, memory.boundary.clone()));
        all_memories.push(memory);
    }
    for pair in SHORT_LONG_PAIRS {
        for content in [pair.short_content, pair.long_content] {
            let memory = build_memory(pair.boundary, content)?;
            let embedding = bge.embed(content).await?;
            metadata.create_memory(&memory).await?;
            vector_rows.push((memory.id, embedding, memory.boundary.clone()));
            all_memories.push(memory);
        }
    }
    if distractor_count > 0 {
        let distractors = generate_distractors(distractor_count, DISTRACTOR_SEED);
        for d in &distractors {
            let memory = build_memory(d.boundary, &d.content)?;
            let embedding = bge.embed(&d.content).await?;
            metadata.create_memory(&memory).await?;
            vector_rows.push((memory.id, embedding, memory.boundary.clone()));
            all_memories.push(memory);
        }
    }
    println!(
        "Embeddings + metadata complete in {:.1}s for {} memories. Bulk-upserting to LanceDB...",
        embed_t0.elapsed().as_secs_f64(),
        all_memories.len()
    );
    let bulk_t0 = Instant::now();
    vectors.bulk_upsert(&vector_rows).await?;
    println!(
        "bulk_upsert: {} rows in {:?}",
        vector_rows.len(),
        bulk_t0.elapsed()
    );

    // ── Production retriever stack (same composition as scale acceptance) ─
    let semantic: Arc<dyn Retriever> = Arc::new(SemanticRetriever::new(
        metadata.clone(),
        bge.clone(),
        vectors.clone(),
    ));
    let keyword_index = Arc::new(KeywordIndex::new()?);
    keyword_index.bulk_insert(&all_memories).await?;
    println!("KeywordIndex bulk-loaded {} memories", all_memories.len());
    let keyword: Arc<dyn Retriever> = Arc::new(KeywordRetriever::new(
        keyword_index.clone(),
        metadata.clone(),
    ));
    let hybrid: Arc<dyn Retriever> = Arc::new(HybridRetriever::new(semantic, keyword.clone()));
    let retriever: Arc<dyn Retriever> = Arc::new(AbstainingRetriever::new(hybrid, keyword));

    // ── Per-query retrieval, top-K print, and structural-substring check ─
    println!("\n=== Retrieval-only diagnostic (no LLM) ===\n");
    let mut diagnostic_results: Vec<(String, bool, usize, usize)> = Vec::new(); // (query_id, both_found, n_with_lit_a, n_with_lit_b)

    for q in &production_queries {
        let mut boundaries = Vec::with_capacity(q.authorized_boundaries.len());
        for b in &q.authorized_boundaries {
            boundaries.push(Boundary::new(b)?);
        }
        let rq = RetrievalQuery {
            query_text: q.query_text.clone(),
            authorized_boundaries: boundaries,
            max_results: TOP_K,
            options: RetrievalOptions::default(),
        };
        let retrieve_t0 = Instant::now();
        let hits: Vec<RetrievedMemory> = retriever.retrieve(rq).await?;
        let retrieve_elapsed = retrieve_t0.elapsed();

        let kind = if HARD_NEG_QUERY_IDS.contains(&q.id.as_str()) {
            "hard-negative"
        } else if SHORT_LONG_QUERY_IDS.contains(&q.id.as_str()) {
            "short-long"
        } else {
            "contradiction"
        };

        println!(
            "--- {} ({}) — \"{}\" — retrieved {} in {:?} ---",
            q.id,
            kind,
            truncate_for_display(&q.query_text, 80),
            hits.len(),
            retrieve_elapsed
        );

        let subs = structural_substrings(&q.id);
        let (mut n_a, mut n_b) = (0_usize, 0_usize);

        for (i, rm) in hits.iter().enumerate() {
            let content = &rm.memory.content;
            let (has_a, has_b) = match subs {
                Some((a, b)) => (content.contains(a), content.contains(b)),
                None => (false, false),
            };
            if has_a {
                n_a += 1;
            }
            if has_b {
                n_b += 1;
            }
            let mark = match (has_a, has_b) {
                (true, true) => " *BOTH*",
                (true, false) => " *A*",
                (false, true) => " *B*",
                (false, false) => "",
            };
            let id_str = rm.memory.id.0.to_string();
            let short = id_str.chars().take(8).collect::<String>();
            println!(
                "  [{:>2}] score={:.4} boundary={} id={}{}  | {}",
                i + 1,
                rm.score,
                rm.memory.boundary,
                short,
                mark,
                truncate_for_display(content, SNIPPET_CHARS)
            );
        }

        let both_found = match subs {
            Some(_) => n_a > 0 && n_b > 0,
            None => true,
        };
        if let Some((a, b)) = subs {
            let verdict = if both_found {
                "BOTH literals present in top-K (retrieval correct)"
            } else if n_a > 0 || n_b > 0 {
                "ONLY ONE literal present (retrieval partial)"
            } else {
                "NEITHER literal present (retrieval missed completely)"
            };
            println!(
                "  >>> structural check: a={:?} appears in {}/{} hits; b={:?} appears in {}/{} hits — {}",
                a,
                n_a,
                hits.len(),
                b,
                n_b,
                hits.len(),
                verdict
            );
        } else {
            println!("  >>> (hard-negative query — no structural substrings to check)");
        }
        println!();

        diagnostic_results.push((q.id.clone(), both_found, n_a, n_b));
    }

    println!("\n=== Diagnostic verdict per query ===");
    for (qid, both, na, nb) in &diagnostic_results {
        let subs = structural_substrings(qid);
        match subs {
            Some(_) => {
                let v = if *both { "PASS" } else { "FAIL" };
                println!("  {qid:<4} retrieval-{v}  (lit_a hits={na}, lit_b hits={nb})");
            }
            None => println!("  {qid:<4} (hard-negative — no structural check)"),
        }
    }

    Ok(())
}

fn truncate_for_display(s: &str, max_chars: usize) -> String {
    let trimmed: String = s.chars().take(max_chars).collect();
    let suffix = if s.chars().count() > max_chars {
        "…"
    } else {
        ""
    };
    let single_line = trimmed.replace(['\n', '\r'], " ");
    format!("{single_line}{suffix}")
}

fn build_memory(boundary: &str, content: &str) -> Result<Memory> {
    let boundary = Boundary::new(boundary)?;
    let memory = Memory::try_new(NewMemory {
        content: content.to_string(),
        memory_type: MemoryType::Semantic,
        boundary,
        source_agent: None,
        confidence: 0.9,
        valid_from: None,
        valid_until: None,
        metadata: serde_json::json!({}),
    })?;
    Ok(memory)
}

fn open_bge_provider() -> Result<Arc<dyn EmbeddingProvider>> {
    let fixture_root = vault_embedding_test_fixtures()?;
    let model = fixture_root.join("model.onnx");
    let tokenizer = fixture_root.join("tokenizer.json");
    let ort_lib = fixture_root.join("onnxruntime.dll");
    for p in [&model, &tokenizer, &ort_lib] {
        ensure!(p.exists(), "missing BGE fixture {p:?}");
    }
    let provider = BgeSmallProvider::open(&model, &tokenizer, &ort_lib)?;
    Ok(Arc::new(provider))
}

fn vault_retrieval_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> Result<PathBuf> {
    vault_retrieval_root()
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("no grandparent for vault-retrieval"))
}

fn vault_embedding_test_fixtures() -> Result<PathBuf> {
    let p = repo_root()?
        .join("crates")
        .join("vault-embedding")
        .join("test-fixtures")
        .join("bge-small-en-v1.5");
    ensure!(p.exists(), "bge fixtures missing at {p:?}");
    Ok(p)
}
