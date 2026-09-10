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
    pub(crate) fn prefix() -> &'static str {
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
    pub(crate) fn clone_connection(&self, project_id: &str) -> Result<Connection> {
        let reader = self.get_project(project_id)?.get();
        let conn = reader
            .try_clone()
            .context("failed to clone DuckDB connection for the quack query surface")?;
        conn.execute_batch("USE lake;")
            .context("failed to set default schema on cloned connection")?;
        Ok(conn)
    }

    pub(crate) fn project_ids(&self) -> Vec<String> {
        self.connections
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Every project that has data and therefore needs periodic maintenance,
    /// whether or not it currently holds a pooled connection.
    ///
    /// [`Self::project_ids`] returns only the live connection map, which is the
    /// wrong set for anything that must not miss a tenant. `run_evictor` drops
    /// any project idle for 300s while retention runs hourly, so a low-traffic
    /// tenant is almost never pooled at the moment retention fires. Measured on
    /// the hosted box on 2026-08-31: one tenant had accumulated 3,227 snapshots
    /// and 3,030 data files spanning nine days under a seven-day policy, and the
    /// container log showed a single retention run, for the one tenant that
    /// ingests continuously and is therefore always pooled.
    ///
    /// The durable source is the per-project directory on disk, which both
    /// storage modes create and neither removes: `<data_dir>/tenants/<id>` when
    /// distributed, `<data_dir>/<id>` otherwise. Verified against production,
    /// where the hosted volume holds a `tenants/` entry per tenant even though
    /// its Parquet lives in S3.
    ///
    /// Unioned with the live map so a project created in this process is covered
    /// before anything has been flushed to disk for it.
    ///
    /// A directory that cannot be read is skipped rather than fatal: retention
    /// covering fewer projects is bad, and retention failing to start at all is
    /// worse.
    pub fn known_project_ids(&self) -> Vec<String> {
        let mut ids = self.project_ids();
        let root = if self.storage_mode.is_distributed() {
            self.data_dir.join("tenants")
        } else {
            self.data_dir.clone()
        };
        match std::fs::read_dir(&root) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                        continue;
                    }
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    // `sanitize_tenant_id` owns the definition of a legal tenant
                    // id, including the reserved names (`extensions`, `tenants`)
                    // that sit beside per-project directories in local mode.
                    // Reusing it keeps this from drifting into a second, weaker
                    // copy of that rule.
                    if crate::storage::sanitize_tenant_id(&name).is_err() {
                        continue;
                    }
                    if !ids.contains(&name) {
                        ids.push(name);
                    }
                }
            }
            Err(error) => {
                tracing::warn!(
                    dir = %root,
                    "could not enumerate project directories; maintenance will \
                     cover only the projects currently pooled: {error}"
                );
            }
        }
        ids
    }

    /// Refresh last_access for an existing project entry so it is not evicted as idle.
    /// Callers (e.g. long-running per-project flusher tasks) should call this periodically
    /// to prevent the idle evictor from removing their connection while they are alive.
    #[tracing::instrument(skip(self), fields(project_id = %project_id))]
    pub(crate) fn touch_project(&self, project_id: &str) {
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
        let temp_dir = project_dir.join(".tmp");
        std::fs::create_dir_all(&temp_dir)
            .with_context(|| format!("failed to create DuckDB temp directory: {temp_dir}"))?;
        let temp_dir_sql = temp_dir.as_str().replace('\'', "''");
        conn.execute_batch(&format!(
            "SET extension_directory = '{}'; SET temp_directory = '{temp_dir_sql}';",
            ext_dir.as_str(),
        ))
        .context("failed to set DuckDB extension and temp directories")?;
        tracing::info!(%temp_dir, "DuckDB temp directory configured");
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
                    // Catalog policy. Three groups, all set once per attach and
                    // persisted by DuckLake in the metadata schema.
                    //
                    // WRITE FORMAT. zstd + Parquet v2 is the only supported
                    // on-disk format: half the size of DuckLake's snappy + v1
                    // default and a faster scan, measured on production files.
                    //
                    // MAINTENANCE. `CHECKPOINT <catalog>` already IS DuckLake's
                    // maintenance pass. Its implementation runs, in order:
                    // flush_inlined_data, expire_snapshots, merge_adjacent_files,
                    // rewrite_data_files, cleanup_old_files,
                    // delete_orphaned_files. The flusher has called
                    // `CHECKPOINT lake` periodically all along, but production had
                    // persisted only parquet_compression and parquet_version.
                    // Snapshot expiry was disabled, rewrites required 95% of a
                    // file to be deleted, and the compaction target was DuckLake's
                    // 512 MiB default. Measured with the exact policy below on a
                    // 300-file fixture after a retention DELETE:
                    //
                    //   current files before CHECKPOINT   303
                    //   current files after CHECKPOINT      1 (624,000 rows intact)
                    //
                    // Physical files remain for the configured two-day grace;
                    // current-snapshot file count is the compaction measure.
                    //
                    // That is why a hand-rolled reclaim existed here before, and
                    // why it was unbounded enough to block a tenant's writer
                    // thread for twenty minutes in production. DuckLake does
                    // this incrementally per checkpoint when told the policy.
                    //
                    // DuckLake defaults auto_compact to true, a 512 MiB target
                    // file (equal to sigiro's whole DuckDB memory limit), a 0.95
                    // rewrite threshold, two-day file deletion, and NO snapshot
                    // expiry. The values below make the existing checkpoint
                    // useful without allowing one rewrite to target the whole
                    // memory budget.
                    //
                    // SAFETY. Profile inlining stays disabled until DuckLake can
                    // safely combine it with concurrent retention deletes.
                    // Persisting zero also replaces the unsafe 1,000-row policy
                    // in existing catalogs when they next attach.
                    //
                    // `expire_older_than` is tied to the row retention window:
                    // once rows past that age are deleted, snapshots that old
                    // can only serve time-travel into data that no longer
                    // exists. `delete_older_than` is a grace period before an
                    // unreferenced file is removed, kept far above the 10s query
                    // timeout so an in-flight scan can never lose its file.
                    // Both are GLOBAL-only in DuckLake and rejected if scoped.
                    let retention_days = crate::config::RETENTION_DAYS
                        .value()
                        .context("invalid SIGIRO_RETENTION_DAYS")?;
                    crate::schema::create_tables(conn, Self::prefix())?;
                    set_ducklake_catalog_policy(conn, retention_days)?;
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
                            // Same irreversible-lockdown rule: the system_stats
                            // community extension backs the `sys_*`
                            // host-statistics functions that federated fleet
                            // queries reach (federate::SYS_STATS_FUNCTIONS).
                            // It is NOT statically linked the way
                            // parquet/json/icu are, so it needs INSTALL (a
                            // network download from the COMMUNITY repo) while
                            // external access is still open — afterwards it can
                            // never load.
                            //
                            // Gated on the quack surface, exactly like the two
                            // installs above: `sys_*` is reachable ONLY through
                            // that surface, so installing it when the surface
                            // is off would add a boot-time dependency on an
                            // external repo to every deployment that cannot use
                            // the feature — including the hosted box, where
                            // SIGIRO_QUERY_SURFACE defaults to `none`.
                            crate::storage::install_extensions_from(
                                conn,
                                &["system_stats"],
                                crate::storage::ExtensionRepo::Community,
                            )
                            .context(
                                "failed to install/load system_stats before locking down \
                                 filesystem access",
                            )?;
                        }
                        self.lock_filesystem(conn, &ext_dir)?;
                    }
                    return Ok(());
                }
                Err(e) => {
                    // Permanent failures (backend/data_path mismatch, bad
                    // credentials, misconfiguration) never clear — retrying only
                    // wedges the pool behind ATTACH_ATTEMPTS blocking sleeps while
                    // every incoming request piles up. Fail fast and loud; only
                    // genuinely transient causes earn the retry loop.
                    let fatal = e
                        .downcast_ref::<crate::storage::AttachError>()
                        .filter(|a| !a.retryable)
                        .map(|a| a.category);
                    if let Some(category) = fatal {
                        return Err(e.context(format!(
                            "DuckLake attach failed with a permanent error \
                             (category {category}); not retrying"
                        )));
                    }
                    tracing::warn!(
                        attempt,
                        max = ATTACH_ATTEMPTS,
                        "DuckLake attach failed (transient): {e:#}"
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
    /// `parquet` (DuckLake's storage format), `json` (used by
    /// `resource_attributes::JSON->>...` in diagnose queries), and `icu` (all
    /// timezone arithmetic — `TIMESTAMPTZ`, `now() - INTERVAL`, `AT TIME ZONE`,
    /// `epoch(now())`, the bread-and-butter "last N hours" time-bounded
    /// observability query) are normally autoloaded on first use — but autoloading
    /// IS external access, so once `enable_external_access` is off it can never
    /// happen and every first use fails with "Loading external extensions is
    /// disabled through configuration". icu is a first-party timezone/collation
    /// extension with no filesystem or network access at query time, so pre-loading
    /// it does not widen the lockdown surface.
    ///
    /// LOAD, never INSTALL. `INSTALL` forces a repo download into the baked
    /// extension dir (/opt/sigiro/extensions), which is read-only on the
    /// distroless image and fails with "Permission denied"; `LOAD` uses whatever
    /// copy is already available with no network access.
    ///
    /// **What satisfies these three depends on the platform, and assuming
    /// otherwise is what broke this.** The Linux builds link the Dockerfile's
    /// ar-merged static archive, so parquet/json/icu are genuinely built in.
    /// macOS uses Cargo's `bundled` feature — DuckDB core without those
    /// fragments — so there they must exist as files, which is what
    /// `storage::prewarm_extensions` now guarantees by asking DuckDB which ones
    /// are `STATICALLY_LINKED` rather than hardcoding an answer. An earlier
    /// version of this comment asserted all three were always static; on macOS
    /// that made every query fail with a 400 for a missing json.duckdb_extension
    /// while /health still returned 200.
    ///
    /// The names live in `storage::LOCKDOWN_PRELOADED_EXTENSIONS` so the prewarm
    /// and this batch cannot drift apart.
    fn lock_filesystem(&self, conn: &Connection, ext_dir: &camino::Utf8Path) -> Result<()> {
        let data_dir = self.data_dir.as_str().replace('\'', "''");
        let ext = ext_dir.as_str().replace('\'', "''");
        let preload: String = crate::storage::LOCKDOWN_PRELOADED_EXTENSIONS
            .iter()
            .map(|name| format!("LOAD {name};"))
            .collect();
        conn.execute_batch(&preload)
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

    /// Remove a project's connection (for account deletion).
    #[tracing::instrument(skip(self), fields(project_id = %project_id))]
    pub fn remove(&self, project_id: &str) {
        if let Some((_, entry)) = self.connections.remove(project_id) {
            entry.shutdown();
        }
    }
}

/// Persist the policy consumed by DuckLake's native `CHECKPOINT` maintenance.
///
/// DuckLake checkpoints already run the full sequence: flush inlined data,
/// expire snapshots, compact adjacent files, rewrite files with enough deleted
/// rows, clean unreferenced files, and delete storage orphans. The writer calls
/// `CHECKPOINT lake` hourly, so these options are the scheduler and
/// bounds. Do not duplicate that sequence in sigiro.
pub(crate) fn set_ducklake_catalog_policy(conn: &Connection, retention_days: u32) -> Result<()> {
    let expiry = if retention_days == 0 {
        String::new()
    } else {
        format!("{retention_days} days")
    };
    conn.execute_batch(&format!(
        "CALL lake.set_option('parquet_compression', 'zstd'); \
         CALL lake.set_option('parquet_version', '2'); \
         CALL lake.set_option('auto_compact', true); \
         CALL lake.set_option('target_file_size', '32MB'); \
         CALL lake.set_option('rewrite_delete_threshold', 0.2); \
         CALL lake.set_option('expire_older_than', '{expiry}'); \
         CALL lake.set_option('delete_older_than', '2 days'); \
         CALL lake.set_option('data_inlining_row_limit', 0, table_name => 'sigiro_profiles');"
    ))
    .context("failed to set DuckLake catalog policy")
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
    use crate::storage::{DataLocation, S3BucketName, S3Config, S3Region, StorageConfig};

    fn s3_config(bucket: &str) -> StorageConfig {
        let data = if bucket.is_empty() {
            DataLocation::LocalFilesystem
        } else {
            DataLocation::S3(
                S3Config::new(
                    S3BucketName::try_new(bucket.to_string()).unwrap(),
                    S3Region::try_new("us-east-1".to_string()).unwrap(),
                    "https://example.r2.cloudflarestorage.com".parse().unwrap(),
                    "key".to_string(),
                    "secret".to_string(),
                )
                .unwrap(),
            )
        };
        StorageConfig::new("postgresql://u:***@localhost:5432/db".to_string(), data).unwrap()
    }

    /// M-B: mirrors the `lock_local` gate in `attach_ducklake` — local-SQLite
    /// mode (`None`) and distributed-with-local-data mode (empty bucket) both
    /// take the lockdown path; only distributed-with-S3-data skips it.
    #[test]
    fn lock_local_gate_locks_unless_s3_bucket_configured() {
        let lock_local = |storage_config: Option<&StorageConfig>| {
            storage_config.is_none_or(|config| !config.uses_s3())
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

    /// Every extension `lock_filesystem` preloads is satisfiable on THIS build,
    /// either statically linked or present as a cached file.
    ///
    /// This is the regression guard for the class of bug that has now hit this
    /// surface three times: `prewarm_extensions` and `lock_filesystem` each held
    /// their own belief about what the binary contains, and nothing checked that
    /// they agreed. The third instance shipped in the published macOS binaries —
    /// `sigiro serve` started, `/health` returned 200, and every query 400'd on a
    /// missing `json.duckdb_extension`.
    ///
    /// Deliberately asserts the *outcome* (the batch succeeds) rather than
    /// mirroring the list, so it stays honest on whichever platform runs it: on
    /// Linux the extensions are static and this passes with an empty extension
    /// dir; on macOS it passes only because the prewarm cached them.
    #[test]
    fn lock_filesystem_loads_are_prewarmed() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = camino::Utf8Path::from_path(dir.path()).unwrap().to_owned();

        // Same call `sigiro fetch-extensions` makes.
        crate::storage::prewarm_extensions(&data_dir)
            .expect("prewarm must succeed on a fresh data dir");

        let ext_dir = crate::storage::resolve_extension_dir(&data_dir);
        let pool = ProjectPool::new(data_dir, 10, crate::storage::StorageMode::LocalFile);
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "SET extension_directory = '{}';",
            ext_dir.as_str().replace('\'', "''")
        ))
        .unwrap();

        pool.lock_filesystem(&conn, &ext_dir).expect(
            "every LOCKDOWN_PRELOADED_EXTENSIONS entry must be loadable after prewarm — \
             if this fails, prewarm_extensions is not caching something lock_filesystem needs",
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
