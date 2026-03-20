use anyhow::{Context, Result};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

/// Tells the agent what data exists and what's missing.
/// Without this, an agent might conclude "no errors in logs" when there ARE no logs —
/// confusing absence of problems with absence of data.
#[derive(Serialize, Debug, PartialEq)]
pub struct Coverage {
    pub has_spans: bool,
    pub has_logs: bool,
    pub has_metrics: bool,
    pub span_count: i64,
    pub log_template_count: i64,
    pub metric_series_count: i64,
    /// Human-readable gap descriptions for the agent.
    /// Empty when all signal types are present.
    pub gaps: Vec<String>,
}

fn compute_coverage(spans_total: i64, logs_total: i64, metrics_count: usize) -> Coverage {
    let has_spans = spans_total > 0;
    let has_logs = logs_total > 0;
    let has_metrics = metrics_count > 0;

    let mut gaps = Vec::new();
    if !has_spans {
        gaps.push("no spans found — service may not be instrumented for tracing".to_string());
    }
    if !has_logs {
        gaps.push("no logs found — consider enabling log collection".to_string());
    }
    if !has_metrics {
        gaps.push("no metrics found — no gauge/sum/histogram data in this window".to_string());
    }
    if has_spans && !has_logs {
        gaps.push("spans exist but no logs — rely on span data for diagnosis".to_string());
    }

    Coverage {
        has_spans,
        has_logs,
        has_metrics,
        span_count: spans_total,
        log_template_count: logs_total,
        metric_series_count: metrics_count as i64,
        gaps,
    }
}

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
    /// Hard limits applied to this response. The AI agent MUST check these fields.
    /// If any total > returned count, the data is incomplete and the agent should
    /// acknowledge this in its analysis or request a narrower time window.
    pub truncation: Truncation,
    /// What data exists and what's missing. The agent MUST check gaps before concluding
    /// "no problems" — absence of data != absence of problems.
    pub coverage: Coverage,
    /// Per-operation aggregates: error rate, percentiles, exemplar trace_id.
    /// This is the primary triage view — the agent should read this FIRST to identify
    /// which operations are broken, then drill into spans/logs for root cause.
    pub operation_summaries: Vec<OperationSummary>,
    pub metrics: Vec<MetricBucket>,
    pub spans: Vec<SpanRow>,
    /// parent_span_id -> [child_span_id]. Root spans (no parent) are not keys.
    pub span_trees: BTreeMap<String, Vec<String>>,
    pub logs: Vec<LogTemplate>,
    pub linked_traces: Vec<LinkedTrace>,
}

/// Surfaces data completeness to the AI agent. Without this, the agent cannot know
/// whether its analysis is based on partial or complete data — leading to confident
/// but incorrect root cause conclusions.
#[derive(Serialize)]
pub struct Truncation {
    /// Total spans matching the query vs. returned (capped at 50)
    pub spans_total: i64,
    pub spans_returned: i64,
    /// Total distinct log templates vs. returned (capped at 30)
    pub logs_total: i64,
    pub logs_returned: i64,
    /// Total error trace_ids found vs. linked (capped at 5)
    pub error_traces_total: i64,
    pub error_traces_returned: i64,
    /// Total linked spans fetched vs. cap (200)
    pub linked_spans_total: i64,
    pub linked_spans_returned: i64,
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

/// Pre-aggregated per-operation summary. 3x more token-efficient than raw spans.
/// An agent can read 10 summaries (~3KB) instead of 50 flat spans (~10KB)
/// and immediately identify which operations are broken.
///
/// Includes baseline comparison from the preceding window (4x the investigation window).
/// Example: investigating last 15m → baseline is the 1h before that.
/// If baseline_error_pct is Some(0.5) and error_pct is 12.0, the agent knows
/// error rate jumped 24x — this is anomalous, not normal.
#[derive(Serialize)]
pub struct OperationSummary {
    pub span_name: String,
    pub count: i64,
    pub error_count: i64,
    pub error_pct: f64,
    pub duration_p50_ns: f64,
    pub duration_p95_ns: f64,
    pub duration_p99_ns: f64,
    /// One real error trace_id for drill-down. None if no errors for this operation.
    pub exemplar_error_trace_id: Option<String>,
    /// Baseline metrics from the preceding window (4x investigation window duration).
    /// None if no historical data exists for this operation.
    pub baseline_error_pct: Option<f64>,
    pub baseline_p95_ns: Option<f64>,
    pub baseline_count: Option<i64>,
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

