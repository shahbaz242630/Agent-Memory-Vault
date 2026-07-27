//! Diagnostic: is the clustering gate mis-set, and what would a lower one cost?
//! (HANDOFF session-27 open finding — 5 of 12 same-meaning pairs never
//! clustered, so no consolidation can reach them and they stay in a user's
//! vault as permanent duplicates.)
//!
//! **Outcome: it was mis-set. This diagnostic moved the gate 0.92 → 0.84
//! (ADR-097).** It is kept as the standing instrument for the next such
//! decision — re-run it before touching the gate, the dedup axes, or any
//! fixture whose route through the pipeline depends on where it sits relative
//! to the gate. Phases C and D exist because a hardcoded threshold in a test
//! silently survived the last change.
//!
//! ## What this measures
//!
//! The consolidator's Phase 1 (`vault-consolidator::phases::cluster`) re-embeds
//! each memory's RAW `content` and keeps neighbour edges where
//! `cos(seed, candidate) >= merge_similarity_threshold` (0.84 since ADR-097), capped
//! at `TOP_K_NEIGHBORS = 5` per seed, then unions surviving edges by transitive
//! closure and drops singletons. This example reproduces that algorithm exactly,
//! so the numbers printed are the numbers the gate actually sees.
//!
//! ## Two phases, deliberately separated
//!
//! **Phase A — validity anchor.** The 12 same-meaning pairs from the session-27
//! battery, alone, exactly as the live 2026-07-25 run saw them (24 memories). Its
//! predicted outcomes are checked against what that run ACTUALLY did. If they
//! disagree, this diagnostic does not reproduce reality and everything below it
//! is void. Kept isolated because adding memories changes top-5 neighbour
//! competition — mixing corpora would break the anchor for a reason unrelated to
//! the gate.
//!
//! **Phase B — the real decision.** The same 12 pairs PLUS labelled
//! CONTRADICTORY and COMPLEMENTARY pairs, which are the classes a lower gate
//! actually endangers. `phases/dedup.rs` records these from a 2026-05-31
//! calibration as cosine 0.785–0.883 and 0.643–0.820 respectively — ranges that
//! OVERLAP the same-meaning range measured here, so a threshold move cannot be
//! justified on same-meaning data alone. Phase B re-measures both classes with
//! the same instrument and sweeps the gate, reporting per class what each
//! setting would newly cluster.
//!
//! A contradictory or complementary pair that clusters is not automatically
//! merged — it must still clear the deterministic-dedup gate
//! (`NEAR_IDENTICAL_COS` 0.93 + `NEAR_IDENTICAL_LEX` 0.80) or be handed to the
//! model. But clustering is what puts it on that path at all, which today it
//! never reaches.
//!
//! ## Running
//!
//! ```text
//! cargo run -p vault-embedding --example paraphrase_cluster_gate
//! ```
//!
//! Model paths default to the checked-in bge-small test fixtures; override with
//! `VAULT_MODEL_PATH` / `VAULT_TOKENIZER_PATH` / `VAULT_ORT_LIB_PATH`.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use vault_embedding::{BgeSmallProvider, EmbeddingProvider};

/// Production clustering gate (`ConsolidatorConfig::merge_similarity_threshold`).
///
/// **Mirrored by hand, and it has to be.** `vault-embedding` sits BELOW
/// `vault-consolidator` in the dependency order (BRD §2: dependencies flow
/// downward only), so this example cannot import the real config without
/// creating a cycle. Keep it in step with
/// `ConsolidatorConfig::default().merge_similarity_threshold` — **0.84 since
/// ADR-097** (was 0.92, the BRD §5.6 line 904 value, which this diagnostic is
/// what moved).
const MERGE_THRESHOLD: f32 = 0.84;

