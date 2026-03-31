use anyhow::{Context, Result};
use arrow::record_batch::RecordBatch;
use crossbeam_channel::Receiver;
use dashmap::DashMap;
use duckdb::Connection;
use otlp2records::MetricBatches;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ingest::IngestMsg;
use crate::pool::ProjectPool;
use crate::wal;

/// Max rows buffered per table before forcing a flush.
const FLUSH_ROW_THRESHOLD: usize = 10_000;

/// Max time between flushes, even if row threshold not reached.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Per-project flusher state. Each project gets its own flusher thread
/// so project A's load cannot block project B.
struct ProjectFlusher {
    conn: Arc<Mutex<Connection>>,
    prefix: &'static str,
    wal_dir: std::path::PathBuf,
    /// Buffered RecordBatches per table, flushed in bulk.
    spans_buf: Vec<RecordBatch>,
    logs_buf: Vec<RecordBatch>,
    gauge_buf: Vec<RecordBatch>,
    sum_buf: Vec<RecordBatch>,
    histogram_buf: Vec<RecordBatch>,
    exp_histogram_buf: Vec<RecordBatch>,
    /// Row counts for flush threshold.
    spans_rows: usize,
    logs_rows: usize,
    metrics_rows: usize,
    last_flush: Instant,
}

impl ProjectFlusher {
    fn new(
        conn: Arc<Mutex<Connection>>,
        prefix: &'static str,
        wal_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            conn,
            prefix,
            wal_dir,
            spans_buf: Vec::new(),
            logs_buf: Vec::new(),
            gauge_buf: Vec::new(),
            sum_buf: Vec::new(),
            histogram_buf: Vec::new(),
            exp_histogram_buf: Vec::new(),
            spans_rows: 0,
            logs_rows: 0,
            metrics_rows: 0,
            last_flush: Instant::now(),
        }
    }

    fn buffer_traces(&mut self, batch: RecordBatch) {
        self.spans_rows += batch.num_rows();
        self.spans_buf.push(batch);
    }

    fn buffer_logs(&mut self, batch: RecordBatch) {
        self.logs_rows += batch.num_rows();
        self.logs_buf.push(batch);
    }

    fn buffer_metrics(&mut self, batches: MetricBatches) {
        if let Some(b) = batches.gauge {
            self.metrics_rows += b.num_rows();
            self.gauge_buf.push(b);
        }
        if let Some(b) = batches.sum {
            self.metrics_rows += b.num_rows();
            self.sum_buf.push(b);
        }
        if let Some(b) = batches.histogram {
            self.metrics_rows += b.num_rows();
            self.histogram_buf.push(b);
        }
        if let Some(b) = batches.exp_histogram {
            self.metrics_rows += b.num_rows();
            self.exp_histogram_buf.push(b);
        }
    }

    fn should_flush(&self) -> bool {
        let total = self.spans_rows + self.logs_rows + self.metrics_rows;
        total >= FLUSH_ROW_THRESHOLD || self.last_flush.elapsed() >= FLUSH_INTERVAL
    }

    /// Flush all buffered batches to DuckDB using a persistent appender per table.
    /// Acquires the mutex ONCE for the entire flush, writes all tables, releases.
    fn flush(&mut self, project_id: &str) -> Result<()> {
        if self.spans_rows + self.logs_rows + self.metrics_rows == 0 {
            return Ok(());
        }

        // WAL: serialize buffers to disk before DuckDB append
        let p = self.prefix;
        let tbl_spans = format!("{p}sigiro_spans");
        let tbl_logs = format!("{p}sigiro_logs");
        let tbl_gauge = format!("{p}sigiro_metrics_gauge");
        let tbl_sum = format!("{p}sigiro_metrics_sum");
        let tbl_hist = format!("{p}sigiro_metrics_histogram");
        let tbl_ehist = format!("{p}sigiro_metrics_exp_histogram");
        let wal_entries: Vec<(&str, &[RecordBatch])> = vec![
            (&tbl_spans, &self.spans_buf[..]),
            (&tbl_logs, &self.logs_buf[..]),
            (&tbl_gauge, &self.gauge_buf[..]),
            (&tbl_sum, &self.sum_buf[..]),
            (&tbl_hist, &self.histogram_buf[..]),
            (&tbl_ehist, &self.exp_histogram_buf[..]),
        ];
        let wal_path = wal::write_wal(&self.wal_dir, project_id, &wal_entries)?;

        let conn = self.conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;

        // Flush spans
        if !self.spans_buf.is_empty() {
            let table = format!("{}sigiro_spans", self.prefix);
            let mut appender = conn
                .appender(&table)
                .with_context(|| format!("failed to create {table} appender"))?;
            for batch in self.spans_buf.drain(..) {
                appender
                    .append_record_batch(batch)
                    .with_context(|| format!("failed to append {table} batch"))?;
            }
            // Appender::drop flushes automatically
        }

        // Flush logs
        if !self.logs_buf.is_empty() {
            let table = format!("{}sigiro_logs", self.prefix);
            let mut appender = conn
                .appender(&table)
                .with_context(|| format!("failed to create {table} appender"))?;
            for batch in self.logs_buf.drain(..) {
                appender
                    .append_record_batch(batch)
                    .with_context(|| format!("failed to append {table} batch"))?;
            }
        }

        // Flush metrics (4 tables)
        flush_metric_buf(
            &conn,
            &mut self.gauge_buf,
            self.prefix,
            "sigiro_metrics_gauge",
        )?;
        flush_metric_buf(&conn, &mut self.sum_buf, self.prefix, "sigiro_metrics_sum")?;
        flush_metric_buf(
            &conn,
            &mut self.histogram_buf,
            self.prefix,
            "sigiro_metrics_histogram",
        )?;
        flush_metric_buf(
            &conn,
            &mut self.exp_histogram_buf,
            self.prefix,
            "sigiro_metrics_exp_histogram",
        )?;

        // DuckDB flush succeeded — remove WAL file
        if let Err(e) = std::fs::remove_file(&wal_path) {
            tracing::warn!(path = %wal_path.display(), "failed to remove WAL file: {e}");
        }

        let total = self.spans_rows + self.logs_rows + self.metrics_rows;
        tracing::debug!(
            project_id,
            total,
            spans = self.spans_rows,
            logs = self.logs_rows,
            metrics = self.metrics_rows,
            "flushed batches"
        );

        self.spans_rows = 0;
        self.logs_rows = 0;
        self.metrics_rows = 0;
        self.last_flush = Instant::now();
        Ok(())
    }
}

