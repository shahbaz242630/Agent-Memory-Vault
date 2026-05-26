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
//! - **Atomic write**: `<vault_root>/reports/<boundary>.report.json.tmp`
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

/// Directory under the vault root where REPORT artifacts live. Created
/// lazily by [`write_report_atomic`] on first write.
pub const REPORTS_DIRNAME: &str = "reports";

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
}

/// One structured fact inside a topic. The fields are exactly what the
/// agent-facing `memory.read` response shape carries at Commit 6 — no
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
    }
}

/// Write a [`Report`] to disk atomically at
/// `<vault_root>/reports/<boundary>.report.json`.
///
/// **Pattern**: write to `<final>.tmp`, `fsync`, `rename` to `<final>`.
/// POSIX `rename(2)` + Windows `MoveFileEx(REPLACE_EXISTING)` are both
/// atomic when source + target share a volume (always the case here).
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
pub fn write_report_atomic(report: &Report, vault_root: &Path) -> VaultResult<PathBuf> {
    let reports_dir = vault_root.join(REPORTS_DIRNAME);
    std::fs::create_dir_all(&reports_dir)?;

    let filename = format!("{}.report.json", report.boundary.as_str());
    let target_path = reports_dir.join(&filename);
    let tmp_path = reports_dir.join(format!("{filename}.tmp"));

    let json = serde_json::to_vec_pretty(report)?;

    // Write to .tmp first, fsync to durable storage, then rename. Any
    // failure before the rename leaves the previous REPORT (if any)
    // intact. A stale `.tmp` file may persist if the process is killed
    // between fsync and rename — the NEXT consolidator run truncates
    // it via `OpenOptions::truncate(true)` so no cleanup-on-acquire
    // step is needed.
    {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(&json)?;
        file.sync_all()?;
    }

    std::fs::rename(&tmp_path, &target_path)?;
    Ok(target_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topics::Topic;
    use vault_core::{MemoryType, NewMemory};

    fn boundary(name: &str) -> Boundary {
        Boundary::new(name).expect("test boundary must validate")
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

        let path = write_report_atomic(&report, tmp.path()).unwrap();
        assert_eq!(
            path,
            tmp.path().join("reports").join("personal.report.json")
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

        let path = write_report_atomic(&original, tmp.path()).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let restored: Report = serde_json::from_str(&contents).unwrap();
        assert_eq!(
            original, restored,
            "Report MUST round-trip cleanly through atomic-write + JSON parse"
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

        write_report_atomic(&report_v1, tmp.path()).unwrap();
        write_report_atomic(&report_v2, tmp.path()).unwrap();

        let path = tmp.path().join("reports").join("personal.report.json");
        let contents = std::fs::read_to_string(&path).unwrap();
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

        write_report_atomic(&report, tmp.path()).unwrap();
        assert!(
            reports_dir.exists(),
            "write_report_atomic MUST create reports dir if missing"
        );
    }
}
