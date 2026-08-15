//! Per-boundary REPORT artifact (T0.3.x Batch A, Commit 4 — ADR-053).
//!
//! Structured JSON the read pipeline (Commit 6) consumes to enrich
//! retrieved candidates with topic tags + supersede the previous
//! "Qwen-7B reads raw memories" pattern. **No LLM ingests this** — it's
//! agent-facing structured data, not narrative.
//!
//! ## Shape (locked-next-arc plan iteration 3 § Contract 1)
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "boundary": "personal",
//!   "generated_at": "2026-05-26T03:00:00Z",
//!   "consolidator_run_id": "uuid...",
//!   "facts_by_topic": {
//!     "blood_pressure_readings": [
//!       { "fact": "BP was 132/85 on 2026-05-20", "memory_id": "...",
//!         "as_of": "2026-05-20T08:00:00Z", "confidence": 0.95,
//!         "source_agent": "claude" }
//!     ],
//!     "learning_spanish": [...]
//!   }
//! }
//! ```
//!
//! `facts_by_topic` is a [`BTreeMap`] for deterministic JSON output —
//! topic ordering is alphabetical by label, which makes diffing
//! consecutive nightly REPORTs cheap.
//!
//! ## Lifecycle
//!
//! - **Sealed at rest (ADR-SEC-007)**: the JSON above is the PLAINTEXT
//!   shape. It is never written to disk in that form — it is sealed in
//!   memory via [`vault_storage::seal_vault_blob`] and only the encrypted
//!   envelope reaches the filesystem, at `<boundary>.report.sealed`.
//! - **Atomic write**: `<vault_root>/reports/<boundary>.report.sealed.tmp`
//!   → `Write::write_all` → `File::sync_all` → `std::fs::rename` to the
//!   final path. POSIX `rename(2)` is atomic; Windows `MoveFileEx` with
//!   the default `MOVEFILE_REPLACE_EXISTING` is atomic when source and
//!   target are on the same volume (which is always the case here —
//!   both paths live under the vault root). A reader of the REPORT file
//!   thus sees either the **old** valid REPORT or the **new** valid
//!   REPORT, never a half-written file. No separate file lock needed.
//! - **Versioning**: only the latest REPORT per boundary is kept. If a
//!   bad REPORT lands, the next nightly run fixes it. No version
//!   history at V0.2; Commit 6 staleness-tier health-warnings cover the
//!   "nobody re-ran the consolidator in N days" case.
//! - **Granularity**: one REPORT file per boundary so cross-boundary
//!   reads don't cascade-fail if one boundary's REPORT is corrupt —
//!   the read pipeline at Commit 6 surfaces `REPORT_MISSING` per
//!   boundary independently.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vault_core::{Boundary, Memory, MemoryId, VaultResult};

use crate::topics::TopicMap;

/// Locked schema-version pin. Read pipeline at Commit 6 refuses to
/// consume a REPORT with a higher schema_version than it understands —
/// forward-compat guard against silent contract drift if a future
/// consolidator commit lands a schema bump without a coordinated read
/// pipeline update.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

/// REPORT on-disk layout — the filename, the suffixes, and the AAD input
/// string — is owned by [`vault_storage::report_layout`] and re-exported
/// here so existing `vault_consolidator::REPORTS_DIRNAME` callers keep
/// working.
///
/// **It lives down there, not here, on purpose (ADR-SEC-007).** The REPORT
/// is written by this crate and read by `vault-retrieval`, which does not
/// depend on this one. The relative path is hashed into the AEAD's
/// associated data, so a one-character divergence between the writer's
/// string and the reader's would seal fine, write fine, and then fail every
/// unseal — degrading silently to `REPORT_MISSING` with no error at the
/// point of the mistake. One definition, in the crate both sides already
/// depend on, removes that failure mode by construction.
pub use vault_storage::{
    sealed_report_filename, sealed_report_relative_path, REPORTS_DIRNAME,
    REPORT_LEGACY_PLAINTEXT_SUFFIX, REPORT_SEALED_SUFFIX,
};

