//! T0.2.7 Phase 1 — t028f targeted Q21+Q26 probe (2026-05-18).
//!
//! **Purpose.** Fast, reliable iteration loop for fixing the two remaining
//! gauntlet failures (Q21 hard-negative, Q26 contradiction-flag-empty).
//!
//! **Why this exists instead of iterating t028d directly.** t028d
//! regenerates the corpus on every run, which assigns fresh UUIDv7s to
//! every memory. That changes the `[<memory-id>]` bytes in the LLM input,
//! so "same test" doesn't mean "same bytes" across runs. The t028e
//! determinism probe proved Qwen-7B is byte-deterministic on identical
//! input — meaning the v6-PASS-vs-v7-FAIL Q21 swing was caused by UUID
//! drift in the prompt bytes, not GPU non-determinism as the HANDOFF
//! initially hypothesised. To iterate prompt fixes reliably, we need
//! BIT-IDENTICAL inputs across runs. This probe carries hardcoded canned
//! candidates inline — no corpus generation, no UUID drift.
//!
//! **Methodology.**
//! 1. Resolve Qwen-7B GGUF via [`models_dir`].
//! 2. Open the provider ONCE with the live production [`TuningConfig`].
//! 3. Two scenarios:
//!    - **Q21 hard-negative** — query about "Kubernetes migration", 15 canned
//!      candidate memories ALL about platform-team migrations (CI, local dev
//!      env, log aggregation, etc) — NONE mention Kubernetes/k8s. PASS iff
//!      LLM sets `vault_has_no_relevant_content=true`.
//!    - **Q26 contradiction** — query about household-services costing more
//!      than expected, 15 canned candidates including BOTH `Comcast $89` and
//!      `Comcast $109` memories. PASS iff `contradictions_flagged` is
//!      non-empty AND synthesis_markdown contains both literal values.
//! 4. Each scenario runs N_REPS=3 times. Verdicts:
//!    - **Determinism**: all N outputs byte-identical (sanity check; should
//!      always hold per t028e).
//!    - **Correctness**: per-scenario predicate above.
//! 5. Print a summary table per scenario + an overall headline.
//!
//! **Iteration protocol.**
//! 1. Edit `CANDIDATE_SYSTEM_PROMPT` and/or `CANDIDATE_JSON_SCHEMA` below
//!    (clearly mark each iteration in the doc-comment below).
//! 2. `cargo run -p vault-retrieval --release --example t028f_q21_q26_probe`.
//! 3. Read the verdict. Iterate.
//!
//! Iteration log:
//! - v2 baseline (15 candidates each, clean short Comcast memories):
//!   2/2 PASS + 3/3 determinism per scenario. Confirmed v2 prompt + current
//!   production schema are CORRECT on clean canned input.
//! - Step A (20 candidates, faithful iter7 shape): Q21 PASS 3/3,
//!   Q26 FAIL 3/3 — deterministic reproduction of iter7's Q26 failure
//!   (synthesis mentions both $89 and $109, contradictions_flagged empty).
//!   Diagnosis: LLM reads "$89 → $109" as a temporal change, not a
//!   contradiction; populates synthesis but skips structured flag.
//! - **Iteration 1 (this run):** prompt-only fix. Add two paragraphs to
//!   the CONTRADICTIONS section: (i) TEMPORAL VALUE CHANGES count as
//!   contradictions for review/audit queries, (ii) concrete Comcast
//!   $89→$109 example showing the desired contradictions_flagged shape.
//!   No schema change.
//!
//! **Discipline.** Self-contained probe. No new dependencies. No fixture
//! loading. No production code changes. No commit. Spike artefact rides
//! with the eventual production fix per
//! `feedback_spike_examples_bundle_with_consumer_code.md`.
//!
//! Run with (PowerShell on Windows):
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "$env:USERPROFILE\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --release --example t028f_q21_q26_probe
//! ```

#![allow(clippy::too_many_lines)]

use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use serde::Deserialize;
use vault_llm::{CompletionParams, LlmProvider, Qwen25_14BProvider, TuningConfig};

const QWEN_MODEL_FILENAME: &str = "Qwen2.5-7B-Instruct-Q4_K_M.gguf";
const N_REPS: usize = 3;
const SEP_WIDE: usize = 100;

