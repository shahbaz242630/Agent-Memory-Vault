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
use zeroize::Zeroizing;

/// REPORT on-disk layout, re-exported from [`vault_storage::report_layout`].
///
/// **Was a hand-duplicated `const` here until ADR-SEC-007.** That was
/// acceptable while the filename was the only shared fact. It stopped being
/// acceptable once the artifact became encrypted: the relative path is
/// hashed into the AEAD's associated data, so writer and reader must agree
/// byte-for-byte or every unseal fails. A drifted copy would not break the
/// build — it would silently turn every REPORT read into `REPORT_MISSING`.
///
/// The original rationale (no cross-crate dependency on the sibling
/// `vault-consolidator`) is preserved: `vault-storage` is not a sibling but
/// a shared lower layer this crate already depends on.
pub use vault_storage::{sealed_report_relative_path, REPORTS_DIRNAME, REPORT_SEALED_SUFFIX};

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

/// Production impl reading SEALED REPORTs from
/// `<vault_root>/reports/<boundary>.report.sealed` via `tokio::fs`.
///
/// # Encryption (ADR-SEC-007)
///
/// The artifact is an XChaCha20-Poly1305 envelope, not JSON. This loader
/// holds the at-rest key and unseals in memory; the decrypted JSON never
/// touches disk.
#[derive(Clone)]
pub struct FilesystemReportLoader {
    vault_root: PathBuf,
    /// At-rest key (K3), zeroized on drop. Never logged, never `Debug`-ed
    /// — which is why this struct hand-rolls `Debug` below instead of
    /// deriving it (BRD §11.5.3: "No `Debug` impl on key types").
    at_rest_key: Zeroizing<[u8; 32]>,
}

impl std::fmt::Debug for FilesystemReportLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesystemReportLoader")
            .field("vault_root", &self.vault_root)
            .field("at_rest_key", &"<redacted>")
            .finish()
    }
}

impl FilesystemReportLoader {
    /// Construct against the vault root directory and the at-rest key.
    /// The loader joins `<vault_root>/reports/<boundary>.report.sealed` at
    /// read time per boundary — no eager directory listing.
    #[must_use]
    pub fn new(vault_root: PathBuf, at_rest_key: &[u8; 32]) -> Self {
        Self {
            vault_root,
            at_rest_key: Zeroizing::new(*at_rest_key),
        }
    }

    /// Resolve the on-disk path for a given boundary. Exposed for
    /// diagnostic / test purposes; production code goes through
    /// [`Self::load`].
    pub fn path_for(&self, boundary: &Boundary) -> PathBuf {
        self.vault_root
            .join(REPORTS_DIRNAME)
            .join(vault_storage::sealed_report_filename(boundary))
    }
}

#[async_trait]
impl ReportLoader for FilesystemReportLoader {
    #[tracing::instrument(skip(self), fields(boundary = %boundary.as_str()))]
    async fn load(&self, boundary: &Boundary) -> VaultResult<Option<LoadedReport>> {
        let path = self.path_for(boundary);
        let sealed = match tokio::fs::read(&path).await {
            Ok(b) => b,
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

        // SP-4 fail-securely. A wrong key, tampered bytes, or a REPORT
        // moved in from another boundary all land here. We surface it as
        // an ERROR (loud) but return `None` so the read pipeline degrades
        // via its existing REPORT_MISSING path rather than dying — the
        // same posture as a genuinely absent REPORT, because an
        // unauthenticatable REPORT is exactly as trustworthy as no REPORT.
        //
        // There is deliberately NO plaintext fallback. Attempting a JSON
        // parse when the unseal fails would reintroduce ADR-SEC-007 the
        // first time anyone shipped a half-migrated vault.
        let plaintext = match vault_storage::unseal_vault_blob(
            &sealed,
            &self.at_rest_key,
            &sealed_report_relative_path(boundary),
        ) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(
                    target: "vault_retrieval::report_io",
                    path = %path.display(),
                    error = %e,
                    "REPORT failed to unseal (wrong key, tampering, or a REPORT \
                     from another boundary); treating as REPORT_MISSING, \
                     ADR-SEC-007"
                );
                return Ok(None);
            }
        };

