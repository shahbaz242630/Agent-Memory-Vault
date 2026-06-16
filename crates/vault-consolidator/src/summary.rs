//! Markdown summary generator for [`ConsolidationReport::summary_markdown`].
//!
//! Implements BRD §5.6 lines 959-973 — the human-readable summary the user
//! reads after a consolidation run. Per T0.2.3 iteration 3 §item-4 lock,
//! Merges + Contradictions sections contain **per-boundary sub-sections
//! inside the outer Run-scoped document**, NOT separate runs per boundary.
//! Decay section is aggregate (BRD §5.6 line 968 "no per-memory detail").
//!
//! File-placement + signature decisions are documented in ADR-047 (HANDOFF.md).
//!
//! [`ConsolidationReport::summary_markdown`]: crate::consolidator::ConsolidationReport::summary_markdown

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::time::Duration;

use vault_core::Boundary;

use crate::consolidator::{AppliedMergeWithContext, BoundarySummary, ConflictReview, RunState};

/// Snippet length cap (in chars) for pre-merge memory content and merged
/// text in the Merges section. Keeps the document scannable per BRD §5.6
/// line 973 (~500-2000 words total, < 2-minute read).
const SNIPPET_MAX_CHARS: usize = 80;

/// Literal footer phrase pinned by `footer_emits_checkpoint_id_and_rollback_hint`.
/// T0.2.5 (Checkpoint & Rollback) shipped, so the footer now points at the real
/// rollback command; the pinning test catches accidental drift.
const FOOTER_ROLLBACK_HINT: &str =
    "roll it back with `vault-cli checkpoint rollback <checkpoint-id>`";

/// Generate the human-readable Markdown summary for one consolidation run.
///
/// Pure function over the orchestrator's [`RunState`]: no I/O, no async,
/// no clock reads (timestamps live on `RunState`). The orchestrator calls
/// this once at the end of [`Consolidator::run_consolidation`] and stores
/// the result on [`ConsolidationReport::summary_markdown`].
///
/// **`checkpoint_id`:** the real checkpoint id (or `"none (no changes this
/// run)"`) the caller captured this run (T0.2.5). The footer renders it
/// verbatim alongside the `vault-cli checkpoint rollback` command hint.
///
/// [`Consolidator::run_consolidation`]: crate::consolidator::Consolidator::run_consolidation
/// [`ConsolidationReport::summary_markdown`]: crate::consolidator::ConsolidationReport::summary_markdown
pub(crate) fn generate_summary_markdown(state: &RunState, checkpoint_id: &str) -> String {
    let mut out = String::new();
    write_header(&mut out, state);
    write_merges_section(&mut out, &state.per_boundary);
    write_contradictions_section(&mut out, &state.per_boundary);
    write_decay_section(&mut out, state.memories_decayed);
    write_footer(&mut out, checkpoint_id);
    out
}

fn write_header(out: &mut String, state: &RunState) {
    let date = state.started_at.format("%Y-%m-%d");
    writeln!(out, "# Consolidation Run — {date}").expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
    writeln!(
        out,
        "**Duration:** {}",
        format_duration_human(state.duration)
    )
    .expect("writing to String never fails");
    writeln!(
        out,
        "**Total memories processed:** {}",
        state.memories_processed
    )
    .expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
}

fn format_duration_human(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3_600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        let hours = secs / 3_600;
        let mins = (secs % 3_600) / 60;
        let s = secs % 60;
        format!("{hours}h {mins}m {s}s")
    }
}

fn write_merges_section(out: &mut String, per_boundary: &BTreeMap<Boundary, BoundarySummary>) {
    writeln!(out, "## Merges").expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
    for (boundary, summary) in per_boundary {
        if summary.applied_merges.is_empty() {
            continue;
        }
        writeln!(out, "### {}", boundary.as_str()).expect("writing to String never fails");
        writeln!(out).expect("writing to String never fails");
        for amwc in &summary.applied_merges {
            write_merge_entry(out, amwc);
        }
    }
}

fn write_merge_entry(out: &mut String, amwc: &AppliedMergeWithContext) {
    writeln!(
        out,
        "- **Cluster #{} → merged into `{}`** ({} pre-merge memories, summed access count {}, max confidence {:.2})",
        amwc.cluster.id,
        amwc.applied.new_memory_id,
        amwc.cluster.size(),
        amwc.applied.summed_access_count,
        amwc.applied.max_confidence,
    )
    .expect("writing to String never fails");
    writeln!(out, "  - Pre-merge memories:").expect("writing to String never fails");
    for (id, content) in &amwc.pre_merge_contents {
        writeln!(out, "    - `{}` — \"{}\"", id, truncate_snippet(content))
            .expect("writing to String never fails");
    }
    writeln!(
        out,
        "  - Consolidated text: \"{}\"",
        truncate_snippet(&amwc.merged_text)
    )
    .expect("writing to String never fails");
    writeln!(out, "  - LLM reasoning: {}", amwc.reasoning).expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
}

