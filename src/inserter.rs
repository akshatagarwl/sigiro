//! Per-project writers.
//!
//! **Contract (#186).** An ingest request is acknowledged only after its rows
//! are committed to the project's DuckLake catalog. A local catalog is a DuckDB
//! file with its own WAL, so a committed row survives a crash. Every request
//! either commits or gets an error its sender can retry; nothing is buffered
//! after a success response, and nothing acknowledged is dropped.
//!
//! **Group commit.** Each project has one writer thread that owns the
//! project's single DuckDB writer connection. It takes every request that is
//! queued when it wakes, up to [`GROUP_COMMIT_BYTES`], and commits them as one
//! DuckLake snapshot. Under load the groups grow; when idle each request
//! commits on its own. With DuckLake data inlining on (local catalogs, see
//! `pool::LOCAL_INLINING_ROW_LIMIT`) a small commit is a catalog row insert, not
//! a Parquet file, so there is no flush timer or row threshold to tune (#228).
//!
//! **Memory (#188).** Queued requests hold byte permits from a process-wide and
//! a per-project budget until their commit finishes. A request that cannot get
//! its permits within [`QUEUE_WAIT`] is rejected with a retryable 503, so the
//! queued, decoded Arrow data is bounded in bytes, not in messages.
//!
//! **Maintenance (#228, #223).** The writer runs `CHECKPOINT lake` (DuckLake's
//! compaction and cleanup) on its own schedule. A failed checkpoint never
//! rejects ingest: it is retried with backoff, falls back to bounded steps, and
//! is reported on `/health` and as metrics. Only the disk cap rejects ingest.
//!
//! **Lifecycle (#190, #192).** A writer with nothing to do exits, returns its
//! connection to the pool and leaves the project evictable; the next request
//! starts a new one. Shutdown has one deadline for every writer; a writer
//! still running at the deadline is interrupted and reported.

use anyhow::{Context, Result};
use arrow::array::{Array, StringArray, UInt64Array};
use arrow::record_batch::RecordBatch;
use dashmap::DashMap;
use duckdb::Connection;
use sigiro_segments::MetricBatches;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot};
use tracing_attributes::instrument;

use crate::pool::ProjectPool;
use crate::types::{IngestMsg, SignalTable};
use sigiro_core::drain::DrainTree;

pub use crate::disk_gate::DiskCapacityState;
pub use crate::storage_health::StorageHealth;

/// Errors from the ingest router.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// The project's writer stopped before it committed this request.
    #[error("writer stopped for project {project_id}")]
    WriterStopped { project_id: String },
    /// Failed to open the project's DuckLake catalog.
    #[error("connection failed for project {project_id}: {source:#}")]
    ConnectionFailed {
        project_id: String,
        #[source]
        source: anyhow::Error,
    },
    /// On-disk footprint reached the configured `SIGIRO_MAX_DISK_BYTES` cap —
    /// ingest is rejected to keep the box from filling its disk. Maps to HTTP
    /// 507 / gRPC `resource_exhausted`, distinct from an internal 500.
    #[error("disk capacity exceeded — ingest rejected to protect the box")]
    DiskCapacityExceeded,
    /// The queued-bytes budget stayed full for [`QUEUE_WAIT`]: the writer is
    /// behind. Maps to HTTP 503 + `Retry-After` / gRPC `unavailable`.
    #[error("ingest queue for project {project_id} is full — retry shortly")]
    QueueFull { project_id: String },
    /// The commit failed. Nothing from this request was stored, so the sender
    /// may retry it. Maps to HTTP 503 / gRPC `unavailable`.
    #[error("commit failed for project {project_id}: {message}")]
    CommitFailed { project_id: String, message: String },
    /// Expensive work reached its process-wide concurrency budget.
    #[error(transparent)]
    Admission(#[from] crate::workload::AdmissionError),
}

/// Largest group of queued requests committed as one transaction. Bounds the
/// size of one DuckLake snapshot and of the writer's in-flight data.
const GROUP_COMMIT_BYTES: usize = 8 * 1024 * 1024;

/// How long a request waits for queued-bytes permits before it is rejected.
const QUEUE_WAIT: Duration = Duration::from_secs(2);

/// A writer with no requests and no maintenance due for this long exits and
/// returns its connection, so the project can be evicted (#190).
const WRITER_IDLE: Duration = Duration::from_secs(60);

/// How often a busy writer re-reads its data-file and inlined-row counts for
/// `/health` and metrics. Each read is a few catalog queries.
const HEALTH_REFRESH: Duration = Duration::from_secs(10);

/// Bytes per semaphore permit. Permits are counted in KiB so a budget of
/// gigabytes fits the semaphore's `u32` request size.
const PERMIT_BYTES: usize = 1024;

/// The process-wide and per-project queued-bytes budgets (#188).
#[derive(Clone)]
struct QueueBudget {
    global: Arc<Semaphore>,
    global_permits: u32,
    project_permits: u32,
}

impl QueueBudget {
    fn new(global_bytes: usize, project_bytes: usize) -> Self {
        let permits = |bytes: usize| -> u32 {
            u32::try_from((bytes / PERMIT_BYTES).max(1)).unwrap_or(u32::MAX)
        };
        let global_permits = permits(global_bytes);
        let project_permits = permits(project_bytes).min(global_permits);
        Self {
            global: Arc::new(Semaphore::new(global_permits as usize)),
            global_permits,
            project_permits,
        }
    }

    /// Permits a request of `bytes` needs. Every request costs at least one,
    /// so the message count is bounded too; a request larger than a project's
    /// whole budget takes the whole budget and runs alone.
    fn permits_for(&self, bytes: usize) -> u32 {
        let permits = u32::try_from(bytes.div_ceil(PERMIT_BYTES)).unwrap_or(u32::MAX);
        permits.clamp(1, self.project_permits)
    }

