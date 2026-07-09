pub mod analysis;

use crate::types::{StatusCode, TimestampMicros};
use anyhow::{Context, Result};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use itertools::Itertools;

// Re-export pure types and functions from analysis submodule.
pub use analysis::{
    AdaptiveConfig, Coverage, LogEntry, LogPattern, ServiceDependency, SignalCoverage, Span,
    SpanKind, TemplateVariable, TimelineEntry, Trace, Window, adaptive_config, build_timeline,
    compute_coverage, us_to_iso,
};

pub struct QueryCtx<'a> {
    pub conn: &'a Connection,
    pub service: &'a str,
    pub from_ts: i64,
    pub to_ts: i64,
    pub prefix: &'a str,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ServiceContext {
    pub service_name: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub namespace: Option<String>,
    pub deployment: Option<String>,
    /// F-26: distinct pod count when `service.instance.id` is present in
    /// resource attributes, else 0. Use `has_instance_id` to distinguish
    /// "no pods identified" from "service not seen".
    pub pod_count: i64,
    pub has_instance_id: bool,
    pub dependencies: Vec<ServiceDependency>,
    pub dependents: Vec<ServiceDependency>,
}

pub fn query_service_context(ctx: &QueryCtx) -> Result<ServiceContext> {
    let meta_sql = format!(
        "SELECT
        ANY_VALUE(resource_attributes::JSON->>'k8s.namespace.name') AS namespace,
        ANY_VALUE(resource_attributes::JSON->>'k8s.deployment.name') AS deployment,
        CASE WHEN count(DISTINCT service_instance_id) > 0 THEN count(DISTINCT service_instance_id) ELSE 0 END AS pod_count,
        (count(DISTINCT service_instance_id) > 0) AS has_instance_id,
        min(timestamp)::VARCHAR AS first_seen,
        max(timestamp)::VARCHAR AS last_seen
    FROM {}sigiro_spans
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
      AND span_name NOT LIKE '%TraceService/Export%'",
        ctx.prefix
    );

    let (namespace, deployment, pod_count, has_instance_id, first_seen, last_seen) = ctx
        .conn
        .query_row(
            &meta_sql,
            duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
            |row| {
                Ok((
                    row.get::<_, String>(0).ok().filter(|s| !s.is_empty()),
                    row.get::<_, String>(1).ok().filter(|s| !s.is_empty()),
                    row.get::<_, i64>(2)?,
                    row.get::<_, bool>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .context("service metadata query failed")?;

    let dependencies = query_service_dependencies(ctx, true)?;
    let dependents = query_service_dependencies(ctx, false)?;

    Ok(ServiceContext {
        service_name: ctx.service.to_string(),
        first_seen,
        last_seen,
        namespace,
        deployment,
        pod_count,
        has_instance_id,
        dependencies,
        dependents,
    })
}

/// Query service dependencies with span_count and error_count.
fn query_service_dependencies(
    ctx: &QueryCtx,
    is_dependency: bool,
) -> Result<Vec<ServiceDependency>> {
    let error_code = StatusCode::Error.as_i32();
    let (_join_cond, _filter_cond) = if is_dependency {
        (
            "s1.trace_id = s2.trace_id",
            "s1.service_name = ? AND s2.service_name != ?",
        )
    } else {
        (
            "s1.trace_id = s2.trace_id",
            "s2.service_name = ? AND s1.service_name != ?",
        )
    };

    let service_param = ctx.service;
    let other_param = ctx.service;

    let export_filter = "AND s1.span_name NOT LIKE '%TraceService/Export%'";
    let sql_with_params = if is_dependency {
        format!(
            "SELECT
                s2.service_name,
                count(*) AS span_count,
                count(CASE WHEN s2.status_code = {error_code} THEN 1 END) AS error_count
            FROM {}sigiro_spans s1
            JOIN {}sigiro_spans s2 ON s1.trace_id = s2.trace_id
            WHERE s1.service_name = ? AND s2.service_name != ?
              AND s1.span_kind IN (3, 4)
              {export_filter}
              AND s1.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
            GROUP BY s2.service_name
            ORDER BY span_count DESC
            LIMIT 20",
            ctx.prefix, ctx.prefix
        )
    } else {
        format!(
            "SELECT
                s1.service_name,
                count(*) AS span_count,
                count(CASE WHEN s1.status_code = {error_code} THEN 1 END) AS error_count
            FROM {}sigiro_spans s1
            JOIN {}sigiro_spans s2 ON s1.trace_id = s2.trace_id
            WHERE s2.service_name = ? AND s1.service_name != ?
              AND s1.span_kind IN (3, 4)
              {export_filter}
              AND s2.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
            GROUP BY s1.service_name
            ORDER BY span_count DESC
            LIMIT 20",
            ctx.prefix, ctx.prefix
        )
    };

    query_collect(
        ctx.conn,
        &sql_with_params,
        duckdb::params![service_param, other_param, ctx.from_ts, ctx.to_ts],
        |row| {
            let service_name = row
                .get::<_, String>("service_name")
                .context("service_name")?;
            let span_count = row.get::<_, i64>("span_count").context("span_count")?;
            let error_count = row.get::<_, i64>("error_count").context("error_count")?;
            let error_rate = if span_count > 0 {
                (error_count as f64 / span_count as f64) * 100.0
            } else {
                0.0
            };
            Ok(ServiceDependency {
                service_name,
                span_count,
                error_count,
                error_rate,
            })
        },
    )
}

// ---------------------------------------------------------------------------
// query_collect<T> — generic query→collect helper
// ---------------------------------------------------------------------------

fn query_collect<T>(
    conn: &Connection,
    sql: &str,
    params: impl duckdb::Params,
    parse_row: impl Fn(&duckdb::Row<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| anyhow::anyhow!("DuckDB prepare failed: {e}\nSQL: {sql}"))?;
    let mut rows = stmt
        .query(params)
        .map_err(|e| anyhow::anyhow!("DuckDB query failed: {e}\nSQL: {sql}"))?;
    let mut results = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|e| anyhow::anyhow!("DuckDB row iteration failed: {e}\nSQL: {sql}"))?
    {
        let row_result = parse_row(row)
            .map_err(|e| anyhow::anyhow!("DuckDB row parsing failed: {e}\nSQL: {sql}"));
        results.push(row_result?);
    }
    Ok(results)
}

// ---------------------------------------------------------------------------
// DerivedEvent — auto-detected deploys and K8s events
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Debug)]
pub struct DerivedEvent {
    pub detected_at: String,
    pub event_type: String,
    pub description: String,
}

pub fn query_derived_events(ctx: &QueryCtx) -> Result<Vec<DerivedEvent>> {
    let mut events = Vec::new();

    let deploy_sql = format!(
        "WITH versioned AS (
        SELECT timestamp, resource_attributes::JSON->>'service.version' AS version
        FROM {}sigiro_spans
        WHERE service_name = ?
          AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
    )
    SELECT min(timestamp)::VARCHAR AS detected_at, version
    FROM versioned
    WHERE version IS NOT NULL
    GROUP BY version
    ORDER BY detected_at",
        ctx.prefix
    );

    let versions = query_collect(
        ctx.conn,
        &deploy_sql,
        duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
        |row| {
            Ok((
                row.get::<_, String>(0).context("deploy detected_at")?,
                row.get::<_, String>(1).context("deploy version")?,
            ))
        },
    )?;

    for (detected_at, version) in &versions {
        events.push(DerivedEvent {
            detected_at: detected_at.clone(),
            event_type: "deploy".to_string(),
            description: format!("version {version} first seen"),
        });
    }

    let k8s_sql = format!(
        "SELECT timestamp::VARCHAR AS detected_at, body AS description
    FROM {}sigiro_logs
    WHERE service_name = ?
      AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
      AND (body LIKE '%OOMKilled%' OR body LIKE '%Unhealthy%'
        OR body LIKE '%BackOff%' OR body LIKE '%FailedScheduling%'
        OR body LIKE '%ScalingReplicaSet%')
    LIMIT 20",
        ctx.prefix
    );

    let k8s_events = query_collect(
        ctx.conn,
        &k8s_sql,
        duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
        |row| {
            Ok(DerivedEvent {
                detected_at: row.get::<_, String>(0).context("k8s event detected_at")?,
                event_type: "k8s_event".to_string(),
                description: row.get::<_, String>(1).context("k8s event description")?,
            })
        },
    )?;
    events.extend(k8s_events);

    Ok(events)
}

