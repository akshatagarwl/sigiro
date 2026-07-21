pub mod analysis;

use anyhow::{Context, Result};
use duckdb::Connection;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use itertools::Itertools;

use crate::types::{StatusCode, TimestampMicros};

// Re-export pure types and functions from analysis submodule.
pub use analysis::{
    Coverage, ServiceDependency, SignalCoverage, Window, compute_coverage, metric_bucket_interval,
    us_to_iso,
};

// ---------------------------------------------------------------------------
// InvestigateRequest — API input (from_ts/to_ts mandatory)
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, validator::Validate)]
#[validate(schema(function = "validate_window", skip_on_field_errors = true))]
pub struct InvestigateRequest {
    pub service: Option<String>,
    pub services: Option<Vec<String>>,
    #[validate(custom(function = "validate_micros"))]
    pub from_ts: i64,
    #[validate(custom(function = "validate_micros"))]
    pub to_ts: i64,
}

// Timestamps are MICROSECONDS since the Unix epoch (matching the span/log
// `timestamp` column). Fail LOUD on the wrong unit: seconds/millis are still
// "positive" but resolve to ~1970, matching nothing — a false "no telemetry".
fn validate_micros(ts: i64) -> Result<(), validator::ValidationError> {
    const MICROS_MIN: i64 = 1_000_000_000_000_000; // ~2001-09 in µs — catches s / ms
    const MICROS_MAX: i64 = 10_000_000_000_000_000; // ~2286 in µs — catches ns
    if (MICROS_MIN..MICROS_MAX).contains(&ts) {
        return Ok(());
    }
    let hint = match ts {
        t if t <= 0 => "must be a positive Unix timestamp",
        t if t < 100_000_000_000 => "far too small to be a timestamp",
        t if t < 100_000_000_000_000 => "looks like SECONDS — multiply by 1_000_000 for µs",
        t if t < MICROS_MIN => "looks like MILLISECONDS — multiply by 1_000 for µs",
        _ => "looks like NANOSECONDS — divide by 1_000 for µs",
    };
    let mut err = validator::ValidationError::new("timestamp_micros");
    err.message = Some(
        format!(
            "not a plausible microsecond timestamp ({hint}); expected a 16-digit µs value since \
             the Unix epoch, e.g. 1780000000000000"
        )
        .into(),
    );
    Err(err)
}

fn validate_window(req: &InvestigateRequest) -> Result<(), validator::ValidationError> {
    if req.to_ts <= req.from_ts {
        let mut err = validator::ValidationError::new("window");
        err.message = Some("to_ts must be greater than from_ts".into());
        return Err(err);
    }
    Ok(())
}

const MAX_SERVICES: usize = 10;

// ---------------------------------------------------------------------------
// Response types — every section maps 1:1 to a CTE in the single query
// ---------------------------------------------------------------------------

/// One ranked, self-contained "what deserves attention" item — the first
/// thing an agent reads. Assembled in Rust from data the single query already
/// returned (zero extra SQL, zero extra memory).
#[derive(Serialize, Deserialize, Debug)]
pub struct Finding {
    /// "critical" | "warning" | "info" — findings are sorted by this.
    pub severity: String,
    /// Machine-readable category: "anomalous_operation", "errors_without_logs",
    /// "coverage_gap", "missing_resource_attributes".
    pub kind: String,
    /// Human/agent-readable one-liner with the concrete numbers.
    pub summary: String,
    /// Ready-to-run /v1/query SQL for the evidence behind this finding,
    /// when one exists.
    pub drill_down_sql: Option<String>,
}

