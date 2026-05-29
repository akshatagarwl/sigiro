use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use dashmap::DashMap;
use duckdb::Connection;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Instant;

/// Per-project DuckDB connection pool.
/// Each project gets its own DuckLake catalog + Parquet directory for data isolation.
/// The DuckDB connection is in-memory (just a coordinator) — all data lives in
/// the `.ducklake` catalog file + Parquet files under the project directory.
pub struct ProjectPool {
    pub connections: DashMap<String, PoolEntry>,
    data_dir: Utf8PathBuf,
    max_connections: usize,
}

pub struct PoolEntry {
    /// Single DuckDB connection shared by both flusher (writes) and HTTP API (reads).
    /// Wrapped in Arc<Mutex> so reads and writes are serialized — DuckDB does not
    /// support concurrent access on the same connection anyway.
    conn: Arc<Mutex<Connection>>,
    last_access: Instant,
}

impl PoolEntry {
    /// Checkpoint DuckLake before dropping so data is persisted to Parquet.
    fn shutdown(&self) {
        let guard = self.conn.lock();
        if let Err(e) = guard.execute_batch("CHECKPOINT lake;") {
            tracing::warn!("DuckDB CHECKPOINT failed during shutdown: {e}");
        }
    }
}

impl ProjectPool {
    pub fn new(data_dir: Utf8PathBuf, max_connections: usize) -> Self {
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

    /// Create a connection for a project (idempotent — setup operation).
    ///
    /// Used for project initialization. If two callers race, the loser re-enters
    /// and finds the winner's connection (no duplicate created).
    ///
    /// Thread-safe: uses DashMap entry API to prevent concurrent callers from
    /// creating duplicate connections for the same project.
    #[tracing::instrument(skip(self), fields(project_id = %project_id))]
    pub fn create_project(&self, project_id: &str) -> Result<Arc<Mutex<Connection>>> {
        use dashmap::mapref::entry::Entry;

        match self.connections.entry(project_id.to_string()) {
            Entry::Occupied(ref mut entry) => {
                entry.get_mut().last_access = Instant::now();
                Ok(Arc::clone(&entry.get().conn))
            }
            Entry::Vacant(entry) => {
                // Evict if at capacity before creating new.
                // Entry guard is dropped first to avoid holding two entries.
                drop(entry);

                if self.connections.len() >= self.max_connections {
                    self.evict_oldest();
                }

                let project_dir = self.data_dir.join(project_id);
                let conn = self.open_connection(&project_dir).with_context(|| {
                    format!("failed to open DuckDB connection for project {project_id}")
                })?;

                let shared = Arc::new(Mutex::new(conn));

                // Re-enter via entry API — another task may have inserted while we
                // were in open_connection (which downloads extensions on cold start).
                match self.connections.entry(project_id.to_string()) {
                    Entry::Occupied(ref mut entry) => {
                        // Another task won the race — use their connection, discard ours.
                        // Our connection's Drop will clean up.
                        entry.get_mut().last_access = Instant::now();
                        tracing::info!(project_id, "reused DuckDB connection (race loser)");
                        Ok(Arc::clone(&entry.get().conn))
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(PoolEntry {
                            conn: Arc::clone(&shared),
                            last_access: Instant::now(),
                        });
                        tracing::info!(project_id, "opened DuckDB connection (DuckLake)");
                        Ok(shared)
                    }
                }
            }
        }
    }

    /// Get the connection for a project (strict — no auto-create).
    ///
    /// Returns only an existing connection. Fails if the project was not created.
    /// Use `create_project()` first to initialize a project.
    #[tracing::instrument(skip(self), fields(project_id = %project_id))]
    pub fn get_project(&self, project_id: &str) -> Result<Arc<Mutex<Connection>>> {
        self.connections
            .get(project_id)
            .map(|entry| Arc::clone(&entry.conn))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "connection not found for project {project_id} — \
                     call create_project() first"
                )
            })
    }

    /// Refresh last_access for an existing project entry so it is not evicted as idle.
    /// Callers (e.g. long-running per-project flusher tasks) should call this periodically
    /// to prevent the idle evictor from removing their connection while they are alive.
    #[tracing::instrument(skip(self), fields(project_id = %project_id))]
    pub fn touch_project(&self, project_id: &str) {
        if let Some(mut entry) = self.connections.get_mut(project_id) {
            entry.last_access = Instant::now();
        }
    }

    #[tracing::instrument(skip(self, project_dir), fields(project_dir = %project_dir))]
    fn open_connection(&self, project_dir: &camino::Utf8Path) -> Result<Connection> {
        let conn = Connection::open_in_memory()
            .with_context(|| "failed to open in-memory DuckDB".to_string())?;

        // Persistent local extension cache — share across tests via env var
        let ext_dir = crate::config::EXTENSION_DIR
            .value()
            .ok()
            .filter(|s| !s.is_empty())
            .map(Utf8PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("extensions"));
        conn.execute_batch(&format!(
            "SET extension_directory = '{}';",
            ext_dir.as_str()
        ))
        .context("failed to set DuckDB extension_directory")?;

        // Cap memory per connection.
        conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
            .context("failed to set DuckDB memory limit")?;

        // Extensions are downloaded on first use (cold start). This may take 20-30s.
        // Run `sigiro fetch-extensions` to pre-download for offline deployments.
        tracing::info!("DuckDB: loading extensions (downloading on first use, may take a moment)...");

        // Initialize DuckLake
        if let Some(config) = crate::storage::load_config()? {
            crate::storage::init_distributed(&conn, &config, project_dir)?;
        } else {
            crate::storage::init_default(&conn, project_dir)?;
        }

        crate::schema::create_tables(&conn, Self::prefix())?;
        crate::schema::load_fleet_extensions(&conn)?;

        // Ensure DuckLake tables are discoverable without schema prefix.
        conn.execute_batch("USE lake;")
            .context("failed to set DuckDB default schema")?;

        Ok(conn)
    }

    /// Evict connections idle longer than the given duration.
    /// Checkpoints DuckLake before dropping so data is persisted to Parquet.
    #[tracing::instrument(skip(self))]
    pub fn evict_idle(&self, max_idle: std::time::Duration) {
        let now = Instant::now();
        let before = self.connections.len();
        self.connections.retain(|_, entry| {
            if now.duration_since(entry.last_access) >= max_idle {
                entry.shutdown();
                false
            } else {
                true
            }
        });
        let evicted = before - self.connections.len();
        if evicted > 0 {
            tracing::info!(evicted, "evicted idle DuckDB connections");
        }
    }

    /// Evict the oldest connection to make room for a new one.
    /// Checkpoints DuckLake before dropping so data is persisted to Parquet.
    #[tracing::instrument(skip(self))]
    fn evict_oldest(&self) {
        let oldest = self
            .connections
            .iter()
            .min_by_key(|entry| entry.last_access)
            .map(|entry| entry.key().clone());
        if let Some(key) = oldest {
            if let Some((_, entry)) = self.connections.remove(&key) {
                entry.shutdown();
            }
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
    #[tracing::instrument(skip(self), fields(project_id = %project_id))]
    pub fn remove(&self, project_id: &str) {
        self.connections.remove(project_id);
    }

    /// Create a dedicated DuckDB connection for federation (quack server).
    /// This connection is separate from project pool connections because quack server
    /// runs in background threads that must remain alive for the lifetime of the server.
    pub fn create_federation_connection(&self) -> Result<Arc<Mutex<Connection>>> {
        let conn = Connection::open_in_memory()
            .context("failed to open in-memory DuckDB for federation")?;

        let ext_dir = crate::config::EXTENSION_DIR
            .value()
            .ok()
            .filter(|s| !s.is_empty())
            .map(Utf8PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("extensions"));

        conn.execute_batch(&format!(
            "SET extension_directory = '{}';",
            ext_dir.as_str()
        ))
        .context("failed to set extension_directory")?;

        conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
            .context("failed to set DuckDB memory limit")?;

        // Attach DuckLake catalog so the quack server serves actual project data.
        // Collectors need their local DuckLake catalog attached for remote queries.
        let project_dir = self.data_dir.join("dev_project");
        let ducklake_attached = if let Ok(Some(config)) = crate::storage::load_config() {
            crate::storage::init_distributed(&conn, &config, &project_dir)
                .context("failed to init DuckLake for federation connection")?;
            true
        } else {
            crate::storage::init_default(&conn, &project_dir)
                .context("failed to init local DuckLake for federation connection")?;
            true
        };

        // Only create the lake schema manually when DuckLake is not attached.
        // DuckLake attaches its own "lake" catalog; creating a schema with the
        // same name causes "Ambiguous reference to catalog or schema" errors.
        if !ducklake_attached {
            conn.execute_batch("CREATE SCHEMA IF NOT EXISTS lake;")
                .context("failed to create lake schema for federation")?;
        }

        crate::schema::create_tables(&conn, Self::prefix())?;

        Ok(Arc::new(Mutex::new(conn)))
    }

    /// Same as `create_federation_connection` but returns an owned `Connection`
    /// (not wrapped in `Arc<Mutex>`) so it can be moved to a background thread.
    pub fn create_raw_federation_connection(&self) -> Result<Connection> {
        let conn = Connection::open_in_memory()
            .context("failed to open in-memory DuckDB for federation")?;

        let ext_dir = crate::config::EXTENSION_DIR
            .value()
            .ok()
            .filter(|s| !s.is_empty())
            .map(Utf8PathBuf::from)
            .unwrap_or_else(|| self.data_dir.join("extensions"));

        conn.execute_batch(&format!(
            "SET extension_directory = '{}';",
            ext_dir.as_str()
        ))
        .context("failed to set extension_directory")?;

        conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
            .context("failed to set DuckDB memory limit")?;

        // Attach DuckLake catalog so the quack server serves actual project data.
        // Collectors need their local DuckLake catalog attached for remote queries.
        let project_dir = self.data_dir.join("dev_project");
        let ducklake_attached = if let Ok(Some(config)) = crate::storage::load_config() {
            crate::storage::init_distributed(&conn, &config, &project_dir)
                .context("failed to init DuckLake for federation connection")?;
            true
        } else {
            crate::storage::init_default(&conn, &project_dir)
                .context("failed to init local DuckLake for federation connection")?;
            true
        };

        // Only create the lake schema manually when DuckLake is not attached.
        // DuckLake attaches its own "lake" catalog; creating a schema with the
        // same name causes "Ambiguous reference to catalog or schema" errors.
        if !ducklake_attached {
            conn.execute_batch("CREATE SCHEMA IF NOT EXISTS lake;")
                .context("failed to create lake schema for federation")?;
        }

        crate::schema::create_tables(&conn, Self::prefix())?;

        Ok(conn)
    }
}