    /// Wait up to `wait` for both permits. The project's are taken first, so
    /// one busy project waits on its own share before it can hold global ones.
    async fn acquire(
        &self,
        project: &Arc<Semaphore>,
        bytes: usize,
        wait: Duration,
    ) -> Option<QueuePermit> {
        let permits = self.permits_for(bytes);
        tokio::time::timeout(wait, async {
            let project = Arc::clone(project).acquire_many_owned(permits).await.ok()?;
            let global = Arc::clone(&self.global)
                .acquire_many_owned(permits)
                .await
                .ok()?;
            Some(QueuePermit {
                _project: project,
                _global: global,
            })
        })
        .await
        .ok()
        .flatten()
    }

    fn queued_bytes(&self) -> usize {
        (self.global_permits as usize - self.global.available_permits()) * PERMIT_BYTES
    }
}

/// Byte permits held by one queued request until its commit finishes.
struct QueuePermit {
    _project: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
}

/// One queued ingest request.
struct Job {
    msg: IngestMsg,
    bytes: usize,
    ack: oneshot::Sender<Result<(), String>>,
    _permit: QueuePermit,
}

enum WriterCommand {
    Ingest(Box<Job>),
    RetentionCleanup {
        days: u32,
        ack: oneshot::Sender<Result<(), String>>,
    },
    Stop,
}

/// Why a writer thread stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriterExit {
    Idle,
    Stopped,
}

/// A running writer, shared between the router and its thread.
struct WriterHandle {
    id: u64,
    tx: std_mpsc::Sender<WriterCommand>,
    /// False once the thread has decided to exit. Senders check it and send
    /// under the same lock, so every request sent while it is true is still
    /// in the channel when the thread drains it on the way out.
    open: parking_lot::Mutex<bool>,
    project_budget: Arc<Semaphore>,
    interrupt: Arc<duckdb::InterruptHandle>,
    thread: parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl WriterHandle {
    /// Queue a command unless the writer is exiting; returns it back if so.
    fn send(&self, command: WriterCommand) -> std::result::Result<(), WriterCommand> {
        let open = self.open.lock();
        if !*open {
            return Err(command);
        }
        self.tx.send(command).map_err(|error| error.0)
    }

    fn close(&self) {
        *self.open.lock() = false;
    }

    fn is_open(&self) -> bool {
        *self.open.lock()
    }

    /// Open and its thread still running: requests sent now get committed.
    fn is_serving(&self) -> bool {
        self.is_open() && !self.has_died()
    }

    fn take_thread(&self) -> Option<std::thread::JoinHandle<()>> {
        self.thread.lock().take()
    }

    fn has_died(&self) -> bool {
        self.is_open()
            && self
                .thread
                .lock()
                .as_ref()
                .is_some_and(std::thread::JoinHandle::is_finished)
    }
}

/// Routes ingest to per-project writers and waits for each commit.
pub struct MsgRouter {
    pool: Arc<ProjectPool>,
    writers: Arc<DashMap<String, Arc<WriterHandle>>>,
    initializers: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    next_writer_id: AtomicU64,
    budget: QueueBudget,
    maintenance: MaintenanceSchedule,
    disk: Arc<DiskCapacityState>,
    health: Arc<StorageHealth>,
    workloads: crate::workload::WorkloadBudget,
    alerts: Option<crate::alerts::AlertStore>,
    stopping: std::sync::atomic::AtomicBool,
}

impl MsgRouter {
    /// The production router: budgets and schedule from config, the given
    /// disk gate, and alert evaluation on log ingest.
    pub fn new(
        pool: Arc<ProjectPool>,
        disk: Arc<DiskCapacityState>,
        alerts: Option<crate::alerts::AlertStore>,
    ) -> Result<Self> {
        let budget = QueueBudget::new(
            crate::config::ingest_queue_bytes()?,
            crate::config::ingest_project_queue_bytes()?,
        );
        let maintenance = MaintenanceSchedule::from_config()?;
        Ok(Self::with_parts(pool, disk, alerts, budget, maintenance))
    }

