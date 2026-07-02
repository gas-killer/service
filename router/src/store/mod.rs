//! SQLite persistence layer for the router.
//!
//! [`SqliteStore`] owns the single durable store behind the ingress: API keys and task
//! state both live in this one database. The store opens a connection pool, applies its
//! embedded schema migrations on startup, and hands the pool to components that need it.
//!
//! Schema is defined by migration files under `router/migrations/`, embedded into the
//! binary at compile time so no migration files need to ship in the runtime image. Each
//! feature that needs a table adds its own migration; this module owns only connection
//! setup and the migration runner.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

/// Directory holding the router's persistent data when `DATA_DIR` is unset.
///
/// In production this is a mounted PVC (see the Helm chart); the path must be writable by
/// the container's user.
const DEFAULT_DATA_DIR: &str = "/data";

/// Filename of the SQLite database within the data directory.
const DATABASE_FILENAME: &str = "router.db";

/// How long a write waits for a competing writer's lock before erroring. SQLite serializes
/// writers, so a brief contention window is expected under concurrent ingress load.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Embedded schema migrations, applied in order on startup.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// A handle to the router's SQLite connection pool.
///
/// Cheap to clone — the inner pool is reference-counted — so it can be shared across the
/// ingress and orchestrator and every clone reads and writes the same database.
#[derive(Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Opens the database under `DATA_DIR` (default `/data`), creating it if absent, and
    /// applies all pending migrations.
    ///
    /// Returns an error if the data directory cannot be created, the database cannot be
    /// opened, or a migration fails. Callers must abort startup on error so the router
    /// never serves traffic against an unmigrated store.
    pub async fn connect() -> anyhow::Result<Self> {
        Self::connect_at(&resolve_db_path()).await
    }

    /// Opens the database at an explicit filesystem path, creating the parent directory and
    /// the database file if needed, then applies pending migrations.
    pub async fn connect_at(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating data directory {}", parent.display()))?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT);

        let pool = SqlitePoolOptions::new()
            .connect_with(options)
            .await
            .with_context(|| format!("opening sqlite database at {}", path.display()))?;

        Self::migrate(pool, &path.display().to_string()).await
    }

    /// Opens a transient in-memory database for tests.
    ///
    /// Each new connection to a bare in-memory database gets its own empty schema, so the
    /// pool is capped at a single connection to keep one migrated database alive for the
    /// store's lifetime.
    #[cfg(test)]
    pub async fn connect_in_memory() -> anyhow::Result<Self> {
        let options = "sqlite::memory:"
            .parse::<SqliteConnectOptions>()
            .context("building in-memory sqlite options")?
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .context("opening in-memory sqlite database")?;

        Self::migrate(pool, ":memory:").await
    }

    /// Applies pending migrations against `pool` and wraps it in a [`SqliteStore`].
    async fn migrate(pool: SqlitePool, path: &str) -> anyhow::Result<Self> {
        MIGRATOR
            .run(&pool)
            .await
            .context("applying database migrations")?;

        tracing::info!(
            path,
            migrations = MIGRATOR.iter().count(),
            "sqlite store ready"
        );

        Ok(Self { pool })
    }

    /// Borrows the connection pool for issuing queries.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Runs a trivial liveness query against the store, returning an error if the pool
    /// cannot serve it — e.g. the backing volume was detached, filled, or went read-only.
    /// Drives the `gas_killer_db_up` metric.
    pub async fn health_check(&self) -> anyhow::Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .context("sqlite health check")?;
        Ok(())
    }
}

/// Resolves the database path from the `DATA_DIR` environment variable.
fn resolve_db_path() -> PathBuf {
    db_path_from_dir(std::env::var("DATA_DIR").ok())
}

/// Builds the database path from an optional data directory, applying [`DEFAULT_DATA_DIR`]
/// when unset, as `<dir>/router.db`. Split from [`resolve_db_path`] so both the default and
/// override branches are testable without mutating process-wide environment state.
fn db_path_from_dir(dir: Option<String>) -> PathBuf {
    let dir = dir.unwrap_or_else(|| DEFAULT_DATA_DIR.to_string());
    Path::new(&dir).join(DATABASE_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn in_memory_store_migrates_and_serves_queries() {
        let store = SqliteStore::connect_in_memory()
            .await
            .expect("in-memory store should open and migrate");

        let one: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(store.pool())
            .await
            .expect("a trivial query should round-trip through the pool");
        assert_eq!(one, 1);

        // The migration runner always provisions its bookkeeping table, even with no
        // migrations of our own yet.
        let tracking_table: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM sqlite_master WHERE name = '_sqlx_migrations'",
        )
        .fetch_one(store.pool())
        .await
        .expect("migration bookkeeping table should exist");
        assert_eq!(tracking_table, 1);
    }

    #[tokio::test]
    async fn file_store_is_durable_and_idempotent_across_reopen() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("router.db");

        // First open creates and migrates the database.
        {
            let store = SqliteStore::connect_at(&path)
                .await
                .expect("first open should create and migrate");

            let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(store.pool())
                .await
                .expect("journal_mode pragma should be readable");
            assert_eq!(journal_mode.to_lowercase(), "wal");
        }

        assert!(
            path.exists(),
            "database file should persist after the pool is dropped"
        );

        // Reopening the same database re-runs migrations as a no-op rather than failing.
        let store = SqliteStore::connect_at(&path)
            .await
            .expect("reopening an existing database should succeed");
        let one: i64 = sqlx::query_scalar("SELECT 1")
            .fetch_one(store.pool())
            .await
            .expect("reopened store should serve queries");
        assert_eq!(one, 1);
    }

    #[tokio::test]
    async fn health_check_succeeds_on_open_store() {
        let store = SqliteStore::connect_in_memory()
            .await
            .expect("in-memory store should open");
        store
            .health_check()
            .await
            .expect("health check should pass against an open store");
    }

    #[test]
    fn db_path_defaults_to_data_dir_when_unset() {
        assert_eq!(db_path_from_dir(None), Path::new("/data/router.db"));
    }

    #[test]
    fn db_path_honors_data_dir_override() {
        assert_eq!(
            db_path_from_dir(Some("/app/data".to_string())),
            Path::new("/app/data/router.db")
        );
    }
}
