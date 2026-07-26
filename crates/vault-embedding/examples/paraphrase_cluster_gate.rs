//! Diagnostic: is the 0.92 clustering gate mis-set, and what would a lower one
//! cost? (HANDOFF session-27 open finding — 5 of 12 same-meaning pairs never
//! clustered, so no consolidation can reach them and they stay in a user's
//! vault as permanent duplicates.)
//!
//! ## What this measures
//!
//! The consolidator's Phase 1 (`vault-consolidator::phases::cluster`) re-embeds
//! each memory's RAW `content` and keeps neighbour edges where
//! `cos(seed, candidate) >= merge_similarity_threshold` (default 0.92), capped
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

/// Production clustering gate (`ConsolidatorConfig::merge_similarity_threshold`,
/// BRD §5.6 line 904).
const MERGE_THRESHOLD: f32 = 0.92;

/// Production NN cap per seed (`vault-consolidator::phases::cluster::TOP_K_NEIGHBORS`).
const TOP_K_NEIGHBORS: usize = 5;

/// Deterministic-dedup gates (`vault-consolidator::phases::dedup`).
const NEAR_IDENTICAL_COS: f32 = 0.93;
const NEAR_IDENTICAL_LEX: f32 = 0.80;

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
    println!("threshold shipped today: {MERGE_THRESHOLD}, top-K {TOP_K_NEIGHBORS} per seed\n");

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

    println!("--- PHASE A: the 12 same-meaning pairs (corpus = 24, as the live run) ---");
    // Header written as one literal: column widths match the row format below.
    println!("pair  cosine contain  predicted        observed (live)  text");

    let clusters_a = cluster_at(&cos_para, MERGE_THRESHOLD);
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

    // ---- Sweep: what each gate catches, and what it newly endangers ----
    println!("\n--- threshold sweep (full Phase-1 algorithm over all {n} memories) ---");
    // Header written as one literal (ASCII only — these logs get read on both
    // CI legs and non-UTF-8 consoles mangle box/tick glyphs).
    println!("thresh   para/12      contra/8      comple/8  chained  note");

    let mut t = 0.96_f32;
    while t >= 0.799 {
        let clusters = cluster_at(&cos, t);

        let mut para_ok = 0usize;
        let mut contra_bad = 0usize;
        let mut comple_bad = 0usize;
        let mut chained = 0usize;

        for cl in &clusters {
            let cases_in: HashSet<usize> = cl.iter().map(|&i| case_of(i)).collect();
            if cases_in.len() == 1 && cl.len() == 2 {
                // A clean, whole, single-case cluster.
                let ci = case_of(cl[0]);
                match cases[ci].2 {
                    Class::Paraphrase => para_ok += 1,
                    Class::Contradictory => contra_bad += 1,
                    Class::Complementary => comple_bad += 1,
                }
            } else {
                chained += 1;
            }
        }

        let note = if (t - MERGE_THRESHOLD).abs() < 1e-6 {
            "<-- SHIPPED TODAY"
        } else {
            ""
        };
        println!("{t:>6.2} {para_ok:>9} {contra_bad:>13} {comple_bad:>13} {chained:>8}  {note}");

        t -= 0.01;
    }

    println!(
        "\npara = same-meaning pairs correctly clustered (want 12). contra/comple = pairs \
         that must NOT be collapsed but would now enter the merge path. chained = clusters \
         spanning more than one case, or partial (over-merge)."
    );

    Ok(())
}
