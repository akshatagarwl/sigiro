use anyhow::{Context, Result};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
pub struct InvestigateRequest {
    pub service: String,
    pub from_ts: Option<i64>,
    pub to_ts: Option<i64>,
}

#[derive(Serialize)]
pub struct InvestigationBlock {
    pub service: String,
    pub window: Window,
    pub metrics: Vec<MetricBucket>,
    pub spans: Vec<SpanRow>,
    pub span_trees: HashMap<String, Vec<String>>,
    pub logs: Vec<LogTemplate>,
    pub linked_traces: Vec<LinkedTrace>,
}

#[derive(Serialize)]
pub struct Window {
    pub from_ts: i64,
    pub to_ts: i64,
}

#[derive(Serialize)]
pub struct MetricBucket {
    pub bucket: String,
    pub metric_name: String,
    pub p95: f64,
    pub avg: f64,
    pub count: i64,
}

#[derive(Serialize, Clone)]
pub struct SpanRow {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub span_name: String,
    pub duration: i64,
    pub status_code: i32,
    pub service_name: String,
}

#[derive(Serialize)]
pub struct LogTemplate {
    pub body: String,
    pub severity_text: String,
    pub count: i64,
    pub first_seen: String,
    pub last_seen: String,
}

#[derive(Serialize)]
pub struct LinkedTrace {
    pub trace_id: String,
    pub spans: Vec<SpanRow>,
}

pub fn investigate(
    conn: &Arc<Mutex<Connection>>,
    req: &InvestigateRequest,
) -> Result<InvestigationBlock> {
    let now_us = chrono::Utc::now().timestamp_micros();
    let to_ts = req.to_ts.unwrap_or(now_us);
    let from_ts = req.from_ts.unwrap_or(to_ts - 15 * 60 * 1_000_000);

    let conn = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;

    let metrics =
        query_metrics(&conn, &req.service, from_ts, to_ts).context("metrics query failed")?;
    let spans = query_spans(&conn, &req.service, from_ts, to_ts).context("spans query failed")?;
    let logs = query_logs(&conn, &req.service, from_ts, to_ts).context("logs query failed")?;

    let span_trees = build_span_trees(&spans);

    let linked_traces = query_linked_traces(&conn, &spans).context("linked traces query failed")?;

    Ok(InvestigationBlock {
        service: req.service.clone(),
        window: Window { from_ts, to_ts },
        metrics,
        spans,
        span_trees,
        logs,
        linked_traces,
    })
}

fn query_metrics(
    conn: &Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
) -> Result<Vec<MetricBucket>> {
    let sql = "SELECT time_bucket(INTERVAL '1 minute', timestamp) AS bucket,
               metric_name,
               approx_quantile(value, 0.95) AS p95,
               avg(value) AS avg,
               count(*) AS n
        FROM sigiro_metrics_gauge
        WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        GROUP BY bucket, metric_name
        ORDER BY bucket, metric_name";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params![service, from_ts, to_ts])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(MetricBucket {
            bucket: row.get::<_, String>(0).unwrap_or_default(),
            metric_name: row.get::<_, String>(1).unwrap_or_default(),
            p95: row.get::<_, f64>(2).unwrap_or(0.0),
            avg: row.get::<_, f64>(3).unwrap_or(0.0),
            count: row.get::<_, i64>(4).unwrap_or(0),
        });
    }
    Ok(results)
}

fn query_spans(conn: &Connection, service: &str, from_ts: i64, to_ts: i64) -> Result<Vec<SpanRow>> {
    let sql = "SELECT trace_id, span_id, parent_span_id, span_name, duration,
               status_code, service_name
        FROM sigiro_spans
        WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        ORDER BY CASE WHEN status_code = 2 THEN 0 ELSE 1 END, duration DESC
        LIMIT 50";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params![service, from_ts, to_ts])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(SpanRow {
            trace_id: row.get::<_, String>(0).unwrap_or_default(),
            span_id: row.get::<_, String>(1).unwrap_or_default(),
            parent_span_id: row.get::<_, String>(2).unwrap_or_default(),
            span_name: row.get::<_, String>(3).unwrap_or_default(),
            duration: row.get::<_, i64>(4).unwrap_or(0),
            status_code: row.get::<_, i32>(5).unwrap_or(0),
            service_name: row.get::<_, String>(6).unwrap_or_default(),
        });
    }
    Ok(results)
}

fn query_logs(
    conn: &Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
) -> Result<Vec<LogTemplate>> {
    let sql = "SELECT body, severity_text, count(*) AS count,
               min(timestamp)::VARCHAR AS first_seen, max(timestamp)::VARCHAR AS last_seen
        FROM sigiro_logs
        WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        GROUP BY body, severity_text
        ORDER BY count DESC
        LIMIT 30";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params![service, from_ts, to_ts])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(LogTemplate {
            body: row.get::<_, String>(0).unwrap_or_default(),
            severity_text: row.get::<_, String>(1).unwrap_or_default(),
            count: row.get::<_, i64>(2).unwrap_or(0),
            first_seen: row.get::<_, String>(3).unwrap_or_default(),
            last_seen: row.get::<_, String>(4).unwrap_or_default(),
        });
    }
    Ok(results)
}

fn build_span_trees(spans: &[SpanRow]) -> HashMap<String, Vec<String>> {
    let mut trees: HashMap<String, Vec<String>> = HashMap::new();
    for span in spans {
        if !span.parent_span_id.is_empty() {
            trees
                .entry(span.parent_span_id.clone())
                .or_default()
                .push(span.span_id.clone());
        }
    }
    trees
}

fn query_linked_traces(conn: &Connection, spans: &[SpanRow]) -> Result<Vec<LinkedTrace>> {
    // Collect error trace_ids, cap at 5
    let error_trace_ids: Vec<String> = spans
        .iter()
        .filter(|s| s.status_code == 2)
        .map(|s| s.trace_id.clone())
        .collect::<Vec<_>>();

    let unique: Vec<&String> = {
        let mut seen = std::collections::HashSet::new();
        error_trace_ids
            .iter()
            .filter(move |id| seen.insert(id.as_str().to_string()))
            .take(5)
            .collect()
    };

    if unique.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders: Vec<String> = unique.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, duration,
                status_code, service_name
         FROM sigiro_spans
         WHERE trace_id IN ({})
         ORDER BY trace_id, timestamp
         LIMIT 200",
        placeholders.join(", ")
    );

    let mut stmt = conn.prepare(&sql)?;

    // Build params for the IN clause
    let params: Vec<&dyn duckdb::ToSql> = unique.iter().map(|s| s as &dyn duckdb::ToSql).collect();
    let mut rows = stmt.query(params.as_slice())?;

    let mut trace_map: HashMap<String, Vec<SpanRow>> = HashMap::new();
    while let Some(row) = rows.next()? {
        let span = SpanRow {
            trace_id: row.get::<_, String>(0).unwrap_or_default(),
            span_id: row.get::<_, String>(1).unwrap_or_default(),
            parent_span_id: row.get::<_, String>(2).unwrap_or_default(),
            span_name: row.get::<_, String>(3).unwrap_or_default(),
            duration: row.get::<_, i64>(4).unwrap_or(0),
            status_code: row.get::<_, i32>(5).unwrap_or(0),
            service_name: row.get::<_, String>(6).unwrap_or_default(),
        };
        trace_map
            .entry(span.trace_id.clone())
            .or_default()
            .push(span);
    }

    Ok(trace_map
        .into_iter()
        .map(|(trace_id, spans)| LinkedTrace { trace_id, spans })
        .collect())
}
