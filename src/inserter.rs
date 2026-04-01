use anyhow::{Context, Result};
use arrow::record_batch::RecordBatch;
use crossbeam_channel::Receiver;
use dashmap::DashMap;
use duckdb::Connection;
use otlp2records::MetricBatches;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::ingest::IngestMsg;
use crate::pool::ProjectPool;

/// Max rows buffered per table before forcing a flush.
const FLUSH_ROW_THRESHOLD: usize = 10_000;

/// Max time between flushes, even if row threshold not reached.
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Signal types that map to DuckDB tables. Adding a new signal type
/// (e.g. Profiles) requires only adding a variant here and handling it
/// in the IngestMsg match — the flush path iterates the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum SignalTable {
    Spans,
    Logs,
    MetricsGauge,
    MetricsSum,
    MetricsHistogram,
    MetricsExpHistogram,
}

impl SignalTable {
    /// DuckDB table name (with prefix for DuckLake mode).
    fn table_name(self, prefix: &str) -> String {
        let suffix = match self {
            Self::Spans => "sigiro_spans",
            Self::Logs => "sigiro_logs",
            Self::MetricsGauge => "sigiro_metrics_gauge",
            Self::MetricsSum => "sigiro_metrics_sum",
            Self::MetricsHistogram => "sigiro_metrics_histogram",
            Self::MetricsExpHistogram => "sigiro_metrics_exp_histogram",
        };
        format!("{prefix}{suffix}")
    }
}

/// Per-project flusher state. Each project gets its own flusher thread
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
}

impl ProjectFlusher {
    fn new(conn: Arc<Mutex<Connection>>, prefix: &'static str) -> Self {
        let signals = [
            SignalTable::Spans,
            SignalTable::Logs,
            SignalTable::MetricsGauge,
            SignalTable::MetricsSum,
            SignalTable::MetricsHistogram,
            SignalTable::MetricsExpHistogram,
        ];
        let table_names: BTreeMap<SignalTable, String> =
            signals.iter().map(|s| (*s, s.table_name(prefix))).collect();
        let buffers: BTreeMap<SignalTable, Vec<RecordBatch>> =
            signals.iter().map(|s| (*s, Vec::new())).collect();

        Self {
            conn,
            table_names,
            buffers,
            total_rows: 0,
            last_flush: Instant::now(),
        }
    }

    fn buffer(&mut self, signal: SignalTable, batch: RecordBatch) {
        self.total_rows += batch.num_rows();
        self.buffers.entry(signal).or_default().push(batch);
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
            let mut appender = conn
                .appender(table)
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

        tracing::debug!(project_id, rows = self.total_rows, "flushed batches");

        self.total_rows = 0;
        self.last_flush = Instant::now();
        Ok(())
    }
}

/// Run the inserter: routes messages to per-project flusher threads.
///
/// Architecture: one dispatcher thread fans out to per-project channels.
/// Each project gets a dedicated flusher thread that buffers RecordBatches
/// and flushes to DuckDB on timer (1s) or count (10k rows) threshold.
/// Project A's ingest load cannot block project B.
#[allow(clippy::needless_pass_by_value)] // Receiver is consumed by the for loop
pub fn run_inserter(pool: &Arc<ProjectPool>, rx: Receiver<IngestMsg>) -> Result<()> {
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
                let handle = std::thread::Builder::new()
                    .name(format!("flusher-{pid}"))
                    .spawn(move || {
                        run_project_flusher(&pid, conn, prx, prefix);
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
) {
    let mut flusher = ProjectFlusher::new(conn, prefix);

    loop {
        // Use recv_timeout so we flush on the timer even if no new messages arrive.
        match rx.recv_timeout(FLUSH_INTERVAL) {
            Ok(msg) => {
                buffer_msg(&mut flusher, msg);
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
                    buffer_msg(&mut flusher, remaining);
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

/// Route an IngestMsg into the flusher's signal-keyed buffers.
fn buffer_msg(flusher: &mut ProjectFlusher, msg: IngestMsg) {
    match msg {
        IngestMsg::Traces { batch, .. } => flusher.buffer(SignalTable::Spans, batch),
        IngestMsg::Logs { batch, .. } => flusher.buffer(SignalTable::Logs, batch),
        IngestMsg::Metrics { batches, .. } => flusher.buffer_metrics(batches),
    }
}
