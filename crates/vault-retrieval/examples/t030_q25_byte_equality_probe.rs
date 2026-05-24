//! T0.2.7 Phase 5 Step 2 byte-equality probe — answer "is the variance
//! across SCALE=1000 Q25 runs coming from retrieval, prompt construction,
//! or the LLM?"
//!
//! Surfaced 2026-05-23 after the third SCALE=1000 acceptance run produced
//! a *third distinct LLM behavior* on Q25:
//!
//! | Run | Path | Q25 verdict | What the LLM did |
//! |---|---|---|---|
//! | pre-bulk_upsert | per-row | PASS | flagged=1, prose elided, structured saved |
//! | last-session post-bulk | bulk_upsert | FAIL | flagged=1, no literal in either channel |
//! | this-session post-bulk | bulk_upsert | FAIL | flagged=0 (LLM didn't detect at all) |
//!
//! Architecture is identical between all three runs (same prompt v9, same
//! Qwen-7B at T=0.0 seed=42, same fixture, same distractor seed, same
//! retrieval stack). The t029 retrieval-only diagnostic (last session)
//! proved both Q25 literals appear in the top-20 candidate set under
//! bulk_upsert — but t029 ran in *its own process* and didn't compare
//! across-process state.
//!
//! Per `feedback_byte_equality_probe_before_non_determinism_hunt.md`:
//! before hunting LLM-side non-determinism, confirm that the inputs to
//! the LLM are actually byte-identical across runs.
//!
//! # What this probe measures
//!
//! 1. **Within-process determinism (5 trials).** Set up the SCALE=1000
//!    corpus once, then call the production retrieval stack 5 times for
//!    Q25 within the same process. For each call, build the LLM prompt
//!    via [`vault_retrieval::read_pipeline::build_user_prompt`] (the same
//!    function `ReadPipeline::read` uses in production) and compare bytes.
//!
//! 2. **Across-process diff bait.** Print the full first-trial prompt to
//!    stdout. Running this probe twice (separate processes) and diffing
//!    the captured logs reveals whether bulk_upsert produces deterministic
//!    Lance state across process invocations.
//!
//! # Decision tree
//!
//! - **All 5 within-process prompts byte-identical:** retrieval is
//!   deterministic in-process. Compare with a second process to test
//!   across-process determinism.
//!   - If across-process also identical → variance is LLM-side (Vulkan
//!     compute / kernel scheduling). Document as known Qwen-7B behavior;
//!     proceed to SCALE=10000.
//!   - If across-process differs → bulk_upsert produces non-deterministic
//!     Lance state across process spawns. Pause Phase 5 ship; investigate.
//!
//! - **Within-process prompts differ:** retrieval itself is non-deterministic
//!   in-process. Investigate immediately — likely HNSW iteration order,
//!   Tantivy segment merge ordering, or RRF tie-breaking instability.
//!
//! # Running
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "C:\Users\shahb\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --example t030_q25_byte_equality_probe --release
//! ```
//!
//! Expected wall ~5-10 min (no LLM): ~30s release relink + ~5 min BGE
//! embedding for 1000 memories + ~5s bulk_upsert + ~2 min Tantivy index +
//! 5 × ~1s retrieval calls.

#![cfg(target_os = "windows")]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, ensure, Context, Result};
use serde::Deserialize;

use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_embedding::{BgeSmallProvider, EmbeddingProvider, EMBEDDING_DIM};
use vault_retrieval::read_pipeline::build_user_prompt;
use vault_retrieval::{
    AbstainingRetriever, HybridRetriever, KeywordIndex, KeywordRetriever, RetrievalOptions,
    RetrievalQuery, RetrievedMemory, Retriever, SemanticRetriever,
};
use vault_storage::{LanceVectorStore, MetadataStore, SqlCipherKey, VectorStore};

const TEST_AT_REST_KEY: [u8; 32] = [0xab; 32];
const DISTRACTOR_SEED: u64 = 0x5CA1_EACD_EED0;
const SCALE: usize = 1000;
const TOP_K: usize = 20;
const NUM_TRIALS: usize = 5;

// ── Q25-only query setup (matches the scale-acceptance test) ─────────────

const Q25_ID: &str = "Q25";

// ── Short-long pairs (verbatim port — included so the corpus matches the
// failing-test corpus exactly, even though only Q25 is probed) ───────────

struct ShortLongPair {
    boundary: &'static str,
    short_content: &'static str,
    long_content: &'static str,
}

const SHORT_LONG_PAIRS: &[ShortLongPair] = &[
    ShortLongPair {
        boundary: "work",
        short_content: "Ergonomic mouse approved at $89/unit in the Q1 hardware budget.",
        long_content: "Hardware procurement reconciliation note from finance: \
            the actual purchase of the ergonomic mouse came in at $145/unit \
            instead of the approved $89/unit. The $56-per-unit overage \
            multiplied across the 24-seat order produces a $1,344 variance \
            against the Q1 hardware budget line item. Finance is asking us \
            to absorb the variance out of the Q2 contingency reserve rather \
            than re-open the Q1 budget for a single line item revision.",
    },
    ShortLongPair {
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
    },
    ShortLongPair {
        boundary: "work",
        short_content: "Office Wi-Fi vendor budget: $2,500/mo.",
        long_content: "Facilities-operations rollup for the second half of \
            the year covering the workplace-tech vendor consolidation we \
            kicked off in March. Headline change relevant to the IT budget: \
            office Wi-Fi vendor renewed at $4,200 per month after a \
            negotiation cycle that lasted six weeks.",
    },
];

