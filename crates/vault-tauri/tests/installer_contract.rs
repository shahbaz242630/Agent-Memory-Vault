//! Rust↔WiX contract guards for the MSI installer fragment.
//!
//! Same idea as `frontend_contract.rs`, one language boundary over: the
//! installer is XML that no Rust compiler ever sees, so every fact shared
//! between `installer.wxs` and the crate is a silent-drift risk. These guards
//! are pure string analysis over `include_str!`ed sources — no WiX toolset, no
//! MSI, no new dependencies — so they run on every platform in CI even though
//! the fragment itself is only ever processed by `tauri build` on Windows.
//!
//! ## Why these specific guards (ADR-SEC-005 close, 2026-07-28)
//!
//! **1. The task id is duplicated across languages.** Uninstall removes the
//! scheduled task by NAME. The name is a Rust constant the app registers with
//! and a literal in XML the uninstaller deletes with. If they ever diverge the
//! failure is invisible: the app still schedules, the uninstaller still
//! "succeeds", and the task is simply left behind on the user's machine
//! forever. Nothing else in the build would notice.
//!
//! **2. `NOT UPGRADINGPRODUCTCODE` looks like noise and is not.** `REMOVE="ALL"`
//! is also true while a major upgrade removes the OLD product, so dropping that
//! clause makes every app update silently switch the user's automatic
//! maintenance off. It is exactly the kind of clause a future edit "tidies
//! away", so it is pinned with the reason attached.
//!
//! **3. A fragment is only compiled in if something references it.** WiX pulls
//! a `<Fragment>` in via a reference — here, `tauri.conf.json`'s
//! `bundle.windows.wix.componentRefs`. Rename a component on one side and the
//! whole fragment (env vars, PATH, vault-cli.exe, the uninstall action) is
//! dropped from the MSI **with no error at all**. Guard 3 pins each
//! `componentRef` to a real `Component Id`, and guard 4 pins the uninstall
//! action into the same single fragment so it cannot be orphaned by being
//! moved to one nothing references.
//!
//! These tests cannot prove the MSI behaves — only a real install/uninstall
//! does that (done live for ADR-SEC-005). They prove the fragment still says
//! what the crate thinks it says.
//!
//! ## ⚠️ Every guard runs against COMMENT-STRIPPED markup, and that is load-bearing
//!
//! The first run of these tests failed, and it was right to. `installer.wxs`
//! documents itself heavily, so its comments quote the very strings the guards
//! search for — the task id, `Return="ignore"`, even the word `<Fragment>`.
//! Scanning the raw file, **three of the four guards would have passed on prose
//! alone**: the uninstall `ExeCommand` could have been deleted outright and the
//! suite would still have gone green, because the explanation of it survived.
//!
//! A guard that a comment can satisfy is not a guard. [`markup()`] strips XML
//! comments once and every assertion runs against the result, so these tests
//! can only ever be satisfied by real markup. The stripper has its own
//! non-vacuity check — see [`comment_stripper_actually_strips`].

use vault_tauri::commands::maintenance::MAINTENANCE_TASK_ID;

/// The WiX fragment that ships the installer's non-Tauri-generated pieces.
///
/// ⚠️ Raw text, comments included. Assert against [`markup()`], never this.
const INSTALLER_WXS: &str = include_str!("../windows/fragments/installer.wxs");

/// Tauri's bundle config — the only thing that references the fragment.
const TAURI_CONF: &str = include_str!("../tauri.conf.json");

/// WiX id of the uninstall-time task removal action.
const REMOVE_ACTION_ID: &str = "RemoveMaintenanceTask";

/// `installer.wxs` with every XML comment removed.
///
/// The fragment's own documentation quotes the strings these guards look for,
/// so searching the raw file would let prose stand in for markup. See the
/// module docs — this is why the suite's first run failed.
fn markup() -> String {
    let mut out = String::with_capacity(INSTALLER_WXS.len());
    let mut rest = INSTALLER_WXS;

    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + "-->".len()..],
            // Unterminated comment: everything after it is commented out.
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// Guard 0 — the stripper itself is not a no-op.
// ---------------------------------------------------------------------------

#[test]
fn comment_stripper_actually_strips() {
    let stripped = markup();

    assert!(
        stripped.len() < INSTALLER_WXS.len(),
        "markup() removed nothing — either installer.wxs lost all its comments \
         or the stripper broke. Every other guard in this file relies on it, \
         and a no-op stripper silently restores the prose-satisfies-guard hole \
         that made this suite fail on its first run."
    );

    assert!(
        !stripped.contains("<!--") && !stripped.contains("-->"),
        "markup() left comment delimiters behind"
    );

    // And it must not have eaten the markup along with the prose.
    assert!(
        stripped.contains("<CustomAction") && stripped.contains("<Component "),
        "markup() stripped real markup, not just comments"
    );
}

// ---------------------------------------------------------------------------
// Guard 1 — the task id must match the Rust constant, byte for byte.
// ---------------------------------------------------------------------------

