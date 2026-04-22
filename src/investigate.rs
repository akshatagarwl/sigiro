use anyhow::{Context, Result};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};

/// Common query parameters shared by all investigation sub-queries.
/// Tiger Style: functions should have ≤4 parameters — this struct bundles
/// the 5 common params (conn, service, from_ts, to_ts, prefix) into one.
pub struct QueryCtx<'a> {
    pub conn: &'a Connection,
    pub service: &'a str,
    pub from_ts: i64,
    pub to_ts: i64,
    pub prefix: &'a str,
}

/// Tells the agent what data exists and what's missing.
/// Without this, an agent might conclude "no errors in logs" when there ARE no logs —
/// confusing absence of problems with absence of data.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Coverage {
    pub has_spans: bool,
    pub has_logs: bool,
    pub has_metrics: bool,
    pub has_profiles: bool,
    pub span_count: i64,
    pub log_template_count: i64,
    pub metric_series_count: i64,
    pub profile_sample_count: i64,
    /// Human-readable gap descriptions for the agent.
    /// Empty when all signal types are present.
    pub gaps: Vec<String>,
}

pub fn compute_coverage(
    spans_total: i64,
    logs_total: i64,
    metrics_count: usize,
    profile_samples: i64,
) -> Coverage {
    let has_spans = spans_total > 0;
    let has_logs = logs_total > 0;
    let has_metrics = metrics_count > 0;
    let has_profiles = profile_samples > 0;

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
        has_profiles,
        span_count: spans_total,
        log_template_count: logs_total,
        metric_series_count: metrics_count as i64,
        profile_sample_count: profile_samples,
        gaps,
    }
}

/// Auto-derived service metadata and topology from existing OTLP data.
/// Agent gets namespace, pod count, and service dependencies in 1 round instead of 3.
#[derive(Serialize, Deserialize, Debug)]
pub struct ServiceContext {
    pub service_name: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    /// k8s.namespace.name from resource_attributes (if present)
    pub namespace: Option<String>,
    /// k8s.deployment.name from resource_attributes (if present)
    pub deployment: Option<String>,
    /// Distinct service_instance_id values in the window
    pub pod_count: i64,
    /// Services this service calls (downstream, from CLIENT/PRODUCER spans)
    pub dependencies: Vec<String>,
    /// Services that call this service (upstream, from CLIENT/PRODUCER spans)
    pub dependents: Vec<String>,
}