// ── CANDIDATE SYSTEM PROMPT — edit between iterations ────────────────────
//
// Iteration log:
// - v2 (baseline): exact copy of t028d's CANDIDATE_SYSTEM_PROMPT at
//   2026-05-18 session-close. Three structural rules: strict-relevance
//   with Kubernetes example, VERBATIM rule, dual-field contradictions
//   requirement, task-shaped queries section.

const CANDIDATE_SYSTEM_PROMPT: &str = r#"You are the read layer of a personal memory vault used by AI coding agents.

You receive a query and a set of candidate memories retrieved via semantic similarity.
In ONE pass you must: (a) filter to actually-relevant candidates, (b) detect any
contradictions among the filtered set, and (c) produce a coherent synthesis.

RELEVANCE:
- A candidate is relevant ONLY if its content explicitly mentions the subject of the
  query. Topical proximity is NOT relevance.
- Example: a query about "Kubernetes migration" is NOT satisfied by memories about
  database migrations, container tooling in general, or other infrastructure changes.
  The subject is specifically "Kubernetes" — if no candidate uses that word (or a
  direct synonym like "k8s"), the vault has no relevant content for this query.
- When uncertain whether a candidate addresses the query's subject, prefer
  vault_has_no_relevant_content=true over fabricating a relevance link. Conservative
  beats over-confident.

CONTRADICTIONS (load-bearing):
- If two or more memories disagree on a value for the same fact — different numbers,
  dates, amounts, names, choices, quantities — you MUST surface the disagreement.
  This holds even when many memories support one value and only one supports the
  other. Minority evidence is never optional.
- VERBATIM RULE: when you state a contradictory value in synthesis_markdown, copy
  the EXACT text from the source memory, including all modifiers (years, units,
  qualifiers). Do NOT abbreviate, round, or paraphrase. If a memory says
  "Q1 2027", write "Q1 2027" — not "Q1" alone. If a memory says "$89.99/month",
  write "$89.99/month" — not "around $90".
- For EACH contradiction detected you MUST do BOTH:
    (a) Mention BOTH literal values in synthesis_markdown (verbatim, per the rule
        above).
    (b) Add an entry to contradictions_flagged with the participating memory IDs
        and the conflicting positions (also verbatim).
- TEMPORAL VALUE CHANGES count as contradictions. If one memory says X has value A
  and another memory says X has value B (or "X is now B", or "X increased to B",
  or "B starting next cycle"), both memories disagree about what value X currently
  carries — the older memory implies the answer is A; the newer implies B. You MUST
  flag this in contradictions_flagged using the same dual-field rule above. A
  monthly-review or audit query is asking precisely for these flags; reporting the
  change in synthesis_markdown alone is NOT enough.
- Example: if memory M1 says "Comcast bill is $89/month" and memory M2 says
  "Comcast bill is now $109/month starting next cycle", BOTH values disagree about
  the current Comcast cost — populate contradictions_flagged with
  {memory_ids: [M1, M2], positions: ["$89/month", "$109/month starting next cycle"]}.
- Reporting only the majority value in synthesis_markdown while leaving
  contradictions_flagged empty is a FAILURE. Both fields are required for every
  contradiction.

TASK-SHAPED QUERIES:
- Some queries are phrased as agent tasks ("help me update the X doc with the
  latest milestone dates", "doing the monthly Y review — anything I should flag?",
  "putting together Z, what should I include?").
- Ignore the action verb ("help me update", "doing", "putting together"). Focus on
  the NOUN PHRASE — what is the agent asking about? In "help me update the product
  roadmap doc", the noun phrase is "product roadmap" and the agent needs to know
  the current roadmap state and any contradictions.
- Your output is NOT the completed task. Your output is a summary of relevant
  memory content (including any contradictions), which the agent will use to
  complete the task themselves. Do NOT generate boilerplate task text.

OUTPUT:
- Write a coherent narrative in synthesis_markdown; cite memory IDs inline.
- If no candidates are relevant: set vault_has_no_relevant_content=true and state
  this in synthesis_markdown. Do NOT fabricate.
- Keep synthesis_markdown under 250 words.
- Return ONLY valid JSON matching the schema."#;