/// The gate in force during the live 2026-07-25 run that Phase A checks itself
/// against — **not** the shipped gate, and it must never be changed to track it.
///
/// Phase A's job is to prove this example reproduces a SPECIFIC historical run
/// before any conclusion is drawn from Phases B-D. That run executed at 0.92,
/// so the anchor has to replay 0.92. Pointing it at `MERGE_THRESHOLD` would
/// make it "verify" the instrument against a gate the observations never came
/// from, and every pair the newer gate rescues would be reported as the
/// instrument DISAGREEING with reality — a false alarm on the one check whose
/// whole purpose is to be trustworthy.
const ANCHOR_THRESHOLD: f32 = 0.92;

/// Production NN cap per seed (`vault-consolidator::phases::cluster::TOP_K_NEIGHBORS`).
const TOP_K_NEIGHBORS: usize = 5;

/// Deterministic-dedup gates (`vault-consolidator::phases::dedup`).
const NEAR_IDENTICAL_COS: f32 = 0.93;
const NEAR_IDENTICAL_LEX: f32 = 0.80;

/// Acceptance floors per ADR-045 §d, mirrored from
/// `vault-consolidator/tests/acceptance.rs`. A gate change that breaches
/// either of these breaks the BRD §6.2 clustering ship-gate.
const PRECISION_FLOOR: f64 = 0.95;
const RECALL_FLOOR: f64 = 0.90;

/// What the live 2026-07-25 consolidation run did with a same-meaning pair.
/// Recovered from `battery/reports/personal.report.json` (the surviving facts)
/// cross-checked against that run's counts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Observed {
    /// Clustered, then collapsed by deterministic dedup (no model call).
    Deduped,
    /// Clustered, failed the dedup gate, merged by the model.
    MergedByModel,
    /// Never clustered — both variants survive as permanent duplicates.
    NeverClustered,
}

impl Observed {
    fn clustered(self) -> bool {
        !matches!(self, Observed::NeverClustered)
    }

    fn label(self) -> &'static str {
        match self {
            Observed::Deduped => "deduped",
            Observed::MergedByModel => "model-merged",
            Observed::NeverClustered => "NEVER CLUSTERED",
        }
    }
}

/// Semantic class of a labelled pair — what the consolidator SHOULD do with it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// Same fact, different wording. SHOULD cluster and collapse to one.
    Paraphrase,
    /// Same subject, conflicting value. Must NOT be merged — merging destroys
    /// one of the two claims. Handled by the separate topic-level A5
    /// contradiction pass, which does not use this gate.
    Contradictory,
    /// Related subject, genuinely different facts. Both are true and both must
    /// survive; merging loses information.
    Complementary,
}

impl Class {
    fn label(self) -> &'static str {
        match self {
            Class::Paraphrase => "paraphrase",
            Class::Contradictory => "contradictory",
            Class::Complementary => "complementary",
        }
    }
}

/// The session-27 paraphrase battery, verbatim. Ordered near-identical →
/// heavily reworded; each pair on a distinct topic.
const PARAPHRASE: [(&str, &str, Observed); 12] = [
    (
        "The user prefers dark mode in their code editor.",
        "The user prefers dark mode in their code editors.",
        Observed::Deduped,
    ),
    (
        "The user drinks black coffee every morning.",
        "The user drinks black coffee each morning.",
        Observed::Deduped,
    ),
    (
        "The user's favourite text editor is Neovim.",
        "The user's preferred text editor is Neovim.",
        Observed::Deduped,
    ),
    (
        "The user works remotely from home three days a week.",
        "The user works from home remotely for three days each week.",
        Observed::Deduped,
    ),
    (
        "The user prefers short, direct answers.",
        "The user likes concise, straightforward replies.",
        Observed::NeverClustered,
    ),
    (
        "The user is allergic to peanuts.",
        "The user has a peanut allergy.",
        Observed::MergedByModel,
    ),
    (
        "The user runs a small design studio in London.",
        "The user owns a small design agency based in London.",
        Observed::NeverClustered,
    ),
    (
        "The user prefers to be contacted by email.",
        "The user's preferred contact method is email.",
        Observed::NeverClustered,
    ),
    (
        "The user is a vegetarian.",
        "The user does not eat meat.",
        Observed::NeverClustered,
    ),
    (
        "The user's primary programming language is Rust.",
        "The user mainly programs in Rust.",
        Observed::NeverClustered,
    ),
    (
        "The user has two children.",
        "The user is a parent of two kids.",
        Observed::MergedByModel,
    ),
    (
        "The user goes to the gym on Mondays and Thursdays.",
        "The user attends the gym every Monday and Thursday.",
        Observed::MergedByModel,
    ),
];

