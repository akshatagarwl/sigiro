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
    /// Dedicated write connection — used by the flusher (ingester).
    /// All schema CREATE TABLE calls also go through here (idempotent).
    write_conn: Arc<Mutex<Connection>>,
    /// Dedicated read connection — used by query handlers (HTTP API).
    /// Shares the same DuckLake catalog + Parquet files as write_conn.
    /// Opens its own DuckDB coordinator so reads never contend with writes.
    read_conn: Arc<Mutex<Connection>>,
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

    /// Get or create the dedicated write connection for a project.
    /// Used by the flusher (ingester). Also creates the read connection.
    /// Creates the DuckLake catalog and tables on first access.
    pub fn get_or_create(&self, project_id: &str) -> Result<Arc<Mutex<Connection>>> {
        // Fast path: write connection exists
        if let Some(ref mut entry) = self.connections.get_mut(project_id) {
            entry.last_access = Instant::now();
            return Ok(Arc::clone(&entry.write_conn));
        }

        // Evict if at capacity before creating new
        if self.connections.len() >= self.max_connections {
            self.evict_oldest();
        }

        let project_dir = self.data_dir.join(project_id);

        let (write_conn, read_conn) = self.open_conn_pair(&project_dir).with_context(|| {
            format!("failed to open DuckDB connections for project {project_id}")
        })?;

        let write_shared = Arc::new(Mutex::new(write_conn));
        let read_shared = Arc::new(Mutex::new(read_conn));

        self.connections.insert(
            project_id.to_string(),
            PoolEntry {
                write_conn: Arc::clone(&write_shared),
                read_conn: Arc::clone(&read_shared),
                last_access: Instant::now(),
            },
        );

        tracing::info!(project_id, "opened DuckDB connection pair (DuckLake)");
        Ok(write_shared)
    }

    /// Get the dedicated read connection for a project.
    /// Used by query handlers (HTTP API). Never contended by writes.
    /// Returns error if the project connection hasn't been created yet —
    /// callers should call get_or_create first to initialize.
    pub fn get_read(&self, project_id: &str) -> Result<Arc<Mutex<Connection>>> {
        if let Some(ref mut entry) = self.connections.get_mut(project_id) {
            entry.last_access = Instant::now();
            Ok(Arc::clone(&entry.read_conn))
        } else {
            Err(anyhow::anyhow!(
                "read connection requested but project {project_id} not initialized — \
                 call get_or_create() first"
            ))
        }
    }

    /// Open a pair of DuckDB connections that share the same DuckLake catalog + Parquet files.
    /// write_conn initializes DuckLake schema; read_conn opens its own coordinator pointing
    /// to the same catalog. Both connections read/write Parquet files through the shared
    /// catalog.sqlite — no contention since DuckDB coordinators don't share in-memory state.
    fn open_conn_pair(&self, project_dir: &std::path::Path) -> Result<(Connection, Connection)> {
        // Write connection — owns schema init
        let write_conn = self
            .open_connection(project_dir)
            .context("failed to open write connection")?;

        // Read connection — separate coordinator, same catalog
        let read_conn = self
            .open_connection(project_dir)
            .context("failed to open read connection")?;

        Ok((write_conn, read_conn))
    }

    fn open_connection(&self, project_dir: &std::path::Path) -> Result<Connection> {
        let conn = Connection::open_in_memory()
            .with_context(|| format!("failed to open in-memory DuckDB"))?;

        // Persistent local extension cache
        let ext_dir = self.data_dir.join("extensions");
        std::fs::create_dir_all(&ext_dir).context("failed to create extensions directory")?;
        if let Some(p) = ext_dir.to_str() {
            conn.execute_batch(&format!("SET extension_directory = '{p}';"))
                .context("failed to set DuckDB extension_directory")?;
        }

        // Cap memory per connection; busy_timeout prevents indefinite reader blocks.
        conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1; SET busy_timeout = 5000;")
            .context("failed to set DuckDB memory limit")?;

        // Initialize DuckLake
        if let Some(config) = crate::storage::load_config()? {
            crate::storage::init_distributed(&conn, &config, project_dir)?;
        } else {
            crate::storage::init_default(&conn, project_dir)?;
        }

        crate::schema::create_tables(&conn, Self::prefix())?;
        crate::schema::load_fleet_extensions(&conn)?;

        Ok(conn)
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
