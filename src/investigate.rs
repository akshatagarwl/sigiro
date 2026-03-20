use anyhow::{Context, Result};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
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
    /// parent_span_id -> [child_span_id]. Root spans (no parent) are not keys.
    pub span_trees: BTreeMap<String, Vec<String>>,
    pub logs: Vec<LogTemplate>,
    pub linked_traces: Vec<LinkedTrace>,
}

#[derive(Serialize)]
pub struct Window {
    pub from_ts: i64,
    pub to_ts: i64,
    pub from_iso: String,
    pub to_iso: String,
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
    /// Duration in nanoseconds
    pub duration_ns: i64,
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

fn us_to_iso(us: i64) -> String {
    chrono::DateTime::from_timestamp_micros(us)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
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
        window: Window {
            from_ts,
            to_ts,
            from_iso: us_to_iso(from_ts),
            to_iso: us_to_iso(to_ts),
        },
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
    // Query gauge + sum tables (the two that have a scalar `value` column)
    let sql = "SELECT time_bucket(INTERVAL '1 minute', timestamp) AS bucket,
               metric_name,
               approx_quantile(value, 0.95) AS p95,
               avg(value) AS avg,
               count(*) AS n
        FROM (
            SELECT timestamp, metric_name, value, service_name FROM sigiro_metrics_gauge
            UNION ALL
            SELECT timestamp, metric_name, value, service_name FROM sigiro_metrics_sum
        )
        WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        GROUP BY bucket, metric_name
        ORDER BY bucket, metric_name";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params![service, from_ts, to_ts])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(MetricBucket {
            bucket: row.get::<_, String>(0).context("bucket")?,
            metric_name: row.get::<_, String>(1).context("metric_name")?,
            p95: row.get::<_, f64>(2).unwrap_or(0.0),
            avg: row.get::<_, f64>(3).unwrap_or(0.0),
            count: row.get::<_, i64>(4).context("count")?,
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
        results.push(parse_span_row(row)?);
    }
    Ok(results)
}

fn parse_span_row(row: &duckdb::Row<'_>) -> Result<SpanRow> {
    Ok(SpanRow {
        trace_id: row.get::<_, String>(0).context("trace_id")?,
        span_id: row.get::<_, String>(1).context("span_id")?,
        parent_span_id: row.get::<_, String>(2).unwrap_or_default(),
        span_name: row.get::<_, String>(3).context("span_name")?,
        duration_ns: row.get::<_, i64>(4).context("duration")?,
        status_code: row.get::<_, i32>(5).context("status_code")?,
        service_name: row.get::<_, String>(6).context("service_name")?,
    })
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
            severity_text: row.get::<_, String>(1).context("severity_text")?,
            count: row.get::<_, i64>(2).context("count")?,
            first_seen: row.get::<_, String>(3).unwrap_or_default(),
            last_seen: row.get::<_, String>(4).unwrap_or_default(),
        });
    }
    Ok(results)
}

fn build_span_trees(spans: &[SpanRow]) -> BTreeMap<String, Vec<String>> {
    let mut trees: BTreeMap<String, Vec<String>> = BTreeMap::new();
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
    let mut seen = HashSet::new();
    let unique: Vec<&str> = spans
        .iter()
        .filter(|s| s.status_code == 2)
        .filter_map(|s| {
            if seen.insert(s.trace_id.as_str()) {
                Some(s.trace_id.as_str())
            } else {
                None
            }
        })
        .take(5)
        .collect();

    if unique.is_empty() {
        return Ok(Vec::new());
    }

    let placeholders = vec!["?"; unique.len()].join(", ");
    let sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, duration,
                status_code, service_name
         FROM sigiro_spans
         WHERE trace_id IN ({placeholders})
         ORDER BY trace_id, timestamp
         LIMIT 200"
    );

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn duckdb::ToSql> = unique.iter().map(|s| s as &dyn duckdb::ToSql).collect();
    let mut rows = stmt.query(params.as_slice())?;

    let mut trace_map: BTreeMap<String, Vec<SpanRow>> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let span = parse_span_row(row)?;
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