// ---------------------------------------------------------------------------
// InvestigateRequest — API input (from_ts/to_ts now mandatory)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct InvestigateRequest {
    pub service: Option<String>,
    pub services: Option<Vec<String>>,
    pub from_ts: i64,
    pub to_ts: i64,
}

const MAX_SERVICES: usize = 10;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct InvestigationBlock {
    pub service: String,
    pub window: Window,
    pub truncation: Truncation,
    pub coverage: Coverage,
    pub context: ServiceContext,
    pub events: Vec<DerivedEvent>,
    pub operation_summaries: Vec<OperationSummary>,
    pub metrics: Vec<MetricBucket>,
    /// Full traces grouped by trace_id.
    pub traces: Vec<Trace>,
    /// Unified timeline merged from spans, events, and logs.
    pub timeline: Vec<TimelineEntry>,
    /// Log patterns (aggregated).
    pub log_patterns: Vec<LogPattern>,
    /// Raw chronological logs (limited).
    pub logs: Vec<LogEntry>,
    pub linked_traces: Vec<Trace>,
    pub profiles: Vec<ProfileSummary>,
}

#[derive(Serialize, Deserialize)]
pub struct Truncation {
    pub spans_total: i64,
    pub spans_returned: i64,
    pub logs_total: i64,
    pub logs_returned: i64,
    pub error_traces_total: i64,
    pub error_traces_returned: i64,
    pub linked_spans_total: i64,
    pub linked_spans_returned: i64,
}

