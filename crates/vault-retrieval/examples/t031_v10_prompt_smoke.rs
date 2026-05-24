//! T0.2.7 Phase 5 Step 2 v10 prompt LLM smoke — answer "does Qwen-7B
//! actually understand the rank-indexed candidate format AND emit valid
//! JSON with rank strings in `memory_ids`?"
//!
//! Surfaced 2026-05-23 after the t030 byte-equality probe proved
//! retrieval is deterministic and the variance was UUID-driven prompt
//! noise. The fix (rank-indexed candidate prefixes `[1] <content>`,
//! `[2] <content>`, ...) is structural but unverified at the LLM end —
//! does Qwen-7B follow the v10 prompt's "use rank strings as
//! contradictions_flagged.memory_ids" instruction? Code-level unit
//! tests pin the prompt format; this probe pins the LLM behavior on
//! that format.
//!
//! # What this probe measures
//!
//! Build a tiny hardcoded 3-memory corpus with one obvious contradiction
//! pair (Q1 2027 GA vs Q2 2027 GA — the exact shape Q25 exercises) and
//! one distractor. Bypass retrieval entirely — feed the 3 memories as
//! `RetrievedMemory` directly through [`build_user_prompt`] and call
//! Qwen-7B with the v10 [`READ_TIME_SYSTEM_PROMPT`]. Parse the output
//! and assert:
//!
//! - Valid JSON matching [`ReadResponse`].
//! - `vault_has_no_relevant_content == false` (the contradiction pair
//!   IS relevant to the GA launch question).
//! - `contradictions_flagged` is non-empty (the model detected the
//!   Q1↔Q2 disagreement).
//! - At least one entry's `memory_ids` contains rank strings (`"1"` /
//!   `"2"`) and NOT UUIDs. This is the load-bearing v10 contract.
//! - Both literal positions (`"Q1 2027"` and `"Q2 2027"`) appear in
//!   the structured `positions` field somewhere across the flagged
//!   entries.
//!
//! # Running
//!
//! ```powershell
//! $env:LIBCLANG_PATH = "C:\Users\shahb\scoop\apps\llvm\current\bin"
//! $env:PATH = "$env:LIBCLANG_PATH;$env:PATH"
//! cargo run -p vault-retrieval --example t031_v10_prompt_smoke --release
//! ```
//!
//! Expected wall ~3-5 min: ~30s release relink + ~15-20s Qwen-7B load
//! + 1 × ~30-90s inference + parse.

// File-level `#![cfg(target_os = "windows")]` was struck at T0.2.3 close
// commit 11 fix-forward (2026-05-24): non-Windows CI hit `error[E0601]:
// main function not found in crate`. Replaced with per-item cfg + non-
// Windows stub `main` below; file-level allow suppresses unused/dead-code
// warnings for the now-unreachable helpers on non-Windows. Per
// [[cfg-gate-transitively-platform-only-items]].
#![cfg_attr(not(target_os = "windows"), allow(unused, dead_code))]

#[cfg(not(target_os = "windows"))]
fn main() {
    eprintln!(
        "t031 is a Windows-only spike artifact (Vulkan llama-cpp backend). \
         Skipped on this platform."
    );
}

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{ensure, Context, Result};

use vault_core::{Boundary, Memory, MemoryType, NewMemory};
use vault_llm::{CompletionParams, LlmProvider, Qwen25_14BProvider, TuningConfig};
use vault_retrieval::read_pipeline::{
    build_user_prompt, ContradictionRef, ReadResponse, READ_TIME_JSON_SCHEMA,
    READ_TIME_SYSTEM_PROMPT,
};
use vault_retrieval::RetrievedMemory;

