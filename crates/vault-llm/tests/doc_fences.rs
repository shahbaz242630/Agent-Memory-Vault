//! Guard: no doc-comment code fence in this crate's library source may be
//! tagged `ignore`.
//!
//! ## Why this test exists
//!
//! The weekly `real-model smoke` CI job (`.github/workflows/ci.yml`) runs:
//!
//! ```text
//! cargo test -p vault-llm -- --ignored --nocapture --test-threads=1
//! ```
//!
//! rustdoc turns a fence tagged `ignore` into an `#[ignore]`d test. So
//! `--ignored` selects **exactly** the blocks marked "don't run" and forces
//! them to compile. The usual reason to reach for that tag — an illustrative
//! pseudo-code snippet that was never meant to compile — therefore fails the
//! job, and fails it *after* the real-model tests have already passed, which
//! makes the failure read as a model regression when it is a documentation
//! typo.
//!
//! That is not hypothetical. `phi4_mini.rs`'s config example did exactly this
//! and left the job red every week from 2026-06-22 to 2026-07-27 (6+
//! consecutive failures), and the comment above the `--test-threads=1` flag in
//! `ci.yml` records an *earlier* month-long red stretch on the same job. A
//! permanently-red alarm trains us to stop believing CI — the failure mode
//! behind the documented 22-commit silent-failure stretch (CLAUDE.md,
//! T0.1.6 → T0.1.9). A broken alarm is a safety problem, not a tidiness one.
//!
//! ## What to use instead
//!
//! - prose or pseudo-code that cannot compile → tag the fence `text`
//! - a real snippet that must compile but must not run → tag it `no_run`
//! - a snippet that must compile and is expected to fail → tag it
//!   `compile_fail`
//!
//! Scope is `src/` only: doctests are collected from the library target, which
//! is what `--ignored` sweeps. This file lives under `tests/`, so its own
//! mention of the forbidden tag (in the constant below) is not self-flagging.

use std::fs;
use std::path::{Path, PathBuf};

/// The fence tag that must never appear in library source. See module docs.
const FORBIDDEN_FENCE: &str = "```ignore";

fn collect_rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir failed for {}: {e}", dir.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|e| panic!("dir entry failed in {}: {e}", dir.display()));
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_ignore_tagged_doc_fences_in_library_source() {
    let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = Vec::new();
    collect_rust_sources(&src_dir, &mut sources);

    // Non-vacuity: if the walk ever stops finding files, the guard silently
    // passes forever. That is the same class of bug it exists to prevent.
    assert!(
        !sources.is_empty(),
        "found no .rs sources under {} — the guard would be vacuous",
        src_dir.display()
    );

    let mut offenders = Vec::new();
    for path in &sources {
        let text = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("read failed for {}: {e}", path.display()));
        for (idx, line) in text.lines().enumerate() {
            if line.contains(FORBIDDEN_FENCE) {
                offenders.push(format!("{}:{}", path.display(), idx + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "doc fence(s) tagged `ignore` found in vault-llm library source:\n  {}\n\n\
         `cargo test -p vault-llm -- --ignored` (the weekly real-model smoke job) \
         forces these to COMPILE, so a non-compiling illustrative snippet turns \
         that job red indefinitely. Use `text` for prose, `no_run` for a snippet \
         that must compile but not run. See this file's module docs.",
        offenders.join("\n  ")
    );
}