    let conn = conn.lock().map_err(|e| {
        tracing::error!("DuckDB connection mutex poisoned");
        anyhow::anyhow!("{e}")
    })?;

    let mut metrics =
        query_metrics(&conn, &req.service, from_ts, to_ts).context("metrics query failed")?;
    let histogram_metrics = query_histogram_metrics(&conn, &req.service, from_ts, to_ts)
        .context("histogram metrics query failed")?;
    metrics.extend(histogram_metrics);

    let operation_summaries = query_operation_summaries(&conn, &req.service, from_ts, to_ts)
        .context("operation summaries query failed")?;

    let spans_total = count_spans(&conn, &req.service, from_ts, to_ts)?;
    let spans = query_spans(&conn, &req.service, from_ts, to_ts).context("spans query failed")?;

    let logs_total = count_log_templates(&conn, &req.service, from_ts, to_ts)?;
    let logs = query_logs(&conn, &req.service, from_ts, to_ts).context("logs query failed")?;

    let span_trees = build_span_trees(&spans);

    let error_traces_total = count_error_traces(&conn, &req.service, from_ts, to_ts)?;
    let linked_traces =
        query_linked_traces(&conn, &spans, from_ts, to_ts).context("linked traces query failed")?;
    let linked_spans_returned: i64 = linked_traces.iter().map(|lt| lt.spans.len() as i64).sum();

    let coverage = compute_coverage(spans_total, logs_total, metrics.len());