#[derive(Serialize, Deserialize)]
pub struct MetricBucket {
    pub bucket: String,
    pub metric_name: String,
    pub p95: f64,
    pub avg: f64,
    pub count: i64,
}

#[derive(Serialize, Deserialize)]
pub struct ProfileSummary {
    pub function: String,
    pub profile_type: String,
    pub total_value: i64,
    pub unit: String,
    pub sample_count: i64,
}

#[derive(Serialize, Deserialize)]
pub struct OperationSummary {
    pub span_name: String,
    pub count: i64,
    pub error_count: i64,
    pub error_pct: f64,
    pub duration_p50_ns: f64,
    pub duration_p95_ns: f64,
    pub duration_p99_ns: f64,
    pub exemplar_error_trace_id: Option<String>,
    pub baseline_error_pct: Option<f64>,
    pub baseline_p95_ns: Option<f64>,
    pub baseline_count: Option<i64>,
    /// Whether this operation is anomalous compared to baseline.
    pub is_anomalous: Option<bool>,
    /// Reason for anomaly if detected.
    pub anomaly_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Top-level investigate orchestration
// ---------------------------------------------------------------------------

pub fn investigate(
    conn: &Connection,
    req: &InvestigateRequest,
    prefix: &str,
) -> Result<BTreeMap<String, InvestigationBlock>> {
    let services = resolve_services(req)?;
    let mut results = BTreeMap::new();
    for service in &services {
        let block =
            investigate_one(conn, service, req.from_ts, req.to_ts, prefix).map_err(|e| {
                tracing::error!(service = %service, error = %e,
                    "investigation query failed for service"
                );
                anyhow::anyhow!("investigation failed for service {service}: {e}")
            })?;
        results.insert(service.clone(), block);
    }
    Ok(results)
}

fn resolve_services(req: &InvestigateRequest) -> Result<Vec<String>> {
    let services = match (&req.service, &req.services) {
        (Some(s), None) => vec![s.clone()],
        (None, Some(list)) => list.clone(),
        (Some(s), Some(list)) => std::iter::once(s.clone())
            .chain(list.iter().cloned())
            .unique()
            .collect(),
        (None, None) => anyhow::bail!("either 'service' or 'services' must be provided"),
    };
    anyhow::ensure!(!services.is_empty(), "services list must not be empty");
    anyhow::ensure!(
        services.len() <= MAX_SERVICES,
        "max {MAX_SERVICES} services per investigation, got {}",
        services.len()
    );
    Ok(services)
}

#[tracing::instrument(skip(conn), fields(service = %service))]
fn investigate_one(
    conn: &Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
    prefix: &str,
) -> Result<InvestigationBlock> {
    let config = adaptive_config(from_ts, to_ts);
    let ctx = QueryCtx {
        conn,
        service,
        from_ts,
        to_ts,
        prefix,
    };

    let profile_sample_count = count_profile_samples(&ctx)?;
    let (spans_total, error_traces_total) = count_span_stats(&ctx)?;
    let logs_total = count_log_templates(&ctx)?;

    let metrics =
        query_all_metrics(&ctx, config.bucket_interval).context("metrics query failed")?;

    let operation_summaries =
        query_operation_summaries(&ctx).context("operation summaries query failed")?;

    let (spans, traces) = if config.span_limit > 0 {
        let spans = query_full_spans(&ctx, config.span_limit).context("spans query failed")?;
        let traces = group_spans_into_traces(&spans);
        (spans, traces)
    } else {
        (Vec::new(), Vec::new())
    };

    let logs = query_raw_logs(&ctx, 50).context("raw logs query failed")?;
    let mut log_patterns =
        query_log_patterns(&ctx, config.log_limit).context("log patterns query failed")?;
    enrich_pattern_unique_values(&ctx, &mut log_patterns)
        .context("enrich pattern unique values failed")?;
    let linked_traces = query_linked_traces(&ctx, &spans).context("linked traces query failed")?;
    let linked_spans_returned: i64 = linked_traces.iter().map(|lt| lt.spans.len() as i64).sum();

    // Build timeline from spans, events, and logs.
    let derived_events: Vec<(String, String, String)> = query_derived_events(&ctx)?
        .iter()
        .map(|e| {
            (
                e.detected_at.clone(),
                e.event_type.clone(),
                e.description.clone(),
            )
        })
        .collect();
    let timeline = build_timeline(&spans, &derived_events, &logs);

    let profiles = query_profiles(&ctx, 20).context("profiles query failed")?;
    let coverage = compute_coverage(spans_total, logs_total, metrics.len(), profile_sample_count);
    let context = query_service_context(&ctx).context("service context query failed")?;
    let events = query_derived_events(&ctx).context("derived events query failed")?;

    Ok(InvestigationBlock {
        service: service.to_string(),
        window: Window::new(
            TimestampMicros::from_micros(from_ts),
            TimestampMicros::from_micros(to_ts),
        )?,
        coverage,
        context,
        events,
        truncation: Truncation {
            spans_total,
            spans_returned: spans.len() as i64,
            logs_total,
            logs_returned: logs.len() as i64,
            error_traces_total,
            error_traces_returned: linked_traces.len() as i64,
            linked_spans_total: linked_spans_returned,
            linked_spans_returned,
        },
        operation_summaries,
        metrics,
        traces,
        timeline,
        log_patterns,
        logs,
        linked_traces,
        profiles,
    })
}

/// Groups spans into traces.
fn group_spans_into_traces(spans: &[Span]) -> Vec<Trace> {
    let mut trace_map: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for span in spans {
        trace_map
            .entry(span.trace_id.clone())
            .or_default()
            .push(span.clone());
    }

    let mut traces = Vec::new();
    for (trace_id, mut trace_spans) in trace_map {
        if trace_spans.is_empty() {
            continue;
        }
        // Find root span (no parent).
        let root_idx = trace_spans
            .iter()
            .position(|s| s.parent_span_id.is_empty())
            .unwrap_or(0);
        let root_span = trace_spans.remove(root_idx);
        let trace = Trace::new(trace_id, root_span, trace_spans);
        traces.push(trace);
    }

    traces
}

// ---------------------------------------------------------------------------
// Query functions
// ---------------------------------------------------------------------------

fn query_all_metrics(ctx: &QueryCtx, bucket_interval: &str) -> Result<Vec<MetricBucket>> {
    let sql = format!(
        "WITH scalar AS (
            SELECT timestamp, metric_name, service_name,
                   value AS p95_src, value AS avg_src, 1 AS cnt
            FROM {}sigiro_metrics_gauge
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
            UNION ALL
            SELECT timestamp, metric_name, service_name,
                   value, value, 1
            FROM {}sigiro_metrics_sum
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        ),
        histo AS (
            SELECT timestamp, metric_name, service_name,
                   max AS p95_src, CASE WHEN count > 0 THEN sum/count ELSE 0 END AS avg_src, count AS cnt
            FROM {}sigiro_metrics_histogram
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
            UNION ALL
            SELECT timestamp, metric_name, service_name,
                   max, CASE WHEN count > 0 THEN sum/count ELSE 0 END, count
            FROM {}sigiro_metrics_exp_histogram
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        ),
        combined AS (
            SELECT timestamp, metric_name, p95_src, avg_src, cnt FROM scalar
            UNION ALL
            SELECT timestamp, metric_name, p95_src, avg_src, cnt FROM histo
        )
        SELECT time_bucket(INTERVAL '{bucket_interval}', timestamp)::VARCHAR AS bucket,
               metric_name,
               approx_quantile(p95_src, 0.95) AS p95,
               avg(avg_src) AS avg,
               sum(cnt) AS n
        FROM combined
        GROUP BY bucket, metric_name
        ORDER BY bucket, metric_name",
        ctx.prefix, ctx.prefix, ctx.prefix, ctx.prefix
    );

