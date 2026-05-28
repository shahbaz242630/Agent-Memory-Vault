//! Canonical-save normalization for incoming `memory_write` content.
//!
//! Belt-and-braces companion to the `memory_write` MCP tool description's
//! canonical-save contract (T0.2.7 close, 2026-05-25 lock at
//! `vault_mcp::server::tool_write`). The tool description teaches calling
//! agents the six canonical rules; this module catches the most common
//! drift cases server-side so smaller / cheaper LLMs that don't follow
//! the description perfectly still produce canonical-shape memories.
//!
//! ## Why server-side normalization
//!
//! Tool descriptions are guidance, not enforcement. Claude follows them
//! well; GPT mostly does; smaller / older models drift. The cross-platform
//! thesis — Claude saves it today, GPT reads it correctly tomorrow —
//! requires every memory to land in canonical shape regardless of which
//! LLM saved it. Server-side normalization is the floor that holds even
//! when the description-following LLM is the weakest link.
//!
//! ## Rules applied (in order)
//!
//! 1. **Trim** leading/trailing whitespace.
//! 2. **Reject empty** content after trim → `VaultError::InvalidInput`.
//! 3. **Strip conversation framing** prefixes:
//!    - "When [the user was] asked, " / "When asked, " → ""
//!    - "The user told me [that] " / "The user said [that] " / etc. → "The user "
//! 4. **Strip agent self-reference** prefixes (drops the "I think/learned/..."
//!    framing, keeps the underlying fact):
//!    - "I think [that] " / "I believe [that] " / "I learned [that] " /
//!      "I noticed [that] " / "I observed [that] " → ""
//! 5. **Rewrite first-person about-the-user** to third-person:
//!    - "I prefer X" → "The user prefers X" (+s on the verb)
//!    - Same for like / love / hate / use / want / need.
//! 6. **Capitalize** the first character if it's now lowercase (post-strip).
//! 7. **Append terminal period** if missing.
//!
//! Content length is NOT capped here. `vault_core::Memory::validate`
//! enforces the only storage cap (`MAX_MEMORY_CONTENT_BYTES` = 100 KB);
//! the embedder truncates at its 512-token window. Long memories
//! (paragraph-scale session summaries) are stored whole — only the
//! embedding is truncated (store-whole / embed-truncate, 2026-05-28).
//!
//! Auto-fix is silent (per 2026-05-25 product decision) — agents don't
//! see "your content was normalized" feedback. Reasoning: smoother UX,
//! avoids retry round-trips. The tool description does the teaching; this
//! is the safety net.
//!
//! ## Rules NOT applied (deliberately out of scope)
//!
//! - **Grammatical correction** beyond the specific patterns listed. We
//!   don't try to be a grammar engine; that's the calling LLM's job.
//! - **Semantic deduplication.** Catching duplicates is the consolidator's
//!   job, not the write path's. See [[locked-next-arc-t03x]] for the
//!   architectural reasoning.
//! - **Confidence-field validation.** That happens in `vault_core::Memory::
//!   try_new` via `Memory::validate()`. Normalization is content-only.
//! - **Boundary validation.** That happens at the MCP layer
//!   (`StdioServer::handle_write`) before this code runs.

use vault_core::{VaultError, VaultResult};

/// Normalize `content` to canonical-save shape before storage. Returns
/// `VaultError::InvalidInput` only for emptiness violations; shape-fixes
/// (prefix strip, first-person rewrite, period append) are applied
/// silently. Length is not capped here — `vault_core::Memory::validate`
/// enforces the only storage cap (100 KB) and the embedder truncates at
/// its token window (store-whole / embed-truncate, 2026-05-28).
///
/// Idempotent: `normalize(normalize(x)) == normalize(x)` for all `x`
/// that pass the empty check. Pinned by the `normalize_is_idempotent`
/// test below.
pub(crate) fn normalize_for_canonical_save(content: &str) -> VaultResult<String> {
    let trimmed = content.trim();

    if trimmed.is_empty() {
        return Err(VaultError::InvalidInput(
            "memory content cannot be empty".into(),
        ));
    }

    let mut s = trimmed.to_string();
    s = strip_conversation_framing(&s);
    s = strip_agent_self_reference(&s);
    s = rewrite_first_person_to_third(&s);
    s = capitalize_first_char(&s);
    s = ensure_terminal_period(&s);
    Ok(s)
}

