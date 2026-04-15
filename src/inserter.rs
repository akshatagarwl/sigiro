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
use crate::ingest::IngestMsg;
use crate::pool::ProjectPool;

/// Max rows buffered per table before forcing a flush.
const FLUSH_ROW_THRESHOLD: usize = 10_000;

/// Max time between flushes, even if row threshold not reached.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Per-project channel capacity. When full, messages are dropped
/// (the OTLP SDK retries). Better to drop one project's data than block all.
const PROJECT_CHANNEL_CAPACITY: usize = 10_000;

/// Signal types that map to DuckDB tables. Adding a new signal type
/// (e.g. Profiles) requires only adding a variant here and handling it
/// in the IngestMsg match — the flush path iterates the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SignalTable {
    Spans,
    Logs,
    MetricsGauge,
    MetricsSum,
    MetricsHistogram,
    MetricsExpHistogram,
    Profiles,
}

impl SignalTable {
    /// All signal table variants.
    pub const ALL: &'static [SignalTable] = &[
        SignalTable::Spans,
        SignalTable::Logs,
        SignalTable::MetricsGauge,
        SignalTable::MetricsSum,
        SignalTable::MetricsHistogram,
        SignalTable::MetricsExpHistogram,
        SignalTable::Profiles,
    ];

    /// DuckDB table name without prefix (e.g. "sigiro_spans").
    pub fn table_suffix(self) -> &'static str {
        match self {
            Self::Spans => "sigiro_spans",
            Self::Logs => "sigiro_logs",
            Self::MetricsGauge => "sigiro_metrics_gauge",
            Self::MetricsSum => "sigiro_metrics_sum",
            Self::MetricsHistogram => "sigiro_metrics_histogram",
            Self::MetricsExpHistogram => "sigiro_metrics_exp_histogram",
            Self::Profiles => "sigiro_profiles",
        }
    }

    /// DuckDB table name (with lake. prefix).
    fn table_name(self, prefix: &str) -> String {
        format!("{}{}", prefix, self.table_suffix())
    }
}

/// Routes IngestMsg to per-project flusher tasks.
///
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
    pub async fn send(&self, msg: IngestMsg) {
        let project_id = msg.project_id().to_string();

        // Fast path: existing sender
        if let Some(entry) = self.project_senders.get(&project_id) {
            let sender = &entry.value().0;
            match sender.try_send(msg) {
                Ok(()) => return,
                Err(tokio::sync::mpsc::error::TrySendError::Full(msg)) => {
                    // Channel full — apply backpressure by blocking send.
                    // This is intentional: the caller (HTTP/gRPC handler) will wait
                    // rather than silently dropping telemetry data.
                    drop(entry);
                    return self.send_blocking(&project_id, msg).await;
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(msg)) => {
                    // Receiver dropped — flusher task exited. Remove stale entry and fall through.
                    drop(entry);
                    self.project_senders.remove(&project_id);
                    // Re-enter with msg to create new flusher
                    return self.send_with_fallback(project_id, msg).await;
                }
            }
        }

        self.send_with_fallback(project_id, msg).await;
    }

    /// Backpressure path: wait for channel capacity instead of dropping.
    async fn send_blocking(&self, project_id: &str, msg: IngestMsg) {
        if let Some(entry) = self.project_senders.get(project_id) {
            if entry.value().0.send(msg).await.is_err() {
                tracing::error!(
                    project_id,
                    "FATAL: project channel closed during blocking send — message DROPPED. \
                     The flusher task for this project has exited unexpectedly."
                );
            }
        }
    }

    /// Slow path: create connection + channel + flusher task.
    async fn send_with_fallback(&self, project_id: String, msg: IngestMsg) {
        // get_or_create is sync — fast path is just a DashMap lookup,
        // slow path opens in-memory DuckDB + creates dirs (low ms).
        let conn = match self.pool.get_or_create(&project_id) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(
                    project_id,
                    "FATAL: failed to get project connection — message DROPPED. \
                     This indicates a corrupt DuckDB state or filesystem error. \
                     Error: {e:#}"
                );
                return;
            }
        };

        let (tx, rx) = mpsc::channel::<IngestMsg>(PROJECT_CHANNEL_CAPACITY);
        let pid = project_id.clone();
        let handle = tokio::spawn(async move {
            run_flusher(&pid, conn, rx).await;
        });

        // Insert — another task may have raced us, that's fine.
        let sender = self
            .project_senders
            .entry(project_id.clone())
            .or_insert((tx, handle))
            .value()
            .0
            .clone();

        if let Err(e) = sender.send(msg).await {
            tracing::error!(
                project_id,
                "FATAL: project channel closed on new sender — message DROPPED. \
                 The flusher task exited immediately after creation. Error: {e}"
            );
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
///
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
        let signals = [
            SignalTable::Spans,
            SignalTable::Logs,
            SignalTable::MetricsGauge,
            SignalTable::MetricsSum,
            SignalTable::MetricsHistogram,
            SignalTable::MetricsExpHistogram,
            SignalTable::Profiles,
        ];
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