    query_collect(
        ctx.conn,
        &sql,
        duckdb::params![
            ctx.service,
            ctx.from_ts,
            ctx.to_ts,
            ctx.service,
            ctx.from_ts,
            ctx.to_ts,
            ctx.service,
            ctx.from_ts,
            ctx.to_ts,
            ctx.service,
            ctx.from_ts,
            ctx.to_ts
        ],
        |row| {
            Ok(MetricBucket {
                bucket: row.get::<_, String>("bucket").context("bucket")?,
                metric_name: row.get::<_, String>("metric_name").context("metric_name")?,
                p95: row.get::<_, f64>("p95").context("p95")?,
                avg: row.get::<_, f64>("avg").context("avg")?,
                count: row.get::<_, i64>("n").context("n")?,
            })
        },
    )
}

fn count_span_stats(ctx: &QueryCtx) -> Result<(i64, i64)> {
    ctx.conn
        .query_row(
            &format!(
                "SELECT count(*) AS total,
                count(DISTINCT CASE WHEN status_code = {} THEN trace_id END) AS error_count
         FROM {}sigiro_spans
         WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
           AND span_name NOT LIKE '%TraceService/Export%'",
                StatusCode::Error.as_i32(),
                ctx.prefix
            ),
            duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
            |row| {
                Ok((
                    row.get::<_, i64>("total")?,
                    row.get::<_, i64>("error_count")?,
                ))
            },
        )
        .context("count span stats failed")
}