pub fn query_service_context(ctx: &QueryCtx) -> Result<ServiceContext> {
    // Query 1: resource attribute extraction
    // ANY_VALUE because resource_attributes varies per span but we want one representative value
    let meta_sql = format!(
        "SELECT
        ANY_VALUE(resource_attributes::JSON->>'k8s.namespace.name') AS namespace,
        ANY_VALUE(resource_attributes::JSON->>'k8s.deployment.name') AS deployment,
        count(DISTINCT service_instance_id) AS pod_count,
        min(timestamp)::VARCHAR AS first_seen,
        max(timestamp)::VARCHAR AS last_seen
    FROM {}sigiro_spans
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        ctx.prefix
    );

    let (namespace, deployment, pod_count, first_seen, last_seen) = ctx
        .conn
        .query_row(
            &meta_sql,
            duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
            |row| {
                Ok((
                    // namespace and deployment are genuinely nullable (not all services run in k8s)
                    row.get::<_, String>(0).ok().filter(|s| !s.is_empty()),
                    row.get::<_, String>(1).ok().filter(|s| !s.is_empty()),
                    row.get::<_, i64>(2)?,
                    // first_seen/last_seen are NULL when no spans match (min/max of empty set)
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .context("service metadata query failed")?;

    // Query 2: downstream dependencies (services this service calls)
    let deps_sql = format!(
        "SELECT DISTINCT s2.service_name
        FROM {}sigiro_spans s1
        JOIN {}sigiro_spans s2 ON s1.trace_id = s2.trace_id
        WHERE s1.service_name = ? AND s2.service_name != ?
          AND s1.span_kind IN (3, 4)
          AND s1.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        LIMIT 20",
        ctx.prefix, ctx.prefix
    );
    let dependencies = query_string_list(ctx.conn, &deps_sql, ctx.service, ctx.from_ts, ctx.to_ts)?;

    // Query 3: upstream dependents (services that call this service)
    let dependents_sql = format!(
        "SELECT DISTINCT s1.service_name
        FROM {}sigiro_spans s1
        JOIN {}sigiro_spans s2 ON s1.trace_id = s2.trace_id
        WHERE s2.service_name = ? AND s1.service_name != ?
          AND s1.span_kind IN (3, 4)
          AND s2.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        LIMIT 20",
        ctx.prefix, ctx.prefix
    );
    let dependents = query_string_list(
        ctx.conn,
        &dependents_sql,
        ctx.service,
        ctx.from_ts,
        ctx.to_ts,
    )?;

    Ok(ServiceContext {
        service_name: ctx.service.to_string(),
        first_seen,
        last_seen,
        namespace,
        deployment,
        pod_count,
        dependencies,
        dependents,
    })
}

#[allow(clippy::too_many_arguments)] // Internal helper with varying SQL; QueryCtx doesn't apply
fn query_string_list(
    conn: &Connection,
    sql: &str,
    service: &str,
    from_ts: i64,
    to_ts: i64,
) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(duckdb::params![service, service, from_ts, to_ts])?;
    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        if let Ok(s) = row.get::<_, String>(0) {
            results.push(s);
        }
    }
    Ok(results)
}

/// Auto-detected event from OTLP data — deploys (version changes) and K8s events
/// (OOMKilled, BackOff, etc.). Correlates errors to recent changes without webhooks.
/// Limitation: only detects events present in OTLP data. External events (GitHub
/// deploys, paging tools) need an optional webhook (not MVP). Most high-value events
/// (deploys, OOM, restarts) DO appear in OTLP resource_attributes and logs.
#[derive(Serialize, Deserialize, Debug)]
pub struct DerivedEvent {
    pub detected_at: String,
    pub event_type: String,
    pub description: String,
}

pub fn query_derived_events(ctx: &QueryCtx) -> Result<Vec<DerivedEvent>> {
    let mut events = Vec::new();

    // Deploy detection: distinct service.version values in the window.
    // Uses a CTE to extract version first, then groups — avoids DuckDB casting issues
    // with repeated ::JSON casts in GROUP BY.
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

    let mut stmt = ctx.conn.prepare(&deploy_sql)?;
    let mut rows = stmt.query(duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts])?;
    let mut versions = Vec::new();
    while let Some(row) = rows.next()? {
        let detected_at: String = row.get::<_, String>(0).context("deploy detected_at")?;
        let version: String = row.get::<_, String>(1).context("deploy version")?;
        versions.push((detected_at, version));
    }
    // If multiple versions exist, each is a deploy event
    for (detected_at, version) in &versions {
        events.push(DerivedEvent {
            detected_at: detected_at.clone(),
            event_type: "deploy".to_string(),
            description: format!("version {version} first seen"),
        });
    }

    // K8s events from log body patterns
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

    let mut stmt = ctx.conn.prepare(&k8s_sql)?;
    let mut rows = stmt.query(duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts])?;
    while let Some(row) = rows.next()? {
        events.push(DerivedEvent {
            detected_at: row.get::<_, String>(0).context("k8s event detected_at")?,
            event_type: "k8s_event".to_string(),
            description: row.get::<_, String>(1).context("k8s event description")?,
        });
    }

    Ok(events)
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct InvestigateRequest {
    /// Single service (convenience — mutually exclusive with `services`)
    #[serde(default)]
    pub service: Option<String>,
    /// Multiple services (max 10)
    #[serde(default)]
    pub services: Option<Vec<String>>,
    pub from_ts: Option<i64>,
    pub to_ts: Option<i64>,
}

const MAX_SERVICES: usize = 10;

#[derive(Serialize, Deserialize)]
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
    /// Auto-derived service metadata + topology. Agent gets namespace, pod count,
    /// and service dependencies without a separate API call.
    pub context: ServiceContext,
    /// Auto-detected deploys and K8s events. Agent correlates errors to recent changes.
    pub events: Vec<DerivedEvent>,
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
    /// Top functions from continuous profiles, ranked by self-time.
    pub profiles: Vec<ProfileSummary>,
}

