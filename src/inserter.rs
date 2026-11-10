use anyhow::{Context, Result};
use arrow::array::{Array, StringArray, UInt64Array};
use arrow::record_batch::RecordBatch;
use dashmap::DashMap;
use duckdb::Connection;
use sigiro_segments::MetricBatches;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle as TokioJoinHandle;
use tracing_attributes::instrument;

use crate::config::BackpressurePolicy;
use crate::pool::ProjectPool;
use crate::types::{IngestMsg, SignalTable};
use sigiro_core::drain::DrainTree;

/// Errors from the ingest router.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// Channel closed — flusher task exited unexpectedly.
    #[error("channel closed for project {project_id}")]
    ChannelClosed { project_id: String },
    /// Failed to get or create a DuckDB connection.
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
    /// DuckLake `CHECKPOINT` has failed on every attempt for
    /// `SIGIRO_MAX_CHECKPOINT_FAILURES` consecutive maintenance cycles, so
    /// queued telemetry can no longer be materialised into Parquet. Accepting
    /// more would grow the catalog without bound — in distributed mode that is
    /// the external Postgres, which `SIGIRO_MAX_DISK_BYTES` never measures.
    /// Maps to HTTP 507 / gRPC `resource_exhausted`, like the disk cap.
    #[error(
        "storage maintenance stalled for project {project_id} — {failures} consecutive DuckLake CHECKPOINT failures; ingest rejected so the catalog cannot grow unbounded"
    )]
    MaintenanceStalled { project_id: String, failures: u32 },
    /// Expensive work reached its process-wide concurrency budget.
    #[error(transparent)]
    Admission(#[from] crate::workload::AdmissionError),
}

/// Per-project DuckLake maintenance health: how many consecutive
/// `CHECKPOINT lake` attempts have failed on each project's writer.
///
/// **Why this gates ingest.** A failed checkpoint means nothing materialises
/// into Parquet, while ingest keeps appending to the catalog. In distributed
/// mode the catalog is an external Postgres that `SIGIRO_MAX_DISK_BYTES` never
/// samples, so the local disk gate cannot see the growth at all. Sustained
/// failure is therefore its own back-pressure signal.
///
/// **Why per project, not per box.** Each project has its own writer thread and
/// its own DuckLake. A corrupt or unwritable single tenant must not reject
/// every other tenant's ingest; a genuine shared-storage outage trips every
/// project independently and reaches the same place.
///
/// **Why consecutive, not cumulative.** A checkpoint that fails once and then
/// succeeds has materialised the backlog — the condition is self-healing and
/// must not be counted against a later, unrelated failure. Any success resets
/// the project to healthy.
#[derive(Debug)]
pub struct MaintenanceHealth {
    /// Consecutive checkpoint failures needed before ingest is shed. 0 disables
    /// shedding entirely (failures are still counted and surfaced on /health).
    threshold: u32,
    /// project_id -> consecutive checkpoint failures. Absent means healthy;
    /// entries are removed on success rather than zeroed, so the map stays the
    /// size of the unhealthy set.
    failures: DashMap<String, u32>,
}

impl MaintenanceHealth {
    pub fn new(threshold: u32) -> Arc<Self> {
        Arc::new(Self {
            threshold,
            failures: DashMap::new(),
        })
    }

    /// A gate that never sheds — the default for callers that have not opted
    /// into shedding.
    pub(crate) fn disabled() -> Arc<Self> {
        Self::new(0)
    }

    /// Record one failed `CHECKPOINT`. Returns the new consecutive count and
    /// whether that count just crossed the shed threshold (so the caller logs
    /// the transition exactly once, not on every subsequent cycle).
    pub fn record_checkpoint_failure(&self, project_id: &str) -> (u32, bool) {
        let mut entry = self.failures.entry(project_id.to_string()).or_insert(0);
        *entry += 1;
        let failures = *entry;
        let tripped = self.threshold > 0 && failures == self.threshold;
        (failures, tripped)
    }

    /// Record a successful `CHECKPOINT`. Returns true when this cleared a
    /// project that had been shedding, so the caller can log the recovery.
    pub fn record_checkpoint_success(&self, project_id: &str) -> bool {
        match self.failures.remove(project_id) {
            Some((_, failures)) => self.threshold > 0 && failures >= self.threshold,
            None => false,
        }
    }

    /// True when this project's writer has failed `threshold` consecutive
    /// checkpoints. Cheap enough for the ingest hot path: one `DashMap` lookup,
    /// and only for projects that have actually failed.
    pub fn is_stalled(&self, project_id: &str) -> bool {
        self.threshold > 0
            && self
                .failures
                .get(project_id)
                .is_some_and(|f| *f >= self.threshold)
    }