        let report: LoadedReport = serde_json::from_slice(&plaintext)?;
        Ok(Some(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("test boundary must validate")
    }

    /// Deterministic at-rest key for tests. Production keys come from the
    /// OS keychain via `vault_app::keychain::derive_at_rest_key`.
    const TEST_KEY: [u8; 32] = [0x5a; 32];
    const OTHER_KEY: [u8; 32] = [0xa5; 32];

    /// Write a REPORT the way the producer now does: SEALED, at
    /// `<boundary>.report.sealed`, with the boundary bound into the AAD.
    fn write_report_json(dir: &std::path::Path, boundary_name: &str, json: &str) {
        write_report_sealed_with(dir, boundary_name, json, &TEST_KEY, boundary_name);
    }

    /// Lower-level variant that lets a test seal under one boundary's AAD
    /// while writing to another boundary's filename — the cross-boundary
    /// swap attack (BRD §11.3.2).
    fn write_report_sealed_with(
        dir: &std::path::Path,
        filename_boundary: &str,
        json: &str,
        key: &[u8; 32],
        aad_boundary: &str,
    ) {
        let reports_dir = dir.join(REPORTS_DIRNAME);
        std::fs::create_dir_all(&reports_dir).unwrap();
        let path = reports_dir.join(format!("{filename_boundary}.{REPORT_SEALED_SUFFIX}"));
        let sealed = vault_storage::seal_vault_blob(
            json.as_bytes(),
            key,
            &sealed_report_relative_path(&boundary(aad_boundary)),
        );
        std::fs::write(&path, sealed).unwrap();
    }

    #[tokio::test]
    async fn load_returns_none_when_report_file_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
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

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
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

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
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
        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
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
        let loader = FilesystemReportLoader::new(PathBuf::from("/tmp/vault"), &TEST_KEY);
        let p = loader.path_for(&boundary("personal"));
        // OS-neutral normalisation: turn any Windows '\' separators into '/'
        // before suffix-matching.
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(
            s.ends_with("/reports/personal.report.sealed"),
            "path MUST end with /reports/<boundary>.report.sealed; got {s}"
        );
        assert!(
            !s.ends_with(".json"),
            "ADR-SEC-007: the REPORT is an encrypted envelope and MUST NOT be \
             named .json, which invites a plaintext-parse fallback; got {s}"
        );
    }

    /// Underscore-separated boundary names (the `Boundary::new` validator
    /// accepts ASCII letters / digits / '-' / '_' — NOT '.'). Sanity
    /// check that the filename path is built verbatim from the boundary
    /// name without escaping or normalisation.
    #[test]
    fn path_for_handles_underscore_separated_boundary_names() {
        let loader = FilesystemReportLoader::new(PathBuf::from("/tmp/vault"), &TEST_KEY);
        let p = loader.path_for(&boundary("work_acme_engineering"));
        let s = p.to_string_lossy().replace('\\', "/");
        assert!(s.ends_with("/reports/work_acme_engineering.report.sealed"));
    }

    // ================================================================
    //   ADR-SEC-007 security contract (BRD §11.13 adversarial tests)
    // ================================================================