/// One per-boundary REPORT artifact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Report {
    pub schema_version: u32,
    pub boundary: Boundary,
    pub generated_at: DateTime<Utc>,
    pub consolidator_run_id: Uuid,
    /// Topic label → ordered facts. `BTreeMap` for deterministic JSON
    /// output (consecutive nightly REPORTs diff cleanly).
    pub facts_by_topic: BTreeMap<String, Vec<ReportFact>>,
    /// `true` when Phi-4 was unavailable or each call failed at topic-
    /// naming time, so cluster `label`s in `facts_by_topic` are the
    /// placeholder form `"topic_<id>"`. Mirrors
    /// [`crate::topics::TopicMap::topic_names_unavailable`]; the Commit 6
    /// structured read pipeline surfaces this as the
    /// `TOPIC_NAMES_UNAVAILABLE` health-warning so the calling agent
    /// knows topic labels are not trustworthy and should be presented
    /// to the user as opaque cluster identifiers, not semantic groupings.
    ///
    /// Additive at ADR-053 Amendment 1 (Commit 6, 2026-05-26). `#[serde(default)]`
    /// makes pre-amendment REPORTs (none exist in practice — Batch A
    /// shipped 2026-05-26 and no nightly run has executed) deserialize
    /// as `false`, preserving backward-compat without a schema_version
    /// bump.
    #[serde(default)]
    pub topic_names_unavailable: bool,
}

/// One structured fact inside a topic. The fields are exactly what the
/// agent-facing `memory_read` response shape carries at Commit 6 — no
/// translation step needed between Report and the MCP read response.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReportFact {
    /// The memory content verbatim. Caller (read pipeline) is
    /// responsible for any truncation / packing decisions.
    pub fact: String,
    pub memory_id: MemoryId,
    /// Fact-time anchor — when the fact became true in the world.
    /// Maps to `Memory::valid_from` per ADR-051's bi-temporal semantics.
    pub as_of: DateTime<Utc>,
    pub confidence: f32,
    pub source_agent: Option<String>,
}

/// Build a [`Report`] by combining a [`TopicMap`] (from
/// [`crate::topics::discover_topics`]) with the boundary's memories.
///
/// Empty topics — those whose `member_ids` are not present in the
/// supplied `memories` slice (e.g., the memory was superseded between
/// topic discovery and report generation) — are dropped from the
/// output, so `facts_by_topic` never contains an empty array.
pub fn generate_report(
    topic_map: &TopicMap,
    memories: &[Memory],
    consolidator_run_id: Uuid,
    generated_at: DateTime<Utc>,
) -> Report {
    let lookup: HashMap<MemoryId, &Memory> = memories.iter().map(|m| (m.id, m)).collect();
    let mut facts_by_topic = BTreeMap::new();
    for topic in &topic_map.topics {
        let mut facts = Vec::with_capacity(topic.member_ids.len());
        for id in &topic.member_ids {
            if let Some(m) = lookup.get(id) {
                facts.push(ReportFact {
                    fact: m.content.clone(),
                    memory_id: m.id,
                    as_of: m.valid_from,
                    confidence: m.confidence,
                    source_agent: m.source_agent.clone(),
                });
            }
        }
        if !facts.is_empty() {
            facts_by_topic.insert(topic.label.clone(), facts);
        }
    }
    Report {
        schema_version: REPORT_SCHEMA_VERSION,
        boundary: topic_map.boundary.clone(),
        generated_at,
        consolidator_run_id,
        facts_by_topic,
        topic_names_unavailable: topic_map.topic_names_unavailable,
    }
}