fn count_log_templates(ctx: &QueryCtx) -> Result<i64> {
    ctx.conn
        .query_row(
            &format!(
                "SELECT count(DISTINCT body) FROM {}sigiro_logs
         WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
                ctx.prefix
            ),
            duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
            |row| row.get(0),
        )
        .context("count log templates failed")
}

fn query_operation_summaries(ctx: &QueryCtx) -> Result<Vec<OperationSummary>> {
    let window_us = ctx.to_ts - ctx.from_ts;
    let baseline_from = ctx.from_ts.saturating_sub(window_us * 4);
    let baseline_to = ctx.from_ts;

    let error_code = StatusCode::Error.as_i32();
    let sql = format!(
        "WITH current AS (
             SELECT span_name,
                    count(*) AS cnt,
                   count(CASE WHEN status_code = {error_code} THEN 1 END) AS err_cnt,
                   count(CASE WHEN status_code = {error_code} THEN 1 END) * 100.0 / count(*) AS err_pct,
                   approx_quantile(duration, 0.50) AS p50,
                   approx_quantile(duration, 0.95) AS p95,
                   approx_quantile(duration, 0.99) AS p99,
                   first(CASE WHEN status_code = {error_code} THEN trace_id END) AS exemplar_error_trace_id
            FROM {}sigiro_spans
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
              AND span_name NOT LIKE '%TraceService/Export%'
            GROUP BY span_name
        ),
        baseline AS (
            SELECT span_name,
                   count(*) AS bl_cnt,
                   count(CASE WHEN status_code = {error_code} THEN 1 END) * 100.0 / count(*) AS bl_err_pct,
                   approx_quantile(duration, 0.95) AS bl_p95
            FROM {}sigiro_spans
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
              AND span_name NOT LIKE '%TraceService/Export%'
            GROUP BY span_name
        )
        SELECT c.span_name, c.cnt, c.err_cnt, c.err_pct, c.p50, c.p95, c.p99, c.exemplar_error_trace_id,
               b.bl_err_pct, b.bl_p95, b.bl_cnt
        FROM current c
        LEFT JOIN baseline b ON c.span_name = b.span_name
        ORDER BY c.err_cnt DESC, c.cnt DESC
        LIMIT 20",
        ctx.prefix, ctx.prefix
    );

    query_collect(
        ctx.conn,
        &sql,
        duckdb::params![
            ctx.service,
            ctx.from_ts,
            ctx.to_ts,
            ctx.service,
            baseline_from,
            baseline_to
        ],
        |row| {
            let span_name = row.get::<_, String>("span_name").context("span_name")?;
            let count = row.get::<_, i64>("cnt").context("cnt")?;
            let error_count = row.get::<_, i64>("err_cnt").context("err_cnt")?;
            let error_pct = row.get::<_, f64>("err_pct").context("err_pct")?;
            let duration_p50_ns = row.get::<_, f64>("p50").context("p50")?;
            let duration_p95_ns = row.get::<_, f64>("p95").context("p95")?;
            let duration_p99_ns = row.get::<_, f64>("p99").context("p99")?;
            let exemplar_error_trace_id =
                row.get::<_, Option<String>>("exemplar_error_trace_id")?;
            let baseline_error_pct = row.get::<_, Option<f64>>("bl_err_pct")?;
            let baseline_p95_ns = row.get::<_, Option<f64>>("bl_p95")?;
            let baseline_count = row.get::<_, Option<i64>>("bl_cnt")?;

            // Detect anomalies.
            let is_anomalous = baseline_error_pct.map(|be| error_pct > be * 2.0 && error_pct > 5.0);

            let anomaly_reason = is_anomalous.filter(|&a| a).map(|_| {
                if let Some(be) = baseline_error_pct {
                    format!(
                        "error rate {:.1}% is {}x higher than baseline {:.1}%",
                        error_pct,
                        (error_pct / be.max(0.1)).round() as i32,
                        be
                    )
                } else {
                    format!("error rate {:.1}% exceeds threshold", error_pct)
                }
            });

            Ok(OperationSummary {
                span_name,
                count,
                error_count,
                error_pct,
                duration_p50_ns,
                duration_p95_ns,
                duration_p99_ns,
                exemplar_error_trace_id,
                baseline_error_pct,
                baseline_p95_ns,
                baseline_count,
                is_anomalous,
                anomaly_reason,
            })
        },
    )
}

