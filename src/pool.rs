use anyhow::{Context, Result};
use dashmap::DashMap;
use duckdb::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Per-project DuckDB connection pool.
/// Each project gets its own DuckLake catalog + Parquet directory for data isolation.
/// The DuckDB connection is in-memory (just a coordinator) — all data lives in
/// the `.ducklake` catalog file + Parquet files under the project directory.
pub struct ProjectPool {
    pub connections: DashMap<String, PoolEntry>,
    data_dir: PathBuf,
    max_connections: usize,
}

pub struct PoolEntry {
    conn: Arc<Mutex<Connection>>,
    last_access: Instant,
}

impl ProjectPool {
    pub fn new(data_dir: PathBuf, max_connections: usize) -> Self {
        Self {
            connections: DashMap::new(),
            data_dir,
            max_connections,
        }
    }

    /// Always returns "lake." — there is no longer a non-DuckLake path.
    pub fn prefix() -> &'static str {
        crate::storage::table_prefix()
    }

    /// Get or create a DuckDB connection for a project.
    /// Creates the DuckLake catalog and tables on first access.
    pub fn get_or_create(&self, project_id: &str) -> Result<Arc<Mutex<Connection>>> {
        // Fast path: connection exists
        if let Some(mut entry) = self.connections.get_mut(project_id) {
            entry.last_access = Instant::now();
            return Ok(Arc::clone(&entry.conn));
        }

        // Evict if at capacity before creating new
        if self.connections.len() >= self.max_connections {
            self.evict_oldest();
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

        let shared = Arc::new(Mutex::new(conn));
        self.connections.insert(
            project_id.to_string(),
            PoolEntry {
                conn: Arc::clone(&shared),
                last_access: Instant::now(),
            },
        );

        tracing::info!(project_id, "opened DuckDB connection (DuckLake)");
        Ok(shared)
    }

    /// Evict connections idle longer than the given duration.
    pub fn evict_idle(&self, max_idle: std::time::Duration) {
        let now = Instant::now();
        let before = self.connections.len();
        self.connections
            .retain(|_, entry| now.duration_since(entry.last_access) < max_idle);
        let evicted = before - self.connections.len();
        if evicted > 0 {
            tracing::info!(evicted, "evicted idle DuckDB connections");
        }
    }

    /// Evict the oldest connection to make room for a new one.
    fn evict_oldest(&self) {
        let oldest = self
            .connections
            .iter()
            .min_by_key(|entry| entry.last_access)
            .map(|entry| entry.key().clone());
        if let Some(key) = oldest {
            self.connections.remove(&key);
            tracing::info!(project_id = %key, "evicted oldest DuckDB connection");
        }
    }

    /// Number of open connections.
    pub fn len(&self) -> usize {
        self.connections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.connections.is_empty()
    }

    /// Remove a project's connection (for account deletion).
    pub fn remove(&self, project_id: &str) {
        self.connections.remove(project_id);
    }
}