    fn with_parts(
        pool: Arc<ProjectPool>,
        disk: Arc<DiskCapacityState>,
        alerts: Option<crate::alerts::AlertStore>,
        budget: QueueBudget,
        maintenance: MaintenanceSchedule,
    ) -> Self {
        Self {
            pool,
            writers: Arc::new(DashMap::new()),
            initializers: DashMap::new(),
            next_writer_id: AtomicU64::new(0),
            budget,
            maintenance,
            disk,
            health: StorageHealth::new(),
            workloads: crate::workload::WorkloadBudget::new(),
            alerts,
            stopping: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Shared handle to the disk-capacity gate, so the storage-maintenance task
    /// can sample and trip/clear the same state the ingest gate reads.
    pub(crate) fn disk_state(&self) -> Arc<DiskCapacityState> {
        Arc::clone(&self.disk)
    }

    /// Per-project checkpoint and file state, for `/health` and metrics.
    pub(crate) fn storage_health(&self) -> Arc<StorageHealth> {
        Arc::clone(&self.health)
    }

    /// Decoded bytes waiting for a commit, all projects.
    pub(crate) fn queued_bytes(&self) -> usize {
        self.budget.queued_bytes()
    }

    /// Explicit lifecycle start. Logs activation and ensures the router is ready.
    /// Callers should invoke this once after `new()` before accepting traffic.
    pub fn start(&self) {
        tracing::info!("MsgRouter started — accepting ingest messages");
    }

    /// True if a writer thread died without exiting through its lifecycle
    /// (a panic). Its project's requests fail until the next request replaces it.
    pub(crate) fn has_dead_writers(&self) -> bool {
        self.writers.iter().any(|entry| entry.value().has_died())
    }

    pub(crate) fn workloads(&self) -> crate::workload::WorkloadBudget {
        self.workloads.clone()
    }

    pub(crate) async fn run_retention_cleanup(&self, days: u32) -> Result<()> {
        // `known_project_ids`, NOT `project_ids`. The latter is the live
        // connection map, and the idle evictor drops anything untouched for
        // 300s while this runs hourly, so a low-traffic tenant was almost never
        // pooled at the moment retention fired and was skipped indefinitely.
        // `writer` below opens a connection for any project that does not have
        // one, so enumerating beyond the pool is safe.
        for project_id in self.pool.known_project_ids() {
            let (ack, done) = oneshot::channel();
            self.submit(&project_id, WriterCommand::RetentionCleanup { days, ack })
                .await
                .map_err(|e| anyhow::anyhow!("failed to queue retention cleanup: {e}"))?;
            match done
                .await
                .with_context(|| format!("retention writer stopped for {project_id}"))?
            {
                Ok(()) => {
                    tracing::debug!(project_id, days, "retention cleanup completed");
                }
                Err(error) => anyhow::bail!("retention cleanup failed for {project_id}: {error}"),
            }
        }
        Ok(())
    }

    /// The project's running writer, starting one if there is none. A writer
    /// that is exiting is joined first: it returns the project's connection to
    /// the pool on its way out, and the new writer takes it from there.
    async fn writer(&self, project_id: &str) -> Result<Arc<WriterHandle>, RouterError> {
        if let Some(handle) = self.writers.get(project_id).map(|e| Arc::clone(e.value()))
            && handle.is_serving()
        {
            return Ok(handle);
        }

        let initializer = self
            .initializers
            .entry(project_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _initializing = initializer.lock().await;
        if let Some(handle) = self.writers.get(project_id).map(|e| Arc::clone(e.value())) {
            if handle.is_serving() {
                return Ok(handle);
            }
            self.join_writer(project_id, &handle).await;
        }
        if self.stopping.load(Ordering::Acquire) {
            return Err(RouterError::WriterStopped {
                project_id: project_id.to_string(),
            });
        }

        let pid = project_id.to_string();
        let permit = self
            .workloads
            .try_acquire(crate::workload::WorkloadClass::ProjectInit)?;
        let pool = Arc::clone(&self.pool);
        let open_pid = pid.clone();
        let conn = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            pool.create_project_writer(&open_pid)
        })
        .await
        .map_err(|source| RouterError::ConnectionFailed {
            project_id: pid.clone(),
            source: anyhow::anyhow!("project initialization task failed: {source}"),
        })?
        .map_err(|source| RouterError::ConnectionFailed {
            project_id: pid.clone(),
            source,
        })?;

        let handle = self.spawn_writer(&pid, conn)?;
        self.writers.insert(pid.clone(), Arc::clone(&handle));
        self.initializers.remove(&pid);
        Ok(handle)
    }

    fn spawn_writer(
        &self,
        project_id: &str,
        conn: Connection,
    ) -> Result<Arc<WriterHandle>, RouterError> {
        let (tx, rx) = std_mpsc::channel();
        let handle = Arc::new(WriterHandle {
            id: self.next_writer_id.fetch_add(1, Ordering::Relaxed),
            tx,
            open: parking_lot::Mutex::new(true),
            project_budget: Arc::new(Semaphore::new(self.budget.project_permits as usize)),
            interrupt: conn.interrupt_handle(),
            thread: parking_lot::Mutex::new(None),
        });
        let writer = Writer {
            project_id: project_id.to_string(),
            handle: Arc::clone(&handle),
            writers: Arc::clone(&self.writers),
            pool: Arc::clone(&self.pool),
            health: Arc::clone(&self.health),
            schedule: self.maintenance,
        };
        let conn_slot = Arc::new(parking_lot::Mutex::new(Some(conn)));
        let thread_conn = Arc::clone(&conn_slot);
        let thread = std::thread::Builder::new()
            .name(format!("sigiro-writer-{project_id}"))
            .spawn(move || {
                let conn = thread_conn.lock().take().expect("connection moves once");
                writer.run(conn, &rx);
            })
            .map_err(|error| {
                if let Some(conn) = conn_slot.lock().take() {
                    self.pool.return_writer(project_id, conn);
                }
                RouterError::ConnectionFailed {
                    project_id: project_id.to_string(),
                    source: anyhow::anyhow!("failed to spawn writer thread: {error}"),
                }
            })?;
        *handle.thread.lock() = Some(thread);
        tracing::info!(project_id, "writer started");
        Ok(handle)
    }

    /// Wait for an exiting writer to finish and forget it.
    async fn join_writer(&self, project_id: &str, handle: &Arc<WriterHandle>) {
        if let Some(thread) = handle.take_thread() {
            match tokio::task::spawn_blocking(move || thread.join()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => tracing::error!(project_id, "writer thread panicked"),
                Err(error) => tracing::error!(project_id, "writer join task failed: {error}"),
            }
        }
        self.writers
            .remove_if(project_id, |_, current| current.id == handle.id);
    }

    /// Queue a command on the project's writer, replacing a writer that
    /// exited between lookup and send.
    async fn submit(
        &self,
        project_id: &str,
        mut command: WriterCommand,
    ) -> Result<(), RouterError> {
        for _ in 0..3 {
            let handle = self.writer(project_id).await?;
            match handle.send(command) {
                Ok(()) => return Ok(()),
                Err(returned) => command = returned,
            }
        }
        Err(RouterError::WriterStopped {
            project_id: project_id.to_string(),
        })
    }

    /// Commit one ingest request. Returns once its rows are committed, or with
    /// the reason they were not.
    #[instrument(skip_all, fields(project_id = %msg.project_id()))]
    pub async fn send(&self, msg: IngestMsg) -> Result<(), RouterError> {
        // Disk-capacity gate: reject at the very top (before any writer/DuckDB
        // work) when the box is over its byte cap. All ingest surfaces funnel
        // through here, so this one check protects every entry point.
        if self.disk.is_over_cap() {
            return Err(RouterError::DiskCapacityExceeded);
        }
        let project_id = msg.project_id().to_string();
        let alert_batch = match &msg {
            IngestMsg::Logs { batch, .. } if self.alerts.is_some() => Some(batch.clone()),
            _ => None,
        };

        let bytes = msg_byte_size(&msg);
        let handle = self.writer(&project_id).await?;
        let permit = self
            .budget
            .acquire(&handle.project_budget, bytes, QUEUE_WAIT)
            .await
            .ok_or_else(|| RouterError::QueueFull {
                project_id: project_id.clone(),
            })?;
        let (ack, committed) = oneshot::channel();
        let job = Box::new(Job {
            msg,
            bytes,
            ack,
            _permit: permit,
        });
        self.submit(&project_id, WriterCommand::Ingest(job)).await?;
        match committed.await {
            Ok(Ok(())) => {}
            Ok(Err(message)) => {
                return Err(RouterError::CommitFailed {
                    project_id,
                    message,
                });
            }
            Err(_) => return Err(RouterError::WriterStopped { project_id }),
        }
        self.process_alerts(&project_id, alert_batch).await;
        Ok(())
    }

    /// Evaluate alert rules on committed logs. Waits for an alert slot rather
    /// than failing the request: the logs are already stored, and a rejection
    /// here would make the sender retry and store them twice.
    async fn process_alerts(&self, project_id: &str, batch: Option<RecordBatch>) {
        let (Some(alerts), Some(batch)) = (self.alerts.clone(), batch) else {
            return;
        };
        let permit = self
            .workloads
            .acquire(crate::workload::WorkloadClass::Maintenance)
            .await;
        let project_id = project_id.to_string();
        let alert_project_id = project_id.clone();
        let result = match tokio::task::spawn_blocking(move || {
            let _permit = permit;
            alerts.process_log_batch(&alert_project_id, &batch)
        })
        .await
        {
            Ok(result) => result,
            Err(source) => {
                tracing::error!(
                    project_id,
                    error = %source,
                    "alert task failed after telemetry was accepted"
                );
                return;
            }
        };
        match result {
            Ok(result) if result.notifications_enqueued > 0 => tracing::info!(
                project_id,
                notifications = result.notifications_enqueued,
                "alert transitions persisted"
            ),
            Ok(_) => {}
            Err(source) => {
                tracing::error!(
                    project_id,
                    error = %source,
                    "alert persistence failed after telemetry was accepted"
                );
            }
        }
    }

    /// Stop every writer within one deadline (`SIGIRO_SHUTDOWN_TIMEOUT_SECS`).
    ///
    /// Writers commit what is already queued, then return their connections.
    /// A writer still running at the deadline has its DuckDB statement
    /// interrupted; its uncommitted requests were never acknowledged, so their
    /// senders retry them. Any writer that did not stop is reported.
    pub async fn shutdown_and_wait(&self) -> Result<()> {
        self.stopping.store(true, Ordering::Release);
        let budget = Duration::from_secs(crate::config::shutdown_timeout_secs()?);
        let writers: Vec<(String, Arc<WriterHandle>)> = self
            .writers
            .iter()
            .map(|entry| (entry.key().clone(), Arc::clone(entry.value())))
            .collect();
        let failures = stop_writers(writers, budget).await;
        anyhow::ensure!(
            failures.is_empty(),
            "writer shutdown failed: {}",
            failures.join("; ")
        );
        Ok(())
    }
}

/// Ask every writer to stop, wait until `budget` has passed in total, then
/// interrupt the stragglers and give them one more second.
async fn stop_writers(writers: Vec<(String, Arc<WriterHandle>)>, budget: Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut joins = Vec::new();
    for (project_id, handle) in writers {
        // A closed writer is already draining; only a running one needs Stop.
        let _ignored = handle.send(WriterCommand::Stop).is_ok();
        if let Some(thread) = handle.take_thread() {
            joins.push((
                project_id,
                handle,
                tokio::task::spawn_blocking(move || thread.join()),
            ));
        }
    }
    let mut failures = Vec::new();
    for (project_id, handle, mut join) in joins {
        let joined = match tokio::time::timeout_at(deadline, &mut join).await {
            Ok(result) => Some(result),
            Err(_) => {
                handle.interrupt.interrupt();
                tokio::time::timeout(Duration::from_secs(1), &mut join)
                    .await
                    .ok()
            }
        };
        match joined {
            Some(Ok(Ok(()))) => {}
            Some(Ok(Err(_))) => failures.push(format!("{project_id}: writer thread panicked")),
            Some(Err(error)) => failures.push(format!("{project_id}: writer join failed: {error}")),
            None => failures.push(format!(
                "{project_id}: writer did not stop within {}s and ignored the interrupt; \
                 its queued requests were not acknowledged",
                budget.as_secs()
            )),
        }
    }
    failures
}

/// When a writer runs `CHECKPOINT lake`.
#[derive(Debug, Clone, Copy)]
struct MaintenanceSchedule {
    /// Routine maintenance interval (`SIGIRO_CHECKPOINT_INTERVAL_SECS`).
    interval: Duration,
    /// Data files written since the last checkpoint that trigger one early.
    /// Derived from the open-files limit, which sigiro never raises.
    file_budget: u64,
    /// Upper bound on files one fallback merge opens.
    merge_files: u64,
    /// First retry delay after a failure; doubles up to `interval`.
    first_retry: Duration,
    /// How long a writer waits for work before it exits ([`WRITER_IDLE`]).
    idle: Duration,
}

impl MaintenanceSchedule {
    fn from_config() -> Result<Self> {
        let fd_limit = crate::storage_health::open_files_limit();
        Ok(Self::new(
            Duration::from_secs(crate::config::checkpoint_interval_secs()?),
            fd_limit,
        ))
    }