    /// Consecutive failure count for a project (0 when healthy).
    pub fn consecutive_failures(&self, project_id: &str) -> u32 {
        self.failures.get(project_id).map_or(0, |f| *f)
    }

    /// Every project currently shedding ingest, for the health endpoint.
    pub fn stalled_projects(&self) -> Vec<String> {
        if self.threshold == 0 {
            return Vec::new();
        }
        self.failures
            .iter()
            .filter(|e| *e.value() >= self.threshold)
            .map(|e| e.key().clone())
            .collect()
    }
}

pub use crate::disk_gate::DiskCapacityState;

/// Per-project channel capacity. When full, messages are dropped
/// (the OTLP SDK retries). Better to drop one project's data than block all.
const PROJECT_CHANNEL_CAPACITY: usize = 10_000;

// CHECKPOINT runs DuckLake compaction and catalog cleanup on the sole writer.
// Large remote catalogs take tens of seconds, so this is hourly maintenance,
// not a flush mechanism.
const DUCKLAKE_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Flusher configuration derived from env vars at startup.
#[derive(Debug, Clone)]
pub struct FlushConfig {
    pub(crate) row_threshold: usize,
    pub(crate) byte_threshold: usize,
    pub(crate) interval: Duration,
    pub(crate) backpressure: BackpressurePolicy,
}

impl FlushConfig {
    pub(crate) fn from_env() -> Self {
        let row_threshold = crate::config::FLUSH_ROW_THRESHOLD
            .value()
            .expect("FLUSH_ROW_THRESHOLD config must be valid");
        let byte_threshold = crate::config::FLUSH_BYTE_THRESHOLD
            .value()
            .expect("FLUSH_BYTE_THRESHOLD config must be valid");
        let interval_secs = crate::config::FLUSH_INTERVAL_SECS
            .value()
            .expect("FLUSH_INTERVAL_SECS config must be valid");
        let backpressure =
            crate::config::backpressure_policy().expect("BACKPRESSURE_POLICY config must be valid");
        Self {
            row_threshold,
            byte_threshold,
            interval: Duration::from_secs(interval_secs),
            backpressure,
        }
    }
}

/// Routes IngestMsg to per-project flusher tasks.
/// Handlers call `router.send(msg)` directly. The router lazily creates
/// per-project bounded channels and tokio tasks on first message per project.
/// No dispatcher thread — handlers fan out directly.
pub struct MsgRouter {
    pool: Arc<ProjectPool>,
    flush_config: FlushConfig,
    project_senders: DashMap<String, (mpsc::Sender<FlusherCommand>, TokioJoinHandle<()>)>,
    project_initializers: DashMap<String, Arc<tokio::sync::Mutex<()>>>,
    drop_count: AtomicUsize,
    disk: Arc<DiskCapacityState>,
    maintenance: Arc<MaintenanceHealth>,
    workloads: crate::workload::WorkloadBudget,
    alerts: Option<crate::alerts::AlertStore>,
}

enum FlusherCommand {
    Ingest(Box<IngestMsg>),
    RetentionCleanup {
        days: u32,
        ack: oneshot::Sender<Result<(), String>>,
    },
}