fn flush_metric_buf(
    conn: &Connection,
    buf: &mut Vec<RecordBatch>,
    prefix: &str,
    table_name: &str,
) -> Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let table = format!("{prefix}{table_name}");
    let mut appender = conn
        .appender(&table)
        .with_context(|| format!("failed to create {table} appender"))?;
    for batch in buf.drain(..) {
        if batch.num_rows() > 0 {
            appender
                .append_record_batch(batch)
                .with_context(|| format!("failed to append {table} batch"))?;
        }
    }
    Ok(())
}

/// Run the inserter: routes messages to per-project flusher threads.
///
/// Architecture: one dispatcher thread fans out to per-project channels.
/// Each project gets a dedicated flusher thread that buffers RecordBatches
/// and flushes to DuckDB on timer (1s) or count (10k rows) threshold.
/// Project A's ingest load cannot block project B.
#[allow(clippy::needless_pass_by_value)] // Receiver is consumed by the for loop
pub fn run_inserter(
    pool: &Arc<ProjectPool>,
    rx: Receiver<IngestMsg>,
    wal_dir: std::path::PathBuf,
) -> Result<()> {
    let prefix = pool.prefix();

    // Per-project sender channels + join handles. Flusher threads are spawned lazily.
    let project_senders: DashMap<String, crossbeam_channel::Sender<IngestMsg>> = DashMap::new();
    let flusher_handles: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

    for msg in &rx {
        let project_id = msg.project_id().to_string();

        // Get or create per-project channel + flusher thread.
        let sender = {
            if let Some(s) = project_senders.get(&project_id) {
                s.clone()
            } else {
                let conn = pool
                    .get_or_create(&project_id)
                    .context("failed to get project connection for inserter")?;

                // Bounded channel: backpressure per project. If a project's flusher
                // falls behind, only THAT project's channel fills up.
                let (tx, prx) = crossbeam_channel::bounded::<IngestMsg>(10_000);

                let pid = project_id.clone();
                let wd = wal_dir.clone();
                let handle = std::thread::Builder::new()
                    .name(format!("flusher-{pid}"))
                    .spawn(move || {
                        run_project_flusher(&pid, conn, prx, prefix, wd);
                    })
                    .with_context(|| format!("failed to spawn flusher for {project_id}"))?;

                flusher_handles
                    .lock()
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                    .push(handle);
                project_senders.insert(project_id.clone(), tx.clone());
                tx
            }
        };

        // Send to project-specific flusher. If channel is full, log and drop.
        // The OTLP SDK will retry. Better to drop one project's data than block all.
        if let Err(e) = sender.try_send(msg) {
            match e {
                crossbeam_channel::TrySendError::Full(_) => {
                    tracing::warn!(project_id, "project channel full, dropping message");
                }
                crossbeam_channel::TrySendError::Disconnected(_) => {
                    // Flusher thread died — remove stale sender, will respawn on next message.
                    tracing::error!(project_id, "flusher thread disconnected, removing");
                    project_senders.remove(&project_id);
                }
            }
        }
    }

    // Main channel closed — drop all senders so flusher threads see Disconnected,
    // then wait for them to drain and flush remaining data.
    drop(project_senders);
    let handles = flusher_handles
        .into_inner()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for handle in handles {
        if let Err(e) = handle.join() {
            tracing::error!("flusher thread panicked: {e:?}");
        }
    }

    Ok(())
}