/// Response envelope for an investigation: per-service blocks plus
/// per-service failures. Partial results by design — one broken service
/// must not discard the other nine (fail loud, per service).
#[derive(Serialize, Deserialize)]
pub struct InvestigateResponse {
    /// Successful investigation blocks, keyed by service name.
    pub results: BTreeMap<String, InvestigationBlock>,
    /// Services whose investigation query failed, keyed by service name,
    /// with the error message. Empty when everything succeeded.
    pub errors: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
pub struct InvestigationBlock {
    /// Ranked list (critical first) of what deserves attention in this
    /// window: anomalous operations, error/log inconsistencies, actionable
    /// coverage gaps. Read this before the raw sections below.
    pub findings: Vec<Finding>,
    pub service: String,
    pub window: Window,
    /// Distinct traces with an error span in the window — the headline
    /// "something's wrong" signal. Drill via the anomalous `operation_summaries`
    /// or the `spans` section, each of which carries a ready-to-run query.
    pub error_traces_total: i64,
    pub coverage: Coverage,
    pub context: ServiceContext,
    pub events: Vec<DerivedEvent>,
    pub operation_summaries: Vec<OperationSummary>,
    pub metrics: Vec<MetricBucket>,
    /// Top spans (errors first, then slowest), as a self-describing sample:
    /// `total` vs the returned rows tells you how much was withheld, and
    /// `drill_all_sql` fetches the rest.
    pub spans: SampledSection<SpanRecord>,
    /// Log patterns (template-deduplicated, ordered by count). Each row carries
    /// a `drill_down_sql` for its raw logs.
    pub log_patterns: Vec<LogPattern>,
    /// Raw logs, newest first — a self-describing sample (see `spans`).
    pub logs: SampledSection<LogRecord>,
    pub profiles: Vec<ProfileSummary>,
}

/// A bounded sample of a larger set that carries its own "there's more, and
/// here's how to get it" affordance, so an agent can decide to drill without
/// guessing. When `total > sample.len()`, run `drill_all_sql` via /v1/query.
#[derive(Serialize, Deserialize, Debug)]
pub struct SampledSection<T> {
    pub sample: Vec<T>,
    pub total: i64,
    /// How the sample was ordered/selected, e.g. "errors first, then slowest".
    pub sampled_by: String,
    /// Query returning the full set for this section, ready to paste into
    /// /v1/query.
    pub drill_all_sql: String,
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

#[derive(Serialize, Deserialize, Debug)]
pub struct DerivedEvent {
    pub detected_at: String,
    pub event_type: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct OperationSummary {
    /// The operation identity spans are grouped by. Usually equals
    /// `span_name`, but generic HTTP spans (span_name is a bare method like
    /// "POST", per OTel HTTP semconv) are disambiguated with
    /// `http.route`/`server.address` from span_attributes — e.g.
    /// "POST api.example.com" — so one giant "POST" bucket doesn't swallow
    /// every outbound call.
    pub operation: String,
    /// The raw OTel span name behind this operation (one representative
    /// value when the group was derived from attributes).
    pub span_name: String,
    pub count: i64,
    pub error_count: i64,
    pub error_pct: f64,
    pub duration_p50_us: f64,
    pub duration_p95_us: f64,
    pub duration_p99_us: f64,
    pub exemplar_error_trace_id: Option<String>,
    pub baseline_error_pct: Option<f64>,
    pub baseline_p95_us: Option<f64>,
    pub baseline_count: Option<i64>,
    /// Whether this operation is anomalous compared to baseline (None when
    /// there is no baseline data to compare against).
    pub is_anomalous: Option<bool>,
    /// Reason for anomaly if detected.
    pub anomaly_reason: Option<String>,
    /// Ready-to-run query for the raw spans behind this operation (errors
    /// first, then slowest). Sits next to `anomaly_reason` on purpose: when an
    /// operation looks wrong, this is exactly how to pull the evidence. Filled
    /// in Rust.
    pub drill_down_sql: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct MetricBucket {
    pub bucket: String,
    pub metric_name: String,
    pub p95: f64,
    pub avg: f64,
    pub count: i64,
}

/// A flat span record. Attributes and events are embedded as real JSON
/// (not escaped strings) so agents can read them directly.
#[derive(Serialize, Deserialize, Debug)]
pub struct SpanRecord {
    pub trace_id: String,
    pub span_id: String,
    /// Empty string for root spans.
    pub parent_span_id: String,
    pub span_name: String,
    /// OTel span kind as stored (integer enum).
    pub span_kind: Option<i32>,
    pub status_code: i32,
    /// OTel status message — for spans that fail without emitting any log
    /// (e.g. HTTP client errors) this is often the only diagnostic string.
    pub status_message: Option<String>,
    pub is_error: bool,
    pub duration_us: i64,
    pub start_ts: i64,
    pub end_ts: i64,
    pub start_iso: String,
    pub span_attributes: Option<serde_json::Value>,
    pub events: Option<serde_json::Value>,
    /// Ready-to-run query for the full trace this span belongs to (every span
    /// sharing its `trace_id`, chronological). Filled in Rust.
    pub drill_down_sql: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LogRecord {
    /// Microseconds since the Unix epoch.
    pub timestamp: i64,
    pub timestamp_iso: String,
    pub severity_text: String,
    pub body: String,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub attributes: Option<serde_json::Value>,
}

/// A log pattern with aggregated statistics.
#[derive(Serialize, Deserialize, Debug)]
pub struct LogPattern {
    /// Template or pattern string.
    pub pattern: String,
    /// Template ID (hash) for filtering raw logs. None if no template was extracted.
    pub template_id: Option<u64>,
    /// Pre-built SQL query the agent can run to fetch all raw logs matching
    /// this pattern in chronological order. Uses `template_id` for precise
    /// filtering. Filled in Rust (needs the caller's window + service).
    pub drill_down_sql: Option<String>,
    /// Example log body.
    pub example_body: Option<String>,
    /// Up to 5 distinct extracted-variable tuples (each a JSON array of the
    /// wildcard values for one log line) — enough to see what varies.
    pub sample_values: Option<serde_json::Value>,
    pub severity_text: String,
    pub count: i64,
    pub error_count: i64,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProfileSummary {
    pub function: String,
    pub profile_type: String,
    pub total_value: i64,
    pub unit: String,
    pub sample_count: i64,
}

// ---------------------------------------------------------------------------
// SqlPayload — the exact JSON shape the single query produces
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct SqlPayload {
    stats: SqlStats,
    context: ServiceContext,
    events: Vec<DerivedEvent>,
    operation_summaries: Vec<OperationSummary>,
    metrics: Vec<MetricBucket>,
    spans: Vec<SpanRecord>,
    log_patterns: Vec<LogPattern>,
    logs: Vec<LogRecord>,
    profiles: Vec<ProfileSummary>,
}

#[derive(Deserialize)]
struct SqlStats {
    spans_total: i64,
    error_traces_total: i64,
    logs_total: i64,
    log_template_count: i64,
    /// Logs with ERROR-class severity in the window — compared against
    /// `error_traces_total` to detect failures that emit no logs at all.
    error_logs_total: i64,
    metric_series_count: i64,
    profile_sample_count: i64,
}

// ---------------------------------------------------------------------------
// Top-level investigate orchestration
// ---------------------------------------------------------------------------

pub fn investigate(
    conn: &Connection,
    req: &InvestigateRequest,
    prefix: &str,
) -> Result<InvestigateResponse> {
    let services = resolve_services(req)?;
    let mut results = BTreeMap::new();
    let mut errors = BTreeMap::new();
    for service in &services {
        // Partial results: one broken service must not discard the others.
        // Each failure is still loud — logged here, surfaced per-service in
        // `errors`, and fatal when NOTHING succeeded.
        match investigate_one(conn, service, req.from_ts, req.to_ts, prefix) {
            Ok(block) => {
                results.insert(service.clone(), block);
            }
            Err(e) => {
                tracing::error!(service = %service, error = %e,
                    "investigation query failed for service"
                );
                errors.insert(service.clone(), format!("{e:#}"));
            }
        }
    }
    if results.is_empty() && !errors.is_empty() {
        anyhow::bail!(
            "investigation failed for all services: {}",
            errors.iter().map(|(s, e)| format!("{s}: {e}")).join("; ")
        );
    }
    Ok(InvestigateResponse { results, errors })
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

/// ONE DuckDB round-trip per service: a stack of CTEs (one per report
/// section) collapsed into a single JSON document, deserialized straight
/// into `InvestigationBlock`.
#[tracing::instrument(skip(conn), fields(service = %service))]
fn investigate_one(
    conn: &Connection,
    service: &str,
    from_ts: i64,
    to_ts: i64,
    prefix: &str,
) -> Result<InvestigationBlock> {
    let bucket = metric_bucket_interval(from_ts, to_ts);
    let window_us = to_ts - from_ts;
    // Baseline window: 4x the investigation window, ending where it starts.
    let baseline_from = from_ts.saturating_sub(window_us.saturating_mul(4));

    let sql = build_investigation_sql(prefix, bucket);
    let json: String = conn
        .query_row(
            &sql,
            duckdb::params![service, from_ts, to_ts, baseline_from, from_ts],
            |row| row.get(0),
        )
        .map_err(|e| anyhow::anyhow!("investigation query failed: {e}"))?;

    let payload: SqlPayload =
        serde_json::from_str(&json).context("failed to deserialize investigation JSON")?;

    let coverage = compute_coverage(
        payload.stats.spans_total,
        payload.stats.log_template_count,
        payload.stats.metric_series_count as usize,
        payload.stats.profile_sample_count,
    );

    // Every drill query is built here in Rust — it needs the caller's service +
    // window (safely quoted), which the SQL doesn't have handy — so it's pure
    // string work, no extra DB round-trip. Each targets /v1/query verbatim, so
    // an agent that spots a lead can execute it directly to go deeper.
    let err = StatusCode::Error.as_i32();
    let svc = service.replace('\'', "''");
    let window = format!("timestamp BETWEEN make_timestamp({from_ts}) AND make_timestamp({to_ts})");
    // Reproduce the win_spans filter exactly so a drilled count reconciles with
    // the `total` we reported (win_spans drops OTLP export self-spans).
    let span_scope = format!(
        "service_name = '{svc}' AND {window} AND span_name NOT LIKE '%TraceService/Export%'"
    );

    // #1: raw logs behind each pattern.
    let mut log_patterns = payload.log_patterns;
    for p in &mut log_patterns {
        p.drill_down_sql = p.template_id.map(|tid| {
            format!(
                "SELECT * FROM {prefix}sigiro_logs \
                 WHERE service_name = '{svc}' AND template_id = {tid} AND {window} \
                 ORDER BY timestamp ASC"
            )
        });
    }

    // #1: raw spans behind each operation (errors first, then slowest).
    // Bare-HTTP-method groups must filter on the SAME derived expression the
    // query grouped by — `span_name = 'POST'` would also match POST spans
    // that were bucketed under "POST some.host" — so the drilled rows
    // reconcile exactly with this operation's count.
    let mut operation_summaries = payload.operation_summaries;
    for op in &mut operation_summaries {
        let filter = if HTTP_METHOD_SPAN_NAMES.contains(&op.span_name.as_str()) {
            let operation = op.operation.replace('\'', "''");
            format!("{} = '{operation}'", operation_key_expr(""))
        } else {
            format!("span_name = '{}'", op.span_name.replace('\'', "''"))
        };
        op.drill_down_sql = Some(format!(
            "SELECT * FROM {prefix}sigiro_spans \
             WHERE {span_scope} AND {filter} \
             ORDER BY CASE WHEN status_code = {err} THEN 0 ELSE 1 END, duration DESC"
        ));
    }

    // #1: the full trace each exemplar span belongs to.
    let mut span_rows = payload.spans;
    for s in &mut span_rows {
        let tid = s.trace_id.replace('\'', "''");
        s.drill_down_sql = Some(format!(
            "SELECT * FROM {prefix}sigiro_spans WHERE trace_id = '{tid}' ORDER BY timestamp ASC"
        ));
    }

    // #2: self-describing sampled sections — the returned sample, the true
    // total, how it was ordered, and the query for the full set.
    let spans = SampledSection {
        total: payload.stats.spans_total,
        sampled_by: "errors first, then longest duration".to_string(),
        drill_all_sql: format!(
            "SELECT * FROM {prefix}sigiro_spans \
             WHERE {span_scope} \
             ORDER BY CASE WHEN status_code = {err} THEN 0 ELSE 1 END, duration DESC"
        ),
        sample: span_rows,
    };
    let logs = SampledSection {
        total: payload.stats.logs_total,
        sampled_by: "most recent first".to_string(),
        drill_all_sql: format!(
            "SELECT * FROM {prefix}sigiro_logs \
             WHERE service_name = '{svc}' AND {window} ORDER BY timestamp DESC"
        ),
        sample: payload.logs,
    };

    // Top-level findings — ranked judgments assembled from data already in
    // hand. The one array an agent reads first.
    let mut findings = Vec::new();
    for op in &operation_summaries {
        if op.is_anomalous == Some(true) {
            findings.push(Finding {
                severity: "critical".to_string(),
                kind: "anomalous_operation".to_string(),
                summary: format!(
                    "operation '{}': {}",
                    op.operation,
                    op.anomaly_reason
                        .as_deref()
                        .unwrap_or("flagged anomalous vs baseline")
                ),
                drill_down_sql: op.drill_down_sql.clone(),
            });
        }
    }
    if payload.stats.error_traces_total > 0 && payload.stats.error_logs_total == 0 {
        findings.push(Finding {
            severity: "warning".to_string(),
            kind: "errors_without_logs".to_string(),
            summary: format!(
                "{} error trace(s) but zero ERROR-severity logs in this window — these \
                 failures are only visible via span status/status_message; check that \
                 exceptions are logged and exported",
                payload.stats.error_traces_total
            ),
            drill_down_sql: Some(format!(
                "SELECT * FROM {prefix}sigiro_spans \
                 WHERE {span_scope} AND status_code = {err} ORDER BY timestamp DESC"
            )),
        });
    }
    if coverage.has_spans && !coverage.has_logs {
        findings.push(Finding {
            severity: "warning".to_string(),
            kind: "coverage_gap".to_string(),
            summary: "spans present but no logs in this window — export OTLP logs so \
                      errors can be correlated with their messages"
                .to_string(),
            drill_down_sql: None,
        });
    }
    if coverage.has_spans
        && payload.context.namespace.is_none()
        && payload.context.deployment.is_none()
    {
        findings.push(Finding {
            severity: "info".to_string(),
            kind: "missing_resource_attributes".to_string(),
            summary: "no k8s resource attributes (k8s.namespace.name / k8s.deployment.name) \
                      on spans — the context section is empty; enable OTel resource \
                      detectors to fill it"
                .to_string(),
            drill_down_sql: None,
        });
    }
    findings.sort_by_key(|f| match f.severity.as_str() {
        "critical" => 0,
        "warning" => 1,
        _ => 2,
    });

    Ok(InvestigationBlock {
        findings,
        service: service.to_string(),
        window: Window::new(
            TimestampMicros::from_micros(from_ts),
            TimestampMicros::from_micros(to_ts),
        )?,
        error_traces_total: payload.stats.error_traces_total,
        coverage,
        context: payload.context,
        events: payload.events,
        operation_summaries,
        metrics: payload.metrics,
        spans,
        log_patterns,
        logs,
        profiles: payload.profiles,
    })
}

// ---------------------------------------------------------------------------
// The single query
// ---------------------------------------------------------------------------

/// Span names that are bare HTTP methods (OTel HTTP semconv names client
/// spans this way) — their real identity lives in span_attributes.
const HTTP_METHOD_SPAN_NAMES: &[&str] =
    &["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"];

/// The ONE derived-operation expression, shared verbatim between the
/// investigation query (grouping) and the per-operation `drill_down_sql`
/// (filtering) so drilled rows always reconcile with the reported counts.
/// `q` is the column qualifier ("s." inside the query, "" in drill SQL).
///
/// A bare HTTP-method span name gets ` <http.route>` (server spans) or
/// ` <server.address>` (client spans) appended when present — NOT url.path,
/// which is unbounded-cardinality (ids in paths). Uses only functions on the
/// /v1/query scalar allowlist (json_extract_string, nullif, coalesce) so the
/// drill SQL passes `validate_read_only`.
fn operation_key_expr(q: &str) -> String {
    format!(
        "CASE WHEN {q}span_name IN ('GET', 'POST', 'PUT', 'DELETE', 'PATCH', 'HEAD', 'OPTIONS') \
         THEN {q}span_name || COALESCE(' ' || COALESCE(\
NULLIF(json_extract_string({q}span_attributes, 'http.route'), ''), \
NULLIF(json_extract_string({q}span_attributes, 'server.address'), '')), '') \
         ELSE {q}span_name END"
    )
}

/// Builds the one-shot investigation query. Five bind parameters, in order:
/// service, from_ts (µs), to_ts (µs), baseline_from (µs), baseline_to (µs).
///
/// Every section is a CTE; the final SELECT packs them into one JSON object
/// via scalar subqueries, so the whole investigation is a single round-trip.
pub fn build_investigation_sql(prefix: &str, bucket: &str) -> String {
    let err = StatusCode::Error.as_i32();
    let op_key = operation_key_expr("s.");
    // The in-window span filter, repeated by the CTEs that need columns
    // win_spans deliberately does not carry (see win_spans comment).
    // Duplication marked for later review: SQL has no way to share a
    // predicate without also sharing a projection.
    let span_win = "s.service_name = a.service \
      AND s.timestamp BETWEEN a.from_ts AND a.to_ts \
      AND s.span_name NOT LIKE '%TraceService/Export%'";

    format!(
        r#"
WITH args AS (
    SELECT ?::VARCHAR AS service,
           make_timestamp(?::BIGINT) AS from_ts,
           make_timestamp(?::BIGINT) AS to_ts,
           make_timestamp(?::BIGINT) AS bl_from,
           make_timestamp(?::BIGINT) AS bl_to
),
-- Deliberately NARROW: this CTE is referenced by several sections, so DuckDB
-- materializes it. The wide VARCHAR columns (span_attributes, events_json,
-- resource_attributes) would dominate peak memory; sections that need them
-- (top_spans, deploy_events, svc_meta) hit the base table with the same
-- window predicate instead.
win_spans AS (
    SELECT s.trace_id, s.span_name, s.span_kind, s.status_code, s.duration,
           {op_key} AS operation
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win}
),
win_logs AS (
    SELECT l.*
    FROM {prefix}sigiro_logs l, args a
    WHERE l.service_name = a.service
      AND l.timestamp BETWEEN a.from_ts AND a.to_ts
),
span_stats AS (
    SELECT count(*) AS spans_total,
           count(DISTINCT CASE WHEN status_code = {err} THEN trace_id END) AS error_traces_total
    FROM win_spans
),
log_stats AS (
    SELECT count(*) AS logs_total,
           -- template_id when DRAIN classified the log, hash(body) otherwise:
           -- keeps the count correct for untemplated logs without holding
           -- every distinct body string in memory.
           count(DISTINCT COALESCE(template_id, hash(body))) AS log_template_count,
           count(CASE WHEN severity_text IN ('ERROR', 'ERR', 'FATAL', 'CRITICAL') THEN 1 END) AS error_logs_total
    FROM win_logs
),
profile_stats AS (
    SELECT count(*) AS profile_sample_count
    FROM {prefix}sigiro_profiles p, args a
    WHERE p.service_name = a.service
      AND p.timestamp BETWEEN a.from_ts AND a.to_ts
),
metric_points AS (
    SELECT m.timestamp, m.metric_name, m.value AS p95_src, m.value AS avg_src, 1::BIGINT AS cnt
    FROM {prefix}sigiro_metrics_gauge m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
    UNION ALL
    SELECT m.timestamp, m.metric_name, m.value, m.value, 1::BIGINT
    FROM {prefix}sigiro_metrics_sum m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
    UNION ALL
    SELECT m.timestamp, m.metric_name, m.max,
           CASE WHEN m.count > 0 THEN m.sum / m.count ELSE 0 END, m.count::BIGINT
    FROM {prefix}sigiro_metrics_histogram m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
    UNION ALL
    SELECT m.timestamp, m.metric_name, m.max,
           CASE WHEN m.count > 0 THEN m.sum / m.count ELSE 0 END, m.count::BIGINT
    FROM {prefix}sigiro_metrics_exp_histogram m, args a
    WHERE m.service_name = a.service AND m.timestamp BETWEEN a.from_ts AND a.to_ts
),
metrics AS (
    SELECT time_bucket(INTERVAL '{bucket}', timestamp)::VARCHAR AS bucket,
           metric_name,
           approx_quantile(p95_src, 0.95) AS p95,
           avg(avg_src) AS avg,
           sum(cnt)::BIGINT AS n
    FROM metric_points
    GROUP BY bucket, metric_name
),
ops_current AS (
    SELECT operation,
           any_value(span_name) AS span_name,
           count(*) AS cnt,
           count(CASE WHEN status_code = {err} THEN 1 END) AS err_cnt,
           count(CASE WHEN status_code = {err} THEN 1 END) * 100.0 / count(*) AS err_pct,
           approx_quantile(duration, 0.50) AS p50,
           approx_quantile(duration, 0.95) AS p95,
           approx_quantile(duration, 0.99) AS p99,
           any_value(CASE WHEN status_code = {err} THEN trace_id END) AS exemplar_error_trace_id
    FROM win_spans
    GROUP BY operation
),
ops_baseline AS (
    SELECT {op_key} AS operation,
           count(*) AS bl_cnt,
           count(CASE WHEN s.status_code = {err} THEN 1 END) * 100.0 / count(*) AS bl_err_pct,
           approx_quantile(s.duration, 0.95) AS bl_p95
    FROM {prefix}sigiro_spans s, args a
    WHERE s.service_name = a.service
      AND s.timestamp BETWEEN a.bl_from AND a.bl_to
      AND s.span_name NOT LIKE '%TraceService/Export%'
    GROUP BY operation
),
-- Three anomaly rules, any one of which flags the operation:
--   abs_rule: absolute error-rate floor — chronic failures must flag even
--             when the baseline is equally bad (a 24.6%% error rate is loud
--             regardless of history);
--   rel_rule: error rate doubled vs the 4x-preceding baseline;
--   lat_rule: p95 latency doubled vs baseline AND above 100ms (µs floor
--             keeps micro-operations from tripping it).
-- is_anomalous is NULL only for a brand-new operation with no baseline that
-- trips no absolute rule ("can't judge yet"), never for a judged-healthy one.
operation_summaries AS (
    SELECT operation, span_name, cnt, err_cnt, err_pct, p50, p95, p99,
           exemplar_error_trace_id,
           bl_err_pct, bl_p95, bl_cnt,
           CASE WHEN abs_rule OR rel_rule OR lat_rule THEN true
                WHEN bl_err_pct IS NULL THEN NULL
                ELSE false
           END AS is_anomalous,
           NULLIF(concat_ws('; ',
               CASE WHEN abs_rule THEN
                   printf('error rate %.1f%% exceeds absolute threshold 10%%', err_pct)
                   || CASE WHEN bl_err_pct IS NOT NULL AND bl_err_pct > 5.0
                           THEN printf(' (chronic: baseline %.1f%% is also elevated)', bl_err_pct)
                           ELSE '' END
               END,
               CASE WHEN rel_rule THEN
                   printf('error rate %.1f%% is %dx higher than baseline %.1f%%',
                          err_pct,
                          CAST(round(err_pct / greatest(bl_err_pct, 0.1)) AS INTEGER),
                          bl_err_pct)
               END,
               CASE WHEN lat_rule THEN
                   printf('p95 latency %.1fms is over 2x baseline %.1fms',
                          p95 / 1000.0, bl_p95 / 1000.0)
               END
           ), '') AS anomaly_reason
    FROM (
        SELECT c.operation, c.span_name, c.cnt, c.err_cnt, c.err_pct,
               c.p50, c.p95, c.p99, c.exemplar_error_trace_id,
               b.bl_err_pct, b.bl_p95, b.bl_cnt,
               (c.err_pct >= 10.0 AND c.cnt >= 20) AS abs_rule,
               (b.bl_err_pct IS NOT NULL AND c.err_pct > b.bl_err_pct * 2.0
                AND c.err_pct > 5.0) AS rel_rule,
               (b.bl_p95 IS NOT NULL AND c.p95 > b.bl_p95 * 2.0
                AND c.p95 > 100000) AS lat_rule
        FROM ops_current c
        LEFT JOIN ops_baseline b USING (operation)
    )
    ORDER BY err_cnt DESC, cnt DESC
    LIMIT 20
),
top_spans AS (
    SELECT s.trace_id, s.span_id, COALESCE(s.parent_span_id, '') AS parent_span_id,
           s.span_name, s.span_kind, s.status_code, s.status_message,
           s.duration AS duration_us,
           epoch_us(s.timestamp) AS start_ts,
           s.end_timestamp AS end_ts,
           s.timestamp::VARCHAR AS start_iso,
           s.span_attributes, s.events_json
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win}
    ORDER BY CASE WHEN s.status_code = {err} THEN 0 ELSE 1 END, s.duration DESC
    LIMIT 25
),
raw_logs AS (
    SELECT epoch_us(timestamp) AS ts_us,
           timestamp::VARCHAR AS timestamp_iso,
           COALESCE(severity_text, 'INFO') AS severity_text,
           body, trace_id, span_id, log_attributes
    FROM win_logs
    ORDER BY timestamp DESC
    LIMIT 50
),
log_patterns AS (
    SELECT COALESCE(NULLIF(t.template_text, ''), l.body) AS pattern,
           any_value(l.template_id) AS template_id,
           any_value(l.body) AS example_body,
           l.severity_text,
           count(*) AS cnt,
           count(CASE WHEN l.severity_text IN ('ERROR', 'ERR', 'FATAL', 'CRITICAL') THEN 1 END) AS error_count,
           min(l.timestamp)::VARCHAR AS first_seen,
           max(l.timestamp)::VARCHAR AS last_seen,
           list_slice(
               list(DISTINCT l.template_values)
                   FILTER (WHERE l.template_values IS NOT NULL AND l.template_values != '[]'),
               1, 5) AS sample_values
    FROM win_logs l
    LEFT JOIN {prefix}sigiro_log_templates t ON l.template_id = t.template_id
    GROUP BY pattern, l.severity_text
    ORDER BY cnt DESC
    LIMIT 50
),
deploy_events AS (
    -- Anti-join against the baseline window: a version already running
    -- before from_ts is steady state, not a deploy — without this, every
    -- investigation fabricated a "deploy" event at window start, exactly
    -- the false lead an agent seizes on.
    SELECT min(detected_at) AS detected_at,
           'deploy' AS event_type,
           'version ' || version || ' first seen' AS description
    FROM (
        SELECT s.timestamp::VARCHAR AS detected_at,
               s.resource_attributes::JSON->>'service.version' AS version
        FROM {prefix}sigiro_spans s, args a
        WHERE {span_win}
    )
    WHERE version IS NOT NULL
      AND version NOT IN (
          SELECT (s.resource_attributes::JSON->>'service.version')
          FROM {prefix}sigiro_spans s, args a
          WHERE s.service_name = a.service
            AND s.timestamp BETWEEN a.bl_from AND a.bl_to
            AND (s.resource_attributes::JSON->>'service.version') IS NOT NULL
      )
    GROUP BY version
),
k8s_events AS (
    SELECT timestamp::VARCHAR AS detected_at,
           'k8s_event' AS event_type,
           body AS description
    FROM win_logs
    WHERE body LIKE '%OOMKilled%' OR body LIKE '%Unhealthy%'
       OR body LIKE '%BackOff%' OR body LIKE '%FailedScheduling%'
       OR body LIKE '%ScalingReplicaSet%'
    LIMIT 20
),
events AS (
    SELECT * FROM deploy_events
    UNION ALL
    SELECT * FROM k8s_events
),
profiles AS (
    SELECT p.stack_trace[len(p.stack_trace)] AS function,
           COALESCE(p.profile_type, 'unknown') AS profile_type,
           sum(p.value)::BIGINT AS total_value,
           COALESCE(any_value(p.unit), '') AS unit,
           count(*) AS sample_count
    FROM {prefix}sigiro_profiles p, args a
    WHERE p.service_name = a.service
      AND p.timestamp BETWEEN a.from_ts AND a.to_ts
      AND len(p.stack_trace) > 0
    GROUP BY function, profile_type
    ORDER BY total_value DESC
    LIMIT 20
),
svc_meta AS (
    SELECT any_value(NULLIF(s.resource_attributes::JSON->>'k8s.namespace.name', '')) AS namespace,
           any_value(NULLIF(s.resource_attributes::JSON->>'k8s.deployment.name', '')) AS deployment,
           count(DISTINCT s.service_instance_id) AS pod_count,
           count(DISTINCT s.service_instance_id) > 0 AS has_instance_id,
           min(s.timestamp)::VARCHAR AS first_seen,
           max(s.timestamp)::VARCHAR AS last_seen
    FROM {prefix}sigiro_spans s, args a
    WHERE {span_win}
),
-- Dependency CTEs join on pre-DISTINCTed trace ids: (a) each foreign span is
-- counted exactly once (the old span-to-span join multiplied counts by the
-- number of our spans in the trace), and (b) the foreign side is bounded to
-- the window ±1h — traces don't span hours, and the old unbounded join
-- scanned the FULL span history on every investigation.
client_traces AS (
    SELECT DISTINCT trace_id FROM win_spans WHERE span_kind IN (3, 4)
),
svc_traces AS (
    SELECT DISTINCT trace_id FROM win_spans
),
deps AS (
    SELECT s2.service_name,
           count(*) AS span_count,
           count(CASE WHEN s2.status_code = {err} THEN 1 END) AS error_count,
           count(CASE WHEN s2.status_code = {err} THEN 1 END) * 100.0 / count(*) AS error_rate
    FROM {prefix}sigiro_spans s2
    JOIN client_traces ct ON s2.trace_id = ct.trace_id, args a
    WHERE s2.service_name != a.service
      AND s2.timestamp BETWEEN a.from_ts - INTERVAL '1 hour' AND a.to_ts + INTERVAL '1 hour'
    GROUP BY s2.service_name
    ORDER BY span_count DESC
    LIMIT 20
),
dependents AS (
    SELECT s1.service_name,
           count(*) AS span_count,
           count(CASE WHEN s1.status_code = {err} THEN 1 END) AS error_count,
           count(CASE WHEN s1.status_code = {err} THEN 1 END) * 100.0 / count(*) AS error_rate
    FROM {prefix}sigiro_spans s1
    JOIN svc_traces st ON s1.trace_id = st.trace_id, args a
    WHERE s1.service_name != a.service
      AND s1.span_kind IN (3, 4)
      AND s1.span_name NOT LIKE '%TraceService/Export%'
      AND s1.timestamp BETWEEN a.from_ts - INTERVAL '1 hour' AND a.to_ts + INTERVAL '1 hour'
    GROUP BY s1.service_name
    ORDER BY span_count DESC
    LIMIT 20
)
SELECT json_object(
    'stats', json_object(
        'spans_total', (SELECT spans_total FROM span_stats),
        'error_traces_total', (SELECT error_traces_total FROM span_stats),
        'logs_total', (SELECT logs_total FROM log_stats),
        'log_template_count', (SELECT log_template_count FROM log_stats),
        'error_logs_total', (SELECT error_logs_total FROM log_stats),
        'metric_series_count', (SELECT count(DISTINCT metric_name) FROM metric_points),
        'profile_sample_count', (SELECT profile_sample_count FROM profile_stats)
    ),
    'context', json_object(
        'service_name', (SELECT service FROM args),
        'first_seen', (SELECT first_seen FROM svc_meta),
        'last_seen', (SELECT last_seen FROM svc_meta),
        'namespace', (SELECT namespace FROM svc_meta),
        'deployment', (SELECT deployment FROM svc_meta),
        'pod_count', (SELECT pod_count FROM svc_meta),
        'has_instance_id', (SELECT has_instance_id FROM svc_meta),
        'dependencies', (SELECT COALESCE(to_json(list(json_object(
            'service_name', service_name, 'span_count', span_count,
            'error_count', error_count, 'error_rate', error_rate
        ) ORDER BY span_count DESC)), '[]'::JSON) FROM deps),
        'dependents', (SELECT COALESCE(to_json(list(json_object(
            'service_name', service_name, 'span_count', span_count,
            'error_count', error_count, 'error_rate', error_rate
        ) ORDER BY span_count DESC)), '[]'::JSON) FROM dependents)
    ),
    'events', (SELECT COALESCE(to_json(list(json_object(
        'detected_at', detected_at, 'event_type', event_type, 'description', description
    ) ORDER BY detected_at)), '[]'::JSON) FROM events),
    'operation_summaries', (SELECT COALESCE(to_json(list(json_object(
        'operation', operation,
        'span_name', span_name, 'count', cnt, 'error_count', err_cnt, 'error_pct', err_pct,
        'duration_p50_us', p50, 'duration_p95_us', p95, 'duration_p99_us', p99,
        'exemplar_error_trace_id', exemplar_error_trace_id,
        'baseline_error_pct', bl_err_pct, 'baseline_p95_us', bl_p95, 'baseline_count', bl_cnt,
        'is_anomalous', is_anomalous, 'anomaly_reason', anomaly_reason
    ) ORDER BY err_cnt DESC, cnt DESC)), '[]'::JSON) FROM operation_summaries),
    'metrics', (SELECT COALESCE(to_json(list(json_object(
        'bucket', bucket, 'metric_name', metric_name, 'p95', p95, 'avg', avg, 'count', n
    ) ORDER BY bucket, metric_name)), '[]'::JSON) FROM metrics),
    'spans', (SELECT COALESCE(to_json(list(json_object(
        'trace_id', trace_id, 'span_id', span_id, 'parent_span_id', parent_span_id,
        'span_name', span_name, 'span_kind', span_kind, 'status_code', status_code,
        'status_message', status_message,
        'is_error', status_code = {err}, 'duration_us', duration_us,
        'start_ts', start_ts, 'end_ts', end_ts, 'start_iso', start_iso,
        'span_attributes', TRY_CAST(span_attributes AS JSON),
        'events', TRY_CAST(events_json AS JSON)
    ) ORDER BY CASE WHEN status_code = {err} THEN 0 ELSE 1 END, duration_us DESC)),
        '[]'::JSON) FROM top_spans),
    'log_patterns', (SELECT COALESCE(to_json(list(json_object(
        'pattern', pattern, 'template_id', template_id, 'example_body', example_body,
        'severity_text', severity_text, 'count', cnt, 'error_count', error_count,
        'first_seen', first_seen, 'last_seen', last_seen,
        'sample_values', TRY_CAST(sample_values AS JSON[])
    ) ORDER BY cnt DESC)), '[]'::JSON) FROM log_patterns),
    'logs', (SELECT COALESCE(to_json(list(json_object(
        'timestamp', ts_us, 'timestamp_iso', timestamp_iso, 'severity_text', severity_text,
        'body', body, 'trace_id', trace_id, 'span_id', span_id,
        'attributes', TRY_CAST(log_attributes AS JSON)
    ) ORDER BY ts_us DESC)), '[]'::JSON) FROM raw_logs),
    'profiles', (SELECT COALESCE(to_json(list(json_object(
        'function', function, 'profile_type', profile_type, 'total_value', total_value,
        'unit', unit, 'sample_count', sample_count
    ) ORDER BY total_value DESC)), '[]'::JSON) FROM profiles)
)::VARCHAR AS investigation
"#
    )
}
