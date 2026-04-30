//! `vault-cli` — operator command-line interface for the Memory Vault
//! (T0.1.6 Phase C1b + C2).
//!
//! ## Scope (V0.1, founder-only alpha)
//!
//! Dead-letter triage + on-demand divergence check:
//!
//! - `dead-letter list` — show unresolved dead-letter rows
//! - `dead-letter inspect <id>` — show full detail for one row
//! - `dead-letter retry <id>` — re-run the cascade for one row, mark
//!   resolved as `retried_succeeded` or `retried_failed`
//! - `dead-letter acknowledge <id> --reason <text>` — operator accepts the
//!   loss; row stays for audit but no further retries
//! - `divergence-check` — two-tier consistency check (count + sampled
//!   existence) between SQLite and the vector store; non-zero exit on
//!   findings so scripts notice. See ADR-018.
//!
//! Any richer admin surface lives in its own task with its own scope
//! review (per Phase C plan Q4 "tightness constraint").
//!
//! ## Authentication
//!
//! Every invocation requires the master passphrase. Read **stdin only,
//! no-echo** via `rpassword` — no env-var support, per Phase C plan
//! smaller-item-(a). Auth is verified by attempting to open the
//! SQLCipher metadata DB; failure is reported generically as
//! "authentication failed" with no information leak about which check
//! triggered the failure (BRD §11.7.2 / §11.4.4).

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use uuid::Uuid;

use vault_core::{Boundary, MemoryId};
use vault_storage::{
    CascadeOperation, DeadLetterEntry, DivergenceDetector, DivergenceReport, Resolution,
    SqlCipherKey, StorageBackend,
};

#[derive(Parser, Debug)]
#[command(
    name = "vault-cli",
    about = "Memory Vault operator CLI (V0.1).",
    version
)]
struct Cli {
    /// Path to the SQLCipher metadata DB.
    #[arg(long, value_name = "PATH")]
    vault_db: PathBuf,

    /// Path to the LanceDB data directory.
    #[arg(long, value_name = "PATH")]
    vector_dir: PathBuf,

    /// Path to the DuckDB graph database file.
    #[arg(long, value_name = "PATH")]
    graph_db: PathBuf,

    /// Embedding dimension expected by the vector store. Must match the
    /// dimension the vault was created with — passing a mismatched value
    /// will be reported as authentication failure (no info leak).
    #[arg(long, default_value_t = 384)]
    dimension: usize,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Dead-letter queue triage — list / inspect / retry / acknowledge.
    DeadLetter {
        #[command(subcommand)]
        action: DeadLetterAction,
    },
    /// On-demand consistency check between SQLite and the LanceDB
    /// vector store. Reports tier-1 count comparison + tier-2 sampled
    /// existence findings. Per Phase A Q3 / ADR-018.
    DivergenceCheck,
}