/// Write a [`Report`] to disk atomically, SEALED, at
/// `<vault_root>/reports/<boundary>.report.sealed`.
///
/// **Pattern**: serialise → seal → write to `<final>.tmp`, `fsync`,
/// `rename` to `<final>`. POSIX `rename(2)` + Windows
/// `MoveFileEx(REPLACE_EXISTING)` are both atomic when source + target
/// share a volume (always the case here).
///
/// # Encryption (ADR-SEC-007)
///
/// Before ADR-SEC-007 this wrote **plaintext JSON containing verbatim
/// memory text**, readable by anyone with filesystem access and with no
/// key — a direct violation of BRD §11.5.1 ("All data on disk is
/// encrypted. No exceptions."). The payload now goes through
/// [`vault_storage::seal_vault_blob`], the same XChaCha20-Poly1305
/// envelope the vector store and graph snapshot use (SP-5: no new crypto).
///
/// The plaintext JSON is never written to disk at any point — sealing
/// happens in memory, and only the sealed bytes reach the `.tmp` file.
/// Sealing BEFORE the write (rather than sealing the temp file in place)
/// is what makes that true; do not "optimise" it into a streaming write.
///
/// Returns the final path on success so the caller can log it or
/// surface it in the run summary.
///
/// # Errors
///
/// - [`vault_core::VaultError::Io`] — directory creation, write,
///   fsync, or rename failed. Atomic-rename guarantees the previous
///   REPORT file (if any) is untouched on any failure before the
///   final rename step.
/// - [`vault_core::VaultError::Serde`] — JSON serialisation failed
///   (shouldn't happen with our derived `Serialize` impl).
pub fn write_report_atomic(
    report: &Report,
    vault_root: &Path,
    at_rest_key: &[u8; 32],
) -> VaultResult<PathBuf> {
    let reports_dir = vault_root.join(REPORTS_DIRNAME);
    std::fs::create_dir_all(&reports_dir)?;

    let filename = sealed_report_filename(&report.boundary);
    let target_path = reports_dir.join(&filename);
    let tmp_path = reports_dir.join(format!("{filename}.tmp"));

    let json = serde_json::to_vec_pretty(report)?;
    let sealed = vault_storage::seal_vault_blob(
        &json,
        at_rest_key,
        &sealed_report_relative_path(&report.boundary),
    );

    // Write to .tmp first, fsync to durable storage, then rename. Any
    // failure before the rename leaves the previous REPORT (if any)
    // intact. A stale `.tmp` file may persist if the process is killed
    // between fsync and rename — the NEXT consolidator run truncates
    // it via `OpenOptions::truncate(true)` so no cleanup-on-acquire
    // step is needed. The stale `.tmp` is itself sealed, so a crash
    // cannot leave readable fact text behind.
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(&sealed)?;
        file.sync_all()?;
    }

    std::fs::rename(&tmp_path, &target_path)?;
    Ok(target_path)
}

/// Outcome of one [`migrate_plaintext_reports`] sweep. Counts rather than
/// paths so callers can log a summary without echoing vault filenames
/// (which carry boundary names) into logs at info level.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlaintextReportMigration {
    /// Legacy plaintext REPORTs found on disk.
    pub found: usize,
    /// Successfully re-sealed to `<boundary>.report.sealed`.
    pub sealed: usize,
    /// Deleted without re-sealing because the content was unreadable or
    /// its filename did not yield a valid boundary. Data was already
    /// unusable; leaving it would leave plaintext.
    pub discarded: usize,
    /// Plaintext files that could NOT be removed (permissions, lock).
    /// **Non-zero means plaintext memory text is still on disk.**
    pub failed_to_remove: usize,
}

/// Destroy every pre-ADR-SEC-007 plaintext REPORT under
/// `<vault_root>/reports/`, re-sealing the content where possible.
///
/// # Why this exists
///
/// Shipping the sealed writer alone fixes only FUTURE runs. Every vault
/// that has ever run a consolidation already has
/// `reports/<boundary>.report.json` sitting on disk in the clear, and it
/// would sit there indefinitely because only the latest REPORT is kept and
/// the new writer uses a different filename. The vulnerability is the file
/// that is already there.
///
/// # Behaviour
///
/// For each `*.report.json`:
/// 1. Read it and parse the boundary from the filename.
/// 2. If both succeed, seal the bytes to `<boundary>.report.sealed`
///    (via [`write_sealed_bytes_atomic`]) and delete the plaintext.
/// 3. If either fails, delete the plaintext anyway and count it as
///    `discarded` — an unreadable REPORT is worthless, and preserving a
///    worthless file in the clear is strictly worse than losing it. The
///    next consolidation regenerates it; a missing REPORT is an already
///    handled state (`REPORT_MISSING`).
///
/// **Deletion is the point.** Any path through this function that leaves
/// the plaintext file in place is a bug, which is why `failed_to_remove`
/// is reported separately rather than folded into an error.
///
/// Idempotent: a vault with no legacy REPORTs returns an all-zero count.
///
/// # Errors
///
/// - [`vault_core::VaultError::Io`] — the reports directory exists but
///   could not be listed. A MISSING reports directory is not an error
///   (fresh vault) and returns an all-zero count.
pub fn migrate_plaintext_reports(
    vault_root: &Path,
    at_rest_key: &[u8; 32],
) -> VaultResult<PlaintextReportMigration> {
    let reports_dir = vault_root.join(REPORTS_DIRNAME);
    if !reports_dir.is_dir() {
        return Ok(PlaintextReportMigration::default());
    }

    let legacy_tail = format!(".{REPORT_LEGACY_PLAINTEXT_SUFFIX}");
    let mut outcome = PlaintextReportMigration::default();

    for entry in std::fs::read_dir(&reports_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Match `<boundary>.report.json` exactly. `.report.json.tmp` is
        // also plaintext and also has to go, so match the stem instead of
        // requiring the name to END with the suffix.
        let Some(boundary_name) = name.strip_suffix(&legacy_tail).or_else(|| {
            name.strip_suffix(".tmp")
                .and_then(|n| n.strip_suffix(&legacy_tail))
        }) else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        outcome.found += 1;

        let resealed = match (std::fs::read(&path), Boundary::new(boundary_name)) {
            (Ok(bytes), Ok(boundary)) => {
                let sealed = vault_storage::seal_vault_blob(
                    &bytes,
                    at_rest_key,
                    &sealed_report_relative_path(&boundary),
                );
                let target = reports_dir.join(sealed_report_filename(&boundary));
                write_sealed_bytes_atomic(&sealed, &target).is_ok()
            }
            _ => false,
        };

        // Remove the plaintext whether or not re-sealing worked. This is
        // the security-critical step and it is deliberately unconditional.
        match std::fs::remove_file(&path) {
            Ok(()) => {
                if resealed {
                    outcome.sealed += 1;
                } else {
                    outcome.discarded += 1;
                }
            }
            Err(e) => {
                outcome.failed_to_remove += 1;
                tracing::error!(
                    target: "vault_consolidator::report",
                    error = %e,
                    "SECURITY: could not delete a legacy PLAINTEXT REPORT; \
                     memory text remains readable on disk (ADR-SEC-007)"
                );
            }
        }
    }

    Ok(outcome)
}

