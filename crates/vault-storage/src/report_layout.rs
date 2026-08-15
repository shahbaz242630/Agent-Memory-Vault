//! On-disk layout of the sealed consolidator REPORT artifact (ADR-SEC-007).
//!
//! # Why this lives in vault-storage and not with the REPORT itself
//!
//! The REPORT is WRITTEN by `vault-consolidator` and READ by
//! `vault-retrieval`. Those two crates do not depend on each other, so
//! without a shared home each would define its own copy of the filename
//! and of the AAD input string.
//!
//! That duplication would be uniquely nasty here. The relative path is
//! hashed into the AEAD's associated data, so if the writer's string and
//! the reader's string ever diverged by one character, every REPORT would
//! seal correctly, write correctly, and then fail to unseal — the vault
//! would fail CLOSED and silently degrade to `REPORT_MISSING` on every
//! read, with no error at the point of the mistake. A drift bug that
//! disables a feature without breaking a build is exactly the kind this
//! codebase has been bitten by before (see ADR-098's doc fence, ADR-090's
//! `start_with_mcp` divergence).
//!
//! Both crates depend on `vault-storage`, and `vault-storage` owns at-rest
//! concerns, so the definitions live here and there is exactly one of them.

use vault_core::Boundary;

/// Directory under the vault root holding REPORT artifacts.
pub const REPORTS_DIRNAME: &str = "reports";

/// Filename suffix of a SEALED REPORT.
///
/// Deliberately not `.json`: the file is an encrypted envelope, and naming
/// it `.json` invites a future reader to attempt a plaintext parse, fail,
/// and "fix" it with a plaintext fallback — reintroducing the exact
/// ADR-SEC-007 vulnerability.
pub const REPORT_SEALED_SUFFIX: &str = "report.sealed";

/// Pre-ADR-SEC-007 PLAINTEXT suffix, retained solely so the migration
/// sweep can find these files and destroy them. Nothing may read a REPORT
/// through this suffix again.
pub const REPORT_LEGACY_PLAINTEXT_SUFFIX: &str = "report.json";

/// Vault-root-relative path of a boundary's sealed REPORT — and the exact
/// string used as AAD input by [`crate::seal_vault_blob`].
///
/// **Always `/`-separated, never [`std::path::MAIN_SEPARATOR`].** The AAD
/// is a hash of this exact string, so a Windows-vs-POSIX separator
/// difference would produce a vault whose REPORTs are unreadable on the
/// other platform — breaking cross-platform restore and future sync
/// (BRD §6.2). Pinned by test, not left to convention.
///
/// The boundary name is embedded in this string, which is what binds a
/// sealed REPORT to its boundary and defeats the cross-boundary swap
/// attack described in BRD §11.3.2.
#[must_use]
pub fn sealed_report_relative_path(boundary: &Boundary) -> String {
    format!(
        "{REPORTS_DIRNAME}/{}.{REPORT_SEALED_SUFFIX}",
        boundary.as_str()
    )
}

/// Filename (no directory) of a boundary's sealed REPORT.
#[must_use]
pub fn sealed_report_filename(boundary: &Boundary) -> String {
    format!("{}.{REPORT_SEALED_SUFFIX}", boundary.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("test boundary must validate")
    }

    /// The AAD input must be byte-stable across platforms. If this ever
    /// fails on one CI leg and not the other, REPORTs written on one OS
    /// are unreadable on the other.
    #[test]
    fn relative_path_is_forward_slashed_and_stable() {
        let p = sealed_report_relative_path(&boundary("personal"));
        assert_eq!(
            p, "reports/personal.report.sealed",
            "AAD input string drifted; every existing sealed REPORT becomes \
             unreadable and reads silently degrade to REPORT_MISSING"
        );
        assert!(
            !p.contains('\\'),
            "AAD input MUST NOT contain a backslash on any platform"
        );
    }

    /// The boundary must actually appear in the AAD input, or the
    /// cross-boundary swap defence in BRD §11.3.2 does not exist.
    #[test]
    fn relative_path_embeds_the_boundary_name() {
        let personal = sealed_report_relative_path(&boundary("personal"));
        let work = sealed_report_relative_path(&boundary("work"));
        assert_ne!(
            personal, work,
            "SECURITY: two boundaries produced the same AAD input, so their \
             sealed REPORTs are interchangeable on disk"
        );
        assert!(personal.contains("personal"));
        assert!(work.contains("work"));
    }

    #[test]
    fn filename_matches_the_tail_of_the_relative_path() {
        let b = boundary("personal");
        assert!(
            sealed_report_relative_path(&b).ends_with(&sealed_report_filename(&b)),
            "filename and relative path must be derived consistently, or the \
             file written is not the file whose path was bound into the AAD"
        );
    }

    #[test]
    fn legacy_and_sealed_suffixes_are_distinct() {
        assert_ne!(
            REPORT_SEALED_SUFFIX, REPORT_LEGACY_PLAINTEXT_SUFFIX,
            "the migration sweep distinguishes plaintext from sealed by \
             suffix; identical suffixes would make it delete real REPORTs"
        );
    }
}