/// Surfaces data completeness to the AI agent. Without this, the agent cannot know
/// whether its analysis is based on partial or complete data — leading to confident
/// but incorrect root cause conclusions.
#[derive(Serialize, Deserialize)]
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

#[derive(Serialize, Deserialize)]
pub struct Window {
    pub from_ts: i64,
    pub to_ts: i64,
    pub from_iso: String,
    pub to_iso: String,
}

#[derive(Serialize, Deserialize)]
pub struct MetricBucket {
    pub bucket: String,
    pub metric_name: String,
    pub p95: f64,
    pub avg: f64,
    pub count: i64,
}

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize)]
pub struct LogTemplate {
    /// Normalized template with UUIDs, IPs, and long numbers replaced by placeholders.
    /// Multiple log lines that differ only in dynamic values collapse to one template.
    pub template: String,
    /// First-seen exemplar body (original, unnormalized) for this template.
    pub body: String,
    pub severity_text: String,
    pub count: i64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct LinkedTrace {
    pub trace_id: String,
    pub spans: Vec<SpanRow>,
}

/// Top function from continuous profiling, ranked by total self-time.
#[derive(Serialize, Deserialize)]
pub struct ProfileSummary {
    pub function: String,
    pub profile_type: String,
    pub total_value: i64,
    pub unit: String,
    pub sample_count: i64,
}

/// Pre-aggregated per-operation summary. 3x more token-efficient than raw spans.
/// An agent can read 10 summaries (~3KB) instead of 50 flat spans (~10KB)
/// and immediately identify which operations are broken. Includes baseline
/// comparison from the preceding window (4x the investigation window).
/// Example: investigating last 15m → baseline is the 1h before that.
/// If baseline_error_pct is Some(0.5) and error_pct is 12.0, the agent knows
/// error rate jumped 24x — this is anomalous, not normal.
#[derive(Serialize, Deserialize)]
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

pub fn us_to_iso(us: i64) -> Result<String> {
    chrono::DateTime::from_timestamp_micros(us)
        .map(|dt| dt.to_rfc3339())
        .ok_or_else(|| anyhow::anyhow!("invalid timestamp_micros: {us}"))
}

/// Adaptive summarization: vary detail level based on window size.
/// Larger windows get coarser buckets and fewer exemplars to stay under token budget.
/// | Window    | Metric buckets | Span exemplars | Log limit |
/// |-----------|----------------|----------------|-----------|
/// | < 15m     | 1 minute       | 50             | 30        |
/// | 15m - 2h  | 5 minutes      | 20             | 30        |
/// | > 2h      | 1 hour         | 0 (summaries)  | 20        |
pub struct AdaptiveConfig {
    pub bucket_interval: &'static str,
    pub span_limit: i64,
    pub log_limit: i64,
}

pub fn adaptive_config(from_ts: i64, to_ts: i64) -> AdaptiveConfig {
    let window_us = to_ts - from_ts;
    let fifteen_min = 15 * 60 * 1_000_000_i64;
    let two_hours = 2 * 3600 * 1_000_000_i64;

    if window_us <= fifteen_min {
        AdaptiveConfig {
            bucket_interval: "1 minute",
            span_limit: 50,
            log_limit: 30,
        }
    } else if window_us <= two_hours {
        AdaptiveConfig {
            bucket_interval: "5 minutes",
            span_limit: 20,
            log_limit: 30,
        }
    } else {
        AdaptiveConfig {
            bucket_interval: "1 hour",
            span_limit: 0, // operation summaries only
            log_limit: 20,
        }
    }
}

pub fn investigate(
    conn: &Arc<Mutex<Connection>>,
    req: &InvestigateRequest,
    prefix: &str,
) -> Result<BTreeMap<String, InvestigationBlock>> {
    let services = resolve_services(req)?;
    let mut results = BTreeMap::new();
    for service in &services {
        let block = investigate_one(conn, service, req.from_ts, req.to_ts, prefix).map_err(|e| {
            tracing::error!(service = %service, error = %e,
                "investigation query failed for service"
            );
            anyhow::anyhow!("investigation failed for service {service}: {e}")
        })?;
        results.insert(service.clone(), block);
    }
    Ok(results)
}

/// Resolve and validate the service list from the request.
fn resolve_services(req: &InvestigateRequest) -> Result<Vec<String>> {
    let services = match (&req.service, &req.services) {
        (Some(s), None) => vec![s.clone()],
        (None, Some(list)) => list.clone(),
        (Some(s), Some(list)) => {
            // Both provided — merge, deduplicate, preserve order
            let mut merged = vec![s.clone()];
            for item in list {
                if item != s {
                    merged.push(item.clone());
                }
            }
            merged
        }
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

fn investigate_one(
    conn: &Arc<Mutex<Connection>>,
    service: &str,
    from_ts: Option<i64>,
    to_ts: Option<i64>,
    prefix: &str,
) -> Result<InvestigationBlock> {
    let now_us = chrono::Utc::now().timestamp_micros();
    let to_ts = to_ts.unwrap_or(now_us);
    let from_ts = from_ts.unwrap_or(to_ts - 15 * 60 * 1_000_000);

    let conn = conn.lock().map_err(|e| {
        tracing::error!("DuckDB connection mutex poisoned");
        anyhow::anyhow!("{e}")
    })?;

    let config = adaptive_config(from_ts, to_ts);
    let ctx = QueryCtx {
        conn: &conn,
        service,
        from_ts,
        to_ts,
        prefix,
    };

    let metrics =
        query_all_metrics(&ctx, config.bucket_interval).context("metrics query failed")?;

    let operation_summaries =
        query_operation_summaries(&ctx).context("operation summaries query failed")?;

    // Combined: total spans + distinct error trace count in one query (was 2)
    let (spans_total, error_traces_total) = count_span_stats(&ctx)?;
    let spans = if config.span_limit > 0 {
        query_spans(&ctx, config.span_limit).context("spans query failed")?
    } else {
        Vec::new()
    };

    let logs_total = count_log_templates(&ctx)?;
    let logs = query_logs(&ctx, config.log_limit).context("logs query failed")?;

    let span_trees = build_span_trees(&spans);

    let linked_traces = query_linked_traces(&ctx, &spans).context("linked traces query failed")?;
    let linked_spans_returned: i64 = linked_traces.iter().map(|lt| lt.spans.len() as i64).sum();

    let profiles = query_profiles(&ctx, 20).context("profiles query failed")?;
    let profile_sample_count = count_profile_samples(&ctx)?;
    let coverage = compute_coverage(spans_total, logs_total, metrics.len(), profile_sample_count);
    let context = query_service_context(&ctx).context("service context query failed")?;
    let events = query_derived_events(&ctx).context("derived events query failed")?;

    Ok(InvestigationBlock {
        service: service.to_string(),
        window: Window {
            from_ts,
            to_ts,
            from_iso: us_to_iso(from_ts)?,
            to_iso: us_to_iso(to_ts)?,
        },
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
            linked_spans_total: linked_spans_returned, // we don't know the true total without a separate query
            linked_spans_returned,
        },
        operation_summaries,
        metrics,
        spans,
        span_trees,
        logs,
        linked_traces,
        profiles,
    })
}

/// Combined metrics query: gauge/sum scalars + histogram aggregates in one round-trip.
/// Was 2 separate queries; merged via UNION ALL with compatible (bucket, metric_name, p95, avg, n) schema.
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

    let mut stmt = ctx.conn.prepare(&sql)?;
    let mut rows = stmt.query(duckdb::params![
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
    ])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(MetricBucket {
            bucket: row.get::<_, String>(0).context("bucket")?,
            metric_name: row.get::<_, String>(1).context("metric_name")?,
            p95: row.get::<_, f64>(2).context("p95")?,
            avg: row.get::<_, f64>(3).context("avg")?,
            count: row.get::<_, i64>(4).context("count")?,
        });
    }
    Ok(results)
}