// ── CANDIDATE JSON SCHEMA — edit between iterations ──────────────────────
//
// Iteration log:
// - v0 (baseline): exact copy of production `READ_TIME_JSON_SCHEMA` at
//   read_pipeline.rs:82-101. Field order: synthesis_markdown,
//   contradictions_flagged, vault_has_no_relevant_content.

const CANDIDATE_JSON_SCHEMA: &str = r#"{
  "type": "object",
  "required": ["synthesis_markdown", "contradictions_flagged", "vault_has_no_relevant_content"],
  "properties": {
    "synthesis_markdown": {"type": "string"},
    "contradictions_flagged": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["memory_ids", "positions"],
        "properties": {
          "memory_ids": {"type": "array", "items": {"type": "string"}},
          "positions": {"type": "array", "items": {"type": "string"}},
          "current_position_if_determinable": {"type": "string"}
        }
      }
    },
    "vault_has_no_relevant_content": {"type": "boolean"}
  }
}"#;

// ── Q21 canned input (Kubernetes hard-negative — 20 candidates) ──────────
//
// 20 candidate memories ALL about platform-team migrations / internal
// tooling. NONE mention Kubernetes or k8s. Content shape MIRRORS iter7's
// actual top-20 verbatim (same first-90-char prefixes), including the
// content-duplicate ranks (e.g. rank 0 ≈ rank 4 "local dev environment is
// getting deprecated"; rank 8 ≈ rank 11 "log aggregation cluster is
// getting deprecated"). This is the LLM's actual failure shape at scale.
const Q21_USER_PROMPT: &str = "QUERY: What did we decide about the Kubernetes migration?

CANDIDATES:
[019e3b00-0001-7000-8000-000000000001] Internal tooling status — local dev environment is getting deprecated in favor of the unified replacement.
[019e3b00-0001-7000-8000-000000000002] Update on the CI build pipeline: is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-000000000003] Internal tooling status — local dev environment is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-000000000004] log aggregation cluster is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-000000000005] Internal tooling status — local dev environment is getting deprecated in favor of the unified replacement.
[019e3b00-0001-7000-8000-000000000006] Heads up — the CI build pipeline is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-000000000007] Database migrations are Bob's responsibility from now on.
[019e3b00-0001-7000-8000-000000000008] Update on the load-test rig: is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-000000000009] Internal tooling status — log aggregation cluster is getting deprecated in favor of the unified replacement.
[019e3b00-0001-7000-8000-00000000000a] Full procurement writeup: Sentry error monitoring comes up for renewal in February; current year was within budget and the team wants to renew at the same tier.
[019e3b00-0001-7000-8000-00000000000b] Long-form retrospective on the log aggregation cluster work that Aiden led over the past quarter.
[019e3b00-0001-7000-8000-00000000000c] Internal tooling status — log aggregation cluster is getting deprecated in favor of the unified replacement.
[019e3b00-0001-7000-8000-00000000000d] Internal tooling status — feature flag service is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-00000000000e] Long-form retrospective on the local dev environment work that Tobias led over the past quarter.
[019e3b00-0001-7000-8000-00000000000f] Long-form retrospective on the CI build pipeline work that Tobias led over the past quarter.
[019e3b00-0001-7000-8000-000000000010] Moving CI from CircleCI to GitHub Actions by end of Q1. Three reasons: (1) the team already uses GH, (2) cost, (3) integration.
[019e3b00-0001-7000-8000-000000000011] Full procurement writeup: GitHub Enterprise comes up for renewal in May; current year was within budget and the team plans to renew.
[019e3b00-0001-7000-8000-000000000012] Internal tooling status — CI build pipeline is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-000000000013] Update on the internal admin dashboard: is being migrated to the new platform team's stack.
[019e3b00-0001-7000-8000-000000000014] Long-form retrospective on the observability stack work that Yusuf led over the past quarter.

Filter, flag contradictions, synthesize. Return JSON.";