    fn new(interval: Duration, fd_limit: Option<u64>) -> Self {
        // A checkpoint merges files and reads every file of a table; keep the
        // files written between checkpoints, and the files one fallback merge
        // opens, to a quarter of what the process may have open.
        let quarter = fd_limit.map_or(1024, |limit| (limit / 4).max(16));
        Self {
            interval,
            file_budget: quarter,
            merge_files: quarter,
            first_retry: Duration::from_secs(30).min(interval),
            idle: WRITER_IDLE,
        }
    }

    /// Delay before the next attempt after `failures` consecutive failures.
    fn retry_after(&self, failures: u32) -> Duration {
        let doublings = failures.saturating_sub(1).min(16);
        self.first_retry
            .saturating_mul(1 << doublings)
            .min(self.interval)
    }
}

/// The state one writer thread owns.
struct Writer {
    project_id: String,
    handle: Arc<WriterHandle>,
    writers: Arc<DashMap<String, Arc<WriterHandle>>>,
    pool: Arc<ProjectPool>,
    health: Arc<StorageHealth>,
    schedule: MaintenanceSchedule,
}

impl Writer {
    fn run(self, conn: Connection, rx: &std_mpsc::Receiver<WriterCommand>) {
        let project_id = self.project_id.clone();
        let inlining = InliningLimits::for_pool(&self.pool);
        let mut tables = Tables::new(conn, inlining);
        let mut maintenance = Maintenance::new(self.schedule);
        self.health.refresh(&project_id, &tables.conn);
        let exit = self.serve(&mut tables, &mut maintenance, rx);
        // Close first, then drain: a sender that saw the writer open has
        // already queued its request, and every later one starts a new writer.
        self.handle.close();
        while let Ok(command) = rx.try_recv() {
            self.handle_command(&mut tables, &mut maintenance, command, rx);
        }
        if exit == WriterExit::Idle && maintenance.dirty() {
            maintenance.checkpoint(&project_id, &tables.conn, &self.health);
        }
        self.pool
            .return_writer(&project_id, tables.into_connection());
        // Shutdown joins and forgets its writers itself.
        if exit == WriterExit::Idle {
            self.writers
                .remove_if(&project_id, |_, current| current.id == self.handle.id);
        }
        tracing::info!(project_id, ?exit, "writer stopped");
    }

