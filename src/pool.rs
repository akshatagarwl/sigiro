use anyhow::{Context, Result};
use duckdb::Connection;
use moka::sync::Cache;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Per-project DuckDB connection pool.
/// Each project gets its own DuckLake catalog + Parquet directory for data isolation.
/// The DuckDB connection is in-memory (just a coordinator) — all data lives in
/// the `.ducklake` catalog file + Parquet files under the project directory.
pub struct ProjectPool {
    connections: Cache<String, Arc<Mutex<Connection>>>,
    data_dir: PathBuf,
}

impl ProjectPool {
    pub fn new(data_dir: PathBuf, max_connections: usize) -> Self {
        Self::new_with_idle(data_dir, max_connections, Duration::from_secs(300))
    }

    pub fn new_with_idle(
        data_dir: PathBuf,
        max_connections: usize,
        idle_timeout: Duration,
    ) -> Self {
        Self {
            connections: Cache::builder()
                .max_capacity(max_connections as u64)
                .time_to_idle(idle_timeout)
                .build(),
            data_dir,
        }
    }

    /// Always returns "lake." — there is no longer a non-DuckLake path.
    pub fn prefix() -> &'static str {
        crate::storage::table_prefix()
    }

    /// Get or create a DuckDB connection for a project.
    /// Creates the DuckLake catalog and tables on first access.
    pub fn get_or_create(&self, project_id: &str) -> Result<Arc<Mutex<Connection>>> {
        // Fast path: connection exists — moka handles touch on access
        if let Some(conn) = self.connections.get(project_id) {
            return Ok(conn);
        }

        // Slow path: in-memory coordinator, ATTACH per-project DuckLake catalog.
        // Each project gets: {data_dir}/{project_id}/catalog.ducklake
        let project_dir = self.data_dir.join(project_id);

        let conn = Connection::open_in_memory()
            .with_context(|| format!("failed to open in-memory DuckDB for project {project_id}"))?;

        // Cap memory per connection to avoid one project eating all RAM
        conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
            .context("failed to set DuckDB memory limit")?;

        // Initialize DuckLake — always uses SQLite catalog + local Parquet for now.
        crate::storage::init_default(&conn, &project_dir)?;

        crate::schema::create_tables(&conn, Self::prefix())?;

        let loaded = crate::schema::load_fleet_extensions(&conn);
        if !loaded.is_empty() {
            tracing::info!(project_id, extensions = ?loaded, "loaded fleet extensions");
        }

        let shared = Arc::new(Mutex::new(conn));
        self.connections
            .insert(project_id.to_string(), Arc::clone(&shared));

        tracing::info!(project_id, "opened DuckDB connection (DuckLake)");
        Ok(shared)
    }

    /// Evict connections idle longer than the given duration.
    /// moka handles TTL-based eviction automatically; this forces a maintenance cycle.
    pub fn evict_idle(&self, _max_idle: Duration) {
        // moka's time_to_idle handles expiry automatically.
        // run_pending_tasks() forces the background maintenance to process
        // expired entries immediately (useful for testing and explicit cleanup).
        self.connections.run_pending_tasks();
    }

    /// Number of open connections.
    pub fn len(&self) -> usize {
        self.connections.run_pending_tasks();
        self.connections.entry_count() as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Remove a project's connection (for account deletion).
    pub fn remove(&self, project_id: &str) {
        self.connections.invalidate(project_id);
    }

    /// Check if a project connection exists (for tests).
    pub fn contains(&self, project_id: &str) -> bool {
        self.connections.contains_key(project_id)
    }
}
