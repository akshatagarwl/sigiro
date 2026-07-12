use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use dashmap::DashMap;
use duckdb::Connection;
use parking_lot::{Condvar, Mutex};
use std::ops::Deref;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Per-project DuckDB connection pool.
/// Each project gets its own DuckLake catalog + Parquet directory for data isolation.
/// The DuckDB connection is in-memory (just a coordinator) — all data lives in
/// the `.ducklake` catalog file + Parquet files under the project directory.
pub struct ProjectPool {
    pub connections: DashMap<String, PoolEntry>,
    data_dir: Utf8PathBuf,
    max_connections: usize,
    /// Storage mode, resolved ONCE at boot and threaded in here so the pool's
    /// per-project dir selection and DuckLake attach never re-derive it from
    /// `storage::load_config()`.
    storage_mode: crate::storage::StorageMode,
}

pub struct PoolEntry {
    /// The single writer connection while no flusher owns it. A flusher takes
    /// this once and returns it on clean shutdown; no DuckDB work is performed
    /// while holding this mutex.
    writer: Mutex<Option<Connection>>,
    readers: ReaderPool,
    last_access: Instant,
}

impl PoolEntry {
    /// Checkpoint DuckLake before dropping so data is persisted to Parquet.
    fn shutdown(&self) {
        let guard = self.writer.lock();
        if let Some(writer) = guard.as_ref() {
            if let Err(e) = writer.execute_batch("CHECKPOINT lake;") {
                tracing::warn!("DuckDB CHECKPOINT failed during shutdown: {e}");
            }
        } else {
            tracing::debug!("skipping pool-entry shutdown checkpoint; flusher owns writer");
        }
    }
}

#[derive(Clone)]
pub struct ReaderPool {
    inner: Arc<ReaderPoolInner>,
}

struct ReaderPoolInner {
    available: Mutex<Vec<Connection>>,
    ready: Condvar,
}

pub struct ReaderConnection {
    conn: Option<Connection>,
    pool: Arc<ReaderPoolInner>,
}

impl ReaderPool {
    fn new(base: &Connection, size: usize) -> Result<Self> {
        anyhow::ensure!(size > 0, "reader pool size must be greater than zero");
        let mut available = Vec::with_capacity(size);
        for _ in 0..size {
            let reader = base
                .try_clone()
                .context("failed to clone DuckDB reader connection")?;
            reader
                .execute_batch("USE lake;")
                .context("failed to set reader default schema")?;
            available.push(reader);
        }

        Ok(Self {
            inner: Arc::new(ReaderPoolInner {
                available: Mutex::new(available),
                ready: Condvar::new(),
            }),
        })
    }

    pub fn get(&self) -> ReaderConnection {
        let mut available = self.inner.available.lock();
        loop {
            if let Some(conn) = available.pop() {
                return ReaderConnection {
                    conn: Some(conn),
                    pool: Arc::clone(&self.inner),
                };
            }
            self.inner.ready.wait(&mut available);
        }
    }

    pub fn same_pool(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Deref for ReaderConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.conn
            .as_ref()
            .expect("reader connection missing before drop")
    }
}

impl Drop for ReaderConnection {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            let mut available = self.pool.available.lock();
            available.push(conn);
            self.pool.ready.notify_one();
        }
    }
}

struct OpenedProject {
    writer: Connection,
    readers: ReaderPool,
}

impl ProjectPool {
    pub fn new(
        data_dir: Utf8PathBuf,
        max_connections: usize,
        storage_mode: crate::storage::StorageMode,
    ) -> Self {
        Self {
            connections: DashMap::new(),
            data_dir,
            max_connections,
            storage_mode,
        }
    }