/// Query full span data with all attributes.
fn query_full_spans(ctx: &QueryCtx, limit: i64) -> Result<Vec<Span>> {
    let error_code = StatusCode::Error.as_i32();
    let sql = format!(
        "SELECT
            trace_id, span_id, parent_span_id, span_name, duration,
            status_code, service_name, span_kind,
            span_attributes, resource_attributes,
            timestamp AS start_timestamp,
            end_timestamp,
            events_json, links_json,
            scope_name, scope_version,
            trace_state, scope_attributes
        FROM {}sigiro_spans
        WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
          AND span_name NOT LIKE '%TraceService/Export%'
        ORDER BY CASE WHEN status_code = {error_code} THEN 0 ELSE 1 END, duration DESC
        LIMIT {limit}",
        ctx.prefix
    );

    query_collect(
        ctx.conn,
        &sql,
        duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
        parse_full_span,
    )
}

fn parse_full_span(row: &duckdb::Row<'_>) -> Result<Span> {
    let span_kind_val: Option<i32> = row.get(7).ok();
    let span_kind = span_kind_val.and_then(|v| SpanKind::from_repr(v as usize));

    Ok(Span {
        trace_id: row.get::<_, String>(0).context("trace_id")?,
        span_id: row.get::<_, String>(1).context("span_id")?,
        parent_span_id: row.get::<_, String>(2).unwrap_or_default(),
        span_name: row.get::<_, String>(3).context("span_name")?,
        duration_us: row.get::<_, i64>(4).context("duration")?,
        status_code: StatusCode::from_i32(row.get::<_, i32>(5).context("status_code")?),
        service_name: row.get::<_, String>(6).context("service_name")?,
        span_kind,
        span_attributes: row.get::<_, Option<String>>(8)?,
        resource_attributes: row.get::<_, Option<String>>(9)?,
        start_timestamp: row.get::<_, i64>(10).context("start_timestamp")?,
        end_timestamp: row.get::<_, i64>(11).context("end_timestamp")?,
        events_json: row.get::<_, Option<String>>(12)?,
        links_json: row.get::<_, Option<String>>(13)?,
        instrumentation_library_name: row.get::<_, Option<String>>(14)?,
        instrumentation_library_version: row.get::<_, Option<String>>(15)?,
        trace_state: row.get::<_, Option<String>>(16)?,
        scope_span: None,
    })
}

/// Query raw logs in chronological order (limit 50).
pub fn query_raw_logs(ctx: &QueryCtx, limit: i64) -> Result<Vec<LogEntry>> {
    let sql = format!(
        "SELECT
            l.timestamp,
            l.timestamp::VARCHAR AS timestamp_iso,
            l.body,
            COALESCE(l.severity_text, 'INFO') AS severity_text,
            l.trace_id,
            l.span_id,
            l.log_attributes,
            l.scope_name,
            l.scope_version,
            l.resource_attributes,
            l.template_values
        FROM {prefix}sigiro_logs l
        WHERE l.service_name = ? AND l.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        ORDER BY l.timestamp DESC
        LIMIT ?",
        prefix = ctx.prefix
    );

    query_collect(
        ctx.conn,
        &sql,
        duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts, limit],
        |row| {
            Ok(LogEntry {
                timestamp: row.get::<_, i64>(0).context("timestamp")?,
                timestamp_iso: row.get::<_, String>(1).context("timestamp_iso")?,
                body: row.get::<_, String>(2).context("body")?,
                severity_text: row.get::<_, String>(3).context("severity_text")?,
                trace_id: row.get::<_, Option<String>>(4)?,
                span_id: row.get::<_, Option<String>>(5)?,
                attributes_json: row.get::<_, Option<String>>(6)?,
                instrumentation_library_name: row.get::<_, Option<String>>(7)?,
                instrumentation_library_version: row.get::<_, Option<String>>(8)?,
                flags: None,
                resource_attributes_json: row.get::<_, Option<String>>(9)?,
                template_values: row.get::<_, Option<String>>(10)?,
            })
        },
    )
}