/// Combined span counts: total spans + distinct error traces in one query instead of two.
fn count_span_stats(ctx: &QueryCtx) -> Result<(i64, i64)> {
    ctx.conn
        .query_row(
            &format!(
                "SELECT count(*),
                count(DISTINCT CASE WHEN status_code = 2 THEN trace_id END)
         FROM {}sigiro_spans
         WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
                ctx.prefix
            ),
            duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
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
    // Baseline window: 4x the investigation window, immediately preceding it.
    // E.g., investigating last 15m → baseline is the preceding 1h.
    let window_us = ctx.to_ts - ctx.from_ts;
    let baseline_from = ctx.from_ts.saturating_sub(window_us * 4);
    let baseline_to = ctx.from_ts;

    // Single query: current window LEFT JOIN baseline window.
    // The agent sees both current and baseline side-by-side.
    let sql = format!(
        "WITH current AS (
             SELECT span_name,
                    count(*) AS cnt,
                   count(*) FILTER (WHERE status_code = 2) AS err_cnt,
                   count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*) AS err_pct,
                   approx_quantile(duration, 0.50) AS p50,
                   approx_quantile(duration, 0.95) AS p95,
                   approx_quantile(duration, 0.99) AS p99,
                   first(trace_id) FILTER (WHERE status_code = 2) AS exemplar_tid
            FROM {}sigiro_spans
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        ctx.prefix
    );
    let sql = format!(
        "{sql}
            GROUP BY span_name
        ),
        baseline AS (
            SELECT span_name,
                   count(*) AS bl_cnt,
                   count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*) AS bl_err_pct,
                   approx_quantile(duration, 0.95) AS bl_p95
            FROM {}sigiro_spans
            WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        ctx.prefix
    );
    let sql = format!(
        "{sql}
            GROUP BY span_name
        )
        SELECT c.span_name, c.cnt, c.err_cnt, c.err_pct, c.p50, c.p95, c.p99, c.exemplar_tid,
               b.bl_err_pct, b.bl_p95, b.bl_cnt
        FROM current c
        LEFT JOIN baseline b ON c.span_name = b.span_name
        ORDER BY c.err_cnt DESC, c.cnt DESC
        LIMIT 20"
    );

    let mut stmt = ctx.conn.prepare(&sql)?;
    let mut rows = stmt.query(duckdb::params![
        ctx.service,
        ctx.from_ts,
        ctx.to_ts,
        ctx.service,
        baseline_from,
        baseline_to
    ])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(OperationSummary {
            span_name: row.get::<_, String>(0).context("span_name")?,
            count: row.get::<_, i64>(1).context("count")?,
            error_count: row.get::<_, i64>(2).context("error_count")?,
            error_pct: row.get::<_, f64>(3).context("error_pct")?,
            duration_p50_ns: row.get::<_, f64>(4).context("duration_p50_ns")?,
            duration_p95_ns: row.get::<_, f64>(5).context("duration_p95_ns")?,
            duration_p99_ns: row.get::<_, f64>(6).context("duration_p99_ns")?,
            // baseline fields are genuinely nullable — NULL from LEFT JOIN when no historical data
            exemplar_error_trace_id: row.get::<_, String>(7).ok(),
            baseline_error_pct: row.get::<_, f64>(8).ok(),
            baseline_p95_ns: row.get::<_, f64>(9).ok(),
            baseline_count: row.get::<_, i64>(10).ok(),
        });
    }
    Ok(results)
}

