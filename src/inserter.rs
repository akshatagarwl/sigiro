use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use duckdb::Connection;
use otlp2records::{InputFormat, MetricBatches};
use std::sync::{Arc, Mutex};

use crate::ingest::IngestMsg;
use crate::pool::ProjectPool;

/// Run the inserter loop routing messages to per-project DuckDB via the pool.
///
/// OTLP ingest doesn't carry project_id yet (P1 — OTLP ingest auth).
/// Until then, all data routes to "default" project. When OTLP auth lands,
/// IngestMsg will carry project_id and this will route per-project.
pub fn run_inserter(pool: &Arc<ProjectPool>, rx: Receiver<IngestMsg>) -> Result<()> {
    let prefix = pool.prefix();
    for msg in rx {
        // TODO: extract project_id from msg when OTLP ingest auth lands
        let project_id = "default";
        let conn = pool
            .get_or_create(project_id)
            .context("failed to get project connection for inserter")?;
        if let Err(e) = handle_msg(&conn, msg, prefix) {
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