    /// The bug ADR-SEC-007 fixes, asserted at the boundary that matters:
    /// what a person with filesystem access can read.
    #[tokio::test]
    async fn report_on_disk_is_not_readable_without_the_key() {
        let tmp = tempfile::TempDir::new().unwrap();
        let json = serde_json::json!({
            "schema_version": 1,
            "boundary": "personal",
            "generated_at": "2026-05-26T03:00:00Z",
            "consolidator_run_id": "00000000-0000-0000-0000-000000000001",
            "facts_by_topic": {
                "health": [{
                    "fact": "The user was diagnosed with atrial fibrillation.",
                    "memory_id": "00000000-0000-0000-0000-0000000000aa",
                    "as_of": "2026-05-20T08:00:00Z",
                    "confidence": 0.95,
                    "source_agent": "claude"
                }]
            }
        })
        .to_string();
        write_report_json(tmp.path(), "personal", &json);

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
        let raw = std::fs::read(loader.path_for(&boundary("personal"))).unwrap();
        let on_disk = String::from_utf8_lossy(&raw);

        assert!(
            !on_disk.contains("atrial fibrillation"),
            "SECURITY REGRESSION (ADR-SEC-007): the REPORT on disk contains \
             verbatim memory text readable with no key. BRD §11.5.1: all data \
             on disk is encrypted, no exceptions."
        );
        assert!(
            !on_disk.contains("facts_by_topic"),
            "SECURITY REGRESSION: REPORT structure is readable on disk, so the \
             artifact was written unsealed."
        );

        // Control: the same bytes ARE readable WITH the key, so the assertions
        // above are testing encryption and not an empty/missing file.
        let report = loader.load(&boundary("personal")).await.unwrap();
        assert!(
            report.is_some(),
            "control: the sealed REPORT must still load with the correct key"
        );
    }

    /// SP-4 fail-securely. A REPORT that cannot be authenticated is exactly as
    /// trustworthy as no REPORT, so it degrades to the existing
    /// `REPORT_MISSING` path rather than being served.
    #[tokio::test]
    async fn wrong_key_degrades_to_report_missing_and_never_serves_facts() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_report_json(tmp.path(), "personal", &minimal_report_json("personal"));

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &OTHER_KEY);
        let result = loader.load(&boundary("personal")).await.unwrap();
        assert!(
            result.is_none(),
            "a REPORT sealed under a different key MUST NOT be served"
        );
    }

    /// The cross-boundary swap (BRD §11.3.2 / §11.4.3). Copy the `work`
    /// REPORT over `personal.report.sealed`: same vault, same key, so only
    /// the AAD's boundary binding stands between the attacker and one
    /// boundary's facts being served under another's name.
    #[tokio::test]
    async fn report_from_another_boundary_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Sealed with the `work` AAD, but written at personal's filename.
        write_report_sealed_with(
            tmp.path(),
            "personal",
            &minimal_report_json("work"),
            &TEST_KEY,
            "work",
        );

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
        let result = loader.load(&boundary("personal")).await.unwrap();
        assert!(
            result.is_none(),
            "SECURITY: a REPORT sealed for `work` was served as `personal`. \
             Boundary isolation is defeated by a file rename."
        );
    }

    /// Tampering must not yield attacker-influenced content.
    #[tokio::test]
    async fn tampered_report_is_rejected() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_report_json(tmp.path(), "personal", &minimal_report_json("personal"));

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
        let path = loader.path_for(&boundary("personal"));
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0b0000_0001;
        std::fs::write(&path, &bytes).unwrap();

        assert!(
            loader.load(&boundary("personal")).await.unwrap().is_none(),
            "a tampered REPORT MUST be rejected"
        );
    }

    /// A leftover pre-ADR-SEC-007 plaintext `.report.json` must NOT be picked
    /// up as a fallback. If it were, the migration could be skipped and the
    /// vulnerability would persist behind a passing test suite.
    #[tokio::test]
    async fn legacy_plaintext_report_is_never_read() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reports_dir = tmp.path().join(REPORTS_DIRNAME);
        std::fs::create_dir_all(&reports_dir).unwrap();
        std::fs::write(
            reports_dir.join("personal.report.json"),
            minimal_report_json("personal"),
        )
        .unwrap();

        let loader = FilesystemReportLoader::new(tmp.path().to_path_buf(), &TEST_KEY);
        assert!(
            loader.load(&boundary("personal")).await.unwrap().is_none(),
            "ADR-SEC-007: a legacy PLAINTEXT REPORT MUST NOT be read. Any \
             plaintext fallback reintroduces the vulnerability."
        );
    }

    fn minimal_report_json(boundary_name: &str) -> String {
        serde_json::json!({
            "schema_version": 1,
            "boundary": boundary_name,
            "generated_at": "2026-05-26T03:00:00Z",
            "consolidator_run_id": "00000000-0000-0000-0000-000000000001",
            "facts_by_topic": {}
        })
        .to_string()
    }
}