fn query_spans(ctx: &QueryCtx, limit: i64) -> Result<Vec<SpanRow>> {
    let sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, duration,
               status_code, service_name
        FROM {}sigiro_spans
        WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        ORDER BY CASE WHEN status_code = 2 THEN 0 ELSE 1 END, duration DESC
        LIMIT {limit}",
        ctx.prefix
    );

    let mut stmt = ctx.conn.prepare(&sql)?;
    let mut rows = stmt.query(duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts])?;
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
        parent_span_id: row.get::<_, String>(2).unwrap_or_default(), // TRADEOFF: root spans have NULL parent — empty string is correct model representation
        span_name: row.get::<_, String>(3).context("span_name")?,
        duration_ns: row.get::<_, i64>(4).context("duration")?,
        status_code: row.get::<_, i32>(5).context("status_code")?,
        service_name: row.get::<_, String>(6).context("service_name")?,
    })
}

pub fn query_logs(ctx: &QueryCtx, limit: i64) -> Result<Vec<LogTemplate>> {
    // DRAIN templates are computed at insert time (inserter.rs) and stored as template_id.
    // GROUP BY template_id collapses noisy logs; JOIN fetches the template text.
    // first(body) keeps one real exemplar for context.
    let sql = format!(
        "SELECT
            COALESCE(t.template_text, l.body) AS template,
            first(l.body) AS exemplar_body,
            l.severity_text,
            count(*) AS count,
            min(l.timestamp)::VARCHAR AS first_seen,
            max(l.timestamp)::VARCHAR AS last_seen
        FROM {prefix}sigiro_logs l
        LEFT JOIN {prefix}sigiro_log_templates t ON l.template_id = t.template_id
        WHERE l.service_name = ? AND l.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
        GROUP BY template, l.severity_text
        ORDER BY count DESC
        LIMIT ?",
        prefix = ctx.prefix
    );

    let mut stmt = ctx.conn.prepare(&sql)?;
    let mut rows = stmt.query(duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts, limit])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(LogTemplate {
            template: row.get::<_, String>(0).context("template")?,
            body: row.get::<_, String>(1).context("body")?,
            severity_text: row.get::<_, String>(2).context("severity_text")?,
            count: row.get::<_, i64>(3).context("count")?,
            first_seen: row.get::<_, Option<String>>(4)?,
            last_seen: row.get::<_, Option<String>>(5)?,
        });
    }
    Ok(results)
}

