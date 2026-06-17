use crate::otlp::MetricBatches;
use anyhow::{Context, Result};
use arrow::array::{Array, StringArray, UInt64Array};
use arrow::record_batch::RecordBatch;
use dashmap::DashMap;
use duckdb::Connection;
use parking_lot::Mutex;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing_attributes::instrument;

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
}

/// Per-project channel capacity. When full, messages are dropped
/// (the OTLP SDK retries). Better to drop one project's data than block all.
const PROJECT_CHANNEL_CAPACITY: usize = 10_000;

/// Backpressure policy for the per-project channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackpressurePolicy {
    /// Block the sender until space is available. Guarantees no data loss but may
    /// cause backpressure on the OTLP client.
    Block,
    /// Drop the oldest message when the channel is full. This is a lossy policy —
    /// data will be discarded when the channel buffer fills up.
    DropOldest,
}

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
        let backpressure = match crate::config::BACKPRESSURE_POLICY
            .value()
            .as_deref()
            .unwrap_or("block")
        {
            "block" => BackpressurePolicy::Block,
            "drop-oldest" => BackpressurePolicy::DropOldest,
            other => panic!(
                "unknown SIGIRO_BACKPRESSURE_POLICY: {other}. Expected 'block' or 'drop-oldest'"
            ),
        };
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
    project_senders: DashMap<String, (mpsc::Sender<IngestMsg>, JoinHandle<()>)>,
    drop_count: AtomicUsize,
}

impl MsgRouter {
    pub fn new(pool: Arc<ProjectPool>) -> Self {
        Self::with_config(pool, FlushConfig::from_env())
    }

    pub fn with_config(pool: Arc<ProjectPool>, flush_config: FlushConfig) -> Self {
        Self {
            pool,
            flush_config,
            project_senders: DashMap::new(),
            drop_count: AtomicUsize::new(0),
        }
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

    /// Route a message to the appropriate per-project flusher.
    /// Creates the flusher task on first message for a project.
    /// Applies backpressure via bounded channel — sender blocks when full
    /// rather than silently dropping data.
    /// Explicitly create and register a flusher for a project.
    /// DEPRECATED: Prefer using send() which lazily creates the flusher.
    /// Caller (DI container / server setup) can invoke this for eager initialization.
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
                let conn = self.pool.create_project(project_id).map_err(|_| {
                    RouterError::ConnectionFailed {
                        project_id: project_id.to_string(),
                    }
                })?;
                let pool = Arc::clone(&self.pool);
                let cfg = self.flush_config.clone();

                let (tx, rx) = mpsc::channel::<IngestMsg>(PROJECT_CHANNEL_CAPACITY);
                let pid_spawn = pid.clone();
                tracing::info!(project_id = %pid, "register_flusher: spawning new flusher task");
                let handle =
                    tokio::spawn(async move { run_flusher(&pid_spawn, conn, rx, pool, cfg).await });

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
                match sender.send(msg).await {
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
            BackpressurePolicy::DropOldest => match sender.try_send(msg) {
                Ok(()) => Ok(()),
                Err(mpsc::error::TrySendError::Full(_)) => {
                    let count = self.drop_count.fetch_add(1, Ordering::Relaxed) + 1;
                    tracing::warn!(
                        project_id = %project_id_for_err,
                        total_dropped = count,
                        "channel full — DATA LOSS: dropping message (BackpressurePolicy::DropOldest)"
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
            },
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
    conn: Arc<Mutex<Connection>>,
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
    fn new(conn: Arc<Mutex<Connection>>, cfg: &FlushConfig) -> Self {
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
    /// Acquires the mutex ONCE for the entire flush, writes all tables, releases.
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
        let conn = self.conn.lock();

        for (signal, buf) in &mut self.buffers {
            if buf.is_empty() {
                continue;
            }
            let table = &self.table_names[signal];
            // Extract bare table name (strip "lake." prefix for catalog-qualified appender).
            // conn.appender("lake.table") fails — must use appender_to_catalog_and_db.
            let bare_table = table.strip_prefix("lake.").unwrap_or(table);
            let mut appender = conn
                .appender_to_catalog_and_db(bare_table, "lake", "main")
                .with_context(|| format!("failed to create {table} appender"))?;
            for batch in buf.drain(..) {
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
        Ok(())
    }
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
    project_id: &str,
    conn: Arc<Mutex<Connection>>,
    mut rx: mpsc::Receiver<IngestMsg>,
    pool: Arc<ProjectPool>,
    cfg: FlushConfig,
) {
    let flush_interval = cfg.interval;
    let mut flusher = ProjectFlusher::new(conn.clone(), &cfg);
    let mut interval = tokio::time::interval(flush_interval);
    let mut checkpoint_interval = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            biased;

            _ = interval.tick() => {
                pool.touch_project(project_id);
                // F-25: demoted from debug! to trace! (see flush skipped above).
                tracing::trace!(project_id, "flusher timer tick");
                if flusher.should_flush() {
                    if let Err(e) = flusher.flush(project_id) {
                        tracing::error!(project_id, "timer flush failed: {e:#}");
                    }
                }
            }

            _ = checkpoint_interval.tick() => {
                pool.touch_project(project_id);
                let c = Arc::clone(&conn);
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    let guard = c.lock();
                    guard.execute_batch("CHECKPOINT lake;")
                }).await {
                    tracing::warn!(project_id, "DuckLake CHECKPOINT task failed: {e}");
                } else {
                    tracing::debug!(project_id, "DuckLake CHECKPOINT completed");
                }
            }

            msg = rx.recv() => {
                match msg {
                    Some(msg) => {
                        pool.touch_project(project_id);
                        tracing::debug!(project_id, "flusher received message");
                        buffer_msg(&mut flusher, msg);
                        if flusher.should_flush() {
                            if let Err(e) = flusher.flush(project_id) {
                                tracing::error!(project_id, "flush failed: {e:#}");
                            }
                        }
                    }
                    None => {
                        if let Err(e) = flusher.flush(project_id) {
                            tracing::error!(project_id, "final flush failed: {e:#}");
                        }
                        tracing::info!(project_id, "flusher task exiting (channel closed)");
                        break;
                    }
                }
            }
        }
    }

    // Final CHECKPOINT before exit so data is persisted.
    let c = Arc::clone(&conn);
    if let Err(e) = tokio::task::spawn_blocking(move || {
        let guard = c.lock();
        guard.execute_batch("CHECKPOINT lake;")
    })
    .await
    {
        tracing::warn!(project_id, "final DuckLake CHECKPOINT failed: {e}");
    }
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
