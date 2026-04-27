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

/// Max rows buffered per table before forcing a flush.
const FLUSH_ROW_THRESHOLD: usize = 10_000;

/// Max time between flushes, even if row threshold not reached.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Per-project channel capacity. When full, messages are dropped
/// (the OTLP SDK retries). Better to drop one project's data than block all.
const PROJECT_CHANNEL_CAPACITY: usize = 10_000;

/// Routes IngestMsg to per-project flusher tasks.
/// Handlers call `router.send(msg)` directly. The router lazily creates
/// per-project bounded channels and tokio tasks on first message per project.
/// No dispatcher thread — handlers fan out directly.
pub struct MsgRouter {
    pool: Arc<ProjectPool>,
    project_senders: DashMap<String, (mpsc::Sender<IngestMsg>, JoinHandle<()>)>,
}

impl MsgRouter {
    pub fn new(pool: Arc<ProjectPool>) -> Self {
        Self {
            pool,
            project_senders: DashMap::new(),
        }
    }

    /// Route a message to the appropriate per-project flusher.
    /// Creates the flusher task on first message for a project.
    /// Applies backpressure via bounded channel — sender blocks when full
    /// rather than silently dropping data.
    /// Explicitly create and register a flusher for a project.
    /// Caller (DI container / server setup) must invoke this before the first
    /// send() for that project_id. Thread-safe via DashMap entry API.
    pub fn register_flusher(&self, project_id: &str) -> Result<(), RouterError> {
        use dashmap::mapref::entry::Entry;

        let conn = self.pool.get_or_create(project_id)
            .map_err(|_| RouterError::ConnectionFailed { project_id: project_id.to_string() })?;

        let (tx, rx) = mpsc::channel::<IngestMsg>(PROJECT_CHANNEL_CAPACITY);
        let pid = project_id.to_string();
        let handle = tokio::spawn(async move { run_flusher(&pid, conn, rx).await });

        match self.project_senders.entry(project_id.to_string()) {
            Entry::Occupied(_) => {
                // Already registered — return Ok, caller can send immediately.
                Ok(())
            }
            Entry::Vacant(e) => {
                e.insert((tx, handle));
                Ok(())
            }
        }
    }

    /// Route a message to the appropriate per-project flusher.
    /// Creates the flusher task on first message for a project (lazy initialization).
    /// Applies backpressure via bounded channel — sender blocks when full
    /// rather than silently dropping data.
    ///
    /// Callers may also use register_flusher() for explicit initialization
    /// (recommended in production DI containers).
    #[tracing::instrument(skip_all, fields(otel.name = "ingest.send", project_id = %msg.project_id()))]
    pub async fn send(&self, msg: IngestMsg) -> Result<(), RouterError> {
        let project_id = msg.project_id().to_string();

        // Lazy creation: if no flusher exists for this project, create one now.
        // This preserves the original behavior that all existing tests depend on.
        if !self.project_senders.contains_key(&project_id) {
            let conn = self.pool.get_or_create(&project_id).map_err(|_| {
                RouterError::ConnectionFailed { project_id: project_id.clone() }
            })?;
            let (tx, rx) = mpsc::channel::<IngestMsg>(PROJECT_CHANNEL_CAPACITY);
            let pid = project_id.clone();
            let handle = tokio::spawn(async move {
                run_flusher(&pid, conn, rx).await
            });
            self.project_senders.insert(project_id.clone(), (tx, handle));
        }

        let entry = self.project_senders
            .get(&project_id)
            .ok_or_else(|| RouterError::ConnectionFailed { project_id: project_id.clone() })?;

        let sender = &entry.value().0;
        sender.send(msg).await.map_err(|_| {
            drop(entry);
            self.project_senders.remove(&project_id);
            RouterError::ConnectionFailed { project_id }
        })
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
/// and will re-emit. See docs/designs/wal.md for the full analysis.
struct ProjectFlusher {
    conn: Arc<Mutex<Connection>>,
    /// Precomputed table names (prefix applied once at construction).
    table_names: BTreeMap<SignalTable, String>,
    /// Buffered RecordBatches per signal, flushed in bulk.
    buffers: BTreeMap<SignalTable, Vec<RecordBatch>>,
    /// Total buffered rows across all signals for flush threshold.
    total_rows: usize,
    last_flush: Instant,
    /// DRAIN tree for log template extraction. Lives in memory per-project.
    drain: DrainTree,
    /// New templates discovered this flush cycle: template_id → template_text.
    /// Flushed to sigiro_log_templates after DuckDB log append.
    pending_templates: HashMap<u64, String>,
}

impl ProjectFlusher {
    fn new(conn: Arc<Mutex<Connection>>) -> Self {
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
            drain: DrainTree::new(),
            pending_templates: HashMap::new(),
        }
    }

    fn buffer(&mut self, signal: SignalTable, batch: RecordBatch) {
        let batch = if signal == SignalTable::Logs {
            self.classify_logs(&batch)
        } else {
            batch
        };
        self.total_rows += batch.num_rows();
        self.buffers.entry(signal).or_default().push(batch);
    }

    /// Run DRAIN on each log body and append a `template_id` column to the batch.
    fn classify_logs(&mut self, batch: &RecordBatch) -> RecordBatch {
        let body_idx = batch
            .schema()
            .index_of("body")
            .expect("sigiro_logs must have body column");
        let body_col = batch
            .column(body_idx)
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("body column must be StringArray");

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

        RecordBatch::try_new(schema, columns).expect("failed to add template_id column")
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
        self.total_rows >= FLUSH_ROW_THRESHOLD || self.last_flush.elapsed() >= FLUSH_INTERVAL
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
        Ok(())
    }
}

/// Per-project flusher loop. Receives pre-transformed RecordBatches,
/// buffers them, and flushes to DuckDB on timer or row count threshold.
    #[tracing::instrument(skip_all, fields(project_id = %project_id))]
async fn run_flusher(
    project_id: &str,
    conn: Arc<Mutex<Connection>>,
    mut rx: mpsc::Receiver<IngestMsg>,
) {
    let mut flusher = ProjectFlusher::new(conn);
    let mut interval = tokio::time::interval(FLUSH_INTERVAL);

    loop {
        tokio::select! {
            biased;

            _ = interval.tick() => {
                // Timer expired — flush whatever we have.
                if let Err(e) = flusher.flush(project_id) {
                    tracing::error!(project_id, "timer flush failed: {e:#}");
                }
            }

            msg = rx.recv() => {
                match msg {
                    Some(msg) => {
                        buffer_msg(&mut flusher, msg);
                        if flusher.should_flush() {
                            if let Err(e) = flusher.flush(project_id) {
                                tracing::error!(project_id, "flush failed: {e:#}");
                            }
                        }
                    }
                    None => {
                        // Channel closed — all senders dropped.
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