/// Atomic `.tmp` + fsync + rename write of already-sealed bytes.
///
/// Shared by [`write_report_atomic`] and [`migrate_plaintext_reports`] so
/// there is exactly one durability pattern to reason about.
///
/// # Errors
///
/// [`vault_core::VaultError::Io`] on create, write, fsync, or rename.
fn write_sealed_bytes_atomic(sealed: &[u8], target_path: &Path) -> VaultResult<()> {
    let tmp_path = target_path.with_extension("sealed.tmp");
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(sealed)?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp_path, target_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topics::Topic;
    use vault_core::{MemoryType, NewMemory};

    /// Deterministic at-rest key for tests. Production keys come from the
    /// OS keychain via `vault_app::keychain::derive_at_rest_key`.
    const TEST_KEY: [u8; 32] = [0x5a; 32];

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("test boundary must validate")
    }

    /// Read a sealed REPORT off disk and unseal it, mirroring what
    /// `vault_retrieval::FilesystemReportLoader` does in production.
    fn read_sealed(path: &Path, boundary_name: &str) -> Vec<u8> {
        let sealed = std::fs::read(path).expect("sealed REPORT must exist");
        vault_storage::unseal_vault_blob(
            &sealed,
            &TEST_KEY,
            &sealed_report_relative_path(&boundary(boundary_name)),
        )
        .expect("sealed REPORT must unseal with the correct key and path")
    }

    fn make_memory(id_n: u128, content: &str, confidence: f32) -> Memory {
        let mut m = Memory::try_new(NewMemory {
            content: content.to_string(),
            memory_type: MemoryType::Semantic,
            boundary: boundary("personal"),
            source_agent: Some("claude".to_string()),
            confidence,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .expect("test memory must validate");
        m.id = MemoryId(Uuid::from_u128(id_n));
        m
    }

    fn topic_map_with(boundary_name: &str, topics: Vec<Topic>) -> TopicMap {
        TopicMap {
            boundary: boundary(boundary_name),
            topics,
            topic_names_unavailable: false,
        }
    }

    #[test]
    fn generate_report_emits_schema_version_pin() {
        let mems = vec![make_memory(1, "BP 132/85", 0.95)];
        let topics = vec![Topic {
            topic_id: 0,
            label: "blood_pressure".into(),
            member_ids: vec![mems[0].id],
        }];
        let report = generate_report(
            &topic_map_with("personal", topics),
            &mems,
            Uuid::nil(),
            Utc::now(),
        );
        assert_eq!(
            report.schema_version, REPORT_SCHEMA_VERSION,
            "schema_version pin MUST equal REPORT_SCHEMA_VERSION constant; \
             read pipeline at Commit 6 relies on this for forward-compat"
        );
    }

    #[test]
    fn generate_report_groups_facts_by_topic_label() {
        let m1 = make_memory(1, "BP 132/85 yesterday", 0.95);
        let m2 = make_memory(2, "BP 128/82 today", 0.95);
        let m3 = make_memory(3, "Bought groceries", 0.9);
        let topics = vec![
            Topic {
                topic_id: 0,
                label: "blood_pressure".into(),
                member_ids: vec![m1.id, m2.id],
            },
            Topic {
                topic_id: 1,
                label: "shopping".into(),
                member_ids: vec![m3.id],
            },
        ];
        let report = generate_report(
            &topic_map_with("personal", topics),
            &[m1.clone(), m2.clone(), m3.clone()],
            Uuid::nil(),
            Utc::now(),
        );
        assert_eq!(report.facts_by_topic.len(), 2);
        let bp = report.facts_by_topic.get("blood_pressure").unwrap();
        assert_eq!(bp.len(), 2, "blood_pressure topic MUST hold 2 facts");
        assert!(
            bp.iter().any(|f| f.fact == "BP 132/85 yesterday"),
            "facts MUST carry memory content verbatim"
        );
        let shopping = report.facts_by_topic.get("shopping").unwrap();
        assert_eq!(shopping.len(), 1);
    }

    #[test]
    fn generate_report_drops_topics_whose_members_are_not_in_memories_slice() {
        // Topic references a memory that was superseded between topic
        // discovery and report generation; the empty-after-lookup topic
        // gets dropped rather than producing an empty array.
        let m1 = make_memory(1, "present", 0.9);
        let topics = vec![
            Topic {
                topic_id: 0,
                label: "present_topic".into(),
                member_ids: vec![m1.id],
            },
            Topic {
                topic_id: 1,
                label: "ghost_topic".into(),
                member_ids: vec![MemoryId(Uuid::from_u128(999))],
            },
        ];
        let report = generate_report(
            &topic_map_with("personal", topics),
            std::slice::from_ref(&m1),
            Uuid::nil(),
            Utc::now(),
        );
        assert_eq!(
            report.facts_by_topic.len(),
            1,
            "ghost_topic with no resolvable members MUST be dropped from output"
        );
        assert!(report.facts_by_topic.contains_key("present_topic"));
        assert!(!report.facts_by_topic.contains_key("ghost_topic"));
    }

    #[test]
    fn report_serialisation_uses_deterministic_topic_ordering() {
        // BTreeMap inside Report.facts_by_topic gives alphabetic ordering;
        // serde_json::to_string preserves BTreeMap iteration order
        // (NOT alphabetic for HashMap). Pin so a future "helpful" HashMap
        // swap trips the test.
        let m1 = make_memory(1, "a", 0.9);
        let m2 = make_memory(2, "b", 0.9);
        let topics = vec![
            Topic {
                topic_id: 0,
                label: "zebra".into(),
                member_ids: vec![m1.id],
            },
            Topic {
                topic_id: 1,
                label: "apple".into(),
                member_ids: vec![m2.id],
            },
        ];
        let report = generate_report(
            &topic_map_with("personal", topics),
            &[m1.clone(), m2.clone()],
            Uuid::nil(),
            Utc::now(),
        );
        let json = serde_json::to_string(&report).unwrap();
        let apple_pos = json.find("apple").expect("'apple' MUST appear in json");
        let zebra_pos = json.find("zebra").expect("'zebra' MUST appear in json");
        assert!(
            apple_pos < zebra_pos,
            "alphabetic topic ordering MUST hold in serialised JSON; \
             got apple at {apple_pos}, zebra at {zebra_pos}"
        );
    }

    #[test]
    fn write_report_atomic_creates_file_at_expected_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let m1 = make_memory(1, "BP 132/85", 0.95);
        let topics = vec![Topic {
            topic_id: 0,
            label: "blood_pressure".into(),
            member_ids: vec![m1.id],
        }];
        let report = generate_report(
            &topic_map_with("personal", topics),
            &[m1],
            Uuid::nil(),
            Utc::now(),
        );

        let path = write_report_atomic(&report, tmp.path(), &TEST_KEY).unwrap();
        assert_eq!(
            path,
            tmp.path().join("reports").join("personal.report.sealed")
        );
        assert!(path.exists(), "REPORT file MUST exist at returned path");
    }

    #[test]
    fn write_report_atomic_round_trips_through_json_serialization() {
        let tmp = tempfile::TempDir::new().unwrap();
        let m1 = make_memory(1, "BP 132/85", 0.95);
        let topics = vec![Topic {
            topic_id: 0,
            label: "blood_pressure".into(),
            member_ids: vec![m1.id],
        }];
        let original = generate_report(
            &topic_map_with("personal", topics),
            &[m1],
            Uuid::from_u128(42),
            Utc::now(),
        );

        let path = write_report_atomic(&original, tmp.path(), &TEST_KEY).unwrap();
        let restored: Report = serde_json::from_slice(&read_sealed(&path, "personal")).unwrap();
        assert_eq!(
            original, restored,
            "Report MUST round-trip cleanly through atomic-write + seal + \
             unseal + JSON parse"
        );
    }

    #[test]
    fn write_report_atomic_replaces_previous_report_at_same_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let m1 = make_memory(1, "v1", 0.9);
        let topics_v1 = vec![Topic {
            topic_id: 0,
            label: "v1_topic".into(),
            member_ids: vec![m1.id],
        }];
        let report_v1 = generate_report(
            &topic_map_with("personal", topics_v1),
            std::slice::from_ref(&m1),
            Uuid::nil(),
            Utc::now(),
        );

        let m2 = make_memory(2, "v2", 0.9);
        let topics_v2 = vec![Topic {
            topic_id: 0,
            label: "v2_topic".into(),
            member_ids: vec![m2.id],
        }];
        let report_v2 = generate_report(
            &topic_map_with("personal", topics_v2),
            &[m2],
            Uuid::nil(),
            Utc::now(),
        );

        write_report_atomic(&report_v1, tmp.path(), &TEST_KEY).unwrap();
        write_report_atomic(&report_v2, tmp.path(), &TEST_KEY).unwrap();

        let path = tmp.path().join("reports").join("personal.report.sealed");
        let contents = String::from_utf8(read_sealed(&path, "personal")).unwrap();
        assert!(
            contents.contains("v2_topic"),
            "second write MUST atomically replace first; final file must \
             contain v2_topic. Got: {contents}"
        );
        assert!(
            !contents.contains("v1_topic"),
            "first REPORT MUST be replaced wholesale (not appended); \
             final file MUST NOT contain v1_topic. Got: {contents}"
        );
    }

    #[test]
    fn write_report_atomic_creates_reports_dir_if_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reports_dir = tmp.path().join("reports");
        assert!(
            !reports_dir.exists(),
            "test precondition: reports dir MUST not exist before write"
        );

        let m1 = make_memory(1, "fact", 0.9);
        let topics = vec![Topic {
            topic_id: 0,
            label: "t0".into(),
            member_ids: vec![m1.id],
        }];
        let report = generate_report(
            &topic_map_with("personal", topics),
            &[m1],
            Uuid::nil(),
            Utc::now(),
        );

        write_report_atomic(&report, tmp.path(), &TEST_KEY).unwrap();
        assert!(
            reports_dir.exists(),
            "write_report_atomic MUST create reports dir if missing"
        );
    }

    // =====================================================================
    //   ADR-SEC-007 — the REPORT is sealed, and legacy plaintext is purged
    // =====================================================================

    fn write_report_for(tmp: &Path, boundary_name: &str, fact: &str) -> PathBuf {
        let m1 = make_memory(1, fact, 0.95);
        let topics = vec![Topic {
            topic_id: 0,
            label: "t0".into(),
            member_ids: vec![m1.id],
        }];
        let report = generate_report(
            &topic_map_with(boundary_name, topics),
            &[m1],
            Uuid::nil(),
            Utc::now(),
        );
        write_report_atomic(&report, tmp, &TEST_KEY).unwrap()
    }

    /// The headline assertion of ADR-SEC-007: the bytes on disk do not
    /// contain the memory.
    #[test]
    fn written_report_is_not_readable_on_disk() {
        let tmp = tempfile::TempDir::new().unwrap();
        let secret = "The user was prescribed sertraline in March.";
        let path = write_report_for(tmp.path(), "personal", secret);

        let raw = std::fs::read(&path).unwrap();
        let on_disk = String::from_utf8_lossy(&raw);
        assert!(
            !on_disk.contains(secret),
            "SECURITY REGRESSION (ADR-SEC-007): fact text is readable in the \
             REPORT on disk with no key (BRD §11.5.1)"
        );
        assert!(
            !on_disk.contains("facts_by_topic"),
            "SECURITY REGRESSION: REPORT JSON structure readable on disk"
        );

        // Control: with the key it IS recoverable, so the above is proving
        // encryption rather than an empty file.
        let recovered = String::from_utf8(read_sealed(&path, "personal")).unwrap();
        assert!(
            recovered.contains(secret),
            "control: the sealed REPORT must still contain the fact once unsealed"
        );
    }

    /// No `.report.json` may be produced any more — its existence is the
    /// vulnerability.
    #[test]
    fn writer_leaves_no_plaintext_json_artifact() {
        let tmp = tempfile::TempDir::new().unwrap();
        write_report_for(tmp.path(), "personal", "a fact");

        let leftovers: Vec<_> = std::fs::read_dir(tmp.path().join(REPORTS_DIRNAME))
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(REPORT_LEGACY_PLAINTEXT_SUFFIX))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no plaintext REPORT artifact may be written; found {leftovers:?}"
        );
    }

    #[test]
    fn migration_seals_and_deletes_a_legacy_plaintext_report() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reports_dir = tmp.path().join(REPORTS_DIRNAME);
        std::fs::create_dir_all(&reports_dir).unwrap();
        let legacy = reports_dir.join("personal.report.json");
        let secret = r#"{"fact":"The user's PIN hint is his birth year."}"#;
        std::fs::write(&legacy, secret).unwrap();

        let outcome = migrate_plaintext_reports(tmp.path(), &TEST_KEY).unwrap();

        assert_eq!(outcome.found, 1);
        assert_eq!(outcome.sealed, 1);
        assert_eq!(outcome.failed_to_remove, 0);
        assert!(
            !legacy.exists(),
            "SECURITY: the legacy PLAINTEXT REPORT survived migration; the \
             vulnerability persists"
        );

        let sealed_path = reports_dir.join("personal.report.sealed");
        assert!(sealed_path.exists(), "content must be preserved, sealed");
        let recovered = String::from_utf8(read_sealed(&sealed_path, "personal")).unwrap();
        assert_eq!(
            recovered, secret,
            "migration must preserve content verbatim"
        );
    }

    /// An unparseable / unreadable legacy file must still be destroyed.
    /// Preserving a worthless file in the clear is strictly worse than
    /// losing it — the next consolidation regenerates the REPORT.
    #[test]
    fn migration_deletes_a_legacy_report_whose_boundary_name_is_invalid() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reports_dir = tmp.path().join(REPORTS_DIRNAME);
        std::fs::create_dir_all(&reports_dir).unwrap();
        // '.' is rejected by the Boundary validator, so this cannot be
        // re-sealed under a valid AAD.
        let legacy = reports_dir.join("not..valid.report.json");
        std::fs::write(&legacy, "leaked text").unwrap();

        let outcome = migrate_plaintext_reports(tmp.path(), &TEST_KEY).unwrap();

        assert_eq!(outcome.found, 1);
        assert_eq!(outcome.discarded, 1);
        assert_eq!(outcome.sealed, 0);
        assert!(
            !legacy.exists(),
            "SECURITY: an un-resealable plaintext REPORT MUST still be deleted"
        );
    }

    /// A `.report.json.tmp` left by a crash mid-write is ALSO plaintext.
    #[test]
    fn migration_also_destroys_leftover_plaintext_tmp_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reports_dir = tmp.path().join(REPORTS_DIRNAME);
        std::fs::create_dir_all(&reports_dir).unwrap();
        let stale = reports_dir.join("personal.report.json.tmp");
        std::fs::write(&stale, r#"{"fact":"leaked via a crash artifact"}"#).unwrap();

        let outcome = migrate_plaintext_reports(tmp.path(), &TEST_KEY).unwrap();

        assert!(outcome.found >= 1, "the stale .tmp must be found");
        assert!(
            !stale.exists(),
            "SECURITY: a stale plaintext .report.json.tmp MUST be destroyed — \
             it contains the same fact text as the REPORT itself"
        );
    }

    /// Migration must not touch sealed REPORTs, or running it would delete
    /// live data on every startup.
    #[test]
    fn migration_leaves_sealed_reports_untouched() {
        let tmp = tempfile::TempDir::new().unwrap();
        let sealed_path = write_report_for(tmp.path(), "personal", "a fact");
        let before = std::fs::read(&sealed_path).unwrap();

        let outcome = migrate_plaintext_reports(tmp.path(), &TEST_KEY).unwrap();

        assert_eq!(outcome.found, 0, "a sealed REPORT is not a legacy artifact");
        assert!(sealed_path.exists(), "the sealed REPORT MUST survive");
        assert_eq!(
            std::fs::read(&sealed_path).unwrap(),
            before,
            "the sealed REPORT MUST be byte-identical after a migration sweep"
        );
    }

    #[test]
    fn migration_is_a_noop_on_a_fresh_vault() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outcome = migrate_plaintext_reports(tmp.path(), &TEST_KEY).unwrap();
        assert_eq!(outcome, PlaintextReportMigration::default());
    }

    /// Idempotent: startup runs this every time.
    #[test]
    fn migration_is_idempotent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let reports_dir = tmp.path().join(REPORTS_DIRNAME);
        std::fs::create_dir_all(&reports_dir).unwrap();
        std::fs::write(reports_dir.join("personal.report.json"), "{}").unwrap();

        let first = migrate_plaintext_reports(tmp.path(), &TEST_KEY).unwrap();
        let second = migrate_plaintext_reports(tmp.path(), &TEST_KEY).unwrap();

        assert_eq!(first.found, 1);
        assert_eq!(
            second,
            PlaintextReportMigration::default(),
            "a second sweep must find nothing left to do"
        );
    }

    // -------------------------------------------------------------------------
    // ADR-053 Amendment 1 (Commit 6, 2026-05-26) — `topic_names_unavailable`
    // additive field. The Commit 6 structured read pipeline reads this flag to
    // surface the `TOPIC_NAMES_UNAVAILABLE` health-warning; without the
    // producer-side population below + the deserialize-default, the signal
    // would silently disappear at the disk boundary.
    // -------------------------------------------------------------------------

    #[test]
    fn generate_report_propagates_topic_names_unavailable_true_from_topic_map() {
        let m1 = make_memory(1, "fact", 0.9);
        let topics = vec![Topic {
            topic_id: 0,
            label: "topic_0".into(), // placeholder label, signalling Phi-4 unavailable
            member_ids: vec![m1.id],
        }];
        let topic_map = TopicMap {
            boundary: boundary("personal"),
            topics,
            topic_names_unavailable: true,
        };
        let report = generate_report(&topic_map, &[m1], Uuid::nil(), Utc::now());
        assert!(
            report.topic_names_unavailable,
            "generate_report MUST propagate topic_names_unavailable=true from TopicMap; \
             otherwise the Commit 6 read pipeline cannot surface TOPIC_NAMES_UNAVAILABLE"
        );
    }

    #[test]
    fn generate_report_propagates_topic_names_unavailable_false_from_topic_map() {
        let m1 = make_memory(1, "fact", 0.9);
        let topics = vec![Topic {
            topic_id: 0,
            label: "blood_pressure".into(),
            member_ids: vec![m1.id],
        }];
        let topic_map = TopicMap {
            boundary: boundary("personal"),
            topics,
            topic_names_unavailable: false,
        };
        let report = generate_report(&topic_map, &[m1], Uuid::nil(), Utc::now());
        assert!(
            !report.topic_names_unavailable,
            "happy-path topic_names_unavailable=false MUST propagate too \
             (pins the populate direction, not just the truthy case)"
        );
    }

    #[test]
    fn report_deserializes_pre_amendment_json_without_topic_names_unavailable_field() {
        // Pre-ADR-053-Amendment-1 REPORTs (none exist in practice — Batch A
        // shipped 2026-05-26 and no nightly run has executed yet) omit the
        // field. #[serde(default)] makes deserialize succeed with the field
        // set to `false`. This pin protects the backward-compat path so a
        // future "tighten serde" change can't silently break old REPORTs.
        let pre_amendment_json = serde_json::json!({
            "schema_version": 1,
            "boundary": "personal",
            "generated_at": "2026-05-26T03:00:00Z",
            "consolidator_run_id": "00000000-0000-0000-0000-000000000000",
            "facts_by_topic": {}
        })
        .to_string();
        let parsed: Report = serde_json::from_str(&pre_amendment_json).unwrap_or_else(|e| {
            panic!("Pre-amendment Report JSON MUST deserialize via #[serde(default)]; got: {e}")
        });
        assert!(
            !parsed.topic_names_unavailable,
            "missing field MUST default to false (the safe value — no warning surfaced)"
        );
    }
}