/// Strip leading conversation-framing phrases. Case-insensitive on the
/// prefix; the remainder keeps its original casing (so a follow-up
/// capitalization step fixes the new first letter).
fn strip_conversation_framing(s: &str) -> String {
    // Ordered most-specific to least-specific so longer prefixes match
    // first ("When the user was asked," before "When asked,").
    let when_asked_prefixes: &[&str] = &[
        "When the user was asked, ",
        "When the user was asked ",
        "When asked, ",
        "When asked ",
    ];
    for prefix in when_asked_prefixes {
        if let Some(rest) = strip_prefix_ci(s, prefix) {
            return rest.to_string();
        }
    }

    // "The user told me [that] " → "The user "
    // "The user said [that] " → "The user "
    // Plus mentioned / stated. Two-stage: match the verb form, then strip
    // the optional "that " after it.
    let told_me_verbs: &[&str] = &["told me ", "said ", "mentioned ", "stated "];
    for verb in told_me_verbs {
        let full = format!("The user {verb}");
        if let Some(rest) = strip_prefix_ci(s, &full) {
            // Strip optional "that " after the verb.
            let after_that = strip_prefix_ci(rest, "that ").unwrap_or(rest);
            return format!("The user {after_that}");
        }
    }

    s.to_string()
}

/// Strip leading agent self-reference phrases ("I think/believe/...")
/// — drops the agent's epistemic framing, keeps the underlying fact.
fn strip_agent_self_reference(s: &str) -> String {
    let verbs: &[&str] = &[
        "I think ",
        "I believe ",
        "I learned ",
        "I noticed ",
        "I observed ",
    ];
    for verb in verbs {
        if let Some(rest) = strip_prefix_ci(s, verb) {
            // Strip optional "that " after the verb.
            let after_that = strip_prefix_ci(rest, "that ").unwrap_or(rest);
            return after_that.to_string();
        }
    }
    s.to_string()
}

/// Rewrite "I {verb} X" → "The user {verb}s X" for a small fixed verb
/// list. Conservative — only matches at the start of the string and
/// only for these common verbs. More elaborate rewrites are the
/// calling LLM's job per the tool description; this is just the safety
/// net for the most common drift.
fn rewrite_first_person_to_third(s: &str) -> String {
    // Each entry: (from_prefix_lowercase, to_prefix). `from` matched
    // case-insensitively; `to` is fixed-case (always "The user verbs").
    let pairs: &[(&str, &str)] = &[
        ("I prefer ", "The user prefers "),
        ("I like ", "The user likes "),
        ("I love ", "The user loves "),
        ("I hate ", "The user hates "),
        ("I use ", "The user uses "),
        ("I want ", "The user wants "),
        ("I need ", "The user needs "),
    ];
    for (from, to) in pairs {
        if let Some(rest) = strip_prefix_ci(s, from) {
            return format!("{to}{rest}");
        }
    }
    s.to_string()
}

/// Capitalize the first character if it's a lowercase ASCII letter.
/// Non-ASCII first characters (Unicode) are left alone — conservative
/// because Unicode case mapping is locale-dependent and the canonical-
/// save contract assumes English text anyway.
fn capitalize_first_char(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) if first.is_ascii_lowercase() => {
            let upper = first.to_ascii_uppercase();
            let rest: String = chars.collect();
            format!("{upper}{rest}")
        }
        Some(_) => s.to_string(),
    }
}

/// Append `.` if the content doesn't already end in `.`, `!`, or `?`.
fn ensure_terminal_period(s: &str) -> String {
    if s.ends_with('.') || s.ends_with('!') || s.ends_with('?') {
        s.to_string()
    } else {
        format!("{s}.")
    }
}

