//! Schema migration runner for the DuckDB graph store.
//!
//! Mirrors [`crate::migrations`] (the SQLite/SQLCipher runner) in shape, but
//! against a separate `duckdb::Connection` and a separate migration list.
//! The two crates' connection types and `params!` macros are similar enough
//! that the runners read alike but they cannot be unified without a trait
//! layer that buys little for V0.1.
//!
//! See `crate::migrations` for the design rationale (idempotent, gap
//! detection, embedded SQL via `include_str!`).

use duckdb::{params, Connection};
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

/// All graph-store migrations, in order. Append new ones — never edit existing ones.
const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    description: "Initial schema: entities, relationships",
    up: include_str!("0001_initial.sql"),
}];

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
            version     BIGINT  PRIMARY KEY,
            applied_at  TEXT    NOT NULL,
            description TEXT    NOT NULL
        );",
    )
    .map_err(|e| VaultError::Storage(format!("failed to create graph schema_migrations: {e}")))?;

    let current_version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| VaultError::Storage(format!("failed to read graph schema version: {e}")))?;

    debug!(current_version, "checking graph-store schema migrations");

    let pending: Vec<&Migration> = migrations
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        debug!("graph-store schema is up to date");
        return Ok(());
    }

    // Migrations must be contiguous — refuse to run if there are gaps.
    for (expected, m) in (current_version + 1..).zip(&pending) {
        if m.version != expected {
            return Err(VaultError::Storage(format!(
                "graph migration version gap: expected {expected}, found {}",
                m.version,
            )));
        }
    }

    for m in pending {
        info!(version = m.version, description = %m.description, "applying graph migration");

        let tx = conn
            .transaction()
            .map_err(|e| VaultError::Storage(format!("begin tx: {e}")))?;
        tx.execute_batch(m.up)
            .map_err(|e| VaultError::Storage(format!("graph migration {}: {e}", m.version)))?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at, description) VALUES (?, ?, ?)",
            params![m.version, chrono::Utc::now().to_rfc3339(), m.description],
        )
        .map_err(|e| VaultError::Storage(format!("record graph migration {}: {e}", m.version)))?;
        tx.commit().map_err(|e| {
            VaultError::Storage(format!("commit graph migration {}: {e}", m.version))
        })?;
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

        for table in ["entities", "relationships", "schema_migrations"] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM information_schema.tables \
                     WHERE table_schema = 'main' AND table_name = ?",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "expected table {table} to exist");
        }
    }

    #[test]
    fn version_gap_in_migration_list_is_rejected() {
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
        // before any migration runs. See the equivalent SQLite test.
        let m1_exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM information_schema.tables \
                 WHERE table_schema = 'main' AND table_name = 'm1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(m1_exists, 0, "no migrations should have applied");
    }
}