    fn serve(
        &self,
        tables: &mut Tables,
        maintenance: &mut Maintenance,
        rx: &std_mpsc::Receiver<WriterCommand>,
    ) -> WriterExit {
        let mut last_activity = Instant::now();
        let mut last_refresh = Instant::now();
        loop {
            let wake = maintenance
                .next_due()
                .min(last_activity + self.schedule.idle);
            match rx.recv_timeout(wake.saturating_duration_since(Instant::now())) {
                Ok(command) => {
                    if self
                        .handle_command(tables, maintenance, command, rx)
                        .is_some()
                    {
                        return WriterExit::Stopped;
                    }
                    last_activity = Instant::now();
                }
                Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                Err(std_mpsc::RecvTimeoutError::Disconnected) => return WriterExit::Stopped,
            }
            let now = Instant::now();
            if maintenance.is_due(now) {
                maintenance.checkpoint(&self.project_id, &tables.conn, &self.health);
                last_refresh = Instant::now();
            } else if now >= last_activity + self.schedule.idle {
                return WriterExit::Idle;
            } else if now >= last_refresh + HEALTH_REFRESH {
                self.health.refresh(&self.project_id, &tables.conn);
                last_refresh = now;
            }
        }
    }

    /// Handle one command. An ingest also takes every ingest queued behind
    /// it, up to [`GROUP_COMMIT_BYTES`], and commits them together. Returns
    /// `Some` when the writer must stop (a `Stop`, possibly found while
    /// grouping).
    fn handle_command(
        &self,
        tables: &mut Tables,
        maintenance: &mut Maintenance,
        command: WriterCommand,
        rx: &std_mpsc::Receiver<WriterCommand>,
    ) -> Option<WriterExit> {
        match command {
            WriterCommand::Ingest(job) => {
                let mut group = vec![*job];
                let mut bytes = group[0].bytes;
                let mut next = None;
                while bytes < GROUP_COMMIT_BYTES {
                    match rx.try_recv() {
                        Ok(WriterCommand::Ingest(job)) => {
                            bytes += job.bytes;
                            group.push(*job);
                        }
                        Ok(other) => {
                            next = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                self.commit(tables, maintenance, group);
                next.and_then(|command| self.handle_command(tables, maintenance, command, rx))
            }
            WriterCommand::RetentionCleanup { days, ack } => {
                let result = tables
                    .retention_cleanup(&self.project_id, days)
                    .map_err(|e| format!("{e:#}"));
                if ack.send(result).is_err() {
                    tracing::debug!(project_id = %self.project_id, "retention cleanup receiver dropped");
                }
                None
            }
            WriterCommand::Stop => Some(WriterExit::Stopped),
        }
    }

    /// Commit a group in one transaction and acknowledge each request. If the
    /// group fails, each request is retried alone, so one bad request cannot
    /// fail the others.
    fn commit(&self, tables: &mut Tables, maintenance: &mut Maintenance, group: Vec<Job>) {
        let msgs: Vec<&IngestMsg> = group.iter().map(|job| &job.msg).collect();
        match tables.commit(&msgs) {
            Ok(written) => {
                maintenance.wrote(written);
                self.pool.touch_project(&self.project_id);
                for job in group {
                    let _ignored = job.ack.send(Ok(())).is_ok();
                }
            }
            Err(error) if group.len() == 1 => {
                tracing::error!(project_id = %self.project_id, "commit failed: {error:#}");
                let job = group.into_iter().next().expect("one job");
                let _ignored = job.ack.send(Err(format!("{error:#}"))).is_ok();
            }
            Err(error) => {
                tracing::warn!(
                    project_id = %self.project_id,
                    requests = group.len(),
                    "group commit failed, committing each request alone: {error:#}"
                );
                for job in group {
                    self.commit(tables, maintenance, vec![job]);
                }
            }
        }
    }
}

/// What one commit wrote, for scheduling maintenance.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Written {
    rows: u64,
    /// Tables whose rows went to a new Parquet file rather than inline.
    files: u64,
}

/// The most rows DuckLake inlines into the catalog per table and commit.
#[derive(Debug, Clone, Copy)]
struct InliningLimits {
    rows: usize,
    profiles: usize,
}

impl InliningLimits {
    fn for_pool(pool: &ProjectPool) -> Self {
        let rows = crate::pool::inlining_row_limit(pool.storage_mode());
        Self { rows, profiles: 0 }
    }

    fn for_table(self, table: SignalTable) -> usize {
        if table == SignalTable::Profiles {
            self.profiles
        } else {
            self.rows
        }
    }
}

/// When the next `CHECKPOINT lake` runs, and how the last ones went.
struct Maintenance {
    schedule: MaintenanceSchedule,
    next: Instant,
    failures: u32,
    written: Written,
}

impl Maintenance {
    fn new(schedule: MaintenanceSchedule) -> Self {
        Self {
            schedule,
            next: Instant::now() + schedule.interval,
            failures: 0,
            written: Written::default(),
        }
    }

    fn wrote(&mut self, written: Written) {
        self.written.rows += written.rows;
        self.written.files += written.files;
    }

    /// Anything committed since the last successful checkpoint.
    fn dirty(&self) -> bool {
        self.written.rows > 0
    }

    /// Too many new files trigger a checkpoint early, unless one is already
    /// failing (then the backoff decides).
    fn files_due(&self) -> bool {
        self.failures == 0 && self.written.files >= self.schedule.file_budget
    }

    fn next_due(&self) -> Instant {
        if self.files_due() {
            Instant::now()
        } else {
            self.next
        }
    }

    fn is_due(&self, now: Instant) -> bool {
        self.files_due() || now >= self.next
    }

    fn checkpoint(&mut self, project_id: &str, conn: &Connection, health: &StorageHealth) {
        let started = Instant::now();
        let outcome = run_checkpoint(conn, self.schedule.merge_files);
        let elapsed_ms = started.elapsed().as_millis();
        match &outcome {
            CheckpointOutcome::Done => {
                self.failures = 0;
                self.next = Instant::now() + self.schedule.interval;
                self.written = Written::default();
                health.record_checkpoint(project_id, None);
                tracing::info!(project_id, elapsed_ms, "DuckLake CHECKPOINT completed");
            }
            CheckpointOutcome::Failed { error, fallback } => {
                self.failures += 1;
                // While the bounded steps make progress, keep going at the
                // first retry delay; back off only when they fail too.
                let retry = if fallback.is_ok() {
                    self.schedule.first_retry
                } else {
                    self.schedule.retry_after(self.failures)
                };
                self.next = Instant::now() + retry;
                if fallback.is_ok() {
                    // The bounded steps wrote the inlined rows and merged
                    // files, so the file trigger starts over.
                    self.written = Written::default();
                }
                health.record_checkpoint(project_id, Some(error));
                tracing::warn!(
                    project_id,
                    elapsed_ms,
                    failures = self.failures,
                    retry_in_secs = retry.as_secs(),
                    fallback = ?fallback,
                    "DuckLake CHECKPOINT failed; ingest continues: {error}"
                );
            }
        }
        health.refresh(project_id, conn);
    }
}

#[derive(Debug)]
enum CheckpointOutcome {
    Done,
    /// The full checkpoint failed. `fallback` is the result of the bounded
    /// steps run instead.
    Failed {
        error: String,
        fallback: std::result::Result<(), String>,
    },
}

/// Threads a checkpoint may use. DuckLake's merge buffers row groups per
/// thread, and with 4 or more threads it ran out of memory at the 400MB
/// default even with row groups capped (#223).
const CHECKPOINT_THREADS: i64 = 2;

/// How long one fallback keeps merging batches of files. Each batch is small
/// and quick; the budget bounds how long ingest waits behind it.
const FALLBACK_MERGE_BUDGET: Duration = Duration::from_secs(10);

/// Run DuckLake's full maintenance pass. If it fails, run the steps that
/// matter for file count on their own, with the merge bounded to
/// `merge_files` files, so a lake that already holds more files than the
/// process may open still converges, one batch per attempt (#228).
fn run_checkpoint(conn: &Connection, merge_files: u64) -> CheckpointOutcome {
    let full = with_threads(conn, CHECKPOINT_THREADS, || {
        conn.execute_batch("CHECKPOINT lake;")
            .context("DuckLake CHECKPOINT failed")
    });
    let Err(error) = full else {
        return CheckpointOutcome::Done;
    };
    let fallback = with_threads(conn, CHECKPOINT_THREADS, || {
        bounded_maintenance(conn, merge_files, FALLBACK_MERGE_BUDGET)
    })
    .map_err(|e| format!("{e:#}"));
    CheckpointOutcome::Failed {
        error: format!("{error:#}"),
        fallback,
    }
}

/// The parts of a checkpoint that bound the file count, each small enough to
/// fit the open-files limit: write inlined rows out, then merge at most
/// `merge_files` files at a time until a batch merges nothing or `budget`
/// is spent (the next attempt continues), then delete files past their grace
/// period.
fn bounded_maintenance(conn: &Connection, merge_files: u64, budget: Duration) -> Result<()> {
    let step = |sql: &str| {
        conn.execute_batch(sql)
            .with_context(|| format!("{sql} failed"))
    };
    step("CALL ducklake_flush_inlined_data('lake')")?;
    let merge =
        format!("CALL ducklake_merge_adjacent_files('lake', max_compacted_files => {merge_files})");
    let deadline = Instant::now() + budget;
    let mut files = crate::storage_health::data_file_count(conn)?;
    loop {
        step(&merge)?;
        let after = crate::storage_health::data_file_count(conn)?;
        if after >= files || Instant::now() >= deadline {
            break;
        }
        files = after;
    }
    step("CALL ducklake_cleanup_old_files('lake')")
}

/// Run `f` with DuckDB's thread count lowered to at most `threads`, then
/// restore it. The setting is per database, so queries running meanwhile
/// share the lower count.
fn with_threads<T>(conn: &Connection, threads: i64, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let current: i64 = conn
        .query_row("SELECT current_setting('threads')::BIGINT", [], |row| {
            row.get(0)
        })
        .context("failed to read DuckDB threads")?;
    if current <= threads {
        return f();
    }
    conn.execute_batch(&format!("SET threads = {threads};"))
        .context("failed to lower DuckDB threads")?;
    let result = f();
    if let Err(error) = conn.execute_batch(&format!("SET threads = {current};")) {
        tracing::error!("failed to restore DuckDB threads to {current}: {error}");
    }
    result
}

/// The writer's connection and the per-table state its commits need.
struct Tables {
    conn: Connection,
    table_names: BTreeMap<SignalTable, String>,
    inlining: InliningLimits,
    drain: DrainTree,
    stored_values: StoredValues,
}

impl Tables {
    fn new(conn: Connection, inlining: InliningLimits) -> Self {
        let table_names = SignalTable::ALL
            .iter()
            .map(|s| (*s, s.table_name(ProjectPool::prefix())))
            .collect();
        Self {
            conn,
            table_names,
            inlining,
            drain: DrainTree::new(),
            stored_values: StoredValues::default(),
        }
    }

    /// Append every message's rows and commit them as one transaction (one
    /// DuckLake snapshot). On failure nothing is stored.
    fn commit(&mut self, msgs: &[&IngestMsg]) -> Result<Written> {
        let mut batches: BTreeMap<SignalTable, Vec<RecordBatch>> = BTreeMap::new();
        let mut new_values = Vec::new();
        for msg in msgs {
            match msg {
                IngestMsg::Traces { batch, values, .. } => {
                    let values = self.stored_values.keep_new(values)?;
                    if values.num_rows() > 0 {
                        new_values.push(values.clone());
                        batches
                            .entry(SignalTable::SpanValues)
                            .or_default()
                            .push(values);
                    }
                    batches
                        .entry(SignalTable::Spans)
                        .or_default()
                        .push(batch.clone());
                }
                IngestMsg::Logs { batch, .. } => {
                    let classified = self.classify_logs(batch)?;
                    batches
                        .entry(SignalTable::Logs)
                        .or_default()
                        .push(classified);
                }
                IngestMsg::Metrics {
                    batches: metrics, ..
                } => {
                    for (table, batch) in metric_tables(metrics) {
                        batches.entry(table).or_default().push(batch.clone());
                    }
                }
                IngestMsg::Profiles { batch, .. } => {
                    batches
                        .entry(SignalTable::Profiles)
                        .or_default()
                        .push(batch.clone());
                }
            }
        }
        let result = self.append_all(&batches);
        if result.is_err() {
            // Values that were not written must be written next time.
            self.stored_values.forget(&new_values);
        }
        result
    }

    fn append_all(&self, batches: &BTreeMap<SignalTable, Vec<RecordBatch>>) -> Result<Written> {
        self.conn
            .execute_batch("BEGIN TRANSACTION;")
            .context("failed to begin the commit")?;
        let appended = (|| -> Result<Written> {
            let mut written = Written::default();
            for (table, batches) in batches {
                let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
                if rows == 0 {
                    continue;
                }
                append_batches(&self.conn, &self.table_names[table], batches)?;
                written.rows += rows as u64;
                if rows > self.inlining.for_table(*table) {
                    written.files += 1;
                }
            }
            Ok(written)
        })();
        let committed = appended.and_then(|written| {
            self.conn
                .execute_batch("COMMIT;")
                .context("failed to commit")?;
            Ok(written)
        });
        if committed.is_err()
            && let Err(error) = self.conn.execute_batch("ROLLBACK;")
        {
            tracing::debug!("rollback after a failed commit: {error}");
        }
        committed
    }

    /// Run DRAIN on each log body and append its classification to the same row.
    #[instrument(skip_all, fields(rows = batch.num_rows()))]
    fn classify_logs(&mut self, batch: &RecordBatch) -> Result<RecordBatch> {
        let body_idx = batch
            .schema()
            .index_of("body")
            .context("sigiro_logs missing body column")?;
        let body_col = batch
            .column(body_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow::anyhow!("body column is not StringArray"))?;

        let mut template_ids = Vec::with_capacity(batch.num_rows());
        let mut template_values = Vec::with_capacity(batch.num_rows());
        let mut template_texts = Vec::with_capacity(batch.num_rows());
        for i in 0..batch.num_rows() {
            let body = body_col.value(i);
            let (template, id, values) = self.drain.classify(body);
            template_ids.push(id);
            // Store extracted values as JSON array so the original log can be reconstructed.
            let values_json =
                serde_json::to_string(&values).expect("values must serialize to JSON");
            template_values.push(values_json);
            template_texts.push(template);
        }

        let mut columns = batch.columns().to_vec();
        columns.push(Arc::new(UInt64Array::from(template_ids)));
        columns.push(Arc::new(StringArray::from(template_values)));
        columns.push(Arc::new(StringArray::from(template_texts)));

        let mut fields: Vec<arrow::datatypes::Field> = batch
            .schema()
            .fields()
            .iter()
            .map(|field| field.as_ref().clone())
            .collect();
        fields.push(arrow::datatypes::Field::new(
            "template_id",
            arrow::datatypes::DataType::UInt64,
            true,
        ));
        fields.push(arrow::datatypes::Field::new(
            "template_values",
            arrow::datatypes::DataType::Utf8,
            true,
        ));
        fields.push(arrow::datatypes::Field::new(
            "template_text",
            arrow::datatypes::DataType::Utf8,
            true,
        ));
        let schema = Arc::new(arrow::datatypes::Schema::new(fields));

        RecordBatch::try_new(schema, columns).context("failed to add template columns")
    }

    #[tracing::instrument(skip_all, fields(project_id = %project_id, days))]
    fn retention_cleanup(&self, project_id: &str, days: u32) -> Result<()> {
        // Compute the cutoff in RUST and use `make_timestamp` — deliberately NO
        // DuckDB timezone ops. The locked-down runtime never loads the `icu`
        // extension, and EVERY TIMESTAMPTZ path autoloads it and fails: `now()
        // - INTERVAL` ("No function matches -(TIMESTAMPTZ, INTERVAL)"),
        // `AT TIME ZONE`, and even `epoch(now())` ("Extension Autoloading Error:
        // ... 'icu'"). `make_timestamp(<micros>)` is a core scalar (no icu) that
        // builds a naive UTC TIMESTAMP matching the naive-UTC `timestamp`
        // columns, so the comparison is correct with zero timezone dependency.
        let cutoff_micros = (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000)
            - i128::from(days) * 86_400 * 1_000_000;
        for table in self.table_names.values() {
            let sql =
                format!("DELETE FROM {table} WHERE timestamp < make_timestamp({cutoff_micros})");
            self.conn
                .execute_batch(&sql)
                .with_context(|| format!("retention cleanup failed for {table}"))?;
        }

        // DELETE writes DuckLake delete-files. The writer's periodic
        // `CHECKPOINT lake` performs DuckLake's own bounded rewrite,
        // compaction, snapshot expiry, old-file cleanup, and orphan deletion
        // using the catalog policy set in `pool::set_ducklake_catalog_policy`.
        // Do not run that sequence here: the old hand-written reclaim blocked
        // this same writer thread for more than twenty minutes on a tenant
        // with 3,115 files, which stopped its ingest and serialized
        // maintenance for every tenant behind it.
        tracing::info!(project_id, days, "retention cleanup completed");
        Ok(())
    }

    fn into_connection(self) -> Connection {
        self.conn
    }
}

fn metric_tables(batches: &MetricBatches) -> impl Iterator<Item = (SignalTable, &RecordBatch)> {
    [
        (SignalTable::MetricsGauge, batches.gauge.as_ref()),
        (SignalTable::MetricsSum, batches.sum.as_ref()),
        (SignalTable::MetricsHistogram, batches.histogram.as_ref()),
        (
            SignalTable::MetricsExpHistogram,
            batches.exp_histogram.as_ref(),
        ),
    ]
    .into_iter()
    .filter_map(|(table, batch)| batch.map(|batch| (table, batch)))
}

/// The `(day, hash)` of span values this writer has stored, so each distinct
/// value is written once per day. Forgotten on restart and when full; either
/// only costs a repeated row, which readers deduplicate.
#[derive(Default)]
struct StoredValues(std::collections::HashSet<(i64, u128)>);

impl StoredValues {
    const MAX_ENTRIES: usize = 1_000_000;

    fn keys(values: &RecordBatch) -> Result<impl Iterator<Item = (i64, u128)> + '_> {
        let days = values
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::TimestampMicrosecondArray>()
            .context("span values timestamp is not a microsecond timestamp")?;
        let hashes = values
            .column(1)
            .as_any()
            .downcast_ref::<StringArray>()
            .context("span values hash is not a string")?;
        Ok((0..values.num_rows()).map(|i| {
            let hash = u128::from_str_radix(hashes.value(i), 16).unwrap_or_default();
            (days.value(i), hash)
        }))
    }

    /// The rows of `values` not seen before, each once.
    fn keep_new(&mut self, values: &RecordBatch) -> Result<RecordBatch> {
        if self.0.len() + values.num_rows() > Self::MAX_ENTRIES {
            self.0.clear();
        }
        let keep: arrow::array::BooleanArray = Self::keys(values)?
            .map(|key| Some(self.0.insert(key)))
            .collect();
        Ok(arrow::compute::filter_record_batch(values, &keep)?)
    }

    /// Values whose commit failed must be written again next time.
    fn forget(&mut self, unwritten: &[RecordBatch]) {
        for batch in unwritten {
            if let Ok(keys) = Self::keys(batch) {
                for key in keys {
                    self.0.remove(&key);
                }
            }
        }
    }
}

/// Append a table's batches via a DuckLake catalog-qualified appender, inside
/// the caller's transaction.
fn append_batches(conn: &Connection, table: &str, batches: &[RecordBatch]) -> Result<()> {
    // conn.appender("lake.table") fails — must use appender_to_catalog_and_db.
    let bare_table = table.strip_prefix("lake.").unwrap_or(table);
    let mut appender = conn
        .appender_to_catalog_and_db(bare_table, "lake", "main")
        .with_context(|| format!("failed to create {table} appender"))?;
    for batch in batches {
        if batch.num_rows() > 0 {
            appender
                .append_record_batch(batch.clone())
                .with_context(|| format!("failed to append {table} batch"))?;
        }
    }
    appender
        .flush()
        .with_context(|| format!("failed to flush {table} appender"))
}

/// Approximate decoded size of an ingest request.
fn msg_byte_size(msg: &IngestMsg) -> usize {
    match msg {
        IngestMsg::Traces { batch, values, .. } => batch_byte_size(batch) + batch_byte_size(values),
        IngestMsg::Logs { batch, .. } | IngestMsg::Profiles { batch, .. } => batch_byte_size(batch),
        IngestMsg::Metrics { batches, .. } => metric_tables(batches)
            .map(|(_, batch)| batch_byte_size(batch))
            .sum(),
    }
}

/// Approximate in-memory byte size of an Arrow RecordBatch.
/// Sums the buffer sizes of all arrays (data + validity + offsets).
fn batch_byte_size(batch: &RecordBatch) -> usize {
    batch
        .columns()
        .iter()
        .map(|col| array_byte_size(col.as_ref()))
        .sum()
}

fn array_byte_size(array: &dyn Array) -> usize {
    let data = array.to_data();
    let mut total = data
        .buffers()
        .iter()
        .map(arrow::buffer::Buffer::len)
        .sum::<usize>();
    if let Some(nulls) = data.nulls() {
        total += nulls.buffer().len();
    }
    total
}

#[cfg(test)]
mod writer_tests;