/// Query log patterns (aggregated by template).
#[doc(hidden)]
pub fn query_log_patterns(ctx: &QueryCtx, limit: i64) -> Result<Vec<LogPattern>> {
    let sql = format!(
        "SELECT
            COALESCE(NULLIF(t.template_text, ''), l.body) AS pattern,
            first(l.template_id) AS template_id,
            first(l.body) AS example_body,
            first(l.template_values) AS template_values,
            l.severity_text,
            count(*) AS count,
            min(l.timestamp)::VARCHAR AS first_seen,
            max(l.timestamp)::VARCHAR AS last_seen,
            count(CASE WHEN l.severity_text IN ('ERROR', 'ERR', 'FATAL', 'CRITICAL') THEN 1 END) AS error_count
        FROM {prefix}sigiro_logs l
        LEFT JOIN {prefix}sigiro_log_templates t ON l.template_id = t.template_id
        WHERE l.service_name = ? AND l.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        GROUP BY pattern, l.severity_text
        ORDER BY count DESC
        LIMIT ?",
        prefix = ctx.prefix
    );

    query_collect(
        ctx.conn,
        &sql,
        duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts, limit],
        |row| {
            let pattern = row.get::<_, String>("pattern").context("pattern")?;
            let template_id = row.get::<_, Option<u64>>("template_id")?;
            let example_body = row.get::<_, Option<String>>("example_body")?;
            let template_values = row.get::<_, Option<String>>("template_values")?;
            let severity_text = row
                .get::<_, String>("severity_text")
                .context("severity_text")?;
            let count = row.get::<_, i64>("count").context("count")?;
            let first_seen = row.get::<_, Option<String>>("first_seen")?;
            let last_seen = row.get::<_, Option<String>>("last_seen")?;
            let error_count = row.get::<_, i64>("error_count").unwrap_or(0);

            // Build a drill-down query the agent can use to fetch all raw logs
            // for this pattern in chronological order.
            let drill_down_sql = template_id.map(|tid| {
                format!(
                    "SELECT * FROM {prefix}sigiro_logs \\
                     WHERE service_name = '{svc}' \\
                       AND template_id = {tid} \\
                       AND timestamp BETWEEN make_timestamp({from}) AND make_timestamp({to}) \\
                     ORDER BY timestamp ASC",
                    prefix = ctx.prefix,
                    svc = ctx.service,
                    tid = tid,
                    from = ctx.from_ts,
                    to = ctx.to_ts,
                )
            });

            Ok(LogPattern {
                pattern,
                template_id,
                drill_down_sql,
                example_body,
                template_values,
                variables: Vec::new(),
                severity_text,
                count,
                first_seen,
                last_seen,
                error_count,
            })
        },
    )
}
/// Enrich log patterns with structured variable slots.
/// Queries all template_values for each pattern, parses wildcards from the template,
/// and aggregates unique values per slot.
pub fn enrich_pattern_unique_values(ctx: &QueryCtx, patterns: &mut [LogPattern]) -> Result<()> {
    if patterns.is_empty() {
        return Ok(());
    }

    // Build a map from pattern string -> (template_id, Vec<Vec<values>>)
    let mut pattern_map: std::collections::HashMap<String, (Option<u64>, Vec<Vec<String>>)> =
        std::collections::HashMap::new();

    // Seed with template_ids from patterns
    for p in patterns.iter() {
        pattern_map.insert(p.pattern.clone(), (p.template_id, Vec::new()));
    }

    // Query all non-empty template_values for each pattern
    let sql = format!(
        "SELECT
            COALESCE(NULLIF(t.template_text, ''), l.body) AS pattern,
            l.template_values
        FROM {prefix}sigiro_logs l
        LEFT JOIN {prefix}sigiro_log_templates t ON l.template_id = t.template_id
        WHERE l.service_name = ?
          AND l.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
          AND l.template_values IS NOT NULL
          AND l.template_values != '[]'",
        prefix = ctx.prefix
    );

    let mut stmt = ctx
        .conn
        .prepare(&sql)
        .context("prepare enrich_pattern_unique_values")?;
    let mut rows = stmt.query(duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts])?;

    while let Some(row) = rows.next()? {
        let pattern: String = row.get(0)?;
        let values_json: String = row.get(1)?;
        let values: Vec<String> =
            serde_json::from_str(&values_json).context("deserialize values_json")?;

        if let Some((_, slots)) = pattern_map.get_mut(&pattern) {
            // Ensure we have enough slots
            for (pos, val) in values.into_iter().enumerate() {
                if slots.len() <= pos {
                    slots.push(Vec::new());
                }
                let slot = &mut slots[pos];
                if !slot.contains(&val) && slot.len() < 50 {
                    slot.push(val);
                }
            }
        }
    }

    // Build TemplateVariable structs for each pattern
    for pattern in patterns.iter_mut() {
        let template = &pattern.pattern;
        let tokens: Vec<&str> = template.split_whitespace().collect();

        let mut vars = Vec::new();
        for (pos, token) in tokens.iter().enumerate() {
            if token.contains("<*>")
                || token.contains("<NUM>")
                || token.contains("<IP>")
                || token.contains("<UUID>")
                || token.contains("<HEX>")
            {
                let ctx_before = pos
                    .checked_sub(1)
                    .and_then(|p| tokens.get(p))
                    .copied()
                    .unwrap_or("");
                let ctx_after = tokens.get(pos + 1).copied().unwrap_or("");
                vars.push(TemplateVariable {
                    wildcard_index: vars.len(),
                    token_position: pos,
                    context_before: ctx_before.to_string(),
                    context_after: ctx_after.to_string(),
                    values: Vec::new(),
                });
            }
        }

        // Populate values from the aggregated slots
        if let Some((_, slots)) = pattern_map.remove(&pattern.pattern) {
            for (i, slot_values) in slots.into_iter().enumerate() {
                if let Some(var) = vars.get_mut(i) {
                    var.values = slot_values;
                }
            }
        }

        pattern.variables = vars;
    }

    Ok(())
}
fn count_profile_samples(ctx: &QueryCtx) -> Result<i64> {
    let sql = format!(
        "SELECT count(*) FROM {}sigiro_profiles
         WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        ctx.prefix
    );
    ctx.conn
        .query_row(
            &sql,
            duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
            |r| r.get(0),
        )
        .context("profile count query failed")
}