#[cfg(target_os = "windows")]
#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let started = chrono::Utc::now();
    println!(
        "T0.2.7 t031 v10-prompt LLM smoke — started {}",
        started.format("%Y-%m-%d %H:%M:%S UTC")
    );

    // ── Hardcoded 3-memory corpus ────────────────────────────────────────
    //
    // Memory 1 + Memory 2 form the contradiction pair (Q1 2027 vs Q2 2027 GA).
    // Memory 3 is a topically-adjacent distractor (Dec 5 beta launch — same
    // product area, different date axis, NOT a contradiction with Q1/Q2 GA).
    let mem1 = build_memory(
        "work",
        "Product leadership review yesterday: current plan is Q1 2027 GA. \
         Targeting late February for the public launch announcement and \
         early March for the press push. Stakeholders aligned: CEO, CRO, \
         CMO, VP-eng all signed off.",
    )?;
    let mem2 = build_memory(
        "work",
        "Product strategy session this morning — moving GA to Q2 2027 \
         based on the latest beta-readiness assessment. The analytics \
         module that customer interviews flagged as a launch-blocker \
         won't have its V1 feature set complete until late February; \
         pushing GA into Q2 gives the analytics team a clean 8 weeks.",
    )?;
    let mem3 = build_memory(
        "work",
        "Beta launch scheduled for December 5 per the product team's \
         go-to-market plan. Rollout in three waves: first 50 \
         design-partner customers on Dec 5, expand to 500 paid waitlist \
         on Dec 12, open self-serve signup on Dec 19.",
    )?;

    // Candidate list order (matches what retrieval would surface; the
    // contradiction pair are positions 1 and 2 in the prompt).
    let candidates = vec![
        RetrievedMemory {
            memory: mem1.clone(),
            score: 0.91,
            explanation: "t031 hardcoded candidate 1 (contradiction pair member)".to_string(),
        },
        RetrievedMemory {
            memory: mem2.clone(),
            score: 0.88,
            explanation: "t031 hardcoded candidate 2 (contradiction pair member)".to_string(),
        },
        RetrievedMemory {
            memory: mem3.clone(),
            score: 0.74,
            explanation: "t031 hardcoded candidate 3 (distractor)".to_string(),
        },
    ];

    let query = "What's the GA launch date?";
    let prompt = build_user_prompt(query, &candidates);

    println!("\n=== Constructed v10 prompt ===");
    println!("--- BEGIN PROMPT ---");
    println!("{prompt}");
    println!("--- END PROMPT ---");
    println!("Prompt bytes: {}", prompt.len());

    // ── Load Qwen-7B with locked V0.2 TuningConfig ───────────────────────
    println!("\nOpening Qwen2.5-7B-Instruct with locked V0.2 TuningConfig...");
    let qwen_path = models_dir()?.join("Qwen2.5-7B-Instruct-Q4_K_M.gguf");
    ensure!(qwen_path.exists(), "Qwen-7B GGUF missing at {qwen_path:?}");
    let tuning = TuningConfig {
        n_threads: Some(12),
        n_threads_batch: Some(12),
        n_gpu_layers: Some(99),
        ..TuningConfig::default()
    };
    let load_t0 = Instant::now();
    let qwen = Qwen25_14BProvider::open_with_tuning(&qwen_path, tuning).await?;
    println!(
        "Qwen-7B ready in {:.1}s (id={})",
        load_t0.elapsed().as_secs_f64(),
        qwen.model_id()
    );

    // ── Run one inference against the v10 prompt + v10 system prompt ─────
    let params = CompletionParams {
        max_tokens: 1024,
        temperature: 0.0,
        top_p: 1.0,
        seed: Some(42),
        system_prompt: Some(READ_TIME_SYSTEM_PROMPT.to_string()),
    };
    println!("\nInvoking Qwen with v10 system prompt + v10 user prompt...");
    let infer_t0 = Instant::now();
    let raw = qwen
        .complete_json(&prompt, READ_TIME_JSON_SCHEMA, &params)
        .await
        .context("Qwen complete_json")?;
    let infer_wall = infer_t0.elapsed();
    println!("Inference took {:.1}s", infer_wall.as_secs_f64());

    println!("\n=== Raw LLM output ===");
    println!("{raw}");

    // ── Parse + assert ───────────────────────────────────────────────────
    let parsed: ReadResponse = serde_json::from_str(&raw)
        .with_context(|| format!("parse ReadResponse from raw output: {raw:?}"))?;

    println!("\n=== Parsed ReadResponse ===");
    println!(
        "vault_has_no_relevant_content = {}",
        parsed.vault_has_no_relevant_content
    );
    println!("synthesis_markdown = {:?}", parsed.synthesis_markdown);
    println!(
        "contradictions_flagged = {} entries",
        parsed.contradictions_flagged.len()
    );
    for (i, c) in parsed.contradictions_flagged.iter().enumerate() {
        println!(
            "  [{i}] memory_ids = {:?}, positions = {:?}, current = {:?}",
            c.memory_ids, c.positions, c.current_position_if_determinable
        );
    }

    // ── Pass/fail checks ─────────────────────────────────────────────────
    println!("\n=== Assertions ===");
    let mut all_pass = true;

    // 1. vault_has_no_relevant_content should be false (the Q1/Q2 pair IS
    //    relevant to a GA-launch-date question)
    let relevant_ok = !parsed.vault_has_no_relevant_content;
    println!(
        "  [{}] vault_has_no_relevant_content == false",
        if relevant_ok { "PASS" } else { "FAIL" }
    );
    all_pass &= relevant_ok;

    // 2. contradictions_flagged is non-empty
    let detected_ok = !parsed.contradictions_flagged.is_empty();
    println!(
        "  [{}] contradictions_flagged non-empty (got {} entries)",
        if detected_ok { "PASS" } else { "FAIL" },
        parsed.contradictions_flagged.len()
    );
    all_pass &= detected_ok;

    // 3. At least one entry's memory_ids contains rank strings (NOT UUIDs)
    let rank_format_ok = parsed
        .contradictions_flagged
        .iter()
        .any(memory_ids_look_like_ranks);
    println!(
        "  [{}] At least one entry has memory_ids in rank-string format (\"1\"/\"2\"/...)",
        if rank_format_ok { "PASS" } else { "FAIL" }
    );
    all_pass &= rank_format_ok;

    // 4. No memory_ids entry anywhere contains a UUID-shaped string
    let no_uuids_ok = parsed
        .contradictions_flagged
        .iter()
        .flat_map(|c| c.memory_ids.iter())
        .all(|s| !looks_like_uuid(s));
    println!(
        "  [{}] No memory_ids entry contains a UUID-shaped substring",
        if no_uuids_ok { "PASS" } else { "FAIL" }
    );
    all_pass &= no_uuids_ok;

    // 5. Both literal positions ("Q1 2027" + "Q2 2027") appear somewhere
    //    in the structured positions field across all flagged entries
    let all_positions: Vec<&str> = parsed
        .contradictions_flagged
        .iter()
        .flat_map(|c| c.positions.iter().map(|s| s.as_str()))
        .collect();
    let has_q1 = all_positions.iter().any(|p| p.contains("Q1 2027"));
    let has_q2 = all_positions.iter().any(|p| p.contains("Q2 2027"));
    println!(
        "  [{}] Both literal positions (Q1 2027 AND Q2 2027) present in structured positions field",
        if has_q1 && has_q2 { "PASS" } else { "FAIL" }
    );
    all_pass &= has_q1 && has_q2;

    println!(
        "\n=== Overall: {} ===",
        if all_pass { "PASS" } else { "FAIL" }
    );

    if !all_pass {
        anyhow::bail!("v10 prompt smoke FAILED — see assertions above");
    }
    Ok(())
}