#[test]
fn installer_deletes_the_task_id_the_app_actually_registers() {
    // Binding the real constant (rather than re-typing it) means a RENAME is a
    // compile error here, and a VALUE change is this assertion.
    assert!(
        markup().contains(MAINTENANCE_TASK_ID),
        "installer.wxs does not mention the task id the app registers \
         ({MAINTENANCE_TASK_ID:?}).\n\n\
         Uninstall removes the scheduled task BY NAME. If the WiX literal and \
         `MAINTENANCE_TASK_ID` diverge, the uninstaller silently deletes \
         nothing: it still reports success, and the user is left with a task \
         firing daily at a program that no longer exists. Update the \
         ExeCommand in windows/fragments/installer.wxs."
    );

    // Non-vacuity: an empty or truncated constant would satisfy `contains`
    // against almost anything.
    assert!(
        MAINTENANCE_TASK_ID.len() > 8,
        "MAINTENANCE_TASK_ID is suspiciously short ({MAINTENANCE_TASK_ID:?}) — \
         guard 1 would pass vacuously"
    );
}

// ---------------------------------------------------------------------------
// Guard 2 — the removal action's shape: right trigger, upgrade-safe, non-fatal.
// ---------------------------------------------------------------------------

#[test]
fn task_removal_is_upgrade_safe_and_cannot_fail_the_uninstall() {
    let markup = markup();

    assert!(
        markup.contains(REMOVE_ACTION_ID),
        "installer.wxs has no `{REMOVE_ACTION_ID}` custom action — uninstall \
         would leave the scheduled task behind (ADR-SEC-005)"
    );

    let sequence_line = markup
        .lines()
        .find(|line| line.contains("<Custom ") && line.contains(REMOVE_ACTION_ID))
        .unwrap_or_else(|| {
            panic!(
                "`{REMOVE_ACTION_ID}` is defined but never sequenced — a \
                   CustomAction that is not in InstallExecuteSequence never runs"
            )
        });

    assert!(
        sequence_line.contains("REMOVE=\"ALL\""),
        "task removal is not conditioned on REMOVE=\"ALL\": {sequence_line}"
    );

    assert!(
        sequence_line.contains("NOT UPGRADINGPRODUCTCODE"),
        "task removal lost its `NOT UPGRADINGPRODUCTCODE` clause: {sequence_line}\n\n\
         REMOVE=\"ALL\" is ALSO true while a major upgrade removes the OLD \
         product. Without this clause, every app update silently switches the \
         user's automatic maintenance off and never tells them. This clause is \
         not redundant — do not simplify it away."
    );

    assert!(
        sequence_line.contains("Before=\"RemoveFiles\""),
        "task removal is not sequenced before RemoveFiles: {sequence_line}"
    );

    assert!(
        markup.contains("Return=\"ignore\""),
        "the removal action must use Return=\"ignore\": `schtasks /Delete` \
         exits non-zero when the task is absent (never scheduled, or already \
         removed), and that must never fail a user's uninstall"
    );
}

// ---------------------------------------------------------------------------
// Guard 3 — every componentRef in tauri.conf.json resolves to a real component.
// ---------------------------------------------------------------------------

#[test]
fn every_component_ref_resolves_to_a_component_in_the_fragment() {
    let refs = component_refs(TAURI_CONF);

    assert!(
        !refs.is_empty(),
        "parsed zero componentRefs out of tauri.conf.json — the parse broke, \
         so this guard would pass vacuously"
    );

    let markup = markup();
    for id in &refs {
        assert!(
            markup.contains(&format!("Component Id=\"{id}\"")),
            "tauri.conf.json references WiX component {id:?}, which does not \
             exist in installer.wxs.\n\n\
             A WiX fragment is compiled in ONLY because something references \
             it. Break every reference and the entire fragment — env vars, \
             PATH entry, vault-cli.exe, and the uninstall task removal — is \
             dropped from the MSI with no error whatsoever."
        );
    }
}

// ---------------------------------------------------------------------------
// Guard 4 — the removal action shares the referenced components' fragment.
// ---------------------------------------------------------------------------

#[test]
fn the_removal_action_lives_in_the_referenced_fragment() {
    let fragment_count = markup().matches("<Fragment>").count();

    assert_eq!(
        fragment_count, 1,
        "installer.wxs now has {fragment_count} <Fragment> elements. This guard \
         relies on there being exactly one, which is what guarantees the \
         uninstall action sits in the SAME fragment as the components \
         tauri.conf.json references. Splitting them is legal WiX and silently \
         drops whichever fragment nothing references — if you split this file, \
         replace this guard with a real per-fragment containment check."
    );
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Pull the `componentRefs` string array out of `tauri.conf.json` without a
/// JSON dependency (same no-dependency posture as `frontend_contract.rs`).
fn component_refs(conf: &str) -> Vec<String> {
    let Some(start) = conf.find("\"componentRefs\"") else {
        return Vec::new();
    };
    let Some(open) = conf[start..].find('[') else {
        return Vec::new();
    };
    let Some(close) = conf[start + open..].find(']') else {
        return Vec::new();
    };
    let body = &conf[start + open + 1..start + open + close];

    body.split(',')
        .map(|item| item.trim().trim_matches('"').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}
