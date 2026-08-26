//! Source-level coverage gate for the BRD §11.7.3 prompt-injection guard.
//!
//! # Why a source-level test rather than a behavioural one
//!
//! A behavioural test can only prove that the call sites it happens to know
//! about are guarded. The defect class this protects against is precisely the
//! one it would not catch: a SIXTH call site, added later, that nobody
//! remembers to guard.
//!
//! That is not a hypothetical failure mode in this repository — it is the
//! documented root cause of ADR-SEC-007. BRD §11.12 assigned "no plaintext
//! data on disk, ever" to `vault-storage`; the REPORT was written by
//! `vault-consolidator`; every crate was individually compliant and the vault
//! as a whole leaked memories in plaintext for weeks. The lesson recorded
//! there was that the durable fix is not the patch, it is the check that makes
//! the next omission impossible.
//!
//! It nearly repeated during this very change: four of the five call sites
//! live under `src/phases/`, and [`vault_consolidator::topics`] does not — a
//! search scoped to `phases/` finds four of five and reports success.
//!
//! So this test reads the crate's own source and asserts a rule with NO
//! exceptions: every `system_prompt: Some(...)` under `src/` routes through
//! `guarded_system_prompt`. Test-only prompts are included deliberately, both
//! because a real-model probe should exercise the shipped prompt and because
//! an allow-list is a hole that has to be maintained forever.

use std::fs;
use std::path::{Path, PathBuf};

/// The construction every prompt must be built with.
const REQUIRED: &str = "guarded_system_prompt";

/// The pattern that marks a site where a system prompt is supplied to the LLM.
const CALL_SITE: &str = "system_prompt: Some(";

/// How far past the call site to look for [`REQUIRED`]. Generous enough to
/// span a rustfmt-wrapped multi-line call (see `topics.rs`, where the closure
/// is split across three lines) without running into the next statement.
const LOOKAHEAD_BYTES: usize = 200;

fn crate_src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Byte offsets of every REAL (non-comment) [`CALL_SITE`] occurrence in
/// `text`.
///
/// # Why comments must be excluded
///
/// The first version of this test scanned raw source and failed on its own
/// first run — against two pieces of ENGLISH:
///
/// - `phases/merge.rs` carries a comment quoting `system_prompt: Some(...)`
///   while explaining ADR-044 Amendment 1.
/// - `prompt_guard.rs`'s own module docs describe what this very test looks
///   for, so the test flagged its own specification as a violation.
///
/// Both were false positives against correctly-guarded code. Rather than
/// allow-listing the two files — which would have punched a permanent hole
/// exactly where the guard matters most (`merge.rs` writes model output BACK
/// into the vault) — the matcher now understands line comments.
///
/// Deliberately handles `//` line comments only, not `/* */` blocks: this
/// crate uses line comments throughout, and a block-comment parser here would
/// be more machinery than the risk warrants. A `system_prompt: Some(` inside a
/// block comment would produce a false positive, which fails LOUD and visible
/// — the safe direction for a security control to be wrong in.
fn code_call_sites(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut line_start = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("//") {
            let mut from = 0usize;
            while let Some(rel) = line[from..].find(CALL_SITE) {
                out.push(line_start + from + rel);
                from += rel + CALL_SITE.len();
            }
        }
        line_start += line.len();
    }
    out
}