enum WriterCommand {
    Ingest(Box<IngestMsg>),
    FlushIfNeeded,
    Checkpoint,
    RetentionCleanup {
        days: u32,
        ack: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

impl MsgRouter {
    pub fn new(pool: Arc<ProjectPool>) -> Self {
        Self::with_config(pool, FlushConfig::from_env())
    }

    pub(crate) fn with_config(pool: Arc<ProjectPool>, flush_config: FlushConfig) -> Self {
        Self::with_config_and_disk(pool, flush_config, DiskCapacityState::disabled())
    }

    /// Build a router with the disk-capacity gate enabled from config. Used by
    /// the production server (`main::run_serve`); other callers get a disabled
    /// gate via `new`/`with_config`.
    pub fn new_with_disk(pool: Arc<ProjectPool>, disk: Arc<DiskCapacityState>) -> Self {
        Self::with_config_and_disk(pool, FlushConfig::from_env(), disk)
    }

    pub fn new_with_disk_and_alerts(
        pool: Arc<ProjectPool>,
        disk: Arc<DiskCapacityState>,
        alerts: crate::alerts::AlertStore,
    ) -> Self {
        let mut router = Self::with_config_disk_and_maintenance(
            pool,
            FlushConfig::from_env(),
            disk,
            MaintenanceHealth::new(
                crate::config::max_checkpoint_failures()
                    .expect("MAX_CHECKPOINT_FAILURES config must be valid"),
            ),
        );
        router.alerts = Some(alerts);
        router
    }

    pub(crate) fn with_config_and_disk(
        pool: Arc<ProjectPool>,
        flush_config: FlushConfig,
        disk: Arc<DiskCapacityState>,
    ) -> Self {
        Self::with_config_disk_and_maintenance(
            pool,
            flush_config,
            disk,
            MaintenanceHealth::disabled(),
        )
    }

    pub(crate) fn with_config_disk_and_maintenance(
        pool: Arc<ProjectPool>,
        flush_config: FlushConfig,
        disk: Arc<DiskCapacityState>,
        maintenance: Arc<MaintenanceHealth>,
    ) -> Self {
        Self {
            pool,
            flush_config,
            project_senders: DashMap::new(),
            project_initializers: DashMap::new(),
            drop_count: AtomicUsize::new(0),
            disk,
            maintenance,
            workloads: crate::workload::WorkloadBudget::new(),
            alerts: None,
        }
    }

    /// Shared handle to the disk-capacity gate, so the storage-maintenance task
    /// can sample and trip/clear the same state the ingest gate reads.
    pub(crate) fn disk_state(&self) -> Arc<DiskCapacityState> {
        Arc::clone(&self.disk)
    }

    /// Shared handle to the per-project DuckLake maintenance health, so the
    /// health endpoint reports the same state the ingest gate sheds on.
    pub(crate) fn maintenance_health(&self) -> Arc<MaintenanceHealth> {
        Arc::clone(&self.maintenance)
    }

    /// Explicit lifecycle start. Logs activation and ensures the router is ready.
    /// Callers should invoke this once after `new()` before accepting traffic.
    pub fn start(&self) {
        tracing::info!("MsgRouter started — accepting ingest messages");
    }

    /// Return true if any flusher task has panicked or exited unexpectedly.
    pub(crate) fn has_dead_flushers(&self) -> bool {
        self.project_senders
            .iter()
            .any(|entry| entry.value().1.is_finished())
    }

    pub(crate) fn dropped_messages(&self) -> usize {
        self.drop_count.load(Ordering::Relaxed)
    }

    pub(crate) fn workloads(&self) -> crate::workload::WorkloadBudget {
        self.workloads.clone()
    }

    pub(crate) async fn run_retention_cleanup(&self, days: u32) -> Result<()> {
        // `known_project_ids`, NOT `project_ids`. The latter is the live
        // connection map, and the idle evictor drops anything untouched for
        // 300s while this runs hourly, so a low-traffic tenant was almost never
        // pooled at the moment retention fired and was skipped indefinitely.
        // `ensure_flusher` below opens a connection for any project that does
        // not have one, so enumerating beyond the pool is safe.
        for project_id in self.pool.known_project_ids() {
            self.ensure_flusher(&project_id)
                .await
                .map_err(|e| anyhow::anyhow!("failed to ensure retention flusher: {e}"))?;
            let sender = self
                .project_senders
                .get(&project_id)
                .map(|entry| entry.value().0.clone())
                .ok_or_else(|| anyhow::anyhow!("flusher missing for project {project_id}"))?;
            let (ack, done) = oneshot::channel();
            sender
                .send(FlusherCommand::RetentionCleanup { days, ack })
                .await
                .with_context(|| format!("retention flusher channel closed for {project_id}"))?;
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

    /// Ensure a flusher exists for the project (lazy creation on first use).
    /// DuckLake attachment runs once per project on Tokio's blocking pool.
    async fn ensure_flusher(&self, project_id: &str) -> Result<(), RouterError> {
        if self.project_senders.contains_key(project_id) {
            return Ok(());
        }

        let initializer = self
            .project_initializers
            .entry(project_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _initializing = initializer.lock().await;
        if self.project_senders.contains_key(project_id) {
            return Ok(());
        }

        let pid = project_id.to_string();
        let permit = self
            .workloads
            .try_acquire(crate::workload::WorkloadClass::ProjectInit)?;
        let pool_for_open = Arc::clone(&self.pool);
        let open_pid = pid.clone();
        let writer_result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            pool_for_open.create_project_writer(&open_pid)
        })
        .await
        .map_err(|source| RouterError::ConnectionFailed {
            project_id: pid.clone(),
            source: anyhow::anyhow!("project initialization task failed: {source}"),
        })
        .and_then(|result| {
            result.map_err(|source| RouterError::ConnectionFailed {
                project_id: pid.clone(),
                source,
            })
        });
        let writer = match writer_result {
            Ok(writer) => writer,
            Err(error) => return Err(error),
        };

        let pool = Arc::clone(&self.pool);
        let cfg = self.flush_config.clone();
        let maintenance = Arc::clone(&self.maintenance);
        let (tx, rx) = mpsc::channel::<FlusherCommand>(PROJECT_CHANNEL_CAPACITY);
        let pid_spawn = pid.clone();
        tracing::info!(project_id = %pid, "ensure_flusher: spawning new flusher task");
        let handle = tokio::spawn(async move {
            run_flusher(pid_spawn, writer, rx, pool, cfg, maintenance).await;
        });
        self.project_senders.insert(pid.clone(), (tx, handle));
        self.project_initializers.remove(&pid);
        tracing::info!(project_id = %pid, "ensure_flusher: inserted new sender into project_senders");
        Ok(())
    }

    /// Route a message to the appropriate per-project flusher.
    /// Creates the flusher task lazily on first message for a project.
    #[instrument(skip_all, fields(project_id = %msg.project_id()))]
    pub async fn send(&self, msg: IngestMsg) -> Result<(), RouterError> {
        // Disk-capacity gate: reject at the very top (before any flusher/DuckDB
        // work) when the box is over its byte cap. All ingest surfaces funnel
        // through here, so this one check protects every entry point.
        if self.disk.is_over_cap() {
            return Err(RouterError::DiskCapacityExceeded);
        }

        let project_id = msg.project_id().to_string();
        // Maintenance gate: this project's writer has failed every checkpoint
        // for `SIGIRO_MAX_CHECKPOINT_FAILURES` consecutive cycles, so nothing
        // it accepts can be materialised. Checked per project (after the
        // box-wide disk gate) so one wedged tenant cannot reject the others.
        if self.maintenance.is_stalled(&project_id) {
            return Err(RouterError::MaintenanceStalled {
                failures: self.maintenance.consecutive_failures(&project_id),
                project_id,
            });
        }

        let project_id_for_err = project_id.clone();
        let alert_batch = match &msg {
            IngestMsg::Logs { batch, .. } if self.alerts.is_some() => Some(batch.clone()),
            _ => None,
        };

        // Lazy creation: ensure flusher exists before sending.
        self.ensure_flusher(&project_id).await?;

        let sender = self
            .project_senders
            .get(&project_id)
            .map(|entry| entry.value().0.clone())
            .ok_or_else(|| RouterError::ConnectionFailed {
                project_id: project_id.clone(),
                source: anyhow::anyhow!("project sender disappeared after flusher initialization"),
            })?;

        let alert_permit = if alert_batch.is_some() {
            Some(
                self.workloads
                    .try_acquire(crate::workload::WorkloadClass::Maintenance)?,
            )
        } else {
            None
        };

        match self.flush_config.backpressure {
            BackpressurePolicy::Block => {
                match sender.send(FlusherCommand::Ingest(Box::new(msg))).await {
                    Ok(()) => {
                        self.process_alerts(&project_id, alert_batch, alert_permit)
                            .await;
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!(project_id = %project_id_for_err, "send failed: channel error: {:?}", e);
                        // Remove the entry and check if the flusher task panicked.
                        if let Some((_, (_, handle))) = self.project_senders.remove(&project_id)
                            && handle.is_finished()
                        {
                            match handle.await {
                                Err(join_err) if join_err.is_panic() => {
                                    tracing::error!(project_id = %project_id_for_err, "FLUSHER TASK PANICKED — data in buffer may be lost: {}", join_err);
                                }
                                Err(join_err) => {
                                    tracing::error!(project_id = %project_id_for_err, "flusher task exited with error: {}", join_err);
                                }
                                Ok(()) => {
                                    tracing::warn!(project_id = %project_id_for_err, "flusher task exited normally (unexpected — should only exit on shutdown)");
                                }
                            }
                        }
                        Err(RouterError::ChannelClosed {
                            project_id: project_id_for_err,
                        })
                    }
                }
            }
            BackpressurePolicy::DropNewest => {
                match sender.try_send(FlusherCommand::Ingest(Box::new(msg))) {
                    Ok(()) => {
                        self.process_alerts(&project_id, alert_batch, alert_permit)
                            .await;
                        Ok(())
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        let count = self.drop_count.fetch_add(1, Ordering::Relaxed) + 1;
                        tracing::warn!(
                            project_id = %project_id_for_err,
                            total_dropped = count,
                            "channel full — DATA LOSS: dropping incoming message (BackpressurePolicy::DropNewest)"
                        );
                        Ok(())
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::error!(project_id = %project_id_for_err, "send failed: channel closed");
                        if let Some((_, (_, handle))) = self.project_senders.remove(&project_id)
                            && handle.is_finished()
                        {
                            match handle.await {
                                Err(join_err) if join_err.is_panic() => {
                                    tracing::error!(project_id = %project_id_for_err, "FLUSHER TASK PANICKED — data in buffer may be lost: {}", join_err);
                                }
                                Err(join_err) => {
                                    tracing::error!(project_id = %project_id_for_err, "flusher task exited with error: {}", join_err);
                                }
                                Ok(()) => {
                                    tracing::warn!(project_id = %project_id_for_err, "flusher task exited normally (unexpected — should only exit on shutdown)");
                                }
                            }
                        }
                        Err(RouterError::ChannelClosed {
                            project_id: project_id_for_err,
                        })
                    }
                }
            }
        }
    }

    async fn process_alerts(
        &self,
        project_id: &str,
        batch: Option<RecordBatch>,
        permit: Option<crate::workload::WorkloadPermit>,
    ) {
        let (Some(alerts), Some(batch), Some(permit)) = (self.alerts.clone(), batch, permit) else {
            return;
        };
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

    /// Shut down and await all flusher tasks. Guarantees all buffered data
    /// is flushed before returning.
    pub async fn shutdown_and_wait(&self) -> Result<()> {
        let project_ids: Vec<String> = self
            .project_senders
            .iter()
            .map(|entry| entry.key().clone())
            .collect();
        let mut flushers = Vec::with_capacity(project_ids.len());
        for project_id in project_ids {
            if let Some((project_id, (sender, handle))) = self.project_senders.remove(&project_id) {
                drop(sender);
                flushers.push((project_id, handle));
            }
        }

        let budget_secs = crate::config::shutdown_timeout_secs()?;
        let mut failures = Vec::new();
        for (project_id, mut handle) in flushers {
            match tokio::time::timeout(Duration::from_secs(budget_secs), &mut handle).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failures.push(format!("{project_id}: flusher task failed: {error}"));
                }
                Err(_) => {
                    handle.abort();
                    if let Err(error) = handle.await
                        && !error.is_cancelled()
                    {
                        failures.push(format!("{project_id}: flusher abort failed: {error}"));
                    }
                    failures.push(format!(
                        "{project_id}: flusher did not stop within {budget_secs} seconds \
                         (raise SIGIRO_SHUTDOWN_TIMEOUT_SECS if this is a loaded machine \
                         rather than a stuck drain)"
                    ));
                }
            }
        }

        anyhow::ensure!(
            failures.is_empty(),
            "flusher shutdown failed: {}",
            failures.join("; ")
        );
        Ok(())
    }
}

/// Per-project flusher state. Each project gets its own flusher task
/// so project A's load cannot block project B.
/// Crash recovery: DuckDB has its own WAL — once `append_record_batch`
/// returns, the data is durable. The only data-loss window is the buffer
/// here (up to 1s / 10k rows between 200 OK and DuckDB append). This is
/// acceptable for a debugging tool where the data source is still running
/// and will re-emit.
struct ProjectFlusher {
    conn: Connection,
    table_names: BTreeMap<SignalTable, String>,
    buffers: BTreeMap<SignalTable, Vec<RecordBatch>>,
    total_rows: usize,
    total_bytes: usize,
    last_flush: Instant,
    row_threshold: usize,
    byte_threshold: usize,
    flush_interval: Duration,
    drain: DrainTree,
    first_flush: bool,
}

impl ProjectFlusher {
    fn new(conn: Connection, cfg: &FlushConfig) -> Self {
        let signals = SignalTable::ALL;
        let table_names: BTreeMap<SignalTable, String> = signals
            .iter()
            .map(|s| (*s, s.table_name(ProjectPool::prefix())))
            .collect();
        let buffers: BTreeMap<SignalTable, Vec<RecordBatch>> =
            signals.iter().map(|s| (*s, Vec::new())).collect();

        Self {
            conn,
            table_names,
            buffers,
            total_rows: 0,
            total_bytes: 0,
            last_flush: Instant::now(),
            row_threshold: cfg.row_threshold,
            byte_threshold: cfg.byte_threshold,
            flush_interval: cfg.interval,
            drain: DrainTree::new(),
            first_flush: true,
        }
    }
    #[instrument(skip_all, fields(signal = ?signal, rows = batch.num_rows()))]
    fn buffer(&mut self, signal: SignalTable, batch: RecordBatch) {
        let batch = if signal == SignalTable::Logs {
            match self.classify_logs(&batch) {
                Ok(classified) => classified,
                Err(e) => {
                    tracing::warn!(
                        rows = batch.num_rows(),
                        "log classification failed, skipping batch: {e:#}"
                    );
                    return;
                }
            }
        } else {
            batch
        };
        let batch_bytes = batch_byte_size(&batch);
        self.total_rows += batch.num_rows();
        self.total_bytes += batch_bytes;
        self.buffers.entry(signal).or_default().push(batch);
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

        let id_array = std::sync::Arc::new(UInt64Array::from(template_ids));
        let values_array = std::sync::Arc::new(StringArray::from(template_values));
        let text_array = std::sync::Arc::new(StringArray::from(template_texts));
        let mut columns = batch.columns().to_vec();
        columns.push(id_array);
        columns.push(values_array);
        columns.push(text_array);

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
        let schema = std::sync::Arc::new(arrow::datatypes::Schema::new(fields));

        RecordBatch::try_new(schema, columns).context("failed to add template columns")
    }

    fn buffer_metrics(&mut self, batches: MetricBatches) {
        if let Some(b) = batches.gauge {
            self.buffer(SignalTable::MetricsGauge, b);
        }
        if let Some(b) = batches.sum {
            self.buffer(SignalTable::MetricsSum, b);
        }
        if let Some(b) = batches.histogram {
            self.buffer(SignalTable::MetricsHistogram, b);
        }
        if let Some(b) = batches.exp_histogram {
            self.buffer(SignalTable::MetricsExpHistogram, b);
        }
    }

    fn should_flush(&self) -> bool {
        self.first_flush
            || self.reached_flush_threshold()
            || self.last_flush.elapsed() >= self.flush_interval
    }

    fn reached_flush_threshold(&self) -> bool {
        self.total_rows >= self.row_threshold || self.total_bytes >= self.byte_threshold
    }

    /// Flush all buffered batches to DuckDB using appenders per table.
    /// Uses appender_to_catalog_and_db for DuckLake catalog-qualified table access.
    #[tracing::instrument(skip_all, fields(project_id = %project_id, rows = self.total_rows))]
    fn flush(&mut self, project_id: &str) -> Result<()> {
        if self.total_rows == 0 {
            // F-25: demoted from debug! to trace! — fires every flush interval
            // per project (default 60s) and was polluting `RUST_LOG=debug` runs
            // with per-project "nothing happened" noise.
            tracing::trace!(project_id, "flush skipped: no rows");
            return Ok(());
        }

        tracing::info!(project_id, rows = self.total_rows, tables = ?self.buffers.keys().collect::<Vec<_>>(), "flushing batches");
        let conn = &self.conn;

        let mut failures: Vec<String> = Vec::new();
        for (signal, buf) in &mut self.buffers {
            if buf.is_empty() {
                continue;
            }
            let table = &self.table_names[signal];
            let batches = std::mem::take(buf);
            let rows: usize = batches.iter().map(RecordBatch::num_rows).sum();
            if let Err(e) = append_batches(conn, table, batches) {
                tracing::error!(
                    project_id,
                    table = %table,
                    rows_lost = rows,
                    "flush failed — dropping buffered batches: {e:#}"
                );
                failures.push(format!("{table}: {e:#}"));
            }
        }

        tracing::info!(
            project_id,
            rows = self.total_rows,
            bytes = self.total_bytes,
            "flushed batches"
        );

        self.total_rows = 0;
        self.total_bytes = 0;
        self.last_flush = Instant::now();
        self.first_flush = false;

        anyhow::ensure!(
            failures.is_empty(),
            "flush failed for {} table(s): {}",
            failures.len(),
            failures.join("; ")
        );
        Ok(())
    }

    #[tracing::instrument(skip_all, fields(project_id = %project_id))]
    fn checkpoint(&self, project_id: &str) -> Result<()> {
        self.conn
            .execute_batch("CHECKPOINT lake;")
            .context("DuckLake CHECKPOINT failed")?;
        tracing::debug!(project_id, "DuckLake CHECKPOINT completed");
        Ok(())
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

        // DELETE writes DuckLake delete-files. The periodic `CHECKPOINT lake`
        // performs DuckLake's own bounded rewrite, compaction, snapshot expiry,
        // old-file cleanup, and orphan deletion using the catalog policy set in
        // `pool::set_ducklake_catalog_policy`. Do not run that sequence here:
        // the old hand-written reclaim blocked this same writer thread for more
        // than twenty minutes on a tenant with 3,115 files, which stopped its
        // telemetry flushes and serialized maintenance for every tenant behind
        // it.
        tracing::info!(project_id, days, "retention cleanup completed");
        Ok(())
    }

    fn into_connection(self) -> Connection {
        self.conn
    }
}

/// Append a table's buffered batches via a DuckLake catalog-qualified appender.
/// Isolated as a free function (not a `&mut self` method) so `ProjectFlusher::flush`
/// can call it per-table inside a loop that also borrows `self.buffers` mutably.
fn append_batches(conn: &Connection, table: &str, batches: Vec<RecordBatch>) -> Result<()> {
    // Extract bare table name (strip "lake." prefix for catalog-qualified appender).
    // conn.appender("lake.table") fails — must use appender_to_catalog_and_db.
    let bare_table = table.strip_prefix("lake.").unwrap_or(table);
    let mut appender = conn
        .appender_to_catalog_and_db(bare_table, "lake", "main")
        .with_context(|| format!("failed to create {table} appender"))?;
    for batch in batches {
        if batch.num_rows() > 0 {
            appender
                .append_record_batch(batch)
                .with_context(|| format!("failed to append {table} batch"))?;
        }
    }
    appender
        .flush()
        .with_context(|| format!("failed to flush {table} appender"))?;
    // Appender::drop cleans up
    Ok(())
}

/// Approximate in-memory byte size of an Arrow RecordBatch.
/// Sums the buffer sizes of all arrays (data + validity + offsets).
fn batch_byte_size(batch: &RecordBatch) -> usize {
    let mut total = 0usize;
    for col in batch.columns() {
        total += array_byte_size(col.as_ref());
    }
    total
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

/// Per-project flusher loop. Receives pre-transformed RecordBatches,
/// buffers them, and flushes to DuckDB on timer or row count threshold.
// This task boundary owns six independent lifecycle resources. Wrapping them in
// a one-use context struct would hide its dependencies without reducing them.
#[allow(clippy::too_many_arguments)]
#[instrument(skip_all, fields(project_id = %project_id))]
async fn run_flusher(
    project_id: String,
    writer: Connection,
    mut rx: mpsc::Receiver<FlusherCommand>,
    pool: Arc<ProjectPool>,
    cfg: FlushConfig,
    maintenance: Arc<MaintenanceHealth>,
) {
    let flush_interval = cfg.interval;
    let (writer_tx, writer_rx) = mpsc::channel::<WriterCommand>(PROJECT_CHANNEL_CAPACITY);
    let writer_project_id = project_id.clone();
    let writer_cfg = cfg.clone();
    let writer_maintenance = Arc::clone(&maintenance);
    let writer_slot = Arc::new(parking_lot::Mutex::new(Some(writer)));
    let writer_for_thread = Arc::clone(&writer_slot);
    let writer_handle = match std::thread::Builder::new()
        .name(format!("sigiro-flusher-{project_id}"))
        .spawn(move || {
            let writer = writer_for_thread
                .lock()
                .take()
                .expect("writer is transferred exactly once");
            run_writer_thread(
                &writer_project_id,
                writer,
                writer_rx,
                &writer_cfg,
                &writer_maintenance,
            )
        }) {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(project_id, "failed to spawn writer thread: {error}");
            if let Some(writer) = writer_slot.lock().take() {
                pool.return_writer(&project_id, writer);
            }
            return;
        }
    };
    let mut interval = tokio::time::interval(flush_interval);
    let mut checkpoint_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + DUCKLAKE_MAINTENANCE_INTERVAL,
        DUCKLAKE_MAINTENANCE_INTERVAL,
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    checkpoint_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;

            _ = interval.tick() => {
                pool.touch_project(&project_id);
                // F-25: demoted from debug! to trace! (see flush skipped above).
                tracing::trace!(project_id, "flusher timer tick");
                match writer_tx.try_send(WriterCommand::FlushIfNeeded) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::error!(project_id, "writer thread stopped before timer flush");
                        break;
                    }
                }
            }