/// Per-project flusher loop. Receives pre-transformed RecordBatches,
/// buffers them, and flushes to DuckDB on timer or row count threshold.
#[allow(clippy::needless_pass_by_value)] // Receiver moved into thread, needs ownership
fn run_project_flusher(
    project_id: &str,
    conn: Arc<Mutex<Connection>>,
    rx: Receiver<IngestMsg>,
    prefix: &'static str,
    wal_dir: std::path::PathBuf,
) {
    let mut flusher = ProjectFlusher::new(conn, prefix, wal_dir);

    loop {
        // Use recv_timeout so we flush on the timer even if no new messages arrive.
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(msg) => {
                match msg {
                    IngestMsg::Traces { batch, .. } => flusher.buffer_traces(batch),
                    IngestMsg::Logs { batch, .. } => flusher.buffer_logs(batch),
                    IngestMsg::Metrics { batches, .. } => flusher.buffer_metrics(batches),
                }
                if flusher.should_flush() {
                    if let Err(e) = flusher.flush(project_id) {
                        tracing::error!(project_id, "flush failed: {e:#}");
                    }
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // Timer expired — flush whatever we have.
                if let Err(e) = flusher.flush(project_id) {
                    tracing::error!(project_id, "timer flush failed: {e:#}");
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                // Dispatcher dropped our sender — drain remaining messages and flush.
                for remaining in rx.try_iter() {
                    match remaining {
                        IngestMsg::Traces { batch, .. } => flusher.buffer_traces(batch),
                        IngestMsg::Logs { batch, .. } => flusher.buffer_logs(batch),
                        IngestMsg::Metrics { batches, .. } => flusher.buffer_metrics(batches),
                    }
                }
                if let Err(e) = flusher.flush(project_id) {
                    tracing::error!(project_id, "final flush failed: {e:#}");
                }
                tracing::info!(project_id, "flusher thread exiting (channel closed)");
                break;
            }
        }
    }
}