/// The matcher is itself load-bearing, so it gets its own test.
///
/// Without this, a future "simplification" back to a plain `contains` would
/// reintroduce the false positives silently — and because they fail CLOSED
/// (reporting a violation that is not real), the reflex fix would be to
/// allow-list the offending file, which is how a guard quietly stops
/// guarding.
#[test]
fn matcher_counts_code_and_ignores_comments() {
    // Real call site — must count.
    assert_eq!(code_call_sites("    system_prompt: Some(x),\n").len(), 1);

    // The two shapes that actually broke this test on its first run.
    assert_eq!(
        code_call_sites("    // ADR-044: `system_prompt: Some(...)` swaps it\n").len(),
        0,
        "a line comment quoting the pattern must not count (phases/merge.rs)"
    );
    assert_eq!(
        code_call_sites("//! every `system_prompt: Some(..)` routes through\n").len(),
        0,
        "a module doc comment must not count (prompt_guard.rs's own docs)"
    );
    assert_eq!(
        code_call_sites("/// pass `system_prompt: Some(v)` to override\n").len(),
        0,
        "a doc comment must not count"
    );

    // An indented comment is still a comment.
    assert_eq!(
        code_call_sites("\t\t  // system_prompt: Some(y)\n").len(),
        0
    );

    // Mixed input: only the code line counts, and the offset must point at
    // the code occurrence rather than the earlier commented one.
    let mixed = "// system_prompt: Some(a)\nlet p = system_prompt: Some(b);\n";
    let hits = code_call_sites(mixed);
    assert_eq!(hits.len(), 1, "only the code line counts");
    assert!(
        hits[0] > mixed.find('\n').expect("has a newline"),
        "the reported offset must be on the second line, not the comment"
    );

    // Trailing comment on a code line: the code still counts. Erring toward
    // counting keeps the guard strict.
    assert_eq!(
        code_call_sites("system_prompt: Some(z), // note\n").len(),
        1
    );
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_system_prompt_in_this_crate_is_guarded() {
    let src = crate_src_dir();
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();

    assert!(
        !files.is_empty(),
        "found no .rs files under {} — the test would pass vacuously",
        src.display()
    );

    let mut unguarded: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for file in &files {
        let text =
            fs::read_to_string(file).unwrap_or_else(|e| panic!("read {}: {e}", file.display()));

        for at in code_call_sites(&text) {
            checked += 1;

            let window_end = (at + LOOKAHEAD_BYTES).min(text.len());
            // Slice on a char boundary: source files carry non-ASCII (em
            // dashes and § in doc comments), so a byte slice can land
            // mid-codepoint and panic with an unrelated message.
            let mut end = window_end;
            while end > at && !text.is_char_boundary(end) {
                end -= 1;
            }

            if !text[at..end].contains(REQUIRED) {
                let line = text[..at].matches('\n').count() + 1;
                unguarded.push(format!(
                    "{}:{}",
                    file.strip_prefix(&src).unwrap_or(file).display(),
                    line
                ));
            }
        }
    }

    // A guard that checks nothing is worse than no guard, because it reads as
    // a passing control. If the call-site pattern is ever refactored away,
    // this fails loudly instead of going quietly green.
    assert!(
        checked >= 5,
        "expected at least the 5 known `{CALL_SITE}` sites, found {checked} — \
         the pattern this test greps for has probably been refactored. Update \
         CALL_SITE deliberately; do not delete this assertion."
    );

    assert!(
        unguarded.is_empty(),
        "BRD §11.7.3 violation — these prompts receive memory content without \
         the untrusted-content guard:\n  {}\n\nWrap the prompt with \
         `guarded_system_prompt(..)` from `crate::prompt_guard`. Memory content \
         reaching a model unguarded is OWASP ASI06 (Memory & Context \
         Poisoning); in `phases::merge` the model's output is written BACK into \
         the vault as memory content, so an injected instruction there rewrites \
         what the user's vault remembers.",
        unguarded.join("\n  ")
    );
}

#[test]
fn the_five_known_call_sites_are_still_where_we_think_they_are() {
    // Documents the inventory as of the guard landing, so a future reader can
    // see at a glance whether the surface grew. Deliberately asserts on file
    // NAMES rather than line numbers, which churn.
    let src = crate_src_dir();
    let mut files = Vec::new();
    rust_files(&src, &mut files);

    let mut with_sites: Vec<String> = files
        .iter()
        .filter(|f| {
            // Same comment-aware matcher as the test above. A plain
            // `contains` here would list `prompt_guard.rs`, whose module docs
            // quote the pattern while explaining this test.
            fs::read_to_string(f)
                .map(|t| !code_call_sites(&t).is_empty())
                .unwrap_or(false)
        })
        .map(|f| {
            f.strip_prefix(&src)
                .unwrap_or(f)
                .display()
                .to_string()
                .replace('\\', "/")
        })
        .collect();
    with_sites.sort();

    assert_eq!(
        with_sites,
        vec![
            "phases/contradiction.rs".to_string(),
            "phases/enrich.rs".to_string(),
            "phases/merge.rs".to_string(),
            "topics.rs".to_string(),
        ],
        "the set of files that supply system prompts changed. That is allowed — \
         but confirm the new one routes through `guarded_system_prompt` and then \
         update this list deliberately. `topics.rs` is in this list precisely \
         because it sits OUTSIDE `phases/` and was nearly missed."
    );
}
