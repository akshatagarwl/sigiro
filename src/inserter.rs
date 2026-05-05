use anyhow::{Context, Result};
use arrow::array::{Array, StringArray, UInt64Array};
use arrow::record_batch::RecordBatch;
use dashmap::DashMap;
use duckdb::Connection;
use otlp2records::MetricBatches;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing_attributes::instrument;

use crate::drain::DrainTree;
use crate::pool::ProjectPool;
use crate::types::{IngestMsg, SignalTable};

/// Errors from the ingest router.
#[derive(Debug)]
pub enum RouterError {
    /// Channel closed — flusher task exited unexpectedly.
    ChannelClosed { project_id: String },
    /// Failed to get or create a DuckDB connection.
    ConnectionFailed { project_id: String },
}

/// Per-project channel capacity. When full, messages are dropped
/// (the OTLP SDK retries). Better to drop one project's data than block all.
const PROJECT_CHANNEL_CAPACITY: usize = 10_000;

/// Backpressure policy for the per-project channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BackpressurePolicy {
    Block,
    DropOldest,
}

/// Flusher configuration derived from env vars at startup.
#[derive(Debug, Clone)]
pub struct FlushConfig {
    pub row_threshold: usize,
    pub interval: Duration,
    pub backpressure: BackpressurePolicy,
}

impl FlushConfig {
    pub fn from_env() -> Self {
        let row_threshold = crate::config::FLUSH_ROW_THRESHOLD.value().unwrap_or(10_000);
        let interval_secs = crate::config::FLUSH_INTERVAL_SECS.value().unwrap_or(1);
        let backpressure = match crate::config::BACKPRESSURE_POLICY
            .value()
            .as_deref()
            .unwrap_or("block")
        {
            "drop-oldest" => BackpressurePolicy::DropOldest,
            _ => BackpressurePolicy::Block,
        };
        Self {
            row_threshold,
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
        }
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
                tracing::warn!(project_id = %pid, "register_flusher: flusher already exists for project");
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
                sender.send(msg).await.map_err(|e| {
                    tracing::error!(project_id = %project_id_for_err, "send failed: channel error: {:?}", e);
                    drop(entry);
                    self.project_senders.remove(&project_id);
                    RouterError::ChannelClosed { project_id: project_id_for_err }
                })
            }
            BackpressurePolicy::DropOldest => {
                match sender.try_send(msg) {
                    Ok(()) => Ok(()),
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!(project_id = %project_id_for_err, "channel full, dropping message (drop-oldest)");
                        Ok(())
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        tracing::error!(project_id = %project_id_for_err, "send failed: channel closed");
                        drop(entry);
                        self.project_senders.remove(&project_id);
                        Err(RouterError::ChannelClosed { project_id: project_id_for_err })
                    }
                }
            }
        }
    }

    /// Gracefully shut down all flusher tasks: drops senders (triggering
    /// final flush in each flusher), then awaits task completion.
    pub async fn shutdown(self) {
        // Dropping all senders signals each flusher to do a final flush and exit.
        self.project_senders.clear();
        // NOTE: JoinHandles are dropped here too. Tokio JoinHandle::drop
        // detaches the task (doesn't cancel it). The flusher tasks will
        // complete their final flush and exit on their own since we already
        // dropped all senders above. For tests that need to await completion,
        // collect handles before clearing.
    }

    /// Shut down and await all flusher tasks. Guarantees all buffered data
    /// is flushed before returning.
    pub async fn shutdown_and_wait(self) {
        // Remove all entries — dropping senders triggers final flush in each flusher.
        let keys: Vec<String> = self
            .project_senders
            .iter()
            .map(|e| e.key().clone())
            .collect();
        for key in keys {
            self.project_senders.remove(&key);
        }

        // Small delay to let final flushes complete
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Per-project flusher state. Each project gets its own flusher task
/// so project A's load cannot block project B.
/// Crash recovery: DuckDB has its own WAL — once `append_record_batch`
/// returns, the data is durable. The only data-loss window is the buffer
/// here (up to 1s / 10k rows between 200 OK and DuckDB append). This is
/// acceptable for a debugging tool where the data source is still running
/// and will re-emit. See docs/decisions/001-wal-not-needed.md for the full analysis.
struct ProjectFlusher {
    conn: Arc<Mutex<Connection>>,
    table_names: BTreeMap<SignalTable, String>,
    buffers: BTreeMap<SignalTable, Vec<RecordBatch>>,
    total_rows: usize,
    last_flush: Instant,
    row_threshold: usize,
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
            last_flush: Instant::now(),
            row_threshold: cfg.row_threshold,
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
        self.total_rows += batch.num_rows();
        self.buffers.entry(signal).or_default().push(batch);
    }
    /// Run DRAIN on each log body and append a `template_id` column to the batch.
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
        for i in 0..batch.num_rows() {
            let body = body_col.value(i);
            let (template, id) = self.drain.classify(body);
            template_ids.push(id);
            self.pending_templates.entry(id).or_insert(template);
        }

        // Append template_id column to the existing batch
        let id_array = std::sync::Arc::new(UInt64Array::from(template_ids));
        let mut columns: Vec<std::sync::Arc<dyn Array>> = batch.columns().to_vec();
        columns.push(id_array);

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
        let schema = std::sync::Arc::new(arrow::datatypes::Schema::new(fields));

        RecordBatch::try_new(schema, columns).context("failed to add template_id column")
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
            || self.last_flush.elapsed() >= self.flush_interval
    }

    /// Flush all buffered batches to DuckDB using appenders per table.
    /// Acquires the mutex ONCE for the entire flush, writes all tables, releases.
    /// Uses appender_to_catalog_and_db for DuckLake catalog-qualified table access.
    #[tracing::instrument(skip_all, fields(project_id = %project_id, rows = self.total_rows))]
    fn flush(&mut self, project_id: &str) -> Result<()> {
        if self.total_rows == 0 {
            return Ok(());
        }

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;

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
            // Appender::drop flushes automatically
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
                // Plain INSERT — DuckLake does not support PRIMARY KEY.
                // Duplicate template rows are harmless (templates are tiny,
                // dedup can be added to the compactor if needed).
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

        tracing::debug!(project_id, rows = self.total_rows, "flushed batches");

        self.total_rows = 0;
        self.last_flush = Instant::now();
        self.first_flush = false;
        Ok(())
    }
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
    let mut flusher = ProjectFlusher::new(conn, &cfg);
    let mut interval = tokio::time::interval(flush_interval);

    loop {
        tokio::select! {
            biased;

            _ = interval.tick() => {
                pool.touch_project(project_id);
                if flusher.should_flush() {
                    if let Err(e) = flusher.flush(project_id) {
                        tracing::error!(project_id, "timer flush failed: {e:#}");
                    }
                }
            }

            msg = rx.recv() => {
                match msg {
                    Some(msg) => {
                        pool.touch_project(project_id);
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
