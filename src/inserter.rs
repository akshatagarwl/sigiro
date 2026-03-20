use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use duckdb::Connection;
use otlp2records::{InputFormat, MetricBatches};
use std::sync::{Arc, Mutex};

use crate::ingest::IngestMsg;

/// Run the inserter loop using a shared connection.
pub fn run_inserter(conn: Arc<Mutex<Connection>>, rx: Receiver<IngestMsg>) -> Result<()> {
    for msg in rx {
        if let Err(e) = handle_msg(&conn, msg) {
            eprintln!("inserter error: {e:#}");
        }
    }
    Ok(())
}

fn handle_msg(conn: &Arc<Mutex<Connection>>, msg: IngestMsg) -> Result<()> {
    match msg {
        IngestMsg::Traces(bytes) => {
            let batch = otlp2records::transform_traces(&bytes, InputFormat::Protobuf)
                .context("failed to transform traces")?;
            if batch.num_rows() > 0 {
                let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut appender = conn
                    .appender("sigiro_spans")
                    .context("failed to create spans appender")?;
                appender
                    .append_record_batch(batch)
                    .context("failed to append spans batch")?;
            }
        }
        IngestMsg::Logs(bytes) => {
            let batch = otlp2records::transform_logs(&bytes, InputFormat::Protobuf)
                .context("failed to transform logs")?;
            if batch.num_rows() > 0 {
                let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut appender = conn
                    .appender("sigiro_logs")
                    .context("failed to create logs appender")?;
                appender
                    .append_record_batch(batch)
                    .context("failed to append logs batch")?;
            }
        }
        IngestMsg::Metrics(bytes) => {
            let batches: MetricBatches =
                otlp2records::transform_metrics(&bytes, InputFormat::Protobuf)
                    .context("failed to transform metrics")?;
            let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Some(batch) = batches.gauge {
                append_metric_batch(&conn, "sigiro_metrics_gauge", batch)?;
            }
            if let Some(batch) = batches.sum {
                append_metric_batch(&conn, "sigiro_metrics_sum", batch)?;
            }
            if let Some(batch) = batches.histogram {
                append_metric_batch(&conn, "sigiro_metrics_histogram", batch)?;
            }
            if let Some(batch) = batches.exp_histogram {
                append_metric_batch(&conn, "sigiro_metrics_exp_histogram", batch)?;
            }
        }
    }
    Ok(())
}

fn append_metric_batch(
    conn: &Connection,
    table: &str,
    batch: arrow::record_batch::RecordBatch,
) -> Result<()> {
    if batch.num_rows() > 0 {
        let mut appender = conn
            .appender(table)
            .with_context(|| format!("failed to create {table} appender"))?;
        appender
            .append_record_batch(batch)
            .with_context(|| format!("failed to append {table} batch"))?;
    }
    Ok(())
}