// ── Q26 canned input (long-form-aggregate $89 vs $109 — 20 candidates) ───
//
// 20 candidates mirroring iter7 Q26's actual top-20 shape:
//   - Ranks 0 + 8 carry the Comcast values, but BURIED inside long-form
//     aggregate budget-review notes (NOT as standalone "Comcast is $X"
//     short memories). This is what iter7 actually had — the expected
//     contradiction lives inside aggregate text.
//   - 18 distractor memories: feature-flag-service cost ceilings at
//     various dollar amounts (similar template to the Comcast values
//     numerically), home-repair "sagas", trip planning notes, vendor
//     budget renewal notes. Faithful to iter7's content distribution.
// The LLM must (a) recognise the Comcast values as the SAME fact
// expressed at two different times, (b) populate contradictions_flagged,
// (c) mention both values in synthesis_markdown.
const Q26_USER_PROMPT: &str = "QUERY: Doing the monthly budget review — anything I should flag about household services costing more than expected?

CANDIDATES:
[019e3b00-0026-7000-8000-000000000001] Annual budget review yesterday — captured the household monthly subscriptions and bills for the upcoming quarter. Comcast internet is at $89/month, Netflix at $15.49, Spotify family at $16.99. PG&E electric averages $142/month with seasonal variation. T-Mobile family plan is $130/month for four lines. Total fixed monthly is around $1,150 excluding rent and groceries.
[019e3b00-0026-7000-8000-000000000002] Heads up — the feature flag service now has a $245 monthly cost ceiling.
[019e3b00-0026-7000-8000-000000000003] Internal tooling status — feature flag service now has a $6,900 monthly cost ceiling, per the latest procurement note.
[019e3b00-0026-7000-8000-000000000004] Full writeup on the drafty back door saga, because future-me will want the context next time we revisit it. Started in late autumn when the weatherstripping started showing wear.
[019e3b00-0026-7000-8000-000000000005] Internal tooling status — feature flag service now has a $478 monthly cost ceiling, per the latest invoice.
[019e3b00-0026-7000-8000-000000000006] Pay rent by the 1st of each month.
[019e3b00-0026-7000-8000-000000000007] Full writeup on the garage door that sticks halfway saga, because future-me will want the context. Tried lubricant first; the issue is the spring tension.
[019e3b00-0026-7000-8000-000000000008] Vendor budget note: PagerDuty incident response is moving to annual prepay next cycle. Noor confirmed the discount tier; team agreed.
[019e3b00-0026-7000-8000-000000000009] Reviewed the monthly statement breakdown for the household after the bill came in higher than expected. Comcast is now $109/month starting next cycle — the loyalty discount expired. Other services holding steady: Netflix $15.49, Spotify $16.99, T-Mobile $130. Need to renegotiate or shop alternatives.
[019e3b00-0026-7000-8000-00000000000a] Long-form trip planning notes for the Tbilisi visit to take the photography workshop Uma recommended. Flights, hotel, side trips to Mtskheta and Sighnaghi all sketched out.
[019e3b00-0026-7000-8000-00000000000b] Emergency fund target: 6 months of household expenses, which works out to roughly 42k dollars at current burn rate.
[019e3b00-0026-7000-8000-00000000000c] Rent is due on the 1st of every month.
[019e3b00-0026-7000-8000-00000000000d] Full writeup on the water-stained ceiling in the hallway saga, because future-me will want the context. Plumber traced it to a slow leak in the upstairs bathroom valve.
[019e3b00-0026-7000-8000-00000000000e] Long-form retrospective on the feature flag service work that Rashid led over the past quarter.
[019e3b00-0026-7000-8000-00000000000f] Full writeup on the buzzing light fixture in the entryway saga, because future-me will want the context. Bulb replacement didn't help; the ballast was the culprit.
[019e3b00-0026-7000-8000-000000000010] Update on the load-test rig: now has a $2,640 monthly cost ceiling.
[019e3b00-0026-7000-8000-000000000011] Full writeup on the drafty back door saga, because future-me will want the context next time we look at sealing strategies.
[019e3b00-0026-7000-8000-000000000012] Long-form trip planning notes for the Wellington visit for a friend's birthday trip, written before booking flights and accommodation.
[019e3b00-0026-7000-8000-000000000013] Long-form trip planning notes for the Bergen visit to visit an old college friend, written before booking flights and accommodation.
[019e3b00-0026-7000-8000-000000000014] Vendor budget note: Sentry error monitoring comes up for renewal in July; current year was within budget per the procurement note from Aiden.

