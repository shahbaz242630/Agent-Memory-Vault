//! Schema migration runner.
//!
//! Migrations are numbered SQL files embedded into the binary at compile time
//! via [`include_str!`]. The runner is *idempotent*: applying twice on an
//! already-up-to-date database is a no-op. Each migration runs inside a
//! transaction and is recorded in `schema_migrations` only on success.
//!
//! Why include the SQL? Predictable behaviour across machines — no surprise
//! when the working directory is different at runtime, no risk of a partial
//! distribution missing a migration file.

use rusqlite::Connection;
use tracing::{debug, info};

use vault_core::{VaultError, VaultResult};

/// One ordered schema migration.
struct Migration {
    /// Monotonic version. Migrations apply in ascending order; gaps not allowed.
    version: i64,
    /// Short human-readable description, recorded for posterity.
    description: &'static str,
    /// SQL to apply. Must be idempotent at the DDL level (`CREATE TABLE IF NOT EXISTS`).
    up: &'static str,
}

/// All migrations, in order. Append new ones — never edit existing ones.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        description: "Initial schema: memories, audit_log",
        up: include_str!("0001_initial.sql"),
    },
    Migration {
        version: 2,
        description: "T0.1.6 cascade infra: retry_queue, dead_letter, pending_sync",
        up: include_str!("0002_cascade_infra.sql"),
    },
];

/// Apply any pending migrations to the open connection. Uses the
/// hard-coded production [`MIGRATIONS`] slice.
///
/// # Errors
///
/// Returns [`VaultError::Storage`] if any migration fails. The transaction
/// for the failing migration is rolled back; previously-applied migrations
/// remain.
pub(crate) fn run(conn: &mut Connection) -> VaultResult<()> {
    run_with_migrations(conn, MIGRATIONS)
}