/// Same subject, conflicting value — the knowledge-update shape the A5 pass
/// exists for. Merging either of these away loses a real claim, so a gate that
/// starts clustering them is taking on risk. Vega/Atlas mirrors the pair already
/// used by `vault-consolidator/tests/contradiction_resolution.rs`.
const CONTRADICTORY: [(&str, &str); 8] = [
    (
        "The user works at Vega Systems.",
        "The user works at Atlas Labs.",
    ),
    ("The user lives in London.", "The user lives in Manchester."),
    (
        "The user's favourite programming language is Rust.",
        "The user's favourite programming language is Python.",
    ),
    (
        "The user prefers tea over coffee.",
        "The user prefers coffee over tea.",
    ),
    (
        "The user's laptop is a MacBook Pro.",
        "The user's laptop is a ThinkPad.",
    ),
    (
        "The user's daily standup is at 9am.",
        "The user's daily standup is at 10:30am.",
    ),
    (
        "The user flies to Berlin twice a year.",
        "The user flies to Berlin four times a year.",
    ),
    (
        "The user's preferred deploy target is AWS.",
        "The user's preferred deploy target is Azure.",
    ),
];

/// Related subject, genuinely different facts — both true, both must survive.
const COMPLEMENTARY: [(&str, &str); 8] = [
    (
        "The user runs 5km every Saturday.",
        "The user swims on Sunday mornings.",
    ),
    (
        "The user's manager is called Priya.",
        "The user's team has six engineers.",
    ),
    (
        "The user is learning Spanish.",
        "The user studied French at school.",
    ),
    (
        "The user uses a standing desk.",
        "The user has a mechanical keyboard.",
    ),
    (
        "The user's daughter is called Mira.",
        "The user's son plays the cello.",
    ),
    (
        "The user takes their coffee with oat milk.",
        "The user buys coffee beans from a local roaster.",
    ),
    (
        "The user reviews pull requests on Friday afternoons.",
        "The user writes release notes at the end of each sprint.",
    ),
    (
        "The user keeps a paper notebook for meeting notes.",
        "The user archives finished projects to an external drive.",
    ),
];

/// Lowercased alphanumeric word tokens. Copied verbatim from
/// `vault-consolidator::phases::dedup::tokens` (that fn is `pub(crate)` so it
/// cannot be imported; duplicated deliberately so the number printed here is
/// the number the gate computes).
fn tokens(s: &str) -> HashSet<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// `|A ∩ B| / min(|A|, |B|)` over word-token sets — verbatim from
/// `phases::dedup::token_containment`.
fn token_containment(a: &str, b: &str) -> f32 {
    let (ta, tb) = (tokens(a), tokens(b));
    let min_len = ta.len().min(tb.len());
    if min_len == 0 {
        return 0.0;
    }
    ta.intersection(&tb).count() as f32 / min_len as f32
}

