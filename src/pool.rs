use anyhow::{Context, Result};
use dashmap::DashMap;
use duckdb::Connection;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Per-project DuckDB connection pool.
/// Each project gets its own DuckDB file for data isolation.
/// Connections open lazily on first request and evict after idle timeout.
pub struct ProjectPool {
    connections: DashMap<String, PoolEntry>,
    data_dir: PathBuf,
    max_connections: usize,
    /// Table prefix ("lake." or "") — same for all projects
    prefix: &'static str,
}

struct PoolEntry {
    conn: Arc<Mutex<Connection>>,
    last_access: Instant,
}

impl ProjectPool {
    pub fn new(data_dir: PathBuf, prefix: &'static str, max_connections: usize) -> Self {
        Self {
            connections: DashMap::new(),
            data_dir,
            max_connections,
            prefix,
        }
    }

    pub fn prefix(&self) -> &'static str {
        self.prefix
    }

    /// Get or create a DuckDB connection for a project.
    /// Creates the DuckDB file and tables on first access.
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

        // Slow path: open DuckDB file, create tables
        let db_path = self.data_dir.join(format!("{project_id}.duckdb"));
        let conn = Connection::open(&db_path)
            .with_context(|| format!("failed to open DuckDB for project {project_id}"))?;

        // Cap memory per connection to avoid one project eating all RAM
        conn.execute_batch("SET memory_limit = '256MB'; SET threads = 1;")
            .context("failed to set DuckDB memory limit")?;

        crate::schema::create_tables(&conn, self.prefix)?;

        let shared = Arc::new(Mutex::new(conn));
        self.connections.insert(
            project_id.to_string(),
            PoolEntry {
                conn: Arc::clone(&shared),
                last_access: Instant::now(),
            },
        );

        tracing::info!(project_id, "opened DuckDB connection");
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pool(dir: &std::path::Path) -> ProjectPool {
        ProjectPool::new(dir.to_path_buf(), "", 10)
    }

    #[test]
    fn get_or_create_opens_connection() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path());

        let conn = pool.get_or_create("proj_a").unwrap();
        assert_eq!(pool.len(), 1);

        // Verify tables were created
        let guard = conn.lock().unwrap();
        let mut stmt = guard.prepare("SELECT count(*) FROM sigiro_spans").unwrap();
        let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn get_returns_same_connection() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path());

        let conn1 = pool.get_or_create("proj_a").unwrap();
        let conn2 = pool.get_or_create("proj_a").unwrap();
        assert!(Arc::ptr_eq(&conn1, &conn2));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn different_projects_different_connections() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path());

        let conn_a = pool.get_or_create("proj_a").unwrap();
        let conn_b = pool.get_or_create("proj_b").unwrap();
        assert!(!Arc::ptr_eq(&conn_a, &conn_b));
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn data_isolation_between_projects() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path());

        // Insert into project A using raw SQL with all required columns
        let conn_a = pool.get_or_create("proj_a").unwrap();
        {
            let guard = conn_a.lock().unwrap();
            guard
                .execute_batch(
                    "INSERT INTO sigiro_metrics_gauge (timestamp, metric_name, service_name, value) \
                     VALUES (current_timestamp, 'cpu', 'svc', 42.0)",
                )
                .unwrap();
        }

        // Project B should have no data
        let conn_b = pool.get_or_create("proj_b").unwrap();
        {
            let guard = conn_b.lock().unwrap();
            let mut stmt = guard
                .prepare("SELECT count(*) FROM sigiro_metrics_gauge")
                .unwrap();
            let count: i64 = stmt.query_row([], |row| row.get(0)).unwrap();
            assert_eq!(count, 0, "project B should have no data from project A");
        }
    }

    #[test]
    fn evict_idle_removes_old_connections() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path());

        pool.get_or_create("proj_a").unwrap();
        pool.get_or_create("proj_b").unwrap();
        assert_eq!(pool.len(), 2);

        // Evict with zero duration — everything is "idle"
        pool.evict_idle(std::time::Duration::ZERO);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn evict_oldest_at_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let pool = ProjectPool::new(dir.path().to_path_buf(), "", 2);

        pool.get_or_create("proj_a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        pool.get_or_create("proj_b").unwrap();
        assert_eq!(pool.len(), 2);

        // Opening a third should evict the oldest (proj_a)
        pool.get_or_create("proj_c").unwrap();
        assert_eq!(pool.len(), 2);
        assert!(pool.connections.contains_key("proj_b"));
        assert!(pool.connections.contains_key("proj_c"));
    }

    #[test]
    fn remove_project() {
        let dir = tempfile::tempdir().unwrap();
        let pool = test_pool(dir.path());

        pool.get_or_create("proj_a").unwrap();
        assert_eq!(pool.len(), 1);

        pool.remove("proj_a");
        assert_eq!(pool.len(), 0);
    }
}