fn truncate_snippet(content: &str) -> String {
    if content.chars().count() <= SNIPPET_MAX_CHARS {
        content.to_string()
    } else {
        let mut truncated: String = content.chars().take(SNIPPET_MAX_CHARS).collect();
        truncated.push('…');
        truncated
    }
}

fn write_contradictions_section(
    out: &mut String,
    per_boundary: &BTreeMap<Boundary, BoundarySummary>,
) {
    writeln!(out, "## Contradictions").expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
    for (boundary, summary) in per_boundary {
        if summary.contradictions.is_empty() {
            continue;
        }
        writeln!(out, "### {}", boundary.as_str()).expect("writing to String never fails");
        writeln!(out).expect("writing to String never fails");
        for conflict in &summary.contradictions {
            write_contradiction_entry(out, conflict);
        }
    }
}

fn write_contradiction_entry(out: &mut String, conflict: &ConflictReview) {
    writeln!(
        out,
        "- **Conflict `{}`** ({} conflicting memories)",
        conflict.conflict_id,
        conflict.conflicting_memory_ids.len()
    )
    .expect("writing to String never fails");
    writeln!(out, "  - Memory IDs:").expect("writing to String never fails");
    for id in &conflict.conflicting_memory_ids {
        writeln!(out, "    - `{id}`").expect("writing to String never fails");
    }
    writeln!(out, "  - LLM reasoning: {}", conflict.reasoning)
        .expect("writing to String never fails");
    writeln!(out, "  - Review queue: (T0.2.15 surfaces a UI link here)")
        .expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
}

fn write_decay_section(out: &mut String, memories_decayed: usize) {
    writeln!(out, "## Decay").expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
    writeln!(out, "**Decayed:** {memories_decayed}").expect("writing to String never fails");
    writeln!(out, "**Archived:** 0").expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
    writeln!(
        out,
        "(Confidence decay is live as of T0.2.4. Cold archive — BRD §5.6 lines \
         995-996 — lands in a follow-up batch; archived count stays 0 until then.)"
    )
    .expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
}