pub fn query_profiles(ctx: &QueryCtx, limit: i64) -> Result<Vec<ProfileSummary>> {
    let sql = format!(
        "SELECT
            stack_trace[len(stack_trace)] AS function,
            COALESCE(profile_type, 'unknown') AS profile_type,
            SUM(value) AS total_value,
            COALESCE(first(unit), '') AS unit,
            count(*) AS sample_count
        FROM {prefix}sigiro_profiles
        WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
          AND len(stack_trace) > 0
        GROUP BY function, profile_type
        ORDER BY total_value DESC
        LIMIT ?",
        prefix = ctx.prefix
    );

    query_collect(
        ctx.conn,
        &sql,
        duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts, limit],
        |row| {
            Ok(ProfileSummary {
                function: row.get::<_, String>(0).context("function")?,
                profile_type: row.get::<_, String>(1).context("profile_type")?,
                total_value: row.get::<_, i64>(2).context("total_value")?,
                unit: row.get::<_, String>(3).context("unit")?,
                sample_count: row.get::<_, i64>(4).context("sample_count")?,
            })
        },
    )
}

fn query_linked_traces(ctx: &QueryCtx, spans: &[Span]) -> Result<Vec<Trace>> {
    let unique: Vec<&str> = spans
        .iter()
        .filter(|s| s.is_error())
        .map(|s| s.trace_id.as_str())
        .unique()
        .take(5)
        .collect();

    if unique.is_empty() {
        return Ok(Vec::new());
    }

    let buffer_us: i64 = 5 * 60 * 1_000_000;
    let linked_from = ctx.from_ts.saturating_sub(buffer_us);
    let linked_to = ctx.to_ts.saturating_add(buffer_us);

    let placeholders = vec!["?"; unique.len()].join(", ");
    let sql = format!(
        "SELECT
            trace_id, span_id, parent_span_id, span_name, duration,
            status_code, service_name, span_kind,
            span_attributes, resource_attributes,
            timestamp AS start_timestamp,
            end_timestamp,
            events_json, links_json,
            scope_name, scope_version,
            trace_state, scope_attributes
         FROM {}sigiro_spans
         WHERE trace_id IN ({placeholders})
           AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
         ORDER BY trace_id, timestamp
         LIMIT 200",
        ctx.prefix
    );

    let mut stmt = ctx.conn.prepare(&sql)?;
    let mut params: Vec<Box<dyn duckdb::ToSql>> = unique
        .iter()
        .map(|s| Box::new(s.to_string()) as Box<dyn duckdb::ToSql>)
        .collect();
    params.push(Box::new(linked_from));
    params.push(Box::new(linked_to));
    let param_refs: Vec<&dyn duckdb::ToSql> = params.iter().map(AsRef::as_ref).collect();
    let mut rows = stmt.query(param_refs.as_slice())?;

    let mut trace_spans_map: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    while let Some(row) = rows.next()? {
        let span = parse_full_span(row)?;
        trace_spans_map
            .entry(span.trace_id.clone())
            .or_default()
            .push(span);
    }

    let mut traces = Vec::new();
    for (trace_id, mut spans) in trace_spans_map {
        if spans.is_empty() {
            continue;
        }
        let root_idx = spans
            .iter()
            .position(|s| s.parent_span_id.is_empty())
            .unwrap_or(0);
        let root_span = spans.remove(root_idx);
        let trace = Trace::new(trace_id, root_span, spans);
        traces.push(trace);
    }

    Ok(traces)
}
