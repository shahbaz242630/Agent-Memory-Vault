//! REPORT artifact loading for the Commit 6 structured read pipeline.
//!
//! The consolidator (in `vault-consolidator/src/report.rs`) produces per-
//! boundary REPORT artifacts at `<vault_root>/reports/<boundary>.report.json`.
//! This module is the read-side counterpart — it loads those artifacts so the
//! [`crate::structured_read_pipeline::StructuredReadPipeline`] can enrich
//! retrieved candidates with topic labels and surface health warnings
//! (staleness, missing, clock skew, topic-name unavailability).
//!
//! ## Architecture choice: parallel `LoadedReport`, not a cross-crate import
//!
//! `vault-retrieval` does NOT depend on `vault-consolidator`. Instead this
//! module defines [`LoadedReport`] + [`LoadedReportFact`] as deserialize-only
//! parallel structs with the same field names. Serde matches them against
//! the JSON shape the consolidator writes. The trade-off — tiny code
//! duplication (5 fields) — buys us:
//!
//! - No new architectural dependency arrow between sibling crates.
//! - Each crate owns its own surface: producer (consolidator) owns
//!   `Report`/`ReportFact`; consumer (retrieval) owns `LoadedReport`/
//!   `LoadedReportFact`.
//! - Independent evolution: the producer can add fields with `#[serde(default)]`
//!   on the producer side without forcing the consumer to recompile.
//!   (ADR-053 Amendment 1's `topic_names_unavailable` lands here at the
//!   same time as the consumer side.)
//!
//! ## Atomic-write contract from the producer side
//!
//! Producer writes via `tmp + fsync + rename`. A reader always sees either
//! the previous valid REPORT or the new valid REPORT — never a half-written
//! file. So this module doesn't need a file lock; a plain `read_to_string`
//! is safe.

use std::collections::BTreeMap;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use vault_core::{Boundary, MemoryId, VaultResult};

/// Directory under the vault root where REPORT artifacts live. Mirrors
/// the producer-side `vault_consolidator::report::REPORTS_DIRNAME`
/// constant — duplicated here rather than imported to keep this module
/// free of the cross-crate dependency on `vault-consolidator`.
pub const REPORTS_DIRNAME: &str = "reports";

/// One per-boundary REPORT artifact, loaded from disk.
///
/// Field shape mirrors `vault_consolidator::report::Report` exactly so the
/// same JSON deserialises into either type. Deserialise-only at the moment
/// (no `Serialize` derive) because the read pipeline never re-emits the
/// REPORT — only the consolidator writes them.
///
/// ## Backward-compat
///
/// `topic_names_unavailable` is `#[serde(default)]` to match the producer
/// side at ADR-053 Amendment 1. Pre-amendment REPORTs (none exist in
/// practice — Batch A shipped 2026-05-26 with no nightly run yet) without
/// the field deserialise with the safe default `false` (= "no warning
/// surfaced").
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LoadedReport {
    pub schema_version: u32,
    pub boundary: Boundary,
    pub generated_at: DateTime<Utc>,
    /// Producer-side `vault_consolidator::report::Report` types this as
    /// `uuid::Uuid`. Stored as `String` here because the read pipeline
    /// never reasons about run_id structurally — it's an opaque audit
    /// handle. Stringly-typed keeps `vault-retrieval` free of a `uuid`
    /// crate dep (currently transitive-only via `vault-core::MemoryId`).
    pub consolidator_run_id: String,
    pub facts_by_topic: BTreeMap<String, Vec<LoadedReportFact>>,
    #[serde(default)]
    pub topic_names_unavailable: bool,
}

/// One structured fact loaded from a REPORT topic. Field shape mirrors
/// `vault_consolidator::report::ReportFact`.
///
/// `memory_id` uses the typed [`MemoryId`] wrapper so the read pipeline
/// gets compile-time guarantees on what it can do with the value
/// (`#[serde(transparent)]` on `MemoryId` means the JSON wire shape is
/// the same UUID string the producer emitted).
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct LoadedReportFact {
    pub fact: String,
    pub memory_id: MemoryId,
    pub as_of: DateTime<Utc>,
    pub confidence: f32,
    pub source_agent: Option<String>,
}

/// Read-side trait for loading the per-boundary REPORT artifact. The
/// production impl is [`FilesystemReportLoader`]; tests substitute an
/// in-memory mock.
///
/// Returns `Ok(None)` when no REPORT exists for the given boundary —
/// the structured read pipeline surfaces this as the `REPORT_MISSING`
/// health-warning rather than treating it as a hard error.
#[async_trait]
pub trait ReportLoader: Send + Sync {
    /// Load the REPORT for `boundary`. `Ok(None)` if the file is missing.
    ///
    /// # Errors
    ///
    /// - [`vault_core::VaultError::Io`] — file existed but read failed
    ///   (permissions, disk error, etc.). Distinct from "file missing".
    /// - [`vault_core::VaultError::Serde`] — file present but the JSON
    ///   did not deserialise as a `LoadedReport`. Indicates a malformed
    ///   REPORT (consolidator bug or external tampering); a hard error
    ///   so the issue surfaces loudly.
    async fn load(&self, boundary: &Boundary) -> VaultResult<Option<LoadedReport>>;
}

/// Production impl reading REPORTs from
/// `<vault_root>/reports/<boundary>.report.json` via `tokio::fs`.
#[derive(Debug, Clone)]
pub struct FilesystemReportLoader {
    vault_root: PathBuf,
}