/// Dot product. bge-small output is L2-normalised (pinned by
/// `test_2_embed_output_is_l2_normalized`), so this IS cosine similarity —
/// same reasoning as `phases::dedup::cosine_similarity`.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Ordered token list — mirrors `phases::dedup::token_sequence`.
fn token_sequence(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Same words, different order — mirrors the ADR-096 third axis in
/// `phases::dedup::is_pure_reordering`. Modelled here so the sweep reflects
/// SHIPPED behaviour rather than the pre-fix code.
fn is_pure_reordering(a: &str, b: &str) -> bool {
    let (seq_a, seq_b) = (token_sequence(a), token_sequence(b));
    if seq_a == seq_b {
        return false;
    }
    let (mut sorted_a, mut sorted_b) = (seq_a, seq_b);
    sorted_a.sort();
    sorted_b.sort();
    sorted_a == sorted_b
}

/// Would this pair be collapsed by DETERMINISTIC dedup — i.e. by plain code,
/// with no model call and no contradiction record?
///
/// This is the dangerous path: it is where a fact can leave the vault without
/// judgement. Reproduces all three axes of `phases::dedup::plan_dedup`,
/// including the ADR-096 order guard.
fn would_blind_collapse(a: &str, b: &str, cos: f32) -> bool {
    cos >= NEAR_IDENTICAL_COS
        && token_containment(a, b) >= NEAR_IDENTICAL_LEX
        && !is_pure_reordering(a, b)
}

/// Exact wording of pinned test premises that reference the merge gate by
/// value. Lowering the gate can silently invalidate a test's PREMISE (not just
/// its assertion), which would turn a real regression into a green run. Each
/// entry is `(test file, what the premise claims, text a, text b)`.
const PINNED_PREMISES: [(&str, &str, &str, &str); 3] = [
    (
        "contradiction_resolution.rs :: A5 + R2 (Vega/Atlas)",
        "ORIGINALLY claimed below the merge gate; ADR-097 re-points these tests \
         at the merge route, so this must now be ABOVE the shipped gate",
        "As of 2026-01-10 the user worked as a structural engineer at Vega Bridgeworks.",
        "As of 2026-04-01 the user works as a structural engineer at Atlas Structures, \
         having left Vega Bridgeworks.",
    ),
    (
        "contradiction_resolution.rs :: co-topical-but-compatible guard",
        "compatible co-topical facts - either route is acceptable, but knowing \
         which one runs decides whether its mock needs to be phase-aware",
        "As of 2026-04-01 the user works as a structural engineer at Atlas Structures.",
        "The user commutes to work by train every weekday.",
    ),
    (
        "contradiction_resolution.rs :: NEW below-gate NN coverage (candidate)",
        "MUST sit below the shipped gate AND above the 0.70 NN candidate floor, \
         so the nearest-neighbour pass is the ONLY route that can catch it",
        "As of 2026-01-10 the user lived in London.",
        "As of 2026-04-01 the user lives in Manchester, having moved from London.",
    ),
];

/// Candidate gate values reported in detail by Phases C and D.
const CANDIDATES: [f32; 4] = [0.92, 0.90, 0.89, 0.84];

/// `phases::candidates::CONTRADICTION_NN_SIMILARITY_FLOOR` — below this, a pair
/// is never even offered to the pairwise contradiction judge. A fixture meant
/// to prove the NN route must sit ABOVE this and BELOW the merge gate.
const CONTRADICTION_NN_FLOOR: f32 = 0.70;

/// Pair-counting precision/recall against `topic_id` ground truth — verbatim
/// definition from `vault-consolidator/tests/acceptance.rs::precision_recall`
/// (ADR-045 §d).
///
/// - TP: same predicted cluster AND same topic. FP: same cluster, different
///   topic. FN: different cluster, same topic.
fn precision_recall(clusters: &[Vec<usize>], topics: &[u32]) -> (f64, f64) {
    let mut predicted: HashMap<usize, usize> = HashMap::new();
    for (ci, members) in clusters.iter().enumerate() {
        for &m in members {
            predicted.insert(m, ci);
        }
    }

    let (mut tp, mut fp, mut fn_) = (0u64, 0u64, 0u64);
    for i in 0..topics.len() {
        for j in (i + 1)..topics.len() {
            let same_topic = topics[i] == topics[j];
            let same_cluster = match (predicted.get(&i), predicted.get(&j)) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            match (same_cluster, same_topic) {
                (true, true) => tp += 1,
                (true, false) => fp += 1,
                (false, true) => fn_ += 1,
                (false, false) => {}
            }
        }
    }

    let precision = if tp + fp == 0 {
        1.0
    } else {
        tp as f64 / (tp + fp) as f64
    };
    let recall = if tp + fn_ == 0 {
        1.0
    } else {
        tp as f64 / (tp + fn_) as f64
    };
    (precision, recall)
}

/// Union-find over `0..n`, mirroring `phases::cluster::union_find_components`.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}

/// Reproduce Phase 1 clustering at `threshold`: per seed keep its top-K
/// neighbours (excluding self) that clear the threshold, union the edges, drop
/// singletons. Returns components as sorted index vectors.
fn cluster_at(cos: &[Vec<f32>], threshold: f32) -> Vec<Vec<usize>> {
    let n = cos.len();
    let mut uf = UnionFind::new(n);

    for (seed, row) in cos.iter().enumerate() {
        let mut ranked: Vec<(usize, f32)> = row
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != seed)
            .map(|(i, &c)| (i, c))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

        for &(neighbour, sim) in ranked.iter().take(TOP_K_NEIGHBORS) {
            if sim >= threshold {
                uf.union(seed, neighbour);
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = uf.find(i);
        groups.entry(root).or_default().push(i);
    }

    let mut out: Vec<Vec<usize>> = groups
        .into_values()
        .filter(|members| members.len() >= 2)
        .map(|mut members| {
            members.sort_unstable();
            members
        })
        .collect();
    out.sort();
    out
}

/// Memories are laid out `[case0.a, case0.b, case1.a, case1.b, ...]`, so a
/// memory's case index is `idx / 2`.
fn case_of(idx: usize) -> usize {
    idx / 2
}

fn fixture_path(env_key: &str, leaf: &str) -> PathBuf {
    if let Ok(v) = std::env::var(env_key) {
        return PathBuf::from(v);
    }
    // Resolve against the crate root captured at compile time rather than the
    // process CWD — `cargo run`'s working directory is the invocation
    // directory, so a relative path would break depending on where it is run
    // from.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("test-fixtures/bge-small-en-v1.5")
        .join(leaf)
}

/// Every labelled case, paraphrase first so Phase A can slice the leading
/// sub-corpus without re-embedding.
fn all_cases() -> Vec<(&'static str, &'static str, Class)> {
    let mut v: Vec<(&'static str, &'static str, Class)> = PARAPHRASE
        .iter()
        .map(|(a, b, _)| (*a, *b, Class::Paraphrase))
        .collect();
    v.extend(
        CONTRADICTORY
            .iter()
            .map(|(a, b)| (*a, *b, Class::Contradictory)),
    );
    v.extend(
        COMPLEMENTARY
            .iter()
            .map(|(a, b)| (*a, *b, Class::Complementary)),
    );
    v
}

fn min_max(vals: &[f32]) -> (f32, f32) {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for &v in vals {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    (lo, hi)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = fixture_path("VAULT_MODEL_PATH", "model.onnx");
    let tokenizer = fixture_path("VAULT_TOKENIZER_PATH", "tokenizer.json");
    let ort_lib = fixture_path("VAULT_ORT_LIB_PATH", "onnxruntime.dll");

    println!("=== paraphrase clustering-gate diagnostic ===");
    println!(
        "threshold shipped today: {MERGE_THRESHOLD} (Phase A replays the live run's \
         {ANCHOR_THRESHOLD}), top-K {TOP_K_NEIGHBORS} per seed\n"
    );

    let provider = BgeSmallProvider::open(&model, &tokenizer, &ort_lib)?;

    let cases = all_cases();
    let texts: Vec<&str> = cases.iter().flat_map(|(a, b, _)| [*a, *b]).collect();

    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
    for t in &texts {
        embeddings.push(provider.embed(t).await?);
    }

    let n = texts.len();
    let cos: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| cosine(&embeddings[i], &embeddings[j]))
                .collect()
        })
        .collect();

    // ================= PHASE A — validity anchor =================
    // The 12 paraphrase pairs ALONE, matching the live run's corpus exactly.
    let para_n = PARAPHRASE.len() * 2;
    let cos_para: Vec<Vec<f32>> = (0..para_n).map(|i| cos[i][0..para_n].to_vec()).collect();

    println!(
        "--- PHASE A: the 12 same-meaning pairs (corpus = 24, as the live run, \
         replayed at that run's {ANCHOR_THRESHOLD} gate) ---"
    );
    // Header written as one literal: column widths match the row format below.
    println!("pair  cosine contain  predicted        observed (live)  text");

    let clusters_a = cluster_at(&cos_para, ANCHOR_THRESHOLD);
    let mut mismatches = 0usize;

    for (p, (a, b, observed)) in PARAPHRASE.iter().enumerate() {
        let (ia, ib) = (p * 2, p * 2 + 1);
        let c = cos[ia][ib];
        let lex = token_containment(a, b);

        let together = clusters_a
            .iter()
            .any(|cl| cl.contains(&ia) && cl.contains(&ib));
        let predicted = if !together {
            "never clusters"
        } else if c >= NEAR_IDENTICAL_COS && lex >= NEAR_IDENTICAL_LEX {
            "dedup (no model)"
        } else {
            "model merge"
        };

        let agrees = together == observed.clustered();
        if !agrees {
            mismatches += 1;
        }
        let flag = if agrees { "" } else { "   <-- DISAGREES" };
        let obs = observed.label();
        println!(
            "{:<4} {c:>7.4} {lex:>7.2}  {predicted:<16} {obs:<16} {a}{flag}",
            p + 1
        );
    }

    println!();
    if mismatches == 0 {
        println!(
            "✅ validity anchor holds: predicted clustering matches all 12 outcomes of the \
             live 2026-07-25 run."
        );
    } else {
        println!(
            "❌ validity anchor FAILED on {mismatches} pair(s) — this diagnostic does not \
             reproduce the live run. Everything below is VOID."
        );
    }

    // ================= PHASE B — the classes a lower gate endangers =========
    println!(
        "\n--- PHASE B: full corpus = {} memories ({} paraphrase, {} contradictory, {} complementary pairs) ---",
        n,
        PARAPHRASE.len(),
        CONTRADICTORY.len(),
        COMPLEMENTARY.len()
    );

    let mut by_class: HashMap<&str, Vec<f32>> = HashMap::new();
    for (idx, (_, _, class)) in cases.iter().enumerate() {
        by_class
            .entry(class.label())
            .or_default()
            .push(cos[idx * 2][idx * 2 + 1]);
    }

    // Cross-case pairs = two memories from DIFFERENT cases (the unrelated mass).
    let mut cross: Vec<(usize, usize, f32)> = Vec::new();
    for (i, row) in cos.iter().enumerate() {
        for (j, &c) in row.iter().enumerate().skip(i + 1) {
            if case_of(i) != case_of(j) {
                cross.push((i, j, c));
            }
        }
    }
    cross.sort_by(|a, b| b.2.total_cmp(&a.2));

    println!("\nmeasured cosine range per class:");
    for class in [
        Class::Paraphrase,
        Class::Contradictory,
        Class::Complementary,
    ] {
        if let Some(vals) = by_class.get(class.label()) {
            let (lo, hi) = min_max(vals);
            println!(
                "  {:<15} {lo:.4} – {hi:.4}   ({} pairs)",
                class.label(),
                vals.len()
            );
        }
    }
    let cross_vals: Vec<f32> = cross.iter().map(|(_, _, c)| *c).collect();
    let (xlo, xhi) = min_max(&cross_vals);
    println!(
        "  {:<15} {xlo:.4} – {xhi:.4}   ({} pairs)",
        "cross-case",
        cross.len()
    );

    println!("\nevery contradictory + complementary pair, measured:");
    for (idx, (a, b, class)) in cases.iter().enumerate() {
        if *class == Class::Paraphrase {
            continue;
        }
        let c = cos[idx * 2][idx * 2 + 1];
        let lab = class.label();
        println!("  {c:.4}  [{lab:<13}]  {a}  ||  {b}");
    }

    println!("\ntop 6 cross-case pairs (unrelated facts scoring highest):");
    for (i, j, c) in cross.iter().take(6) {
        println!("  {c:.4}  {}  ||  {}", texts[*i], texts[*j]);
    }

    // ---- The danger scan: what could STILL be collapsed without judgement ----
    //
    // Deterministic dedup is the ONLY path where a fact leaves the vault with no
    // model call and no contradiction record. ADR-096 closed the
    // reversed-argument case. What would remain dangerous is a NON-permutation
    // contradiction that still clears cosine 0.93 + containment 0.80 — that
    // shape is unaffected by the order guard and unaffected by the cluster
    // threshold, so it must be hunted separately.
    println!("\n--- danger scan: pairs that would collapse with NO judgement ---");
    let mut danger = 0usize;
    for (idx, (a, b, class)) in cases.iter().enumerate() {
        let c = cos[idx * 2][idx * 2 + 1];
        if *class != Class::Paraphrase && would_blind_collapse(a, b, c) {
            danger += 1;
            let lab = class.label();
            println!("  !! {c:.4}  [{lab}]  {a}  ||  {b}");
        }
    }
    if danger == 0 {
        println!(
            "  none - no contradictory or complementary pair clears cosine \
             {NEAR_IDENTICAL_COS} + containment {NEAR_IDENTICAL_LEX} once the ADR-096 \
             order guard is applied."
        );
    }

    // ---- Sweep: what each gate catches, and what it newly endangers ----
    println!("\n--- threshold sweep (full Phase-1 algorithm over all {n} memories) ---");
    println!(
        "  'clustered' = reached the judgement path. 'BLIND' = collapsed by plain code with \
         no judgement - the only column where a fact can vanish unrecorded."
    );
    // Header written as one literal (ASCII only — these logs get read on both
    // CI legs and non-UTF-8 consoles mangle box/tick glyphs).
    println!("thresh   para  paraBLIND   contra  contraBLIND   comple  chained  note");

    let mut t = 0.96_f32;
    while t >= 0.799 {
        let clusters = cluster_at(&cos, t);

        let mut para_ok = 0usize;
        let mut para_blind = 0usize;
        let mut contra_clustered = 0usize;
        let mut contra_blind = 0usize;
        let mut comple_clustered = 0usize;
        let mut chained = 0usize;

        for cl in &clusters {
            let cases_in: HashSet<usize> = cl.iter().map(|&i| case_of(i)).collect();
            if cases_in.len() == 1 && cl.len() == 2 {
                // A clean, whole, single-case cluster.
                let ci = case_of(cl[0]);
                let (a, b, class) = cases[ci];
                let blind = would_blind_collapse(a, b, cos[ci * 2][ci * 2 + 1]);
                match class {
                    Class::Paraphrase => {
                        para_ok += 1;
                        if blind {
                            para_blind += 1;
                        }
                    }
                    Class::Contradictory => {
                        contra_clustered += 1;
                        if blind {
                            contra_blind += 1;
                        }
                    }
                    Class::Complementary => comple_clustered += 1,
                }
            } else {
                chained += 1;
            }
        }

        let note = if (t - MERGE_THRESHOLD).abs() < 1e-6 {
            "<-- SHIPPED"
        } else {
            ""
        };
        println!(
            "{t:>6.2} {para_ok:>6} {para_blind:>10} {contra_clustered:>8} \
             {contra_blind:>12} {comple_clustered:>8} {chained:>8}  {note}"
        );

        t -= 0.01;
    }

    println!(
        "\npara = same-meaning pairs clustered (want 12). contra/comple = pairs that must NOT \
         be collapsed. The BLIND columns are the ones that matter: anything above zero under \
         contraBLIND is a fact leaving the vault with no judgement and no record. chained = \
         clusters spanning more than one case, or partial."
    );

    // ============ PHASE C — the 100-memory acceptance ship-gate =============
    //
    // Phase B's corpus is 28 hand-written pairs. The BRD §6.2 clustering
    // ship-gate is measured on a DIFFERENT and larger artefact: the 100-entry
    // / 20-topic labelled fixture behind
    // `vault-consolidator/tests/acceptance.rs`, which asserts precision >= 0.95
    // AND recall >= 0.90 (ADR-045 §d). Lowering the gate trades precision for
    // recall by construction, so that test is the one a threshold change is
    // most likely to break — and it costs a ~35-minute crate test run to find
    // out. Sweep it here instead, before touching the default.
    //
    // MODEL, NOT PROOF: `acceptance.rs` clusters via `find_candidate_clusters`
    // (storage-backed ANN); this reproduces the same algorithm over exact
    // cosine. Phase A anchors that the reproduction matches live behaviour, but
    // the real test still has to run.
    println!("\n--- PHASE C: acceptance ship-gate sweep (100 memories / 20 topics) ---");

    let fixture_json = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("vault-embedding crate dir has a parent (crates/)")?
        .join("vault-consolidator/tests/fixtures/clustering_acceptance_100.json");

    match std::fs::read(&fixture_json) {
        Err(e) => println!("  SKIPPED - cannot read {}: {e}", fixture_json.display()),
        Ok(bytes) => {
            let parsed: serde_json::Value = serde_json::from_slice(&bytes)?;
            let entries = parsed.as_array().ok_or("fixture must be a JSON array")?;

            let mut topics: Vec<u32> = Vec::with_capacity(entries.len());
            let mut acc_emb: Vec<Vec<f32>> = Vec::with_capacity(entries.len());
            for e in entries {
                let topic = e
                    .get("topic_id")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or("fixture entry missing numeric topic_id")?;
                let text = e
                    .get("memory_text")
                    .and_then(serde_json::Value::as_str)
                    .ok_or("fixture entry missing string memory_text")?;
                topics.push(topic as u32);
                acc_emb.push(provider.embed(text).await?);
            }
            println!(
                "  loaded {} entries from the acceptance fixture",
                topics.len()
            );

            let acc_n = acc_emb.len();
            let acc_cos: Vec<Vec<f32>> = (0..acc_n)
                .map(|i| {
                    (0..acc_n)
                        .map(|j| cosine(&acc_emb[i], &acc_emb[j]))
                        .collect()
                })
                .collect();

            println!("  floors: precision >= {PRECISION_FLOOR}, recall >= {RECALL_FLOOR}");
            println!("thresh   precision   recall   verdict");
            let mut t = 0.96_f32;
            while t >= 0.799 {
                let clusters = cluster_at(&acc_cos, t);
                let (p, r) = precision_recall(&clusters, &topics);
                let pass = p >= PRECISION_FLOOR && r >= RECALL_FLOOR;
                let verdict = if pass { "PASS" } else { "FAIL" };
                let marker = if (t - MERGE_THRESHOLD).abs() < 1e-6 {
                    "  <-- SHIPPED"
                } else if CANDIDATES.iter().any(|c| (t - c).abs() < 1e-6) {
                    "  <-- candidate"
                } else {
                    ""
                };
                println!("{t:>6.2} {p:>11.4} {r:>8.4}   {verdict}{marker}");
                t -= 0.01;
            }
        }
    }

    // ============ PHASE D — do pinned test PREMISES survive? ================
    //
    // A premise assertion states a fact the rest of a test depends on. If a
    // gate change falsifies one, the test fails on the premise line rather than
    // its real assertion — noisy but honest. The worse case is a premise that
    // still passes while no longer meaning what it says. Measure them.
    println!("\n--- PHASE D: pinned test premises vs candidate gates ---");
    for (site, claim, a, b) in PINNED_PREMISES {
        let c = cosine(&provider.embed(a).await?, &provider.embed(b).await?);
        println!("  {site}");
        println!("    claim:  {claim}");
        println!("    cosine: {c:.4}");
        for cand in CANDIDATES {
            let side = if c < cand {
                "below gate"
            } else {
                "AT/ABOVE gate"
            };
            let nn = if c >= CONTRADICTION_NN_FLOOR {
                "reaches NN pass"
            } else {
                "!! BELOW NN FLOOR - unreachable"
            };
            println!("      gate {cand:.2}: {side:<14} | {nn}");
        }
    }

    Ok(())
}
