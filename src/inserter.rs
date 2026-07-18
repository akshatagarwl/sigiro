use crate::otlp::MetricBatches;
use anyhow::{Context, Result};
use arrow::array::{Array, StringArray, UInt64Array};
use arrow::record_batch::RecordBatch;
use dashmap::DashMap;
use duckdb::Connection;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle as TokioJoinHandle;
use tracing_attributes::instrument;

use crate::config::BackpressurePolicy;
use crate::drain::DrainTree;
use crate::pool::ProjectPool;
use crate::types::{IngestMsg, SignalTable};

/// Errors from the ingest router.
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// Channel closed — flusher task exited unexpectedly.
    #[error("channel closed for project {project_id}")]
    ChannelClosed { project_id: String },
    /// Failed to get or create a DuckDB connection.
    #[error("connection failed for project {project_id}")]
    ConnectionFailed { project_id: String },
    /// On-disk footprint reached the configured `SIGIRO_MAX_DISK_BYTES` cap —
    /// ingest is rejected to keep the box from filling its disk. Maps to HTTP
    /// 507 / gRPC `resource_exhausted`, distinct from an internal 500.
    #[error("disk capacity exceeded — ingest rejected to protect the box")]
    DiskCapacityExceeded,
}

/// Box-level disk-capacity gate, shared (`Arc`) between the ingest router —
/// which reads `over_cap` at the top of `send` to reject when tripped — and
/// the storage-maintenance task, which samples `SIGIRO_DATA_DIR` and trips/
/// clears `over_cap` with hysteresis. `cap == 0` disables the gate entirely.
#[derive(Debug)]
pub struct DiskCapacityState {
    /// `SIGIRO_MAX_DISK_BYTES`. 0 disables the gate.
    pub cap: u64,
    /// Most recent sampled on-disk byte size of `SIGIRO_DATA_DIR`.
    pub sampled_bytes: AtomicU64,
    /// Whether ingest is currently rejected (cap breached, not yet recovered).
    pub over_cap: AtomicBool,
    /// Single-flight guard for the self-heal retention cleanup kicked on the
    /// OFF→ON transition.
    pub cleanup_in_flight: AtomicBool,
}

impl DiskCapacityState {
    pub fn new(cap: u64) -> Arc<Self> {
        Arc::new(Self {
            cap,
            sampled_bytes: AtomicU64::new(0),
            over_cap: AtomicBool::new(false),
            cleanup_in_flight: AtomicBool::new(false),
        })
    }

    /// A disabled (`cap == 0`) gate — the no-op default for constructors/tests.
    pub fn disabled() -> Arc<Self> {
        Self::new(0)
    }

    /// True when the gate is armed AND currently tripped. Cheap enough to call
    /// on the ingest hot path.
    pub fn is_over_cap(&self) -> bool {
        self.cap > 0 && self.over_cap.load(Ordering::Relaxed)
    }

    /// Fold a fresh byte sample into the gate with hysteresis: trip when
    /// `bytes >= cap`, and clear only once `bytes <= cap * 9 / 10`. Returns
    /// `Some(true)` on an OFF→ON transition, `Some(false)` on ON→OFF, and
    /// `None` when the state is unchanged. Pure state logic — unit-tested.
    ///
    /// `reliable` is `false` when the disk walk hit an unreadable entry, so the
    /// byte total is an UNDER-count. Tripping is still allowed on an unreliable
    /// sample (an under-count already over the cap is definitely over —
    /// rejecting is fail-safe), but CLEARING is not: an under-count must never
    /// re-open ingest while real usage may still be over.
    ///
    /// MUST be called from a single serialized context (the one maintenance
    /// loop). The load-then-store is not atomic across concurrent callers, so
    /// two overlapping samplers could let a stale under-cap sample clobber a
    /// newer over-cap one — the maintenance task keeps exactly one sampler.
    pub fn apply_sample(&self, bytes: u64, reliable: bool) -> Option<bool> {
        if self.cap == 0 {
            return None;
        }
        self.sampled_bytes.store(bytes, Ordering::Relaxed);
        let was_over = self.over_cap.load(Ordering::Relaxed);
        if !was_over && bytes >= self.cap {
            self.over_cap.store(true, Ordering::Relaxed);
            return Some(true);
        }
        // u128 keeps 90% exact (no `cap/10*9` truncation that breaks small caps,
        // e.g. cap=9 would clear only at 0 bytes) while never overflowing u64.
        let clear_threshold = ((self.cap as u128 * 9) / 10) as u64;
        if was_over && reliable && bytes <= clear_threshold {
            self.over_cap.store(false, Ordering::Relaxed);
            return Some(false);
        }
        None
    }
}