impl FilesystemReportLoader {
    /// Construct against the vault root directory. The loader joins
    /// `<vault_root>/reports/<boundary>.report.json` at read time per
    /// boundary — no eager directory listing.
    #[must_use]
    pub fn new(vault_root: PathBuf) -> Self {
        Self { vault_root }
    }

    /// Resolve the on-disk path for a given boundary. Exposed for
    /// diagnostic / test purposes; production code goes through
    /// [`Self::load`].
    pub fn path_for(&self, boundary: &Boundary) -> PathBuf {
        self.vault_root
            .join(REPORTS_DIRNAME)
            .join(format!("{}.report.json", boundary.as_str()))
    }
}

#[async_trait]
impl ReportLoader for FilesystemReportLoader {
    #[tracing::instrument(skip(self), fields(boundary = %boundary.as_str()))]
    async fn load(&self, boundary: &Boundary) -> VaultResult<Option<LoadedReport>> {
        let path = self.path_for(boundary);
        let contents = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(
                    target: "vault_retrieval::report_io",
                    path = %path.display(),
                    "REPORT artifact not found; returning None for REPORT_MISSING surfacing"
                );
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let report: LoadedReport = serde_json::from_str(&contents)?;
        Ok(Some(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("test boundary must validate")
    }

    fn write_report_json(dir: &std::path::Path, boundary_name: &str, json: &str) {
        let reports_dir = dir.join(REPORTS_DIRNAME);
        std::fs::create_dir_all(&reports_dir).unwrap();
        let path = reports_dir.join(format!("{boundary_name}.report.json"));
        std::fs::write(&path, json).unwrap();
    }

    #[tokio::test]
    async fn load_returns_none_when_report_file_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf());
        let result = loader.load(&boundary("personal")).await.unwrap();
        assert!(
            result.is_none(),
            "load MUST return Ok(None) when the REPORT file is missing — \
             the pipeline surfaces this as REPORT_MISSING rather than a hard error"
        );
    }

    #[tokio::test]
    async fn load_deserialises_valid_report_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        let json = serde_json::json!({
            "schema_version": 1,
            "boundary": "personal",
            "generated_at": "2026-05-26T03:00:00Z",
            "consolidator_run_id": "00000000-0000-0000-0000-000000000001",
            "facts_by_topic": {
                "blood_pressure": [{
                    "fact": "BP 132/85 on 2026-05-20",
                    "memory_id": "00000000-0000-0000-0000-0000000000aa",
                    "as_of": "2026-05-20T08:00:00Z",
                    "confidence": 0.95,
                    "source_agent": "claude"
                }]
            },
            "topic_names_unavailable": false
        })
        .to_string();
        write_report_json(tmp.path(), "personal", &json);

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf());
        let report = loader
            .load(&boundary("personal"))
            .await
            .unwrap()
            .expect("Ok(Some(_)) for a valid REPORT file");

        assert_eq!(report.schema_version, 1);
        assert_eq!(report.boundary.as_str(), "personal");
        assert_eq!(report.facts_by_topic.len(), 1);
        let facts = report.facts_by_topic.get("blood_pressure").unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact, "BP 132/85 on 2026-05-20");
        assert_eq!(facts[0].confidence, 0.95);
        assert_eq!(facts[0].source_agent.as_deref(), Some("claude"));
        assert!(!report.topic_names_unavailable);
    }

    #[tokio::test]
    async fn load_defaults_topic_names_unavailable_to_false_when_field_missing() {
        // Pre-ADR-053-Amendment-1 REPORTs omit the field. #[serde(default)]
        // on the LoadedReport mirrors the producer side: missing → false.
        let tmp = tempfile::TempDir::new().unwrap();
        let json = serde_json::json!({
            "schema_version": 1,
            "boundary": "personal",
            "generated_at": "2026-05-26T03:00:00Z",
            "consolidator_run_id": "00000000-0000-0000-0000-000000000001",
            "facts_by_topic": {}
        })
        .to_string();
        write_report_json(tmp.path(), "personal", &json);

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf());
        let report = loader.load(&boundary("personal")).await.unwrap().unwrap();
        assert!(
            !report.topic_names_unavailable,
            "missing topic_names_unavailable field MUST default to false on the load side too"
        );
    }

    #[tokio::test]
    async fn load_surfaces_serde_error_on_malformed_json() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_report_json(tmp.path(), "personal", "{not valid json");
        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf());
        let err = loader
            .load(&boundary("personal"))
            .await
            .expect_err("malformed JSON must surface as VaultError::Serde");
        assert!(
            matches!(err, vault_core::VaultError::Serde(_)),
            "expected VaultError::Serde, got {err:?}"
        );
    }

    #[test]
    fn path_for_joins_vault_root_reports_and_boundary_name() {
        let loader = FilesystemReportLoader::new(PathBuf::from("/tmp/vault"));
        let p = loader.path_for(&boundary("personal"));
        // OS-neutral normalisation: turn any Windows '\' separators into '/'
        // before suffix-matching.
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(
            s.ends_with("/reports/personal.report.json"),
            "path MUST end with /reports/<boundary>.report.json; got {s}"
        );
    }

    /// Underscore-separated boundary names (the `Boundary::new` validator
    /// accepts ASCII letters / digits / '-' / '_' — NOT '.'). Sanity
    /// check that the filename path is built verbatim from the boundary
    /// name without escaping or normalisation.
    #[test]
    fn path_for_handles_underscore_separated_boundary_names() {
        let loader = FilesystemReportLoader::new(PathBuf::from("/tmp/vault"));
        let p = loader.path_for(&boundary("work_acme_engineering"));
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.ends_with("/reports/work_acme_engineering.report.json"));
    }
}