// ── Distractor generation (verbatim port from t029) ──────────────────────

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

// ── Base fixture loader ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
struct MemoryFixtureEntry {
    #[allow(dead_code)]
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
        "T0.2.7 t030 Q25 byte-equality probe — SCALE={SCALE} — trials={NUM_TRIALS} — started {}",
        started.format("%Y-%m-%d %H:%M:%S UTC")
    );

    // ── Stores + embedder ───────────────────────────────────────────────
    let dir = tempfile::tempdir()?;
    let key = SqlCipherKey::new("t030-passphrase");
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
    let q25 = query_set
        .queries
        .iter()
        .find(|q| q.id == Q25_ID)
        .cloned()
        .with_context(|| format!("query {Q25_ID} missing from fixture"))?;
    println!("Q25 query_text: {:?}", q25.query_text);

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

    // ── Within-process Q25 retrieval × NUM_TRIALS ────────────────────────
    println!("\n=== Within-process Q25 retrieval × {NUM_TRIALS} trials ===\n");

    let mut prompts: Vec<String> = Vec::with_capacity(NUM_TRIALS);
    let mut id_sequences: Vec<Vec<String>> = Vec::with_capacity(NUM_TRIALS);

    for trial in 1..=NUM_TRIALS {
        let mut boundaries = Vec::with_capacity(q25.authorized_boundaries.len());
        for b in &q25.authorized_boundaries {
            boundaries.push(Boundary::new(b)?);
        }
        let rq = RetrievalQuery {
            query_text: q25.query_text.clone(),
            authorized_boundaries: boundaries,
            max_results: TOP_K,
            options: RetrievalOptions::default(),
        };
        let t0 = Instant::now();
        let hits: Vec<RetrievedMemory> = retriever.retrieve(rq).await?;
        let retrieve_elapsed = t0.elapsed();

        let prompt = build_user_prompt(&q25.query_text, &hits);

        let id_seq: Vec<String> = hits
            .iter()
            .map(|h| {
                let short: String = h.memory.id.0.to_string().chars().take(8).collect();
                format!("{short}@{:.4}", h.score)
            })
            .collect();

        println!(
            "Trial {trial}/{NUM_TRIALS} — retrieve {retrieve_elapsed:?} — hits={} — prompt_bytes={}",
            hits.len(),
            prompt.len()
        );
        println!("  top-{} (id@score, in order):", hits.len());
        for (i, item) in id_seq.iter().enumerate() {
            println!("    [{:>2}] {}", i + 1, item);
        }

        prompts.push(prompt);
        id_sequences.push(id_seq);
    }

    // ── Full first-trial prompt dump (for across-process diffing) ────────
    println!("\n=== Full first-trial Q25 prompt (across-process diff bait) ===");
    println!("--- BEGIN PROMPT ---");
    println!("{}", prompts[0]);
    println!("--- END PROMPT ---");

    // ── Within-process verdict ───────────────────────────────────────────
    println!("\n=== Within-process byte-equality verdict ===");
    let first = &prompts[0];
    let all_equal = prompts.iter().all(|p| p == first);
    if all_equal {
        println!("✅ ALL {NUM_TRIALS} within-process Q25 prompts are byte-identical.");
        println!("   → Retrieval is deterministic in-process.");
        println!("   → Across-process check: run this probe a second time, capture both stdouts,");
        println!("     and `diff` the BEGIN PROMPT → END PROMPT blocks. If those also match,");
        println!("     the variance is LLM-side (Vulkan compute / kernel scheduling).");
    } else {
        println!("❌ Within-process Q25 prompts DIFFER across trials.");
        println!("   → Retrieval is NON-deterministic in-process.");
        for (i, p) in prompts.iter().enumerate() {
            let prefix = if p == first { "MATCH" } else { "DIFFER" };
            println!("   Trial {}: {prefix} (bytes={})", i + 1, p.len());
        }
        // Find first byte of divergence between trial 1 and the first trial that differs.
        if let Some((idx, _)) = prompts.iter().enumerate().find(|(_, p)| *p != first) {
            let p = &prompts[idx];
            let first_diff = first
                .as_bytes()
                .iter()
                .zip(p.as_bytes().iter())
                .position(|(a, b)| a != b)
                .unwrap_or_else(|| first.len().min(p.len()));
            let window_start = first_diff.saturating_sub(40);
            let window_end_first = (first_diff + 40).min(first.len());
            let window_end_p = (first_diff + 40).min(p.len());
            println!(
                "   First-divergence byte index between trial 1 and trial {}: {first_diff}",
                idx + 1
            );
            println!(
                "     trial 1 [{window_start}..{window_end_first}]: {:?}",
                &first[window_start..window_end_first]
            );
            println!(
                "     trial {} [{window_start}..{window_end_p}]: {:?}",
                idx + 1,
                &p[window_start..window_end_p]
            );
        }
    }

    // ── ID-sequence verdict (cheaper to inspect than full prompts) ───────
    println!("\n=== Within-process top-K ID-ordering verdict ===");
    let id_first = &id_sequences[0];
    let id_all_equal = id_sequences.iter().all(|s| s == id_first);
    if id_all_equal {
        println!("✅ Top-{TOP_K} ID sequences are identical across all {NUM_TRIALS} trials.");
    } else {
        println!("❌ Top-{TOP_K} ID sequences DIFFER. Per-trial differences:");
        for (i, s) in id_sequences.iter().enumerate() {
            if s != id_first {
                println!("   Trial {}: differs from trial 1", i + 1);
            }
        }
    }

    Ok(())
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