            _ = checkpoint_interval.tick() => {
                pool.touch_project(&project_id);
                match writer_tx.try_send(WriterCommand::Checkpoint) {
                    Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::error!(project_id, "writer thread stopped before checkpoint");
                        break;
                    }
                }
            }

            command = rx.recv() => {
                match command {
                    Some(FlusherCommand::Ingest(msg)) => {
                        pool.touch_project(&project_id);
                        tracing::debug!(project_id, "flusher received message");
                        if writer_tx.send(WriterCommand::Ingest(msg)).await.is_err() {
                            tracing::error!(project_id, "writer thread stopped before ingest");
                            break;
                        }
                    }
                    Some(FlusherCommand::RetentionCleanup { days, ack }) => {
                        pool.touch_project(&project_id);
                        if writer_tx.send(WriterCommand::RetentionCleanup { days, ack }).await.is_err() {
                            tracing::error!(project_id, "writer thread stopped before retention cleanup");
                            break;
                        }
                    }
                    None => {
                        if writer_tx.send(WriterCommand::Shutdown).await.is_err() {
                            tracing::debug!(project_id, "writer already stopped during shutdown");
                        }
                        tracing::info!(project_id, "flusher task exiting (channel closed)");
                        break;
                    }
                }
            }
        }
    }

    drop(writer_tx);
    match tokio::task::spawn_blocking(move || writer_handle.join()).await {
        Ok(Ok(writer)) => {
            pool.return_writer(&project_id, writer);
        }
        Ok(Err(_panic)) => {
            tracing::error!(project_id, "writer thread panicked");
        }
        Err(error) => {
            tracing::error!(project_id, "writer join task failed: {error}");
        }
    }
}