fn write_footer(out: &mut String, checkpoint_id: &str) {
    writeln!(out, "## Footer").expect("writing to String never fails");
    writeln!(out).expect("writing to String never fails");
    writeln!(out, "**Checkpoint ID:** {checkpoint_id}").expect("writing to String never fails");
    writeln!(out, "If this run looks wrong, {FOOTER_ROLLBACK_HINT}.")
        .expect("writing to String never fails");
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use uuid::Uuid;
    use vault_core::MemoryId;

    use crate::phases::cluster::Cluster;
    use crate::phases::merge::AppliedMerge;

    // ── Test helpers ─────────────────────────────────────────────────────────

    fn fixed_started_at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 14, 3, 0, 0).unwrap()
    }

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("test boundary must validate")
    }

    fn fresh_memory_id() -> MemoryId {
        MemoryId(Uuid::now_v7())
    }

    fn empty_run_state() -> RunState {
        RunState {
            started_at: fixed_started_at(),
            duration: Duration::from_secs(12),
            memories_processed: 0,
            memories_decayed: 0,
            per_boundary: BTreeMap::new(),
        }
    }

    fn run_state_with_one_merge_in_work() -> RunState {
        let work = boundary("work");
        let id_a = fresh_memory_id();
        let id_b = fresh_memory_id();
        let new_id = fresh_memory_id();
        let amwc = AppliedMergeWithContext {
            cluster: Cluster {
                id: 0,
                member_row_ids: vec![id_a, id_b],
            },
            applied: AppliedMerge {
                new_memory_id: new_id,
                superseded_memory_ids: vec![id_a, id_b],
                summed_access_count: 7,
                max_confidence: 0.95,
            },
            reasoning: "Both memories paraphrase the same fact.".to_string(),
            merged_text: "Consolidated content describing the same fact.".to_string(),
            pre_merge_contents: vec![
                (
                    id_a,
                    "Original memory A content for the work item".to_string(),
                ),
                (
                    id_b,
                    "Original memory B content for the same work item".to_string(),
                ),
            ],
        };
        let mut summary = BoundarySummary::default();
        summary.applied_merges.push(amwc);

        let mut per_boundary = BTreeMap::new();
        per_boundary.insert(work, summary);

        RunState {
            started_at: fixed_started_at(),
            duration: Duration::from_secs(45),
            memories_processed: 2,
            memories_decayed: 0,
            per_boundary,
        }
    }

    fn run_state_with_one_contradiction_in_personal() -> RunState {
        let personal = boundary("personal");
        let id_a = fresh_memory_id();
        let id_b = fresh_memory_id();
        let conflict = ConflictReview {
            conflict_id: Uuid::nil(),
            boundary: personal.clone(),
            conflicting_memory_ids: vec![id_a, id_b],
            reasoning: "Both reference the same bill but assert conflicting amounts.".to_string(),
            flagged_at: fixed_started_at(),
        };
        let mut summary = BoundarySummary::default();
        summary.contradictions.push(conflict);

        let mut per_boundary = BTreeMap::new();
        per_boundary.insert(personal, summary);

        RunState {
            started_at: fixed_started_at(),
            duration: Duration::from_secs(30),
            memories_processed: 2,
            memories_decayed: 0,
            per_boundary,
        }
    }

    /// Two-boundary fixture for the boundary-separation invariant test. Work
    /// boundary has one merge; personal boundary has one contradiction. Each
    /// boundary's strings carry a distinctive token (`WORK_*` / `PERSONAL_*`)
    /// so the cross-leak assertion can locate them.
    fn run_state_with_work_merges_and_personal_contradictions() -> RunState {
        let work = boundary("work");
        let personal = boundary("personal");

        // Work boundary — one merge with distinctive tokens.
        let w_id_a = fresh_memory_id();
        let w_id_b = fresh_memory_id();
        let w_new = fresh_memory_id();
        let w_amwc = AppliedMergeWithContext {
            cluster: Cluster {
                id: 0,
                member_row_ids: vec![w_id_a, w_id_b],
            },
            applied: AppliedMerge {
                new_memory_id: w_new,
                superseded_memory_ids: vec![w_id_a, w_id_b],
                summed_access_count: 4,
                max_confidence: 0.92,
            },
            reasoning: "WORK_REASONING_TOKEN".to_string(),
            merged_text: "WORK_MERGED_TEXT_TOKEN".to_string(),
            pre_merge_contents: vec![
                (w_id_a, "WORK_PREMERGE_CONTENT_A".to_string()),
                (w_id_b, "WORK_PREMERGE_CONTENT_B".to_string()),
            ],
        };
        let mut w_summary = BoundarySummary::default();
        w_summary.applied_merges.push(w_amwc);

        // Personal boundary — one contradiction with a distinctive token.
        let p_id_a = fresh_memory_id();
        let p_id_b = fresh_memory_id();
        let p_conflict = ConflictReview {
            conflict_id: Uuid::nil(),
            boundary: personal.clone(),
            conflicting_memory_ids: vec![p_id_a, p_id_b],
            reasoning: "PERSONAL_REASONING_TOKEN".to_string(),
            flagged_at: fixed_started_at(),
        };
        let mut p_summary = BoundarySummary::default();
        p_summary.contradictions.push(p_conflict);

        let mut per_boundary = BTreeMap::new();
        per_boundary.insert(work, w_summary);
        per_boundary.insert(personal, p_summary);

        RunState {
            started_at: fixed_started_at(),
            duration: Duration::from_secs(60),
            memories_processed: 4,
            memories_decayed: 0,
            per_boundary,
        }
    }

    // ── Markdown unit tests (6) ─────────────────────────────────────────────

    /// Test 1: Run header contains date + duration + total memories processed
    /// per BRD §5.6 line 965.
    #[test]
    fn header_includes_date_duration_and_total_processed() {
        let state = empty_run_state();
        let md = generate_summary_markdown(&state, "test-cp-001");
        assert!(
            md.contains("# Consolidation Run — 2026-05-14"),
            "header date missing:\n{md}"
        );
        assert!(
            md.contains("**Duration:** 12s"),
            "header duration missing or wrong format:\n{md}"
        );
        assert!(
            md.contains("**Total memories processed:** 0"),
            "header total missing:\n{md}"
        );
    }

    /// Test 2: Per-boundary Merges sub-section renders pre-merge IDs +
    /// content snippets, consolidated text, and LLM reasoning per BRD §5.6
    /// line 966.
    #[test]
    fn per_boundary_merges_section_renders_pre_post_and_reasoning() {
        let state = run_state_with_one_merge_in_work();
        let md = generate_summary_markdown(&state, "test-cp-002");
        assert!(md.contains("## Merges"), "Merges section header missing");
        assert!(
            md.contains("### work"),
            "per-boundary `work` sub-header missing"
        );
        assert!(
            md.contains("Original memory A content"),
            "pre-merge content A snippet missing"
        );
        assert!(
            md.contains("Original memory B content"),
            "pre-merge content B snippet missing"
        );
        assert!(
            md.contains("Consolidated content describing"),
            "consolidated text snippet missing"
        );
        assert!(
            md.contains("Both memories paraphrase the same fact"),
            "LLM reasoning missing"
        );
    }

    /// Test 3: Per-boundary Contradictions sub-section renders the
    /// conflicting memory IDs + LLM reasoning + a review-queue placeholder
    /// pointing at T0.2.15 per BRD §5.6 line 967.
    #[test]
    fn per_boundary_contradictions_section_emits_review_queue_placeholder() {
        let state = run_state_with_one_contradiction_in_personal();
        let md = generate_summary_markdown(&state, "test-cp-003");
        assert!(
            md.contains("## Contradictions"),
            "Contradictions section header missing"
        );
        assert!(
            md.contains("### personal"),
            "per-boundary `personal` sub-header missing"
        );
        assert!(
            md.contains("Both reference the same bill"),
            "contradiction reasoning missing"
        );
        assert!(
            md.contains("T0.2.15"),
            "review queue T0.2.15 placeholder missing — Tauri viewer is the downstream consumer"
        );
    }

    /// Test 4: Decay section renders the aggregate decayed count (BRD §5.6 line
    /// 968 "no per-memory detail"). An empty run decays nothing → 0. Archive
    /// stays 0 (cold archive is the deferred follow-up); the note names it.
    #[test]
    fn decay_aggregate_section_renders_zero_for_empty_run() {
        let state = empty_run_state();
        let md = generate_summary_markdown(&state, "test-cp-004");
        assert!(md.contains("## Decay"), "Decay section header missing");
        assert!(
            md.contains("**Decayed:** 0"),
            "an empty run decays nothing — count must be 0"
        );
        assert!(
            md.contains("**Archived:** 0"),
            "archived count must be 0 — cold archive is a follow-up batch"
        );
        assert!(
            md.contains("Cold archive"),
            "the archive-deferral note must name cold archive as the follow-up"
        );
    }

    /// Test 4b: a run that decayed N facts renders `**Decayed:** N`. Pins the
    /// Phase-4 count flow (RunState.memories_decayed → summary).
    #[test]
    fn decay_section_renders_nonzero_count() {
        let mut state = empty_run_state();
        state.memories_decayed = 7;
        let md = generate_summary_markdown(&state, "test-cp-004b");
        assert!(
            md.contains("**Decayed:** 7"),
            "non-zero decayed count must render verbatim:\n{md}"
        );
    }

    /// Test 5: Footer emits **both** the checkpoint-ID line (reflecting the
    /// `checkpoint_id` input verbatim) **and** the shipped rollback command
    /// hint. Two distinct floor-pin assertions in one `#[test] fn` so any
    /// rephrasing consciously updates BOTH simultaneously.
    #[test]
    fn footer_emits_checkpoint_id_and_rollback_hint() {
        let state = empty_run_state();
        let md = generate_summary_markdown(&state, "test-cp-005");

        // Assertion #1: checkpoint-ID line reflects the input verbatim.
        assert!(
            md.contains("**Checkpoint ID:** test-cp-005"),
            "checkpoint-ID line missing or malformed:\n{md}"
        );

        // Assertion #2: the shipped rollback command hint (T0.2.5). Update this
        // in lockstep with FOOTER_ROLLBACK_HINT.
        assert!(
            md.contains("vault-cli checkpoint rollback"),
            "rollback command hint missing — see FOOTER_ROLLBACK_HINT"
        );
    }

    /// Test 6: Boundary-separation privacy invariant per BRD §5.6 line 971
    /// verbatim "No cross-boundary summarization." For a run touching two
    /// boundaries (work + personal), the work section MUST contain zero
    /// substrings drawn from personal's memory content AND vice versa.
    /// Distinct invariant from per-boundary rendering correctness; cheap
    /// defense-in-depth on a privacy surface.
    #[test]
    fn boundary_separation_no_cross_boundary_content_leak() {
        let state = run_state_with_work_merges_and_personal_contradictions();
        let md = generate_summary_markdown(&state, "test-cp-006");

        // Test setup sanity — both boundaries must actually render.
        assert!(
            md.contains("### work"),
            "work sub-header missing — test setup broken:\n{md}"
        );
        assert!(
            md.contains("### personal"),
            "personal sub-header missing — test setup broken:\n{md}"
        );

        // Extract the work-content block (### work … ## Contradictions).
        let work_header_pos = md.find("### work").expect("### work sub-header");
        let after_work = &md[work_header_pos..];
        let work_end_offset = after_work
            .find("## Contradictions")
            .expect("Contradictions section follows Merges");
        let work_block = &after_work[..work_end_offset];

        // Extract the personal-content block (### personal … ## Decay).
        let personal_header_pos = md.find("### personal").expect("### personal sub-header");
        let after_personal = &md[personal_header_pos..];
        let personal_end_offset = after_personal
            .find("## Decay")
            .expect("Decay section follows Contradictions");
        let personal_block = &after_personal[..personal_end_offset];

        // Cross-leak assertion: work tokens MUST NOT appear in personal block.
        let work_tokens = [
            "WORK_REASONING_TOKEN",
            "WORK_MERGED_TEXT_TOKEN",
            "WORK_PREMERGE_CONTENT_A",
            "WORK_PREMERGE_CONTENT_B",
        ];
        for token in &work_tokens {
            assert!(
                !personal_block.contains(token),
                "boundary-separation breach: work token `{token}` leaked into personal section.\n\nPersonal block:\n{personal_block}"
            );
        }

        // Cross-leak assertion: personal tokens MUST NOT appear in work block.
        let personal_tokens = ["PERSONAL_REASONING_TOKEN"];
        for token in &personal_tokens {
            assert!(
                !work_block.contains(token),
                "boundary-separation breach: personal token `{token}` leaked into work section.\n\nWork block:\n{work_block}"
            );
        }
    }

    /// Test 7: `truncate_snippet` clips at the char ceiling with an ellipsis,
    /// counts chars (not bytes — multi-byte UTF-8 safe), and uses mid-word
    /// truncation (not word-boundary aware — pinning current behavior so a
    /// later "smart truncation" change is a conscious choice). Floor pin
    /// added after T0.2.3 commit 3 plan-iteration 2 fixture rewrite surfaced
    /// the realism gap — pre-rewrite no test exercised the truncation path
    /// because all fixture content was below the 80-char cap.
    #[test]
    fn truncate_snippet_clips_at_char_ceiling_with_ellipsis() {
        // Content under cap returns unchanged (no ellipsis added).
        let short = "hello world";
        assert_eq!(truncate_snippet(short), "hello world");

        // At-exactly-cap returns unchanged.
        let at_cap: String = "a".repeat(SNIPPET_MAX_CHARS);
        assert_eq!(truncate_snippet(&at_cap), at_cap);

        // Over-cap truncates at SNIPPET_MAX_CHARS chars + appends ellipsis.
        let over_cap: String = "x".repeat(SNIPPET_MAX_CHARS + 50);
        let truncated = truncate_snippet(&over_cap);
        assert_eq!(
            truncated.chars().count(),
            SNIPPET_MAX_CHARS + 1,
            "truncated must be exactly {SNIPPET_MAX_CHARS} chars + 1 ellipsis"
        );
        assert!(
            truncated.ends_with('…'),
            "truncated must end with ellipsis: {truncated}"
        );

        // Multi-byte UTF-8 doesn't panic — char-based truncation, not byte-based.
        // Each "café résumé 数据 🎉 " is 16 chars; ×20 = 320 chars, well over cap.
        let utf8: String = "café résumé 数据 🎉 ".repeat(20);
        let utf8_truncated = truncate_snippet(&utf8);
        assert_eq!(
            utf8_truncated.chars().count(),
            SNIPPET_MAX_CHARS + 1,
            "UTF-8 truncation must count chars, not bytes"
        );
        assert!(utf8_truncated.ends_with('…'));
    }

    /// ADR-047 §b pin: verify the 3 orchestration types remain `pub(crate)`
    /// and reachable from this module. If `consolidator.rs` reverts any of
    /// them to private, this test fails to compile. Compile-time visibility
    /// check — no runtime assertions.
    #[test]
    fn pub_crate_promotion_for_summary_consumption_compiles() {
        fn _accepts_run_state(_s: &crate::consolidator::RunState) {}
        fn _accepts_boundary_summary(_s: &crate::consolidator::BoundarySummary) {}
        fn _accepts_amwc(_s: &crate::consolidator::AppliedMergeWithContext) {}
    }
}