/// Case-insensitive prefix strip. Returns `Some(rest)` if `s` starts
/// with `prefix` (ignoring ASCII case), else `None`.
///
/// **Unicode safety:** `split_at(prefix.len())` requires the byte index
/// to be a char boundary. If `s` has a multi-byte UTF-8 character that
/// straddles `prefix.len()` (e.g. `prefix = "When the user was asked, "`
/// at 25 bytes and `s = "The user prefers 日本語 ..."` where byte 25 is
/// inside 語), `split_at` panics. We pre-check `is_char_boundary` and
/// return `None` if it fails: the prefix can't match anyway, because a
/// successful prefix match would have produced an ASCII-only head equal
/// to `prefix` byte-for-byte, which means the byte-`prefix.len()`
/// position MUST be a char boundary.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() < prefix.len() || !s.is_char_boundary(prefix.len()) {
        return None;
    }
    let (head, tail) = s.split_at(prefix.len());
    if head.eq_ignore_ascii_case(prefix) {
        Some(tail)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_content() {
        let result = normalize_for_canonical_save("");
        assert!(matches!(result, Err(VaultError::InvalidInput(_))));
    }

    #[test]
    fn rejects_whitespace_only_content() {
        let result = normalize_for_canonical_save("   \n\t  ");
        assert!(matches!(result, Err(VaultError::InvalidInput(_))));
    }

    #[test]
    fn accepts_long_content_above_former_2000_cap() {
        // Store-whole / embed-truncate (2026-05-28): normalization no
        // longer caps length. A paragraph-scale memory well past the old
        // 2000-char sanity cap must pass — `vault_core::Memory::validate`
        // enforces the only storage cap (100 KB) and the embedder
        // truncates at its token window.
        let long = "a".repeat(5000);
        let result = normalize_for_canonical_save(&long).expect("long content must be accepted");
        // Content preserved: first char capitalized + terminal period
        // appended, so 5000 → 5001 chars.
        assert_eq!(result.chars().count(), 5001);
        assert!(result.ends_with('.'));
    }

    #[test]
    fn strips_when_asked_conversation_prefix() {
        let result = normalize_for_canonical_save("When asked, the user prefers Python")
            .expect("valid content");
        assert_eq!(result, "The user prefers Python.");
    }

    #[test]
    fn strips_when_the_user_was_asked_conversation_prefix() {
        let result = normalize_for_canonical_save("When the user was asked, they chose dark mode")
            .expect("valid content");
        assert_eq!(result, "They chose dark mode.");
    }

    #[test]
    fn strips_user_told_me_conversation_prefix() {
        let result = normalize_for_canonical_save("The user told me that they prefer Rust")
            .expect("valid content");
        assert_eq!(result, "The user they prefer Rust.");
    }

    #[test]
    fn strips_agent_self_reference_i_think() {
        let result = normalize_for_canonical_save("I think the user prefers dark mode")
            .expect("valid content");
        assert_eq!(result, "The user prefers dark mode.");
    }

    #[test]
    fn strips_agent_self_reference_i_learned() {
        let result = normalize_for_canonical_save("I learned that the user works in Rust")
            .expect("valid content");
        assert_eq!(result, "The user works in Rust.");
    }

    #[test]
    fn rewrites_first_person_prefer_to_third() {
        let result =
            normalize_for_canonical_save("I prefer dark mode in editors").expect("valid content");
        assert_eq!(result, "The user prefers dark mode in editors.");
    }

    #[test]
    fn rewrites_first_person_use_to_third() {
        let result = normalize_for_canonical_save("I use VS Code daily").expect("valid content");
        assert_eq!(result, "The user uses VS Code daily.");
    }

    #[test]
    fn appends_terminal_period_when_missing() {
        let result =
            normalize_for_canonical_save("The user prefers Python").expect("valid content");
        assert_eq!(result, "The user prefers Python.");
    }

    #[test]
    fn preserves_existing_terminal_punctuation() {
        let with_period =
            normalize_for_canonical_save("The user prefers Python.").expect("valid content");
        assert_eq!(with_period, "The user prefers Python.");

        let with_question =
            normalize_for_canonical_save("Does the user prefer Python?").expect("valid content");
        assert_eq!(with_question, "Does the user prefer Python?");

        let with_exclamation =
            normalize_for_canonical_save("The user loves Python!").expect("valid content");
        assert_eq!(with_exclamation, "The user loves Python!");
    }

    #[test]
    fn preserves_canonical_content_as_no_op() {
        // Content already in canonical shape should round-trip unchanged.
        let canonical = "The user prefers dark mode in their code editors.";
        let result = normalize_for_canonical_save(canonical).expect("valid content");
        assert_eq!(result, canonical);
    }

    #[test]
    fn capitalizes_first_char_after_strip() {
        // Strip leaves the next word's lowercase first letter; ensure
        // capitalization step fixes it.
        let result = normalize_for_canonical_save("when asked, the user prefers Python")
            .expect("valid content");
        assert_eq!(result, "The user prefers Python.");
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let result =
            normalize_for_canonical_save("  The user prefers Python.  ").expect("valid content");
        assert_eq!(result, "The user prefers Python.");
    }

    #[test]
    fn normalize_is_idempotent() {
        // Pinning the docstring claim — running normalize twice produces
        // the same result as running it once.
        let inputs = [
            "When asked, the user prefers Python",
            "I think the user works in Rust",
            "I prefer dark mode",
            "The user prefers Python",
            "  The user told me that they use VS Code.  ",
        ];
        for input in inputs {
            let once = normalize_for_canonical_save(input).expect("valid input");
            let twice = normalize_for_canonical_save(&once).expect("idempotent valid input");
            assert_eq!(
                once, twice,
                "normalize must be idempotent on input: {input:?}"
            );
        }
    }

    #[test]
    fn handles_unicode_content_without_panic() {
        // Defensive: non-ASCII content should pass through without
        // crashing the byte-counting logic (we use chars().count() for
        // the length check).
        let result = normalize_for_canonical_save("The user prefers 日本語 documentation.")
            .expect("valid content");
        assert_eq!(result, "The user prefers 日本語 documentation.");
    }

    #[test]
    fn case_insensitive_prefix_strip_matches_mixed_case() {
        let result =
            normalize_for_canonical_save("WHEN ASKED, the user prefers Python").expect("valid");
        assert_eq!(result, "The user prefers Python.");

        let result2 = normalize_for_canonical_save("i ThInK the user prefers Rust").expect("valid");
        assert_eq!(result2, "The user prefers Rust.");
    }

    #[test]
    fn longer_prefix_wins_over_shorter() {
        // "When the user was asked, " must match before "When asked, "
        // is even tested (we list longer first in the prefix array).
        let result = normalize_for_canonical_save("When the user was asked, they chose Python")
            .expect("valid content");
        assert_eq!(result, "They chose Python.");
    }
}