/// Like [`run`] but accepts a custom migration slice. Crate-private so
/// only tests can substitute it; production callers go through `run`.
fn run_with_migrations(conn: &mut Connection, migrations: &[Migration]) -> VaultResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     INTEGER PRIMARY KEY,
            applied_at  TEXT    NOT NULL,
            description TEXT    NOT NULL
        );",
    )
    .map_err(|e| VaultError::Storage(format!("failed to create schema_migrations: {e}")))?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| VaultError::Storage(format!("failed to read current schema version: {e}")))?;

    debug!(current_version, "checking schema migrations");

    let pending: Vec<&Migration> = migrations
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        debug!("schema is up to date");
        return Ok(());
    }

    // Migrations must be contiguous — refuse to run if there are gaps,
    // because that almost certainly means migrations were edited or dropped
    // (which can produce silent data corruption).
    for (expected, m) in (current_version + 1..).zip(&pending) {
        if m.version != expected {
            return Err(VaultError::Storage(format!(
                "migration version gap: expected {expected}, found {}",
                m.version,
            )));
        }
    }

    for m in pending {
        info!(version = m.version, description = %m.description, "applying migration");

        let tx = conn
            .transaction()
            .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
        tx.execute_batch(m.up)
            .map_err(|e| VaultError::Storage(format!("migration {}: {e}", m.version)))?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at, description) VALUES (?1, ?2, ?3)",
            rusqlite::params![m.version, chrono::Utc::now().to_rfc3339(), m.description,],
        )
        .map_err(|e| VaultError::Storage(format!("record migration {}: {e}", m.version)))?;
        tx.commit()
            .map_err(|e| VaultError::Storage(format!("commit migration {}: {e}", m.version)))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_memory() -> Connection {
        Connection::open_in_memory().unwrap()
    }

    #[test]
    fn fresh_db_applies_all_migrations() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i64);
    }

    #[test]
    fn running_twice_is_idempotent() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();
        // Second invocation must succeed and apply nothing new.
        run(&mut conn).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, MIGRATIONS.len() as i64);
    }

    #[test]
    fn migrations_create_expected_tables() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        for table in [
            "memories",
            "audit_log",
            "schema_migrations",
            "retry_queue",
            "dead_letter",
            "pending_sync",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected table {table} to exist");
        }
    }

    #[test]
    fn migration_0002_creates_indexes_for_retry_queue_and_dead_letter() {
        // Performance-critical indexes for the cascade orchestrator's
        // hot paths (worker polling, dead-letter list query).
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        for index in [
            "idx_retry_queue_mem_seq",
            "idx_retry_queue_next_attempt",
            "idx_dead_letter_unresolved",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
                    [index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected index {index} to exist");
        }
    }

    #[test]
    fn migration_0002_retry_queue_unique_per_memory_sequence() {
        // The (memory_id, sequence_id) UNIQUE constraint is the FIFO-per-memory
        // anchor (cascade-ordering invariant). Verify the constraint is enforced
        // at the SQL layer.
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        let memory_id = vec![1u8; 16];
        let entry_a = vec![10u8; 16];
        let entry_b = vec![11u8; 16];
        let now = chrono::Utc::now().to_rfc3339();
        let payload = vec![0u8];

        conn.execute(
            "INSERT INTO retry_queue (id, memory_id, operation, payload_format_version, \
             payload, sequence_id, next_attempt_at, created_at) \
             VALUES (?1, ?2, 'write', 1, ?3, 5, ?4, ?4)",
            rusqlite::params![entry_a, memory_id, payload, now],
        )
        .unwrap();

        // Same (memory_id, sequence_id) = collision — must fail.
        let err = conn.execute(
            "INSERT INTO retry_queue (id, memory_id, operation, payload_format_version, \
             payload, sequence_id, next_attempt_at, created_at) \
             VALUES (?1, ?2, 'update', 1, ?3, 5, ?4, ?4)",
            rusqlite::params![entry_b, memory_id, payload, now],
        );
        assert!(err.is_err(), "expected UNIQUE constraint violation");

        // Different sequence_id for the same memory_id — must succeed.
        conn.execute(
            "INSERT INTO retry_queue (id, memory_id, operation, payload_format_version, \
             payload, sequence_id, next_attempt_at, created_at) \
             VALUES (?1, ?2, 'update', 1, ?3, 6, ?4, ?4)",
            rusqlite::params![entry_b, memory_id, payload, now],
        )
        .unwrap();
    }

    #[test]
    fn run_records_each_migration_in_schema_migrations_table() {
        let mut conn = open_memory();
        run(&mut conn).unwrap();

        // schema_migrations has one row per applied migration, with the
        // exact version + description we declared in MIGRATIONS.
        for expected in MIGRATIONS {
            let row: (i64, String) = conn
                .query_row(
                    "SELECT version, description FROM schema_migrations WHERE version = ?1",
                    rusqlite::params![expected.version],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(row.0, expected.version);
            assert_eq!(row.1, expected.description);
        }
    }

    #[test]
    fn forward_migration_applies_next_version_only() {
        // Test the principle: an already-applied migration is never replayed,
        // and a new migration appended after it gets applied exactly once.
        // Uses a synthetic migration list throughout (not the production
        // MIGRATIONS slice) so the test is robust against future additions.
        let mut conn = open_memory();

        let initial: &[Migration] = &[Migration {
            version: 1,
            description: "test v1",
            up: "CREATE TABLE m1 (id INTEGER PRIMARY KEY);",
        }];
        run_with_migrations(&mut conn, initial).unwrap();

        let after_v1: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after_v1, 1);

        let extended: &[Migration] = &[
            // v1 already applied — runner should NOT replay this
            Migration {
                version: 1,
                description: "test v1",
                up: "/* no-op — already applied */",
            },
            Migration {
                version: 2,
                description: "Test forward migration: add a marker table",
                up: "CREATE TABLE migration_marker (id INTEGER PRIMARY KEY);",
            },
        ];
        run_with_migrations(&mut conn, extended).unwrap();

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(total, 2, "expected v2 to be recorded");

        // Marker table created.
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='migration_marker'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1);
    }

    #[test]
    fn version_gap_in_migration_list_is_rejected() {
        // Defensive check: if someone deletes migration v2 by mistake,
        // leaving v1 then v3, the runner must refuse rather than silently
        // skip v2's effects.
        let mut conn = open_memory();
        let gapped: &[Migration] = &[
            Migration {
                version: 1,
                description: "v1",
                up: "CREATE TABLE m1 (id INTEGER PRIMARY KEY);",
            },
            Migration {
                version: 3,
                description: "v3 (gap! v2 missing)",
                up: "CREATE TABLE m3 (id INTEGER PRIMARY KEY);",
            },
        ];
        let err = run_with_migrations(&mut conn, gapped).unwrap_err();
        assert!(
            matches!(&err, VaultError::Storage(s) if s.contains("version gap")),
            "expected gap detection, got {err:?}",
        );

        // Important: v1 also did NOT apply because gap detection happens
        // before any migration runs. (If gap detection happened after, v1
        // would be partially applied — that's worse.)
        let m1_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(m1_exists, 0, "no migrations should have applied");
    }
}