/// Check whether all entries in a ContradictionRef's `memory_ids` look
/// like rank references. Two valid forms are accepted (semantically
/// identical — both unambiguously identify a candidate position, and
/// neither leaks UUIDs):
///
/// 1. Pure-digit strings: `"1"`, `"2"`, `"10"`, ...
/// 2. Bracketed-digit strings matching the prompt's `[N]` prefix:
///    `"[1]"`, `"[2]"`, `"[10]"`, ...
///
/// The bracketed form is what Qwen-7B actually emits on the v10 prompt
/// (it cites verbatim from the prompt). Empty `memory_ids` does NOT
/// count as rank-format (the LLM should have cited something).
fn memory_ids_look_like_ranks(c: &ContradictionRef) -> bool {
    !c.memory_ids.is_empty() && c.memory_ids.iter().all(|s| is_rank_reference(s))
}

fn is_rank_reference(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let inner = if let Some(stripped) = s.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        stripped
    } else {
        s
    };
    !inner.is_empty() && inner.chars().all(|c| c.is_ascii_digit())
}

/// Check whether a string contains a UUID-shaped 8-4-4-4-12 hex pattern
/// anywhere within it. Defence-in-depth against accidental UUID leakage
/// (e.g. if the LLM ignored the v10 instruction and citied the system's
/// internal IDs from somewhere).
fn looks_like_uuid(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 36 {
        return false;
    }
    for start in 0..=bytes.len() - 36 {
        let window = &bytes[start..start + 36];
        if window[8] == b'-' && window[13] == b'-' && window[18] == b'-' && window[23] == b'-' {
            let all_hex = window
                .iter()
                .enumerate()
                .filter(|(i, _)| ![8, 13, 18, 23].contains(i))
                .all(|(_, b)| b.is_ascii_hexdigit());
            if all_hex {
                return true;
            }
        }
    }
    false
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

fn models_dir() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").context("APPDATA must be set")?;
    Ok(PathBuf::from(appdata)
        .join("com.shahbaz242630.memory-vault")
        .join("models"))
}
