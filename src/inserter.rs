use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use duckdb::Connection;
use otlp2records::{InputFormat, MetricBatches};
use std::sync::{Arc, Mutex};

use crate::ingest::IngestMsg;

/// Run the inserter loop using a shared connection.
/// `prefix` is "lake." when DuckLake is active, "" for local mode.
///
/// NOTE: DuckDB appender with cross-catalog names ("lake.sigiro_spans") needs
/// verification with DuckLake. If appender doesn't support it, fall back to
/// INSERT INTO ... SELECT * FROM arrow_scan(...). Test with real DuckLake setup.
pub fn run_inserter(
    conn: &Arc<Mutex<Connection>>,
    rx: Receiver<IngestMsg>,
    prefix: &str,
) -> Result<()> {
    for msg in rx {
        if let Err(e) = handle_msg(conn, msg, prefix) {
            tracing::error!("inserter: {e:#}");
        }
    }
    Ok(())
}

fn handle_msg(conn: &Arc<Mutex<Connection>>, msg: IngestMsg, prefix: &str) -> Result<()> {
    match msg {
        IngestMsg::Traces(bytes) => {
            let batch = otlp2records::transform_traces(&bytes, InputFormat::Protobuf)
                .context("failed to transform traces")?;
            if batch.num_rows() > 0 {
                let table = format!("{prefix}sigiro_spans");
                let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut appender = conn
                    .appender(&table)
                    .with_context(|| format!("failed to create {table} appender"))?;
                appender
                    .append_record_batch(batch)
                    .with_context(|| format!("failed to append {table} batch"))?;
            }
        }
        IngestMsg::Logs(bytes) => {
            let batch = otlp2records::transform_logs(&bytes, InputFormat::Protobuf)
                .context("failed to transform logs")?;
            if batch.num_rows() > 0 {
                let table = format!("{prefix}sigiro_logs");
                let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                let mut appender = conn
                    .appender(&table)
                    .with_context(|| format!("failed to create {table} appender"))?;
                appender
                    .append_record_batch(batch)
                    .with_context(|| format!("failed to append {table} batch"))?;
            }
        }
        IngestMsg::Metrics(bytes) => {
            let batches: MetricBatches =
                otlp2records::transform_metrics(&bytes, InputFormat::Protobuf)
                    .context("failed to transform metrics")?;
            let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
            if let Some(batch) = batches.gauge {
                append_metric_batch(&conn, &format!("{prefix}sigiro_metrics_gauge"), batch)?;
            }
            if let Some(batch) = batches.sum {
                append_metric_batch(&conn, &format!("{prefix}sigiro_metrics_sum"), batch)?;
            }
            if let Some(batch) = batches.histogram {
                append_metric_batch(&conn, &format!("{prefix}sigiro_metrics_histogram"), batch)?;
            }
            if let Some(batch) = batches.exp_histogram {
                append_metric_batch(
                    &conn,
                    &format!("{prefix}sigiro_metrics_exp_histogram"),
                    batch,
                )?;
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