    /// The storage mode this pool was built with (resolved once at boot).
    pub fn storage_mode(&self) -> &crate::storage::StorageMode {
        &self.storage_mode
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
    pub fn create_project(&self, project_id: &str) -> Result<ReaderPool> {
        use dashmap::mapref::entry::Entry;

        match self.connections.entry(project_id.to_string()) {
            Entry::Occupied(ref mut entry) => {
                entry.get_mut().last_access = Instant::now();
                Ok(entry.get().readers.clone())
            }
            Entry::Vacant(entry) => {
                // Evict if at capacity before creating new.
                // Entry guard is dropped first to avoid holding two entries.
                drop(entry);

                if self.connections.len() >= self.max_connections {
                    self.evict_oldest();
                }

                let project_dir = if self.storage_mode.is_distributed() {
                    crate::storage::tenant_data_dir(&self.data_dir, project_id)?
                } else {
                    self.data_dir.join(project_id)
                };
                let opened = self
                    .open_project_connections(project_id, &project_dir)
                    .with_context(|| {
                        format!("failed to open DuckDB connection for project {project_id}")
                    })?;

                // Re-enter via entry API — another task may have inserted while we
                // were in open_connection (which downloads extensions on cold start).
                match self.connections.entry(project_id.to_string()) {
                    Entry::Occupied(ref mut entry) => {
                        // Another task won the race — use their connection, discard ours.
                        // Our connection's Drop will clean up.
                        entry.get_mut().last_access = Instant::now();
                        tracing::info!(project_id, "reused DuckDB connection (race loser)");
                        Ok(entry.get().readers.clone())
                    }
                    Entry::Vacant(entry) => {
                        let readers = opened.readers.clone();
                        entry.insert(PoolEntry {
                            writer: Mutex::new(Some(opened.writer)),
                            readers,
                            last_access: Instant::now(),
                        });
                        tracing::info!(project_id, "opened DuckDB connection (DuckLake)");
                        Ok(opened.readers)
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
    pub fn get_project(&self, project_id: &str) -> Result<ReaderPool> {
        if let Some(mut entry) = self.connections.get_mut(project_id) {
            entry.last_access = Instant::now();
            Ok(entry.readers.clone())
        } else {
            Err(anyhow::anyhow!(
                "connection not found for project {project_id} — \
                 call create_project() first"
            ))
        }
    }

    /// Transfer the single writer connection to the per-project flusher.
    #[tracing::instrument(skip(self), fields(project_id = %project_id))]
    pub fn take_writer(&self, project_id: &str) -> Result<Connection> {
        let entry = self.connections.get(project_id).ok_or_else(|| {
            anyhow::anyhow!(
                "connection not found for project {project_id} — \
                 call create_project() first"
            )
        })?;
        let mut writer = entry.writer.lock();
        writer.take().ok_or_else(|| {
            anyhow::anyhow!("writer already owned by flusher for project {project_id}")
        })
    }

    /// Return the writer connection after a flusher exits cleanly.
    #[tracing::instrument(skip(self, writer), fields(project_id = %project_id))]
    pub fn return_writer(&self, project_id: &str, writer: Connection) {
        if let Some(entry) = self.connections.get(project_id) {
            let mut slot = entry.writer.lock();
            if slot.is_some() {
                tracing::warn!(
                    project_id,
                    "writer returned while idle writer already exists"
                );
                return;
            }
            *slot = Some(writer);
        } else {
            if let Err(e) = writer.execute_batch("CHECKPOINT lake;") {
                tracing::warn!(
                    project_id,
                    "DuckDB CHECKPOINT failed for removed project: {e}"
                );
            }
            tracing::debug!(project_id, "dropped writer for removed project");
        }
    }

    /// Clone a fresh connection sharing the SAME in-memory DuckDB instance
    /// (and therefore the same attached DuckLake catalog) as `project_id`'s
    /// writer/readers. Used by the quack query-surface server, which needs
    /// one long-lived dedicated connection for the life of the process —
    /// not one borrowed from, and competing with, the bounded reader pool.
    pub fn clone_connection(&self, project_id: &str) -> Result<Connection> {
        let reader = self.get_project(project_id)?.get();
        let conn = reader
            .try_clone()
            .context("failed to clone DuckDB connection for the quack query surface")?;
        conn.execute_batch("USE lake;")
            .context("failed to set default schema on cloned connection")?;
        Ok(conn)
    }

    pub fn project_ids(&self) -> Vec<String> {
        self.connections
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
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

    /// Shared setup for a fresh in-memory DuckDB connection: set the
    /// extension cache, cap memory/threads, attach DuckLake (PostgreSQL or
    /// local file), and create the sigiro tables. Used by per-project
    /// connections.
    ///
    /// The attach is retried up to `ATTACH_ATTEMPTS` times with
    /// `ATTACH_BACKOFF` between attempts. This handles the cold-start race
    /// where any node with a PostgreSQL catalog comes up before Postgres
    /// is ready to accept connections — the first attempt fails with
    /// "Connection refused" or "the database system is starting up", and
    /// a later attempt succeeds. For the local-DuckDB-file path (no
    /// Postgres) the first attempt succeeds and the loop is a no-op.
    fn attach_ducklake(
        &self,
        conn: &Connection,
        project_id: &str,
        project_dir: &camino::Utf8Path,
    ) -> Result<()> {
        let ext_dir = crate::storage::resolve_extension_dir(&self.data_dir);
        conn.execute_batch(&format!(
            "SET extension_directory = '{}';",
            ext_dir.as_str()
        ))
        .context("failed to set DuckDB extension_directory")?;
        let memory_limit = crate::config::duckdb_memory_limit()?;
        let memory_limit = memory_limit.replace('\'', "''");
        let threads = crate::config::duckdb_threads()?;
        conn.execute_batch(&format!(
            "SET memory_limit = '{memory_limit}'; SET threads = {threads};"
        ))
        .context("failed to set DuckDB memory/threads")?;

        let mut last_err: Option<anyhow::Error> = None;
        for attempt in 1..=ATTACH_ATTEMPTS {
            let attach_result = match self.storage_mode.distributed_config() {
                Some(config) => {
                    crate::storage::init_distributed(conn, config, project_dir, project_id)
                }
                None => crate::storage::init_default(conn, project_dir),
            };
            match attach_result {
                Ok(()) => {
                    crate::schema::create_tables(conn, Self::prefix())?;
                    // Lock the local filesystem unless data lives in S3 (DuckLake
                    // itself needs network access to reach S3). Local-file mode
                    // and distributed-with-local-data mode both lock down.
                    let lock_local = !self.storage_mode.uses_s3();
                    if lock_local {
                        // `enable_external_access = false` (set inside
                        // lock_filesystem) is instance-global and
                        // irreversible — any extension not already loaded
                        // before that point can never be loaded afterward,
                        // on this or any cloned connection (see
                        // lock_filesystem's own doc comment re: parquet/
                        // json). The quack query-surface extension has the
                        // exact same requirement, so it must load here too,
                        // while access is still open — not later, from the
                        // dedicated connection crate::server::run_servers
                        // clones for it. quack also lazily autoloads `httpfs`
                        // internally for its own network transport (observed
                        // directly via duckdb_logs(): "Failed to load
                        // extension 'httpfs'" on every request once access is
                        // locked down, even though `quack_serve` itself
                        // "succeeds") — local/default mode never installs
                        // httpfs otherwise (only the S3-backed distributed
                        // path does), so it must be preloaded here too.
                        if crate::config::query_surface()? == crate::config::QuerySurface::Quack {
                            conn.execute_batch("INSTALL httpfs; LOAD httpfs;").context(
                                "failed to preload httpfs (required internally by quack) before \
                                 locking down filesystem access",
                            )?;
                            crate::quack::load_and_pin(conn).context(
                                "failed to load/pin the quack extension before locking down \
                                 filesystem access",
                            )?;
                        }
                        self.lock_filesystem(conn, &ext_dir)?;
                    }
                    return Ok(());
                }
                Err(e) => {
                    tracing::warn!(
                        attempt,
                        max = ATTACH_ATTEMPTS,
                        "DuckLake attach failed: {e:#}"
                    );
                    last_err = Some(e);
                    std::thread::sleep(ATTACH_BACKOFF);
                }
            }
        }
        Err(last_err
            .unwrap_or_else(|| anyhow::anyhow!("no attach attempts made"))
            .context(format!(
                "exhausted {ATTACH_ATTEMPTS} DuckLake attach attempts; \
                 is the storage backend (PostgreSQL for distributed, filesystem for local) reachable?"
            )))
    }

    /// Local storage only: restrict DuckDB file access to sigiro's own dirs so no
    /// query (regardless of function name) can read arbitrary paths. Applied AFTER
    /// the DuckLake attach + table creation (attach/extension-load need external
    /// access on). enable_external_access=false is instance-global and irreversible
    /// at runtime, so it propagates to every try_clone'd connection and cannot be
    /// re-widened by a query. Gated on the DATA path, not the catalog: a distributed
    /// (PostgreSQL catalog) setup with LOCAL per-tenant data (no S3 bucket) still
    /// locks down, since its data files sit on this same local filesystem. Only
    /// skipped when data lives in S3 — DuckLake itself needs network access to
    /// reach it, which this lockdown would prevent entirely (not just for user
    /// queries). This is NOT a gap for untrusted queries in S3 mode: with
    /// `enable_external_access` left on, the SQL-level firewall
    /// (`crate::server::validate_read_only` — SELECT-only, `sigiro_*` table
    /// allowlist, reader-function denylist, positive scalar-function allowlist,
    /// WITH-ban) is what the quack query surface's authorization gate runs on
    /// every query (see `crate::quack::install_authorization_gate`), and does so
    /// identically regardless of whether this engine-level lockdown ran.
    ///
    /// `LOAD parquet; LOAD json; LOAD icu;` runs first, while access is still open:
    /// `init_default` only installs `ducklake`/`sqlite`, so `parquet` (DuckLake's
    /// storage format), `json` (used by `resource_attributes::JSON->>...` in
    /// investigate queries), and `icu` (all timezone arithmetic — `TIMESTAMPTZ`,
    /// `now() - INTERVAL`, `AT TIME ZONE`, `epoch(now())`, the bread-and-butter
    /// "last N hours" time-bounded observability query) are normally autoloaded on
    /// first use — but autoloading IS external access, so once
    /// `enable_external_access` is off it can never happen and every first use fails
    /// with "Loading external extensions is disabled through configuration". icu is
    /// a first-party timezone/collation extension with no filesystem or network
    /// access at query time, so pre-loading it does not widen the lockdown surface.
    fn lock_filesystem(&self, conn: &Connection, ext_dir: &camino::Utf8Path) -> Result<()> {
        let data_dir = self.data_dir.as_str().replace('\'', "''");
        let ext = ext_dir.as_str().replace('\'', "''");
        conn.execute_batch(
            "INSTALL parquet; LOAD parquet; INSTALL json; LOAD json; INSTALL icu; LOAD icu;",
        )
        .context("failed to preload DuckDB extensions before locking filesystem access")?;
        conn.execute_batch(&format!(
            "SET allowed_directories = ['{data_dir}', '{ext}']; SET enable_external_access = false;"
        ))
        .context("failed to lock DuckDB filesystem access")?;
        tracing::info!(data_dir = %self.data_dir, "DuckDB filesystem access locked to data + extension dirs");
        Ok(())
    }

    #[tracing::instrument(skip(self, project_dir), fields(project_dir = %project_dir))]
    fn open_project_connections(
        &self,
        project_id: &str,
        project_dir: &camino::Utf8Path,
    ) -> Result<OpenedProject> {
        let base = Connection::open_in_memory()
            .with_context(|| "failed to open in-memory DuckDB".to_string())?;
        tracing::info!(
            "DuckDB: loading extensions on first use (run `sigiro fetch-extensions` to pre-warm)"
        );
        self.attach_ducklake(&base, project_id, project_dir)?;
        // Ensure DuckLake tables are discoverable without schema prefix.
        base.execute_batch("USE lake;")
            .context("failed to set DuckDB default schema")?;
        let writer = base
            .try_clone()
            .context("failed to clone DuckDB writer connection")?;
        writer
            .execute_batch("USE lake;")
            .context("failed to set writer default schema")?;
        let readers = ReaderPool::new(&base, crate::config::duckdb_threads()?)
            .context("failed to create DuckDB reader pool")?;
        Ok(OpenedProject { writer, readers })
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
        if let Some((_, entry)) = self.connections.remove(project_id) {
            entry.shutdown();
        }
    }
}

/// Number of attempts to attach the DuckLake catalog at startup.
/// Cold starts (e.g. a distributed node coming up before PostgreSQL is ready)
/// can fail the first attempt with "Connection refused" or "the database
/// system is starting up". 30 attempts × 1s = 30s of patience is well within
/// the Docker `start_period` budget and the compose healthcheck grace window.
const ATTACH_ATTEMPTS: u32 = 30;

/// Backoff between DuckLake attach attempts. 1s matches typical Postgres
/// initdb time on a small data directory.
const ATTACH_BACKOFF: Duration = Duration::from_secs(1);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StorageConfig;

    fn s3_config(bucket: &str) -> StorageConfig {
        StorageConfig {
            catalog_url: "postgresql://u:p@localhost:5432/db".to_string(),
            s3_bucket: bucket.to_string(),
            s3_region: "us-east-1".to_string(),
            s3_endpoint: "example.r2.cloudflarestorage.com".to_string(),
            s3_key_id: "key".to_string(),
            s3_secret: "secret".to_string(),
            s3_use_ssl: true,
        }
    }

    /// M-B: mirrors the `lock_local` gate in `attach_ducklake` — local-SQLite
    /// mode (`None`) and distributed-with-local-data mode (empty bucket) both
    /// take the lockdown path; only distributed-with-S3-data skips it.
    #[test]
    fn lock_local_gate_locks_unless_s3_bucket_configured() {
        let lock_local = |storage_config: Option<&StorageConfig>| {
            storage_config.is_none_or(|c| c.s3_bucket.is_empty())
        };

        assert!(lock_local(None), "local SQLite mode must stay locked");
        assert!(
            lock_local(Some(&s3_config(""))),
            "distributed catalog + local per-tenant data must lock down"
        );
        assert!(
            !lock_local(Some(&s3_config("sigiro-tenant-data"))),
            "distributed catalog + S3 data must NOT lock down (needs network)"
        );
    }

    /// M-B (asserted-SQL): `lock_filesystem` actually sets
    /// `enable_external_access = false` and restricts `allowed_directories`
    /// on the connection it locks — not just the gate boolean above.
    #[test]
    fn lock_filesystem_disables_external_access() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = camino::Utf8Path::from_path(dir.path()).unwrap().to_owned();
        let ext_dir = data_dir.join("extensions");
        std::fs::create_dir_all(&ext_dir).unwrap();

        let pool = ProjectPool::new(data_dir, 10, crate::storage::StorageMode::LocalFile);
        let conn = Connection::open_in_memory().unwrap();
        pool.lock_filesystem(&conn, &ext_dir).unwrap();

        let locked: bool = conn
            .query_row(
                "SELECT current_setting('enable_external_access')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!locked, "enable_external_access must be false after lock");

        // A path outside allowed_directories must now be rejected.
        let err = conn
            .execute_batch("SELECT * FROM read_text('/etc/hostname');")
            .unwrap_err();
        assert!(
            err.to_string().contains("external")
                || err.to_string().contains("disabled")
                || err.to_string().contains("not allowed"),
            "expected an external-access error, got: {err}"
        );
    }
}