#[derive(Subcommand, Debug)]
enum DeadLetterAction {
    /// List unresolved dead-letter rows, oldest first.
    List {
        /// Maximum number of rows to display.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Show full detail for one dead-letter row by id.
    Inspect {
        /// Dead-letter row id (UUID).
        id: String,
    },
    /// Re-run the cascade for one dead-letter row. On success the row is
    /// marked `retried_succeeded`; on failure it stays for audit and is
    /// marked `retried_failed`.
    Retry {
        /// Dead-letter row id (UUID).
        id: String,
    },
    /// Operator explicitly accepts the loss. The row stays for audit but
    /// is marked `acknowledged` — no further retries.
    Acknowledge {
        /// Dead-letter row id (UUID).
        id: String,
        /// Operator-supplied reason. Recorded for posterity.
        #[arg(long)]
        reason: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    match real_main().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,vault_cli=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

async fn real_main() -> Result<()> {
    let cli = Cli::parse();
    let key = read_passphrase()?;
    let backend = open_backend(&cli, key).await?;

    if backend.degraded().is_degraded() {
        eprintln!(
            "warning: vault opened in degraded mode ({:?}) — some downstream stores are unreadable",
            backend.degraded()
        );
    }

    match cli.command {
        Command::DeadLetter { action } => dispatch_dead_letter(&backend, action).await,
        Command::DivergenceCheck => run_divergence_check(&backend).await,
    }
}

async fn run_divergence_check(backend: &StorageBackend) -> Result<()> {
    let detector = DivergenceDetector::new(backend.clone());
    let report = detector.run().await?;
    print_divergence_report(&report);
    if report.has_findings() {
        // Non-zero exit when there's anything to triage. Scripts can
        // pipe to a notification channel or fail a CI job.
        anyhow::bail!("divergence findings present — see report above");
    }
    Ok(())
}

fn print_divergence_report(r: &DivergenceReport) {
    println!("divergence check at {}", r.run_at.to_rfc3339());
    println!("  sqlite memories  : {}", r.sqlite_memory_count);
    println!("  vector rows      : {}", r.vector_count);
    if r.count_mismatch() {
        let delta = r.sqlite_memory_count as i64 - r.vector_count as i64;
        println!("  count mismatch   : sqlite - vector = {delta} (tier-1 finding)");
    } else {
        println!("  count match      : ok");
    }
    println!("  samples checked  : {}", r.samples_checked);
    if r.missing_in_vector.is_empty() {
        println!("  missing in vector: (none)");
    } else {
        println!(
            "  missing in vector: {} id(s) — tier-2 sampled-existence finding",
            r.missing_in_vector.len()
        );
        for id in &r.missing_in_vector {
            println!("    - {id}");
        }
    }
    println!(
        "  pending_sync resync count: {} (V0.1 stub — see ADR-018 / HANDOFF tech debt)",
        r.pending_sync_resync_count
    );
    if r.has_findings() {
        println!("\nfindings present — investigate via vault-cli dead-letter list / inspect");
    } else {
        println!("\nno findings.");
    }
}

/// Read the master passphrase from stdin with no echo. Returns an
/// `SqlCipherKey` ready to pass to `StorageBackend::open`.
///
/// Per Phase C plan smaller-item-(a): stdin-only, NO env-var support. The
/// stdin path is pipeable for scripts (`cat key.txt | vault-cli ...`)
/// while still giving interactive users a no-echo prompt.
fn read_passphrase() -> Result<SqlCipherKey> {
    // `rpassword::prompt_password` writes the prompt to stderr (so it
    // doesn't pollute stdout if the operator is piping the output) and
    // reads from stdin without echoing.
    let pw = rpassword::prompt_password("vault passphrase: ")
        .context("failed to read passphrase from stdin")?;
    if pw.is_empty() {
        anyhow::bail!("passphrase must not be empty");
    }
    Ok(SqlCipherKey::new(pw))
}

/// Open the storage backend with the supplied paths + key. Generic
/// `authentication failed` on any error — no info leak (BRD §11.7.2 /
/// §11.4.4). Detailed diagnostics go to the local tracing subscriber for
/// dev debugging only.
async fn open_backend(cli: &Cli, key: SqlCipherKey) -> Result<StorageBackend> {
    StorageBackend::open(
        &cli.vault_db,
        &cli.vector_dir,
        &cli.graph_db,
        key,
        cli.dimension,
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "open backend failed");
        anyhow!("authentication failed")
    })
}

async fn dispatch_dead_letter(backend: &StorageBackend, action: DeadLetterAction) -> Result<()> {
    match action {
        DeadLetterAction::List { limit } => list_dead_letters(backend, limit).await,
        DeadLetterAction::Inspect { id } => inspect_dead_letter(backend, &id).await,
        DeadLetterAction::Retry { id } => retry_dead_letter(backend, &id).await,
        DeadLetterAction::Acknowledge { id, reason } => {
            acknowledge_dead_letter(backend, &id, &reason).await
        }
    }
}

async fn list_dead_letters(backend: &StorageBackend, limit: usize) -> Result<()> {
    let entries = backend.dead_letter().list_unresolved(limit).await?;
    if entries.is_empty() {
        println!("(no unresolved dead-letter rows)");
        return Ok(());
    }
    println!(
        "{:<36}  {:<8}  {:<6}  {:<25}  reason",
        "id", "op", "tries", "first_failed_at"
    );
    println!("{}", "-".repeat(110));
    for e in &entries {
        let reason_summary = summarize(&e.failure_reason, 60);
        println!(
            "{:<36}  {:<8}  {:<6}  {:<25}  {}",
            e.id,
            e.failed_operation.as_str(),
            e.attempts_made,
            e.first_failed_at.format("%Y-%m-%dT%H:%M:%SZ"),
            reason_summary,
        );
    }
    println!("\n{} unresolved row(s).", entries.len());
    Ok(())
}

async fn inspect_dead_letter(backend: &StorageBackend, id_str: &str) -> Result<()> {
    let id = Uuid::parse_str(id_str).context("invalid uuid for dead-letter id")?;
    let entry = backend
        .dead_letter()
        .get(id)
        .await?
        .ok_or_else(|| anyhow!("dead-letter row {id} not found"))?;
    print_full_entry(&entry);
    Ok(())
}

async fn retry_dead_letter(backend: &StorageBackend, id_str: &str) -> Result<()> {
    let id = Uuid::parse_str(id_str).context("invalid uuid for dead-letter id")?;
    let entry = backend
        .dead_letter()
        .get(id)
        .await?
        .ok_or_else(|| anyhow!("dead-letter row {id} not found"))?;

    if let Some(existing) = entry.resolution {
        println!(
            "row {id} is already resolved as {} (no action taken)",
            resolution_str(existing)
        );
        return Ok(());
    }

    println!(
        "retrying {} for memory {} (attempts so far: {})...",
        entry.failed_operation.as_str(),
        entry.memory_id,
        entry.attempts_made,
    );

    let cascade_result = run_cascade_synchronous(backend, &entry).await;

    match cascade_result {
        Ok(()) => {
            backend
                .dead_letter()
                .resolve(id, Resolution::RetriedSucceeded)
                .await?;
            println!("retry succeeded; row marked retried_succeeded");
            Ok(())
        }
        Err(e) => {
            backend
                .dead_letter()
                .resolve(id, Resolution::RetriedFailed)
                .await?;
            // Print the error so the operator can see what's still broken,
            // but exit non-zero so scripts notice.
            Err(anyhow!("retry failed; row marked retried_failed: {e}"))
        }
    }
}

async fn acknowledge_dead_letter(
    backend: &StorageBackend,
    id_str: &str,
    reason: &str,
) -> Result<()> {
    if reason.trim().is_empty() {
        anyhow::bail!("--reason must not be empty");
    }
    let id = Uuid::parse_str(id_str).context("invalid uuid for dead-letter id")?;
    let entry = backend
        .dead_letter()
        .get(id)
        .await?
        .ok_or_else(|| anyhow!("dead-letter row {id} not found"))?;
    if let Some(existing) = entry.resolution {
        println!(
            "row {id} is already resolved as {} (no action taken)",
            resolution_str(existing)
        );
        return Ok(());
    }
    backend
        .dead_letter()
        .resolve(id, Resolution::Acknowledged)
        .await?;
    println!("row {id} acknowledged. reason recorded: {reason}");
    Ok(())
}

/// Run the cascade for one dead-letter entry synchronously — vector op
/// only, since V0.1 graph cascade is a no-op for memory writes. Mirrors
/// `RetryWorker::run_cascade` but without fault hooks (operator retry is
/// production-mode by definition).
async fn run_cascade_synchronous(
    backend: &StorageBackend,
    entry: &DeadLetterEntry,
) -> anyhow::Result<()> {
    // Decode the payload. Format-version dispatch lets us evolve the
    // shape later without breaking older rows.
    let payload: vault_storage::cascading::CascadePayloadV1 =
        serde_json::from_value(entry.payload.clone()).context("decode cascade payload")?;

    match entry.failed_operation {
        CascadeOperation::Write | CascadeOperation::Update => {
            let boundary = Boundary::new(payload.boundary).context("payload boundary invalid")?;
            backend
                .vector_store()
                .upsert(&entry.memory_id, &payload.embedding, &boundary)
                .await?;
        }
        CascadeOperation::Delete => {
            backend.vector_store().delete(&entry.memory_id).await?;
        }
    }
    Ok(())
}

fn print_full_entry(e: &DeadLetterEntry) {
    println!("dead-letter row {}", e.id);
    println!("  memory id          : {}", e.memory_id);
    println!("  failed operation   : {}", e.failed_operation.as_str());
    println!("  attempts made      : {}", e.attempts_made);
    println!("  first failed at    : {}", e.first_failed_at.to_rfc3339());
    println!(
        "  last attempted at  : {}",
        e.last_attempted_at.to_rfc3339()
    );
    println!("  payload version    : {}", e.payload_format_version);
    println!(
        "  resolution         : {}",
        e.resolution.map(resolution_str).unwrap_or("pending"),
    );
    if let Some(at) = e.resolved_at {
        println!("  resolved at        : {}", at.to_rfc3339());
    }
    println!("  failure reason:");
    for line in e.failure_reason.lines() {
        println!("    {line}");
    }
    if !e.payload.is_object() && !e.payload.is_array() {
        println!("  payload            : {}", e.payload);
    } else if let Ok(pretty) = serde_json::to_string_pretty(&e.payload) {
        println!("  payload:");
        for line in pretty.lines() {
            println!("    {line}");
        }
    }
}

fn resolution_str(r: Resolution) -> &'static str {
    match r {
        Resolution::RetriedSucceeded => "retried_succeeded",
        Resolution::RetriedFailed => "retried_failed",
        Resolution::Acknowledged => "acknowledged",
        Resolution::AutoRecovered => "auto_recovered",
    }
}