fn count_profile_samples(ctx: &QueryCtx) -> Result<i64> {
    let sql = format!(
        "SELECT count(*) FROM {}sigiro_profiles
         WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)",
        ctx.prefix
    );
    let count: i64 = ctx
        .conn
        .query_row(
            &sql,
            duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts],
            |r| r.get(0),
        )
        .context("span count query failed")?;
    Ok(count)
}

/// Top functions from continuous profiles, ranked by total self-time.
/// Uses stack_trace[1] (leaf function = self time) for aggregation.
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

    let mut stmt = ctx.conn.prepare(&sql)?;
    let mut rows = stmt.query(duckdb::params![ctx.service, ctx.from_ts, ctx.to_ts, limit])?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        results.push(ProfileSummary {
            function: row.get::<_, String>(0).context("function")?,
            profile_type: row.get::<_, String>(1).context("profile_type")?,
            total_value: row.get::<_, i64>(2).context("total_value")?,
            unit: row.get::<_, String>(3).context("unit")?,
            sample_count: row.get::<_, i64>(4).context("sample_count")?,
        });
    }
    Ok(results)
}

pub fn build_span_trees(spans: &[SpanRow]) -> BTreeMap<String, Vec<String>> {
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

fn query_linked_traces(ctx: &QueryCtx, spans: &[SpanRow]) -> Result<Vec<LinkedTrace>> {
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
    let linked_from = ctx.from_ts.saturating_sub(buffer_us);
    let linked_to = ctx.to_ts.saturating_add(buffer_us);

    let placeholders = vec!["?"; unique.len()].join(", ");
    let sql = format!(
        "SELECT trace_id, span_id, parent_span_id, span_name, duration,
                status_code, service_name
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