    Ok(InvestigationBlock {
        service: req.service.clone(),
        window: Window {
            from_ts,
            to_ts,
            from_iso: us_to_iso(from_ts),
            to_iso: us_to_iso(to_ts),
        },
        coverage,
        truncation: Truncation {
            spans_total,
            spans_returned: spans.len() as i64,
            logs_total,
            logs_returned: logs.len() as i64,
            error_traces_total,
            error_traces_returned: linked_traces.len() as i64,
            linked_spans_total: linked_spans_returned, // we don't know the true total without a separate query
            linked_spans_returned,
        },
        operation_summaries,
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
    let sql = "SELECT time_bucket(INTERVAL '1 minute', timestamp)::VARCHAR AS bucket,
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

fn query_histogram_metrics(
    conn: &Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
) -> Result<Vec<MetricBucket>> {
    let sql = "SELECT time_bucket(INTERVAL '1 minute', timestamp)::VARCHAR AS bucket,
               metric_name,
               max(max) AS p95,
               sum(sum)/NULLIF(sum(count),0) AS avg,
               sum(count) AS n
        FROM (
            SELECT timestamp, metric_name, count, sum, max, service_name FROM sigiro_metrics_histogram
            UNION ALL
            SELECT timestamp, metric_name, count, sum, max, service_name FROM sigiro_metrics_exp_histogram
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

fn count_spans(conn: &Connection, service: &str, from_ts: i64, to_ts: i64) -> Result<i64> {
    conn.query_row(
        "SELECT count(*) FROM sigiro_spans
         WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        duckdb::params![service, from_ts, to_ts],
        |row| row.get(0),
    )
    .context("count spans failed")
}

fn count_log_templates(conn: &Connection, service: &str, from_ts: i64, to_ts: i64) -> Result<i64> {
    conn.query_row(
        "SELECT count(DISTINCT body) FROM sigiro_logs
         WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        duckdb::params![service, from_ts, to_ts],
        |row| row.get(0),
    )
    .context("count log templates failed")
}

fn count_error_traces(conn: &Connection, service: &str, from_ts: i64, to_ts: i64) -> Result<i64> {
    conn.query_row(
        "SELECT count(DISTINCT trace_id) FROM sigiro_spans
         WHERE service_name = ? AND status_code = 2
           AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        duckdb::params![service, from_ts, to_ts],
        |row| row.get(0),
    )
    .context("count error traces failed")
}

fn query_operation_summaries(
    conn: &Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
) -> Result<Vec<OperationSummary>> {
    // Baseline window: 4x the investigation window, immediately preceding it.
    // E.g., investigating last 15m → baseline is the preceding 1h.
    let window_us = to_ts - from_ts;
    let baseline_from = from_ts.saturating_sub(window_us * 4);
    let baseline_to = from_ts;

    // Single query: current window LEFT JOIN baseline window.
    // The agent sees both current and baseline side-by-side.
    let sql = "WITH current AS (
            SELECT span_name,
                   count(*) AS cnt,
                   count(*) FILTER (WHERE status_code = 2) AS err_cnt,
                   count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*) AS err_pct,
                   approx_quantile(duration, 0.50) AS p50,
                   approx_quantile(duration, 0.95) AS p95,
                   approx_quantile(duration, 0.99) AS p99,
                   first(trace_id) FILTER (WHERE status_code = 2) AS exemplar_tid
            FROM sigiro_spans
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
            GROUP BY span_name
        ),
        baseline AS (
            SELECT span_name,
                   count(*) AS bl_cnt,
                   count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*) AS bl_err_pct,
                   approx_quantile(duration, 0.95) AS bl_p95
            FROM sigiro_spans
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
            GROUP BY span_name
        )
        SELECT c.span_name, c.cnt, c.err_cnt, c.err_pct, c.p50, c.p95, c.p99, c.exemplar_tid,
               b.bl_err_pct, b.bl_p95, b.bl_cnt
        FROM current c
        LEFT JOIN baseline b ON c.span_name = b.span_name
        ORDER BY c.err_cnt DESC, c.cnt DESC
        LIMIT 20";

    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params![
        service,
        from_ts,
        to_ts,
        service,
        baseline_from,
        baseline_to
    ])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(OperationSummary {
            span_name: row.get::<_, String>(0).context("span_name")?,
            count: row.get::<_, i64>(1).context("count")?,
            error_count: row.get::<_, i64>(2).context("error_count")?,
            error_pct: row.get::<_, f64>(3).unwrap_or(0.0),
            duration_p50_ns: row.get::<_, f64>(4).unwrap_or(0.0),
            duration_p95_ns: row.get::<_, f64>(5).unwrap_or(0.0),
            duration_p99_ns: row.get::<_, f64>(6).unwrap_or(0.0),
            exemplar_error_trace_id: row.get::<_, String>(7).ok(),
            baseline_error_pct: row.get::<_, f64>(8).ok(),
            baseline_p95_ns: row.get::<_, f64>(9).ok(),
            baseline_count: row.get::<_, i64>(10).ok(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_span(trace_id: &str, span_id: &str, parent: &str, status: i32) -> SpanRow {
        SpanRow {
            trace_id: trace_id.to_string(),
            span_id: span_id.to_string(),
            parent_span_id: parent.to_string(),
            span_name: "test".to_string(),
            duration_ns: 1000,
            status_code: status,
            service_name: "svc".to_string(),
        }
    }

    #[test]
    fn build_span_trees_groups_by_parent() {
        let spans = vec![
            make_span("t1", "s1", "", 0),
            make_span("t1", "s2", "s1", 0),
            make_span("t1", "s3", "s1", 0),
            make_span("t1", "s4", "s2", 0),
        ];
        let trees = build_span_trees(&spans);
        assert_eq!(trees.get("s1").unwrap().len(), 2);
        assert_eq!(trees.get("s2").unwrap().len(), 1);
        assert!(!trees.contains_key(""));
    }

    #[test]
    fn build_span_trees_empty() {
        let trees = build_span_trees(&[]);
        assert!(trees.is_empty());
    }

    #[test]
    fn build_span_trees_root_only() {
        let spans = vec![make_span("t1", "s1", "", 0)];
        let trees = build_span_trees(&spans);
        assert!(trees.is_empty());
    }

    #[test]
    fn us_to_iso_valid() {
        let iso = us_to_iso(1_700_000_000_000_000);
        assert!(iso.contains("2023-11"));
    }

    #[test]
    fn us_to_iso_zero() {
        let iso = us_to_iso(0);
        assert!(iso.contains("1970"));
    }

    #[test]
    fn investigate_with_in_memory_db() {
        let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
        crate::schema::create_tables(&conn).expect("create tables");
        let shared = Arc::new(Mutex::new(conn));
        let req = InvestigateRequest {
            service: "nonexistent".to_string(),
            from_ts: None,
            to_ts: None,
        };
        let block = investigate(&shared, &req).expect("investigate should succeed on empty db");
        assert_eq!(block.service, "nonexistent");
        assert!(block.operation_summaries.is_empty());
        assert!(block.metrics.is_empty());
        assert!(block.spans.is_empty());
        assert!(block.logs.is_empty());
        assert!(block.linked_traces.is_empty());
        assert_eq!(block.truncation.spans_total, 0);
        assert_eq!(block.truncation.logs_total, 0);
        assert_eq!(block.truncation.error_traces_total, 0);

        // Coverage: empty db → all gaps present
        assert!(!block.coverage.has_spans);
        assert!(!block.coverage.has_logs);
        assert!(!block.coverage.has_metrics);
        assert_eq!(block.coverage.span_count, 0);
        assert_eq!(block.coverage.log_template_count, 0);
        assert_eq!(block.coverage.metric_series_count, 0);
        assert!(
            block.coverage.gaps.len() >= 3,
            "should have gaps for all signal types"
        );
        assert!(block.coverage.gaps.iter().any(|g| g.contains("no spans")));
        assert!(block.coverage.gaps.iter().any(|g| g.contains("no logs")));
        assert!(block.coverage.gaps.iter().any(|g| g.contains("no metrics")));
    }

    #[test]
    fn investigate_with_real_span_data() {
        let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
        crate::schema::create_tables(&conn).expect("create tables");

        // Insert real span data — simulates what otlp2records would produce
        // Must include all NOT NULL columns: timestamp, end_timestamp, duration, service_name,
        // span_name, span_kind, status_code
        let now_us = chrono::Utc::now().timestamp_micros();
        conn.execute_batch(&format!(
            "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
                parent_span_id, span_name, span_kind, status_code, service_name)
             VALUES
                (make_timestamp({now_us}), {now_us} + 500000000, 500000000, 'trace-1', 'span-1', '', 'POST /checkout', 2, 2, 'checkout'),
                (make_timestamp({now_us}), {now_us} + 200000000, 200000000, 'trace-2', 'span-2', '', 'POST /checkout', 2, 1, 'checkout'),
                (make_timestamp({now_us}), {now_us} + 300000000, 300000000, 'trace-3', 'span-3', '', 'POST /checkout', 2, 2, 'checkout'),
                (make_timestamp({now_us}), {now_us} + 1000000, 1000000, 'trace-4', 'span-4', '', 'GET /health', 2, 1, 'checkout'),
                (make_timestamp({now_us}), {now_us} + 2000000, 2000000, 'trace-5', 'span-5', '', 'GET /health', 2, 1, 'checkout');"
        )).expect("insert spans");

        let shared = Arc::new(Mutex::new(conn));
        let req = InvestigateRequest {
            service: "checkout".to_string(),
            from_ts: Some(now_us - 60_000_000), // 1 min window
            to_ts: Some(now_us + 60_000_000),
        };
        let block = investigate(&shared, &req).expect("investigate");

        // Operation summaries: POST /checkout (3 spans, 2 errors), GET /health (2 spans, 0 errors)
        assert_eq!(block.operation_summaries.len(), 2);
        let checkout_op = &block.operation_summaries[0]; // error-first ordering
        assert_eq!(checkout_op.span_name, "POST /checkout");
        assert_eq!(checkout_op.count, 3);
        assert_eq!(checkout_op.error_count, 2);
        assert!((checkout_op.error_pct - 66.66).abs() < 1.0);
        assert!(checkout_op.exemplar_error_trace_id.is_some());
        // No baseline data exists (all data is in the current window) → None
        assert!(checkout_op.baseline_error_pct.is_none());
        assert!(checkout_op.baseline_p95_ns.is_none());
        assert!(checkout_op.baseline_count.is_none());

        let health_op = &block.operation_summaries[1];
        assert_eq!(health_op.span_name, "GET /health");
        assert_eq!(health_op.count, 2);
        assert_eq!(health_op.error_count, 0);
        assert!(health_op.exemplar_error_trace_id.is_none());

        // Flat spans: all 5 returned, error-first
        assert_eq!(block.spans.len(), 5);
        assert_eq!(block.spans[0].status_code, 2); // errors first
        assert_eq!(block.truncation.spans_total, 5);
        assert_eq!(block.truncation.spans_returned, 5);

        // Linked traces: 2 error traces
        assert_eq!(block.truncation.error_traces_total, 2);

        // Coverage: spans exist, no logs/metrics → appropriate gaps
        assert!(block.coverage.has_spans);
        assert!(!block.coverage.has_logs);
        assert!(!block.coverage.has_metrics);
        assert_eq!(block.coverage.span_count, 5);
        assert!(block.coverage.gaps.iter().any(|g| g.contains("no logs")));
        assert!(
            block
                .coverage
                .gaps
                .iter()
                .any(|g| g.contains("spans exist but no logs"))
        );
        assert!(!block.coverage.gaps.iter().any(|g| g.contains("no spans")));
    }

    #[test]
    fn coverage_no_data() {
        let c = compute_coverage(0, 0, 0);
        assert!(!c.has_spans);
        assert!(!c.has_logs);
        assert!(!c.has_metrics);
        assert!(c.gaps.len() >= 3);
    }

    #[test]
    fn coverage_spans_only() {
        let c = compute_coverage(10, 0, 0);
        assert!(c.has_spans);
        assert!(!c.has_logs);
        assert!(!c.has_metrics);
        // Should have "no logs", "no metrics", and "spans exist but no logs"
        assert!(c.gaps.iter().any(|g| g.contains("spans exist but no logs")));
        assert!(!c.gaps.iter().any(|g| g.contains("no spans")));
    }

    #[test]
    fn coverage_all_signals() {
        let c = compute_coverage(10, 5, 3);
        assert!(c.has_spans);
        assert!(c.has_logs);
        assert!(c.has_metrics);
        assert!(c.gaps.is_empty(), "no gaps when all signals present");
    }

    #[test]
    fn operation_summary_baselines() {
        let conn = duckdb::Connection::open_in_memory().expect("in-memory db");
        crate::schema::create_tables(&conn).expect("create tables");

        let now_us = chrono::Utc::now().timestamp_micros();
        // Baseline window: 10 minutes ago (healthy — 0% errors, fast durations)
        let baseline_ts = now_us - 10 * 60 * 1_000_000;
        // Current window: now (high errors, slow durations)
        conn.execute_batch(&format!(
            "INSERT INTO sigiro_spans (timestamp, end_timestamp, duration, trace_id, span_id,
                parent_span_id, span_name, span_kind, status_code, service_name)
             VALUES
                -- Baseline: healthy
                (make_timestamp({baseline_ts}), {baseline_ts} + 50000000, 50000000, 'bl-1', 'bs-1', '', 'POST /checkout', 2, 1, 'checkout'),
                (make_timestamp({baseline_ts}), {baseline_ts} + 60000000, 60000000, 'bl-2', 'bs-2', '', 'POST /checkout', 2, 1, 'checkout'),
                -- Current: broken
                (make_timestamp({now_us}), {now_us} + 800000000, 800000000, 'cur-1', 'cs-1', '', 'POST /checkout', 2, 2, 'checkout'),
                (make_timestamp({now_us}), {now_us} + 900000000, 900000000, 'cur-2', 'cs-2', '', 'POST /checkout', 2, 2, 'checkout');"
        )).expect("insert spans");

        let shared = Arc::new(Mutex::new(conn));
        // Investigate last 5 minutes → baseline is preceding 20 minutes (covers baseline_ts)
        let req = InvestigateRequest {
            service: "checkout".to_string(),
            from_ts: Some(now_us - 5 * 60 * 1_000_000),
            to_ts: Some(now_us + 60_000_000),
        };
        let block = investigate(&shared, &req).expect("investigate");

        let op = &block.operation_summaries[0];
        assert_eq!(op.span_name, "POST /checkout");
        // Current: 2 spans, both errors
        assert_eq!(op.count, 2);
        assert_eq!(op.error_count, 2);
        assert!((op.error_pct - 100.0).abs() < 0.01);
        // Baseline: 2 spans, 0 errors → 0% error rate
        assert!(op.baseline_error_pct.is_some());
        assert!((op.baseline_error_pct.unwrap() - 0.0).abs() < 0.01);
        assert!(op.baseline_count.is_some());
        assert_eq!(op.baseline_count.unwrap(), 2);
        // Baseline p95 should be around 60ms (60_000_000 ns)
        assert!(op.baseline_p95_ns.is_some());
        assert!(op.baseline_p95_ns.unwrap() < 100_000_000.0); // less than 100ms
    }
}

fn query_linked_traces(
    conn: &Connection,
    spans: &[SpanRow],
    from_ts: i64,
    to_ts: i64,
) -> Result<Vec<LinkedTrace>> {
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

    // ±5 min buffer around the investigation window
    let buffer_us: i64 = 5 * 60 * 1_000_000;
    let linked_from = from_ts.saturating_sub(buffer_us);
    let linked_to = to_ts.saturating_add(buffer_us);

    let placeholders = vec!["?"; unique.len()].join(", ");
    let sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, duration,
                status_code, service_name
         FROM sigiro_spans
         WHERE trace_id IN ({placeholders})
           AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
         ORDER BY trace_id, timestamp
         LIMIT 200"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn duckdb::ToSql>> = unique
        .iter()
        .map(|s| Box::new(s.to_string()) as Box<dyn duckdb::ToSql>)
        .collect();
    params.push(Box::new(linked_from));
    params.push(Box::new(linked_to));
    let param_refs: Vec<&dyn duckdb::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;

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