fn summarize(s: &str, max_chars: usize) -> String {
    let first_line = s.lines().next().unwrap_or("");
    if first_line.chars().count() <= max_chars {
        first_line.to_string()
    } else {
        let mut out: String = first_line.chars().take(max_chars - 1).collect();
        out.push('…');
        out
    }
}

// silence unused-import lint when the cascading::CascadePayloadV1 path is
// not present at integration-test time.
#[allow(dead_code)]
fn _force_link(_id: MemoryId) {}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    //! These are smoke tests against a real SQLCipher tempdir per Phase C
    //! plan Q4 ("vault-cli smoke tests against a real SQLCipher tempdir").
    //! Authentication is exercised end-to-end — no mocking the backend.

    use super::*;

    use std::path::Path;

    use tempfile::TempDir;

    use vault_storage::{NewDeadLetter, PAYLOAD_FORMAT_VERSION};

    const DIM: usize = 4;

    async fn make_backend(tmp: &Path) -> StorageBackend {
        let metadata_path = tmp.join("vault.db");
        let vector_dir = tmp.join("lance");
        let graph_path = tmp.join("graph.duckdb");
        let key = SqlCipherKey::new("vault-cli-test-key");
        StorageBackend::open(&metadata_path, &vector_dir, &graph_path, key, DIM)
            .await
            .unwrap()
    }

    fn embedding(fill: f32) -> Vec<f32> {
        vec![fill; DIM]
    }

    /// Plant a dead-letter row directly via the `DeadLetter::insert` API
    /// so the test doesn't need to spin up a worker.
    async fn plant_dead_letter(
        backend: &StorageBackend,
        memory_id: MemoryId,
        op: CascadeOperation,
        reason: &str,
        payload: serde_json::Value,
    ) -> Uuid {
        let now = chrono::Utc::now();
        let new = NewDeadLetter {
            memory_id,
            failed_operation: op,
            failure_reason: reason.to_string(),
            attempts_made: 8,
            first_failed_at: now - chrono::Duration::seconds(60),
            last_attempted_at: now,
            payload_format_version: PAYLOAD_FORMAT_VERSION,
            payload,
        };
        backend.dead_letter().insert(new).await.unwrap()
    }

    // --------------------------------------------------------------
    // CLI argument parsing
    // --------------------------------------------------------------

    #[test]
    fn cli_parses_dead_letter_list() {
        let cli = Cli::try_parse_from([
            "vault-cli",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "--dimension",
            "16",
            "dead-letter",
            "list",
            "--limit",
            "10",
        ])
        .unwrap();
        assert_eq!(cli.dimension, 16);
        match cli.command {
            Command::DeadLetter {
                action: DeadLetterAction::List { limit },
            } => assert_eq!(limit, 10),
            _ => panic!("expected DeadLetter::List"),
        }
    }

    #[test]
    fn cli_parses_dead_letter_acknowledge() {
        let cli = Cli::try_parse_from([
            "vault-cli",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "dead-letter",
            "acknowledge",
            "deadbeef-dead-beef-dead-beefdeadbeef",
            "--reason",
            "operator accepts loss",
        ])
        .unwrap();
        match cli.command {
            Command::DeadLetter {
                action: DeadLetterAction::Acknowledge { id, reason },
            } => {
                assert_eq!(id, "deadbeef-dead-beef-dead-beefdeadbeef");
                assert_eq!(reason, "operator accepts loss");
            }
            _ => panic!("expected DeadLetter::Acknowledge"),
        }
    }

    #[test]
    fn cli_parses_divergence_check() {
        let cli = Cli::try_parse_from([
            "vault-cli",
            "--vault-db",
            "/tmp/v.db",
            "--vector-dir",
            "/tmp/lance",
            "--graph-db",
            "/tmp/g.duckdb",
            "divergence-check",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::DivergenceCheck));
    }

    #[test]
    fn cli_rejects_missing_required_flag() {
        let result = Cli::try_parse_from([
            "vault-cli",
            "--vault-db",
            "/tmp/v.db",
            // missing --vector-dir
            "--graph-db",
            "/tmp/g.duckdb",
            "dead-letter",
            "list",
        ]);
        assert!(result.is_err(), "should reject missing --vector-dir");
    }

    // --------------------------------------------------------------
    // Subcommand behaviour against a real backend
    // --------------------------------------------------------------

    #[tokio::test]
    async fn list_returns_empty_message_on_clean_vault() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        // Should not error.
        list_dead_letters(&backend, 100).await.unwrap();
    }

    #[tokio::test]
    async fn list_shows_planted_row() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": [], "boundary": "work"});
        let _id = plant_dead_letter(
            &backend,
            mem,
            CascadeOperation::Write,
            "simulated lance io",
            payload,
        )
        .await;
        // Just verify the call doesn't error — output goes to stdout.
        list_dead_letters(&backend, 100).await.unwrap();
    }

    #[tokio::test]
    async fn inspect_returns_not_found_for_unknown_id() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let unknown = Uuid::now_v7().to_string();
        let err = inspect_dead_letter(&backend, &unknown).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn inspect_returns_invalid_uuid() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let err = inspect_dead_letter(&backend, "not-a-uuid")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid uuid"));
    }

    #[tokio::test]
    async fn acknowledge_marks_row_acknowledged() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": [], "boundary": "work"});
        let id = plant_dead_letter(&backend, mem, CascadeOperation::Write, "stuck", payload).await;

        acknowledge_dead_letter(&backend, &id.to_string(), "operator accepts loss")
            .await
            .unwrap();

        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::Acknowledged));
    }

    #[tokio::test]
    async fn acknowledge_rejects_empty_reason() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": [], "boundary": "work"});
        let id = plant_dead_letter(&backend, mem, CascadeOperation::Write, "stuck", payload).await;
        let err = acknowledge_dead_letter(&backend, &id.to_string(), "   ")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("--reason must not be empty"));
    }

    #[tokio::test]
    async fn retry_succeeds_on_clean_lance_and_marks_retried_succeeded() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        // Plant a Write dead-letter with a real payload that the vector
        // store can ingest cleanly.
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": embedding(0.1), "boundary": "work"});
        let id = plant_dead_letter(
            &backend,
            mem,
            CascadeOperation::Write,
            "transient io",
            payload,
        )
        .await;

        retry_dead_letter(&backend, &id.to_string()).await.unwrap();
        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::RetriedSucceeded));

        // Vector store now has the embedding.
        let b = Boundary::new("work").unwrap();
        let hits = backend
            .vector_store()
            .search(&embedding(0.1), 10, &[b])
            .await
            .unwrap();
        assert!(hits.iter().any(|(hid, _)| hid == &mem));
    }

    #[tokio::test]
    async fn retry_dimension_mismatch_marks_retried_failed() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        // Plant a Write dead-letter with the wrong embedding dimension —
        // retrying will fail the vector_store.upsert.
        let mem = MemoryId::new();
        let payload = serde_json::json!({
            "embedding": vec![0.1f32, 0.2, 0.3],  // dim 3, not DIM=4
            "boundary": "work",
        });
        let id =
            plant_dead_letter(&backend, mem, CascadeOperation::Write, "wrong dim", payload).await;

        let err = retry_dead_letter(&backend, &id.to_string())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("retry failed"));
        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::RetriedFailed));
    }

    #[tokio::test]
    async fn retry_on_already_resolved_row_is_noop() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        let mem = MemoryId::new();
        let payload = serde_json::json!({"embedding": embedding(0.1), "boundary": "work"});
        let id = plant_dead_letter(&backend, mem, CascadeOperation::Write, "stuck", payload).await;
        backend
            .dead_letter()
            .resolve(id, Resolution::Acknowledged)
            .await
            .unwrap();

        // Now a "retry" call should print a no-op message and NOT error.
        retry_dead_letter(&backend, &id.to_string()).await.unwrap();
        // Resolution unchanged.
        let entry = backend.dead_letter().get(id).await.unwrap().unwrap();
        assert_eq!(entry.resolution, Some(Resolution::Acknowledged));
    }

    // --------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------

    #[test]
    fn summarize_truncates_long_lines() {
        let long = "a".repeat(200);
        let s = summarize(&long, 60);
        assert_eq!(s.chars().count(), 60);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn summarize_keeps_short_lines() {
        let s = summarize("short", 60);
        assert_eq!(s, "short");
    }

    #[test]
    fn summarize_takes_first_line_only() {
        let multi = "first line\nsecond line";
        assert_eq!(summarize(multi, 60), "first line");
    }

    // --------------------------------------------------------------
    // divergence-check end-to-end
    // --------------------------------------------------------------

    #[tokio::test]
    async fn divergence_check_on_clean_vault_succeeds() {
        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;
        // No memories → no findings → Ok return.
        run_divergence_check(&backend).await.unwrap();
    }

    #[tokio::test]
    async fn divergence_check_returns_err_when_findings_present() {
        use vault_storage::StepResult;
        use vault_storage::{FixedJitter, RetryWorker};

        let tmp = TempDir::new().unwrap();
        let backend = make_backend(tmp.path()).await;

        // Plant a memory + drain the cascade so SQLite + LanceDB are in sync.
        let m = vault_core::Memory::try_new(vault_core::NewMemory {
            content: "doomed".into(),
            memory_type: vault_core::MemoryType::Semantic,
            boundary: Boundary::new("work").unwrap(),
            source_agent: Some("test".into()),
            confidence: 0.9,
            valid_from: None,
            valid_until: None,
            metadata: serde_json::json!({}),
        })
        .unwrap();
        backend.write_memory(&m, &embedding(0.1)).await.unwrap();

        let mut w = RetryWorker::with_jitter(backend.clone(), Box::new(FixedJitter(0.0)));
        let far_future = chrono::Utc::now() + chrono::Duration::seconds(60 * 60);
        loop {
            let r = w.step_at(far_future).await.unwrap();
            if r == StepResult::Idle {
                break;
            }
        }

        // Silently drop the vector row → divergence finding.
        backend.vector_store().delete(&m.id).await.unwrap();

        // run_divergence_check should return Err so scripts notice.
        let err = run_divergence_check(&backend).await.unwrap_err();
        assert!(
            err.to_string().contains("divergence findings present"),
            "expected findings error, got: {err}"
        );
    }
}