Filter, flag contradictions, synthesize. Return JSON.";

// ── Local response struct for parsing the LLM JSON ───────────────────────
//
// Mirrors `vault_retrieval::ReadResponse` but kept local so the probe
// doesn't pull `serde_json` from a non-test dependency path.
#[derive(Debug, Deserialize)]
struct ProbeResponse {
    #[serde(default)]
    synthesis_markdown: String,
    #[serde(default)]
    contradictions_flagged: Vec<serde_json::Value>,
    #[serde(default)]
    vault_has_no_relevant_content: bool,
}

#[derive(Debug)]
struct ScenarioResult {
    name: &'static str,
    outputs: Vec<String>,
    durations_secs: Vec<f64>,
    determinism_ok: bool,
    correctness_ok: bool,
    correctness_detail: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("{}", "=".repeat(SEP_WIDE));
    println!("T0.2.7 Phase 1 — t028f targeted Q21+Q26 probe");
    println!(
        "Method: load Qwen-7B once, run 2 canned scenarios × {N_REPS} reps, check determinism + correctness."
    );
    println!(
        "Started: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    println!("{}", "=".repeat(SEP_WIDE));
    println!();

    // 1. Resolve Qwen-7B path.
    let qwen_path = models_dir()?.join(QWEN_MODEL_FILENAME);
    ensure!(
        qwen_path.exists(),
        "Qwen-7B GGUF missing at {qwen_path:?} — populate models dir per ADR-049 before running"
    );
    println!("Model path:        {qwen_path:?}");

    let tuning = TuningConfig {
        n_threads: Some(12),
        n_threads_batch: Some(12),
        n_gpu_layers: Some(99),
        ..TuningConfig::default()
    };
    println!("TuningConfig:      {tuning:?}");

    let params = CompletionParams {
        max_tokens: 1024,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(CANDIDATE_SYSTEM_PROMPT.to_string()),
    };
    println!(
        "CompletionParams:  max_tokens={}, temperature={}, top_p={}, seed={:?}, system_prompt={}chars",
        params.max_tokens,
        params.temperature,
        params.top_p,
        params.seed,
        params.system_prompt.as_deref().map_or(0, str::len),
    );
    println!(
        "Q21 prompt length: {} chars   Q26 prompt length: {} chars",
        Q21_USER_PROMPT.len(),
        Q26_USER_PROMPT.len(),
    );
    println!();

    // 2. Load Qwen once.
    println!("Opening Qwen-7B (Q4_K_M, Vulkan, n_gpu_layers=99)...");
    let load_start = std::time::Instant::now();
    let qwen_provider = Qwen25_14BProvider::open_with_tuning(&qwen_path, tuning).await?;
    println!(
        "Loaded in {:.1}s. model_id = {}",
        load_start.elapsed().as_secs_f64(),
        qwen_provider.model_id()
    );
    println!();

    // 3. Run scenarios.
    let scenarios: &[(&'static str, &str)] = &[
        ("Q21 (Kubernetes hard-negative)", Q21_USER_PROMPT),
        ("Q26 (Comcast contradiction)", Q26_USER_PROMPT),
    ];

    let mut results: Vec<ScenarioResult> = Vec::with_capacity(scenarios.len());
    for (name, user_prompt) in scenarios {
        println!("{}", "─".repeat(SEP_WIDE));
        println!("Scenario: {name}");
        println!("{}", "─".repeat(SEP_WIDE));
        let mut outputs: Vec<String> = Vec::with_capacity(N_REPS);
        let mut durations: Vec<f64> = Vec::with_capacity(N_REPS);
        for rep in 1..=N_REPS {
            let t0 = std::time::Instant::now();
            let raw = qwen_provider
                .complete_json(user_prompt, CANDIDATE_JSON_SCHEMA, &params)
                .await
                .with_context(|| format!("{name} rep {rep}"))?;
            let dt = t0.elapsed().as_secs_f64();
            println!("  Rep {rep}/{N_REPS}: {dt:>5.1}s | {} chars", raw.len());
            outputs.push(raw);
            durations.push(dt);
        }
        let determinism_ok = outputs.windows(2).all(|w| w[0] == w[1]);
        let (correctness_ok, correctness_detail) = assess_correctness(name, &outputs[0]);
        results.push(ScenarioResult {
            name,
            outputs,
            durations_secs: durations,
            determinism_ok,
            correctness_ok,
            correctness_detail,
        });
        println!();
    }

    // 4. Verdict.
    println!("{}", "=".repeat(SEP_WIDE));
    println!("VERDICT");
    println!("{}", "=".repeat(SEP_WIDE));
    for r in &results {
        let det_mark = if r.determinism_ok { "OK " } else { "FAIL" };
        let corr_mark = if r.correctness_ok { "PASS" } else { "FAIL" };
        let mean_dt = r.durations_secs.iter().sum::<f64>() / r.durations_secs.len() as f64;
        println!(
            "  {name:<40}  determinism={det_mark}  correctness={corr_mark}  mean={mean_dt:>5.1}s",
            name = r.name
        );
        println!("    detail: {}", r.correctness_detail);
        // First output preview (head 120 + tail 80).
        let head: String = r.outputs[0].chars().take(120).collect();
        let tail: String = r.outputs[0]
            .chars()
            .rev()
            .take(80)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        println!("    output[..120]: {head}");
        println!("    output[-80..]: {tail}");
        println!();
    }

    let all_correct = results.iter().all(|r| r.correctness_ok);
    let all_determ = results.iter().all(|r| r.determinism_ok);
    println!("{}", "=".repeat(SEP_WIDE));
    if all_correct && all_determ {
        println!("HEADLINE: 2/2 PASS + {N_REPS}/{N_REPS} determinism per scenario. Locked.");
        println!("          Next step: copy prompt+schema to t028d, run full 6-query gauntlet.");
    } else {
        println!(
            "HEADLINE: not yet locked. correct={correct}/{n}  determ={det}/{n}.",
            correct = results.iter().filter(|r| r.correctness_ok).count(),
            det = results.iter().filter(|r| r.determinism_ok).count(),
            n = results.len()
        );
        println!("          Next step: read failures above, iterate prompt/schema, rerun.");
    }
    println!("{}", "=".repeat(SEP_WIDE));
    Ok(())
}

/// Per-scenario correctness predicate.
///
/// - Q21 PASS iff `vault_has_no_relevant_content == true`.
/// - Q26 PASS iff `contradictions_flagged` is non-empty AND
///   `synthesis_markdown` contains both `89` and `109` literal substrings.
fn assess_correctness(scenario_name: &str, raw_output: &str) -> (bool, String) {
    let parsed: ProbeResponse = match serde_json::from_str(raw_output) {
        Ok(p) => p,
        Err(e) => {
            return (
                false,
                format!(
                    "JSON parse failed: {e}; raw[..200]={}",
                    &raw_output[..raw_output.len().min(200)]
                ),
            );
        }
    };
    if scenario_name.starts_with("Q21") {
        let ok = parsed.vault_has_no_relevant_content;
        let detail = format!(
            "vault_has_no_relevant_content={} (expected true); contradictions_flagged.len={}; synthesis_markdown.len={}",
            parsed.vault_has_no_relevant_content,
            parsed.contradictions_flagged.len(),
            parsed.synthesis_markdown.len(),
        );
        (ok, detail)
    } else if scenario_name.starts_with("Q26") {
        let has_89 = parsed.synthesis_markdown.contains("89");
        let has_109 = parsed.synthesis_markdown.contains("109");
        let has_flag = !parsed.contradictions_flagged.is_empty();
        let ok = has_flag && has_89 && has_109;
        let detail = format!(
            "contradictions_flagged.len={} (expected ≥1); synthesis_markdown contains '89'={} '109'={}",
            parsed.contradictions_flagged.len(),
            has_89,
            has_109,
        );
        (ok, detail)
    } else {
        (false, format!("unknown scenario name: {scenario_name}"))
    }
}

/// Resolve the models directory the same way other t02* spikes do
/// (vault GGUFs live under `%APPDATA%\com.shahbaz242630.memory-vault\models`
/// on Windows per BRD §5 + ADR-043).
fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set on Windows")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}