fn run_writer_thread(
    project_id: &str,
    writer: Connection,
    mut rx: mpsc::Receiver<WriterCommand>,
    cfg: &FlushConfig,
    maintenance: &MaintenanceHealth,
) -> Connection {
    let mut flusher = ProjectFlusher::new(writer, cfg);
    let mut pending = None;
    while let Some(command) = pending.take().or_else(|| rx.blocking_recv()) {
        match command {
            WriterCommand::Ingest(msg) => {
                buffer_msg(&mut flusher, *msg);
                while !flusher.reached_flush_threshold() {
                    match rx.try_recv() {
                        Ok(WriterCommand::Ingest(msg)) => buffer_msg(&mut flusher, *msg),
                        Ok(command) => {
                            pending = Some(command);
                            break;
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }
                if flusher.should_flush()
                    && let Err(e) = flusher.flush(project_id)
                {
                    tracing::error!(project_id, "flush failed: {e:#}");
                }
            }
            WriterCommand::FlushIfNeeded => {
                if flusher.should_flush()
                    && let Err(e) = flusher.flush(project_id)
                {
                    tracing::error!(project_id, "timer flush failed: {e:#}");
                }
            }
            WriterCommand::Checkpoint => {
                // #91: a failing CHECKPOINT used to only `warn!` and continue,
                // so an unwritable/unreachable storage backend could have the
                // catalog absorbing telemetry forever while nothing was ever
                // materialised into Parquet. Every outcome is now recorded, and
                // sustained failure sheds ingest (507 / resource_exhausted) at
                // the top of `MsgRouter::send`.
                match flusher.checkpoint(project_id) {
                    Ok(()) => {
                        if maintenance.record_checkpoint_success(project_id) {
                            tracing::info!(
                                project_id,
                                "DuckLake CHECKPOINT recovered — ingest re-enabled for this project"
                            );
                        }
                    }
                    Err(e) => {
                        let (failures, tripped) = maintenance.record_checkpoint_failure(project_id);
                        if tripped {
                            tracing::error!(
                                project_id,
                                failures,
                                "DuckLake CHECKPOINT failed {failures} consecutive times — REJECTING ingest for this project until a checkpoint succeeds: {e:#}"
                            );
                        } else {
                            tracing::warn!(
                                project_id,
                                failures,
                                "DuckLake CHECKPOINT failed ({failures} consecutive): {e:#}"
                            );
                        }
                    }
                }
            }
            WriterCommand::RetentionCleanup { days, ack } => {
                let result = flusher
                    .retention_cleanup(project_id, days)
                    .map_err(|e| format!("{e:#}"));
                if ack.send(result).is_err() {
                    tracing::debug!(project_id, "retention cleanup receiver dropped");
                }
            }
            WriterCommand::Shutdown => break,
        }
    }

    if let Err(e) = flusher.flush(project_id) {
        tracing::error!(project_id, "final flush failed: {e:#}");
    }
    // A successful flush commits the final DuckLake transaction. CHECKPOINT is
    // compaction/retention maintenance, not the durability boundary, and remains
    // owned by ProjectPool after the writer has been returned without lock overlap.
    flusher.into_connection()
}

/// Route an IngestMsg into the flusher's signal-keyed buffers.
fn buffer_msg(flusher: &mut ProjectFlusher, msg: IngestMsg) {
    match msg {
        IngestMsg::Traces { batch, .. } => flusher.buffer(SignalTable::Spans, batch),
        IngestMsg::Logs { batch, .. } => flusher.buffer(SignalTable::Logs, batch),
        IngestMsg::Metrics { batches, .. } => flusher.buffer_metrics(batches),
        IngestMsg::Profiles { batch, .. } => flusher.buffer(SignalTable::Profiles, batch),
    }
}