/// Per-project channel capacity. When full, messages are dropped
/// (the OTLP SDK retries). Better to drop one project's data than block all.
const PROJECT_CHANNEL_CAPACITY: usize = 10_000;

/// Flusher configuration derived from env vars at startup.
#[derive(Debug, Clone)]
pub struct FlushConfig {
    pub row_threshold: usize,
    pub byte_threshold: usize,
    pub interval: Duration,
    pub backpressure: BackpressurePolicy,
}

impl FlushConfig {
    pub fn from_env() -> Self {
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
    drop_count: AtomicUsize,
    disk: Arc<DiskCapacityState>,
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

    pub fn with_config(pool: Arc<ProjectPool>, flush_config: FlushConfig) -> Self {
        Self::with_config_and_disk(pool, flush_config, DiskCapacityState::disabled())
    }

    /// Build a router with the disk-capacity gate enabled from config. Used by
    /// the production server (`main::run_serve`); tests/other callers get a
    /// disabled gate via `new`/`with_config`.
    pub fn new_with_disk(pool: Arc<ProjectPool>, disk: Arc<DiskCapacityState>) -> Self {
        Self::with_config_and_disk(pool, FlushConfig::from_env(), disk)
    }

    pub fn with_config_and_disk(
        pool: Arc<ProjectPool>,
        flush_config: FlushConfig,
        disk: Arc<DiskCapacityState>,
    ) -> Self {
        Self {
            pool,
            flush_config,
            project_senders: DashMap::new(),
            drop_count: AtomicUsize::new(0),
            disk,
        }
    }

    /// Shared handle to the disk-capacity gate, so the storage-maintenance task
    /// can sample and trip/clear the same state the ingest gate reads.
    pub fn disk_state(&self) -> Arc<DiskCapacityState> {
        Arc::clone(&self.disk)
    }

    /// Explicit lifecycle start. Logs activation and ensures the router is ready.
    /// Callers should invoke this once after `new()` before accepting traffic.
    pub fn start(&self) {
        tracing::info!("MsgRouter started — accepting ingest messages");
    }

    /// Return true if any flusher task has panicked or exited unexpectedly.
    pub fn has_dead_flushers(&self) -> bool {
        self.project_senders
            .iter()
            .any(|entry| entry.value().1.is_finished())
    }

    pub fn dropped_messages(&self) -> usize {
        self.drop_count.load(Ordering::Relaxed)
    }

    pub async fn run_retention_cleanup(&self, days: u32) -> Result<()> {
        for project_id in self.pool.project_ids() {
            self.ensure_flusher(&project_id)
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

    /// Eagerly create and register a flusher for a project. `send()` does this
    /// lazily on first message; call this when you want the flusher (and its
    /// connection) ready before any data arrives.
    #[instrument(skip_all, fields(project_id = %project_id))]
    pub fn register_flusher(&self, project_id: &str) -> Result<(), RouterError> {
        self.ensure_flusher(project_id)
    }

    /// Ensure a flusher exists for the project (lazy creation on first use).
    /// Creates the connection and spawns the flusher task if not yet registered.
    /// Thread-safe via DashMap entry API. No side effects if already initialized.
    fn ensure_flusher(&self, project_id: &str) -> Result<(), RouterError> {
        use dashmap::mapref::entry::Entry;

        let pid = project_id.to_string();
        match self.project_senders.entry(pid.clone()) {
            Entry::Occupied(_) => {
                // Already registered — return Ok, caller can send immediately.
                tracing::debug!(project_id = %pid, "register_flusher: flusher already exists for project");
                Ok(())
            }
            Entry::Vacant(e) => {
                self.pool.create_project(project_id).map_err(|_| {
                    RouterError::ConnectionFailed {
                        project_id: project_id.to_string(),
                    }
                })?;
                let writer = self.pool.take_writer(project_id).map_err(|_| {
                    RouterError::ConnectionFailed {
                        project_id: project_id.to_string(),
                    }
                })?;
                let pool = Arc::clone(&self.pool);
                let cfg = self.flush_config.clone();

                let (tx, rx) = mpsc::channel::<FlusherCommand>(PROJECT_CHANNEL_CAPACITY);
                let pid_spawn = pid.clone();
                tracing::info!(project_id = %pid, "register_flusher: spawning new flusher task");
                let handle = tokio::spawn(async move {
                    run_flusher(pid_spawn, writer, rx, pool, cfg).await;
                });

                e.insert((tx, handle));
                tracing::info!(project_id = %pid, "register_flusher: inserted new sender into project_senders");
                Ok(())
            }
        }
    }

    /// Route a message to the appropriate per-project flusher.
    /// Flusher MUST be registered first via register_flusher().
    /// Returns Err if no flusher is registered for this project.
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
        let project_id_for_err = project_id.clone();

        // Lazy creation: ensure flusher exists before sending
        self.ensure_flusher(&project_id)?;

        let entry =
            self.project_senders
                .get(&project_id)
                .ok_or_else(|| RouterError::ConnectionFailed {
                    project_id: project_id.clone(),
                })?;

        let sender = &entry.value().0;

        match self.flush_config.backpressure {
            BackpressurePolicy::Block => {
                match sender.send(FlusherCommand::Ingest(Box::new(msg))).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        tracing::error!(project_id = %project_id_for_err, "send failed: channel error: {:?}", e);
                        drop(entry);
                        // Remove the entry and check if the flusher task panicked.
                        if let Some((_, (_, handle))) = self.project_senders.remove(&project_id) {
                            if handle.is_finished() {
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
                        }
                        Err(RouterError::ChannelClosed {
                            project_id: project_id_for_err,
                        })
                    }
                }
            }
            BackpressurePolicy::DropNewest => {
                match sender.try_send(FlusherCommand::Ingest(Box::new(msg))) {
                    Ok(()) => Ok(()),
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
                        drop(entry);
                        if let Some((_, (_, handle))) = self.project_senders.remove(&project_id) {
                            if handle.is_finished() {
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
                        }
                        Err(RouterError::ChannelClosed {
                            project_id: project_id_for_err,
                        })
                    }
                }
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

        let mut failures = Vec::new();
        for (project_id, mut handle) in flushers {
            match tokio::time::timeout(Duration::from_secs(5), &mut handle).await {
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
                        "{project_id}: flusher did not stop within 5 seconds"
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
/// and will re-emit. See www/content/docs/decisions/001-wal-not-needed.md for the full analysis.
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
    pending_templates: HashMap<u64, String>,
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
            pending_templates: HashMap::new(),
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
    /// Run DRAIN on each log body and append `template_id` and `template_values` columns.
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
        for i in 0..batch.num_rows() {
            let body = body_col.value(i);
            let (template, id, values) = self.drain.classify(body);
            template_ids.push(id);
            // Store extracted values as JSON array so the original log can be reconstructed.
            let values_json =
                serde_json::to_string(&values).expect("values must serialize to JSON");
            template_values.push(values_json);
            self.pending_templates.entry(id).or_insert(template);
        }

        // Append template_id and template_values columns to the existing batch
        let id_array = std::sync::Arc::new(UInt64Array::from(template_ids));
        let values_array = std::sync::Arc::new(StringArray::from(template_values));
        let mut columns: Vec<std::sync::Arc<dyn Array>> = batch.columns().to_vec();
        columns.push(id_array);
        columns.push(values_array);

        let mut fields: Vec<arrow::datatypes::Field> = batch
            .schema()
            .fields()
            .iter()
            .map(|f| f.as_ref().clone())
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
            || self.total_rows >= self.row_threshold
            || self.total_bytes >= self.byte_threshold
            || self.last_flush.elapsed() >= self.flush_interval
    }

    /// Flush all buffered batches to DuckDB using appenders per table.
    /// Uses appender_to_catalog_and_db for DuckLake catalog-qualified table access.
    #[tracing::instrument(skip_all, fields(project_id = %project_id, rows = self.total_rows))]
    fn flush(&mut self, project_id: &str) -> Result<()> {
        if self.total_rows == 0 {
            // F-25: demoted from debug! to trace! — fires every flush interval
            // per project (default 5s) and was polluting `RUST_LOG=debug` runs
            // with 12 lines/minute per project of "nothing happened" noise.
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

        // Flush new log templates to the lookup table
        if !self.pending_templates.is_empty() {
            let tpl_table = format!(
                "{}sigiro_log_templates",
                self.table_names
                    .get(&SignalTable::Logs)
                    .map_or("", |t| { t.strip_suffix("sigiro_logs").unwrap_or("") })
            );
            for (id, text) in self.pending_templates.drain() {
                // Dedup: skip insert if template already exists.
                // DuckLake does not support UNIQUE constraints, so we check manually.
                let exists: bool = conn
                    .query_row(
                        &format!("SELECT 1 FROM {tpl_table} WHERE template_id = ? LIMIT 1"),
                        duckdb::params![id],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);
                if exists {
                    continue;
                }
                if let Err(e) = conn.execute(
                    &format!("INSERT INTO {tpl_table} (template_id, template_text) VALUES (?, ?)"),
                    duckdb::params![id, text],
                ) {
                    tracing::warn!(
                        project_id,
                        template_id = id,
                        "failed to insert log template: {e}"
                    );
                }
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
        // DELETE alone reclaims ZERO disk in DuckLake — it only writes
        // delete-files, so on-disk Parquet stays (usage can even grow).
        // Expiring the aged snapshots then removing the now-unreferenced data
        // files is what frees bytes. Catalog-wide, run ONCE here after ALL the
        // per-table DELETEs (never inside the loop).
        reclaim_ducklake_disk(&self.conn)?;
        tracing::info!(project_id, days, "retention cleanup completed");
        Ok(())
    }

    fn into_connection(self) -> Connection {
        self.conn
    }
}

/// Reclaim on-disk DuckLake Parquet after retention DELETEs. A DELETE only
/// writes delete-files, so disk usage does not drop (it can even grow) until
/// the aged snapshots are expired and the now-unreferenced data files are
/// removed. Catalog-wide (catalog `lake`), run ONCE after all per-table
/// DELETEs. Works under sigiro's local lockdown (`allowed_directories` +
/// `enable_external_access=false`). Only WHOLLY-aged files are freed — fine,
/// since flushes are time-ordered and the disk cap backstops the rest.
/// The exact sequence was proven against duckdb ducklake d318a545.
fn reclaim_ducklake_disk(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "SELECT * FROM ducklake_expire_snapshots('lake', older_than => now(), dry_run => false);\n\
         SELECT * FROM ducklake_cleanup_old_files('lake', cleanup_all => true, dry_run => false);",
    )
    .context("DuckLake disk reclaim failed (expire_snapshots + cleanup_old_files)")
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
#[instrument(skip_all, fields(project_id = %project_id))]
async fn run_flusher(
    project_id: String,
    writer: Connection,
    mut rx: mpsc::Receiver<FlusherCommand>,
    pool: Arc<ProjectPool>,
    cfg: FlushConfig,
) {
    let flush_interval = cfg.interval;
    let (writer_tx, writer_rx) = mpsc::channel::<WriterCommand>(PROJECT_CHANNEL_CAPACITY);
    let writer_project_id = project_id.clone();
    let writer_cfg = cfg.clone();
    let writer_handle = match std::thread::Builder::new()
        .name(format!("sigiro-flusher-{project_id}"))
        .spawn(move || run_writer_thread(&writer_project_id, writer, writer_rx, &writer_cfg))
    {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(project_id, "failed to spawn writer thread: {error}");
            return;
        }
    };
    let mut interval = tokio::time::interval(flush_interval);
    let mut checkpoint_interval = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            biased;

            _ = interval.tick() => {
                pool.touch_project(&project_id);
                // F-25: demoted from debug! to trace! (see flush skipped above).
                tracing::trace!(project_id, "flusher timer tick");
                if writer_tx.send(WriterCommand::FlushIfNeeded).await.is_err() {
                    tracing::error!(project_id, "writer thread stopped before timer flush");
                    break;
                }
            }

            _ = checkpoint_interval.tick() => {
                pool.touch_project(&project_id);
                if writer_tx.send(WriterCommand::Checkpoint).await.is_err() {
                    tracing::error!(project_id, "writer thread stopped before checkpoint");
                    break;
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
) -> Connection {
    let mut flusher = ProjectFlusher::new(writer, cfg);
    while let Some(command) = rx.blocking_recv() {
        match command {
            WriterCommand::Ingest(msg) => {
                buffer_msg(&mut flusher, *msg);
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
                if let Err(e) = flusher.checkpoint(project_id) {
                    tracing::warn!(project_id, "{e:#}");
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
    if let Err(e) = flusher.checkpoint(project_id) {
        tracing::warn!(project_id, "final DuckLake CHECKPOINT failed: {e:#}");
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::Int32Array;
    use arrow::datatypes::{DataType, Field, Schema};

    // Copied from tests/ducklake_appender_test.rs — no attach helper is exposed
    // via crate::storage, and this test needs private ProjectFlusher access
    // so it must live in-module rather than in tests/.
    fn extension_dir() -> String {
        std::env::var("SIGIRO_EXTENSION_DIR").unwrap_or_else(|_| {
            std::env::var("HOME")
                .map(|home| format!("{home}/.duckdb/extensions"))
                .unwrap_or_else(|_| "/tmp/sigiro-test-extensions".to_string())
        })
    }

    fn attach_ducklake(conn: &Connection, dir: &std::path::Path) {
        let data_path = dir.to_string_lossy();
        conn.execute_batch(&format!("SET home_directory='{data_path}';"))
            .expect("set home_directory");
        conn.execute_batch(&format!("SET extension_directory='{}';", extension_dir()))
            .expect("set extension_directory");
        conn.execute_batch("INSTALL ducklake; LOAD ducklake;")
            .expect("install/load ducklake");
        conn.execute_batch(&format!(
            "ATTACH 'ducklake:{data_path}/catalog.ducklake' AS lake"
        ))
        .expect("attach DuckDB file catalog");
    }

    /// A batch matching the test-only `lake.sigiro_metrics_gauge (id INTEGER)` table.
    fn int_batch(n: usize) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int32, false)]));
        RecordBatch::try_new(
            schema,
            vec![Arc::new(Int32Array::from(
                (0..n as i32).collect::<Vec<_>>(),
            ))],
        )
        .expect("valid int batch")
    }

    fn test_flush_config() -> FlushConfig {
        FlushConfig {
            row_threshold: 1_000_000,
            byte_threshold: 1_000_000_000,
            interval: Duration::from_secs(3600),
            backpressure: BackpressurePolicy::Block,
        }
    }

    #[test]
    fn disk_cap_hysteresis_trips_at_cap_clears_at_90_percent() {
        let disk = DiskCapacityState::new(1000);
        assert!(!disk.is_over_cap(), "starts under cap");

        // Below cap: no transition, stays off.
        assert_eq!(disk.apply_sample(500, true), None);
        assert!(!disk.is_over_cap());

        // Reaching the cap trips it (OFF->ON).
        assert_eq!(disk.apply_sample(1000, true), Some(true));
        assert!(disk.is_over_cap());

        // Still over 90% (900): stays tripped, no transition.
        assert_eq!(disk.apply_sample(950, true), None);
        assert_eq!(disk.apply_sample(901, true), None);
        assert!(disk.is_over_cap());

        // An UNRELIABLE (under-counted) sample must NOT clear, even below 90%.
        assert_eq!(disk.apply_sample(500, false), None);
        assert!(disk.is_over_cap(), "unreliable walk never re-opens ingest");

        // Dropping to <=90% on a reliable sample clears it (ON->OFF).
        assert_eq!(disk.apply_sample(900, true), Some(false));
        assert!(!disk.is_over_cap());

        // A brief spike below the cap does not re-trip.
        assert_eq!(disk.apply_sample(999, true), None);
        assert!(!disk.is_over_cap());
    }

    #[test]
    fn disk_cap_small_cap_clear_threshold_is_exact() {
        // Regression: `cap/10*9` truncated to 0 for cap=9 (could only clear at
        // 0 bytes → permanent deadlock). u128 math gives the exact 90% floor.
        let disk = DiskCapacityState::new(9);
        assert_eq!(disk.apply_sample(9, true), Some(true));
        // 90% of 9 = 8.1 → floor 8; usage 8 must clear, not require 0.
        assert_eq!(disk.apply_sample(8, true), Some(false));
        assert!(!disk.is_over_cap());
    }

    #[test]
    fn disk_cap_zero_is_disabled() {
        let disk = DiskCapacityState::disabled();
        assert_eq!(disk.apply_sample(u64::MAX, true), None);
        assert!(!disk.is_over_cap());
    }

    fn count_parquet_files(dir: &std::path::Path) -> usize {
        walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "parquet")
            })
            .count()
    }

    /// An `n`-row `(id, ts)` batch with every `ts` set `days_ago` days before
    /// now. Batches must be large enough to exceed DuckLake's inline-row limit
    /// so the appender writes Parquet rather than inlining into the catalog.
    fn day_batch(days_ago: i64, n: usize) -> RecordBatch {
        use arrow::array::TimestampMicrosecondArray;
        use arrow::datatypes::TimeUnit;
        let now_us = {
            let odt = time::OffsetDateTime::now_utc();
            odt.unix_timestamp() * 1_000_000 + i64::from(odt.microsecond())
        };
        let ts = now_us - days_ago * 86_400 * 1_000_000;
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Int32, false),
            Field::new(
                "ts",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                false,
            ),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from((0..n as i32).collect::<Vec<_>>())),
                Arc::new(TimestampMicrosecondArray::from(vec![ts; n])),
            ],
        )
        .expect("valid day batch")
    }

    /// Proves the DELETE-then-reclaim sequence actually frees on-disk Parquet,
    /// not just deletes rows. DELETE alone leaves (or grows) files;
    /// `reclaim_ducklake_disk` (expire_snapshots + cleanup_old_files) is what
    /// drops the file count. Real DuckLake, mirrors ducklake_appender_test.rs.
    #[test]
    fn retention_reclaims_ducklake_disk() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        attach_ducklake(&conn, dir.path());
        conn.execute_batch("CREATE TABLE lake.t (id INTEGER, ts TIMESTAMP)")
            .expect("create table");

        // 30 separate appender flushes = 30 committed data files, 3000 rows
        // each, one day per batch going back 0..29 days. Batches are large so
        // the appender writes Parquet rather than inlining into the catalog.
        for i in 0..30 {
            let mut appender = conn
                .appender_to_catalog_and_db("t", "lake", "main")
                .expect("appender");
            appender
                .append_record_batch(day_batch(i64::from(i), 3000))
                .expect("append");
            appender.flush().expect("flush");
        }
        // Materialize inlined/WAL data into Parquet, exactly as the production
        // flusher's periodic `CHECKPOINT lake` does — otherwise small writes
        // stay inlined in the catalog and there are no files to reclaim.
        conn.execute_batch("CHECKPOINT lake;").expect("checkpoint");
        let files_before = count_parquet_files(dir.path());
        let rows_before: i64 = conn
            .query_row("SELECT count(*) FROM lake.t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows_before, 30 * 3000, "all rows present before retention");
        assert!(
            files_before >= 30,
            "expected >=30 data files from 30 commits, got {files_before}"
        );

        // Retention: drop everything older than 7 days (rows for days 8..=29).
        conn.execute_batch("DELETE FROM lake.t WHERE ts < now()::TIMESTAMP - INTERVAL '7 days'")
            .expect("delete old rows");

        // DELETE alone must NOT reclaim disk — the file count does not drop.
        let files_after_delete = count_parquet_files(dir.path());
        assert!(
            files_after_delete >= files_before,
            "DELETE alone must not reclaim files (before={files_before}, after_delete={files_after_delete})"
        );

        // The reclaim sequence under test.
        reclaim_ducklake_disk(&conn).expect("reclaim should succeed");

        let files_after = count_parquet_files(dir.path());
        let rows_after: i64 = conn
            .query_row("SELECT count(*) FROM lake.t", [], |r| r.get(0))
            .unwrap();

        // Days 8..=29 are unambiguously older than 7 days and gone; day 7 sits
        // exactly on the cutoff, so 7 or 8 days' worth of rows may remain.
        assert!(
            (7 * 3000..=8 * 3000).contains(&rows_after),
            "retention should leave ~7 days of rows, got {rows_after}"
        );
        assert!(
            files_after < files_after_delete,
            "reclaim must drop on-disk Parquet (before_reclaim={files_after_delete}, after_reclaim={files_after})"
        );
    }

    #[test]
    fn flush_isolates_per_table_failures() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        attach_ducklake(&conn, dir.path());

        // Deliberately create ONLY the gauge table — sigiro_spans is missing,
        // so its appender creation fails.
        conn.execute_batch("CREATE TABLE lake.sigiro_metrics_gauge (id INTEGER)")
            .expect("create gauge table");

        let cfg = test_flush_config();
        let mut flusher = ProjectFlusher::new(conn, &cfg);

        // Spans sorts before MetricsGauge in the BTreeMap (SignalTable::Spans
        // is the first variant), so the old code's early-return on the Spans
        // error would have starved MetricsGauge entirely.
        flusher.buffer(SignalTable::Spans, int_batch(5));
        flusher.buffer(SignalTable::MetricsGauge, int_batch(7));

        let err = flusher
            .flush("test_project")
            .expect_err("flush should fail: sigiro_spans table does not exist");
        assert!(
            format!("{err:#}").contains("sigiro_spans"),
            "error should mention the failing table sigiro_spans: {err:#}"
        );

        // The healthy MetricsGauge table must still have been written despite
        // the Spans failure — this assertion fails on the old drain-based code.
        let count: i64 = flusher
            .conn
            .query_row(
                "SELECT count(*) FROM lake.sigiro_metrics_gauge",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 7,
            "gauge batch should have been flushed despite the spans failure"
        );

        // Buffers drain and counters reset even though one table failed.
        assert!(
            flusher.buffers.values().all(Vec::is_empty),
            "all buffers should be empty after flush"
        );
        assert_eq!(flusher.total_rows, 0, "total_rows should reset after flush");
        assert_eq!(
            flusher.total_bytes, 0,
            "total_bytes should reset after flush"
        );

        // Recovery: the spans buffer is now empty so it's skipped (not
        // reattempted), and a further gauge flush succeeds cleanly.
        flusher.buffer(SignalTable::MetricsGauge, int_batch(3));
        flusher
            .flush("test_project")
            .expect("second flush should succeed once the failing table is idle");

        let count: i64 = flusher
            .conn
            .query_row(
                "SELECT count(*) FROM lake.sigiro_metrics_gauge",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 10,
            "gauge count should increment after recovery flush"
        );
    }
}
