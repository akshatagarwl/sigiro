//! Self-diagnosis via sigiro's own telemetry — never tail Docker logs.
//!
//! This module implements the `sigiro diagnose` CLI command. It queries the
//! running sigiro instance via the HTTP API to detect common issues:
//!
//! - Error spikes (status_code = 2)
//! - Slow spans (p99 duration)
//! - OOM patterns (memory pressure from span attributes)
//! - Ingest stalls (gap in span timestamps)
//!
//! All data comes from `/v1/query`, `/v1/investigate`, `/v1/traces/{id}` —
//! the same endpoints users already have. No direct DB access, no docker logs.

use anyhow::{Context, Result};
use std::time::Duration;

use crate::client::SigiroClient;
use crate::responses::HealthResponse;

/// Send a SQL query via the HTTP client, retrying on 429 with backoff.
///
/// The diagnose CLI fires many SQL queries in rapid succession. The server's
/// governor config (2 req/sec, burst 10) can reject late ones in the run with
/// HTTP 429. We back off and retry — 5 attempts × 600 ms = 3 s of patience,
/// enough for the bucket to refill while keeping total diagnose latency low.
async fn query_with_retry(client: &SigiroClient, sql: &str) -> Result<Vec<serde_json::Value>> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..5 {
        match client.query(sql).await {
            Ok(rows) => return Ok(rows),
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("Rate limited") && attempt < 4 {
                    tokio::time::sleep(Duration::from_millis(600 * (attempt + 1))).await;
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("query_with_retry: exhausted retries")))
}

/// Run an investigation with the same 429-backoff as SQL queries. The
/// deep-dive fires this after ~17 prior SQL queries in the same run, so
/// it's the most likely call in the report to hit a governor rejection.
async fn investigate_with_retry(
    client: &SigiroClient,
    service: &str,
    from_us: i64,
    to_us: i64,
) -> Result<crate::investigate::InvestigationBlock> {
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..5 {
        match client.investigate(service, from_us, to_us).await {
            Ok(block) => return Ok(block),
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("Rate limited") && attempt < 4 {
                    tokio::time::sleep(Duration::from_millis(600 * (attempt + 1))).await;
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("investigate_with_retry: exhausted retries")))
}

/// Run every diagnostic check and print the report.
///
/// This is a manifest, not logic: each check owns its own SQL *and* its own
/// interpretation of the numbers, and hands back the report lines it wants.
/// `absorb` turns a failed check into a warning so one dead query can't take
/// the run down. Adding a check is one line here plus one function below.
pub async fn run(client: &SigiroClient) -> Result<()> {
    let mut report = DiagnosisReport::new();

    // Every check below queries the API, so a dead endpoint is fatal: say so
    // once instead of emitting twenty identical "check failed" lines.
    if let Err(e) = client.query("SELECT 1").await {
        report.push(
            "connectivity",
            Verdict::fail(format!("sigiro not reachable: {e:#}")),
        );
        report.print();
        return Ok(());
    }
    report.push(
        "connectivity",
        Verdict::pass("sigiro query endpoint responding"),
    );

    report.absorb("data_dir_size", check_data_dir_size());
    report.absorb("health", check_health(client).await);
    report.absorb("errors", check_errors(client).await);
    report.absorb("svc-errors", check_service_errors(client).await);
    report.absorb("latency", check_slow_spans(client).await);
    report.absorb("slow-ops", check_slow_operations(client).await);
    report.absorb("span-kinds", check_span_kind_distribution(client).await);
    report.absorb("ingest", check_ingest_rate(client).await);
    report.absorb("log-errors", check_log_errors(client).await);
    report.absorb("storage", check_storage(client).await);
    report.absorb("dup-spans", check_duplicate_spans(client).await);
    report.absorb("orphan-spans", check_orphan_spans(client).await);
    report.absorb("retention", check_retention());
    report.absorb("metrics-coverage", check_metrics_coverage(client).await);
    report.absorb("ebpf", check_ebpf_status(client).await);
    report.absorb("rate-pressure", check_rate_pressure(client).await);
    report.absorb("config-drift", check_config_drift());
    report.absorb("deep-dive", check_investigate_deep_dive(client).await);
    report.absorb("duckdb", check_duckdb_health(client).await);

    report.print();
    Ok(())
}

// ---------------------------------------------------------------------------
// Row access
//
// `/v1/query` answers in JSON, so every column read is the same three-step
// Option dance (present? right type? else default). These collapse it to one
// call — without them the file grows one four-line chain per column.
// ---------------------------------------------------------------------------

/// Stands in for "no rows" so aggregate queries can read columns off the
/// first row unconditionally: `Value::Null.get(col)` is `None`, which lands
/// on the same default as a missing column.
static NULL_ROW: serde_json::Value = serde_json::Value::Null;

fn head(rows: &[serde_json::Value]) -> &serde_json::Value {
    rows.first().unwrap_or(&NULL_ROW)
}

fn i64_at(row: &serde_json::Value, col: &str) -> i64 {
    row.get(col)
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
}

fn f64_at(row: &serde_json::Value, col: &str) -> f64 {
    row.get(col)
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0)
}

fn str_at(row: &serde_json::Value, col: &str, default: &str) -> String {
    row.get(col)
        .and_then(serde_json::Value::as_str)
        .unwrap_or(default)
        .to_string()
}

fn opt_str_at(row: &serde_json::Value, col: &str) -> Option<String> {
    row.get(col)
        .and_then(serde_json::Value::as_str)
        .map(String::from)
}

/// Micros → milliseconds for a duration column that DuckDB returns as a
/// float (quantile_cont).
fn duration_ms(row: &serde_json::Value, col: &str) -> i64 {
    f64_at(row, col) as i64 / 1000
}

// ---------------------------------------------------------------------------
// Individual checks
//
// Each returns the report lines it wants, or an error for `absorb` to turn
// into a warning. Keeping the verdict next to the query is what keeps `run`
// branchless — the alternative is twenty `match` arms in one function.
// ---------------------------------------------------------------------------

/// Error rate (0–100) for a row carrying `total` and `errors`. Shared by the
/// surface hotspot list and the deep-dive candidate pick so the two cannot
/// disagree about what counts as elevated.
fn error_rate(row: &serde_json::Value) -> f64 {
    let total = i64_at(row, "total").max(1);
    (i64_at(row, "errors") as f64 / total as f64) * 100.0
}

/// A service is a hotspot above this error percentage.
const ELEVATED_ERROR_PCT: f64 = 5.0;

/// The SQL for per-service error rates in the last 15 minutes. Shared so the
/// svc-errors list and the deep-dive candidate rank services identically;
/// they differ only in how many rows they take.
fn service_error_sql(limit: u32) -> String {
    format!(
        "SELECT service_name,
                count(*) AS total,
                count(CASE WHEN status_code = 2 THEN 1 END) AS errors
         FROM lake.sigiro_spans
         WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
         GROUP BY service_name
         HAVING count(*) > 10
         ORDER BY errors DESC
         LIMIT {limit}"
    )
}

/// The shape shared by every "find bad rows, name the worst few" check: run
/// the query, and either nothing turned up or the row count plus a sample of
/// the worst offenders *is* the finding.
///
/// Extracted because four checks were exactly this and nothing else, differing
/// only in SQL, wording, and how one row prints. Checks whose count is not
/// `rows.len()` (a SUM, a filtered subset, a single aggregate row) do their own
/// thing rather than bend to fit this.
async fn sample_check(
    client: &SigiroClient,
    sql: &str,
    failed: &'static str,
    clean: &'static str,
    found: impl FnOnce(usize) -> String,
    hint: impl Fn(&serde_json::Value) -> String,
    sample: usize,
) -> Result<Vec<Verdict>> {
    let rows = query_with_retry(client, sql).await.context(failed)?;
    if rows.is_empty() {
        return Ok(vec![Verdict::pass(clean)]);
    }
    Ok(vec![
        Verdict::warn(found(rows.len())).hints(rows.iter().take(sample).map(hint)),
    ])
}

/// F-10: total bytes used by the data dir and free bytes on the volume.
fn check_data_dir_size() -> Result<Vec<Verdict>> {
    let dir = crate::data_dir()?;
    let mut total: u64 = 0;
    let path: &std::path::Path = dir.as_ref();
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.context("failed to walk data directory")?;
        if entry.file_type().is_file() {
            total += entry
                .metadata()
                .with_context(|| format!("failed to stat {}", entry.path().display()))?
                .len();
        }
    }
    let free = fs4::available_space(path).context("failed to read available disk space")?;

    let mb = bytes_to_mib(total);
    if mb > 1024.0 {
        return Ok(vec![Verdict::warn(format!(
            "{:.1} GB on disk; data dir is large",
            mb / 1024.0
        ))]);
    }
    Ok(vec![Verdict::pass(format!(
        "{mb:.1} MB on disk; {:.1} GB free",
        bytes_to_mib(free) / 1024.0
    ))])
}

/// Check /health for dead flushers.
///
/// `Starting` counts as healthy: the diagnose run is non-critical, and if the
/// server is still warming up the remaining checks will surface that anyway.
async fn check_health(client: &SigiroClient) -> Result<Vec<Verdict>> {
    Ok(vec![match client.health().await? {
        HealthResponse::Ready | HealthResponse::Starting => {
            Verdict::pass("no dead flushers detected")
        }
        HealthResponse::Degraded { .. } => {
            Verdict::warn("dead flusher tasks detected — ingest may be stalled")
        }
    }])
}

/// Count error spans (status_code = 2) in the last 15 minutes.
async fn check_errors(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT COUNT(*) AS cnt,
               (SELECT trace_id FROM lake.sigiro_spans
                WHERE status_code = 2
                  AND timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
                LIMIT 1) AS sample_trace
        FROM lake.sigiro_spans
        WHERE status_code = 2
          AND timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
    "#;
    let rows = client
        .query(sql)
        .await
        .context("error count query failed")?;
    let row = head(&rows);
    let count = i64_at(row, "cnt");
    if count == 0 {
        return Ok(vec![Verdict::pass("no error spans in last 15 min")]);
    }
    Ok(vec![
        Verdict::warn(format!("{count} error spans in last 15 min")).hints(
            opt_str_at(row, "sample_trace")
                .map(|trace_id| format!("sigiro trace {trace_id} — see full context")),
        ),
    ])
}

/// Services with an elevated error rate in the last 15 minutes.
async fn check_service_errors(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let rows = client
        .query(&service_error_sql(10))
        .await
        .context("service errors query failed")?;
    let hotspots: Vec<String> = rows
        .iter()
        .filter(|row| error_rate(row) > ELEVATED_ERROR_PCT)
        .map(|row| {
            format!(
                "  {}: {:.1}% ({} errors)",
                str_at(row, "service_name", "unknown"),
                error_rate(row),
                i64_at(row, "errors")
            )
        })
        .collect();
    if hotspots.is_empty() {
        return Ok(vec![Verdict::pass("no services with elevated error rate")]);
    }
    Ok(vec![
        Verdict::warn(format!(
            "{} services with error rate > {ELEVATED_ERROR_PCT:.0}%",
            hotspots.len()
        ))
        .hints(hotspots.into_iter().take(3)),
    ])
}

/// p99 span duration and the count of spans over 1s in the last 15 min.
async fn check_slow_spans(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT
            quantile_cont(duration, 0.99) AS p99,
            COUNT(CASE WHEN duration > 1000000 THEN 1 END) AS slow_count
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
    "#;
    let rows = client.query(sql).await.context("slow span query failed")?;
    let row = head(&rows);
    let p99_ms = duration_ms(row, "p99");
    if p99_ms > 1000 {
        return Ok(vec![Verdict::warn(format!(
            "p99 span duration = {p99_ms}ms ({} spans > 1s)",
            i64_at(row, "slow_count")
        ))]);
    }
    Ok(vec![Verdict::pass(format!(
        "p99 span duration = {p99_ms}ms"
    ))])
}

/// Top slow operations by service (p95 > 1s) in the last 15 min.
async fn check_slow_operations(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT
            service_name,
            span_name,
            quantile_cont(duration, 0.95) AS p95
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        GROUP BY service_name, span_name
        HAVING quantile_cont(duration, 0.95) > 1000000
        ORDER BY p95 DESC
        LIMIT 10
    "#;
    sample_check(
        client,
        sql,
        "slow operations query failed",
        "no slow operations detected",
        |n| format!("{n} slow operations (>1s p95)"),
        |row| {
            format!(
                "  {}/{}: p95={}ms",
                str_at(row, "service_name", "unknown"),
                str_at(row, "span_name", "unknown"),
                duration_ms(row, "p95")
            )
        },
        3,
    )
    .await
}

/// Span kind mix. Server spans with zero client spans means the service is
/// recording what it receives but not what it calls — a missing egress
/// exporter, which silently truncates every trace at that hop.
async fn check_span_kind_distribution(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT
            count(CASE WHEN span_kind = 2 THEN 1 END) AS server,
            count(CASE WHEN span_kind = 3 THEN 1 END) AS client,
            count(CASE WHEN span_kind = 1 THEN 1 END) AS internal,
            count(CASE WHEN span_kind = 4 THEN 1 END) AS consumer,
            count(CASE WHEN span_kind = 0 THEN 1 END) AS producer
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
    "#;
    let rows = client
        .query(sql)
        .await
        .context("span kind distribution query failed")?;
    let row = head(&rows);
    let [server, client_kind, internal, consumer, producer] =
        ["server", "client", "internal", "consumer", "producer"].map(|col| i64_at(row, col));

    if server + client_kind + internal + consumer + producer == 0 {
        return Ok(vec![Verdict::pass("no spans to analyze")]);
    }
    if server > 0 && client_kind == 0 {
        return Ok(vec![Verdict::warn(
            "server spans present but zero client spans — egress telemetry may be missing",
        )]);
    }
    Ok(vec![Verdict::pass(format!(
        "srv={server} cli={client_kind} int={internal} con={consumer} prod={producer}"
    ))])
}

/// Spans per minute over the last 15 minutes.
async fn check_ingest_rate(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT COUNT(*) AS cnt
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
    "#;
    let rows = client
        .query(sql)
        .await
        .context("ingest rate query failed")?;
    let rate = i64_at(head(&rows), "cnt") as f64 / 15.0;
    if rate < 1.0 {
        return Ok(vec![Verdict::warn(format!(
            "{rate:.1} spans/min — possible stall"
        ))]);
    }
    Ok(vec![Verdict::pass(format!("{rate:.1} spans/min"))])
}

/// Error-level logs in the last 15 minutes, grouped by Drain template.
async fn check_log_errors(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT
            COUNT(*) AS total,
            COALESCE(NULLIF(template_text, ''), body) AS pattern
        FROM lake.sigiro_logs
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
          AND severity_text IN ('ERROR', 'ERR', 'FATAL', 'CRITICAL')
        GROUP BY pattern
        ORDER BY total DESC
        LIMIT 10
    "#;
    let rows = client.query(sql).await.context("log errors query failed")?;
    let total: i64 = rows.iter().map(|row| i64_at(row, "total")).sum();
    if total == 0 {
        return Ok(vec![Verdict::pass("no error-level logs in last 15 min")]);
    }
    let patterns = rows.iter().take(3).map(|row| {
        format!(
            "  ({}x) {}",
            i64_at(row, "total"),
            str_at(row, "pattern", "unknown")
        )
    });
    Ok(vec![
        Verdict::warn(format!("{total} error-level logs in last 15 min")).hints(patterns),
    ])
}

/// Row counts per table plus growth since the last run (via a local state
/// file). A missing table counts as 0 — it just means that signal type has
/// never been ingested, which is not a fault.
async fn check_storage(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let tables = [
        "sigiro_spans",
        "sigiro_logs",
        "sigiro_metrics_sum",
        "sigiro_metrics_gauge",
        "sigiro_metrics_histogram",
    ];
    // Vec, not HashMap: the report should list tables in a stable order.
    let mut counts: Vec<(&str, i64)> = Vec::with_capacity(tables.len());
    for table in tables {
        let sql = format!("SELECT COUNT(*) AS cnt FROM lake.{table}");
        let count = match client.query(&sql).await {
            Ok(rows) => i64_at(head(&rows), "cnt"),
            Err(e) => {
                tracing::debug!(
                    "storage check: table {table} query failed — likely not created yet: {e:#}"
                );
                0
            }
        };
        counts.push((table, count));
    }

    let current_total: i64 = counts.iter().map(|(_, n)| n).sum();
    let mut verdicts = vec![Verdict::pass(format!(
        "{current_total} total rows across all tables"
    ))];
    for (table, count) in counts {
        if count > 1_000_000 {
            verdicts.push(Verdict::warn(format!(
                "{table}: {count} rows — consider retention"
            )));
        }
    }

    if let Ok(prev) = read_previous_storage_total() {
        let delta_pct = if prev > 0 {
            ((current_total - prev) as f64 / prev as f64) * 100.0
        } else {
            0.0
        };
        if delta_pct > 50.0 {
            verdicts.push(Verdict::warn(format!(
                "rapid growth: {prev} → {current_total} rows (+{delta_pct:.0}%) since last check"
            )));
        }
    }
    // Best-effort: a missing state file just means no growth line next run.
    if let Err(e) = write_storage_total(current_total) {
        tracing::debug!("failed to persist storage state: {e:#}");
    }

    Ok(verdicts)
}

fn storage_state_path() -> camino::Utf8PathBuf {
    let dir = crate::data_dir().unwrap_or_else(|_| camino::Utf8PathBuf::from("."));
    dir.join(".diagnose_storage_state.json")
}

fn read_previous_storage_total() -> anyhow::Result<i64> {
    let path = storage_state_path();
    let content =
        std::fs::read_to_string(&path).with_context(|| format!("failed to read {path:?}"))?;
    let state: serde_json::Value =
        serde_json::from_str(&content).context("failed to parse storage state")?;
    state
        .get("total")
        .and_then(serde_json::Value::as_i64)
        .context("missing 'total' field")
}

fn write_storage_total(total: i64) -> anyhow::Result<()> {
    let path = storage_state_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let state = serde_json::json!({ "total": total, "checked_at": now });
    std::fs::write(&path, serde_json::to_string_pretty(&state)?)
        .with_context(|| format!("failed to write {path:?}"))?;
    Ok(())
}

/// Retention days from config.
fn check_retention() -> Result<Vec<Verdict>> {
    Ok(vec![match crate::config::RETENTION_DAYS.value() {
        Ok(days) if days > 0 => Verdict::pass(format!("{days} days configured")),
        _ => Verdict::warn("no retention configured — storage will grow unbounded"),
    }])
}

/// Duplicate (trace_id, span_id) pairs in the last 15 minutes.
///
/// Each duplicate means the same span arrived twice — an OTLP exporter retry
/// storm, or an instrumented library generating conflicting IDs. Either way
/// the resulting trace is unreliable.
async fn check_duplicate_spans(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT trace_id,
               span_id,
               COUNT(*) AS dup_count
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        GROUP BY trace_id, span_id
        HAVING COUNT(*) > 1
        ORDER BY dup_count DESC
        LIMIT 10
    "#;
    sample_check(
        client,
        sql,
        "duplicate spans query failed",
        "no duplicate spans in last 15 min",
        |n| format!("{n} duplicate (trace_id, span_id) pairs in last 15 min"),
        |row| {
            format!(
                "  trace={} span={} ×{}",
                str_at(row, "trace_id", "?"),
                str_at(row, "span_id", "?"),
                i64_at(row, "dup_count")
            )
        },
        3,
    )
    .await
}

/// Spans whose `parent_span_id` is set but whose parent never arrived.
///
/// Common cause: a service in the trace stopped exporting (k8s rollout,
/// network policy, broken init). The parent never shows up, leaving the child
/// branches disconnected from the trace tree.
async fn check_orphan_spans(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT child.service_name,
               child.trace_id,
               child.span_id
        FROM lake.sigiro_spans child
        WHERE child.timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
          AND child.parent_span_id != ''
          AND NOT EXISTS (
            SELECT 1 FROM lake.sigiro_spans parent
            WHERE parent.trace_id = child.trace_id
              AND parent.span_id = child.parent_span_id
          )
        ORDER BY child.timestamp DESC
        LIMIT 10
    "#;
    sample_check(
        client,
        sql,
        "orphan spans query failed",
        "all spans have a present parent",
        |n| format!("{n} spans reference a missing parent in last 15 min — broken trace tree"),
        |row| {
            format!(
                "  {} trace={} span={}",
                str_at(row, "service_name", "unknown"),
                str_at(row, "trace_id", "?"),
                str_at(row, "span_id", "?")
            )
        },
        3,
    )
    .await
}

/// Services that emitted spans but no metrics in the same window — a strong
/// signal that the metrics exporter was never wired up.
async fn check_metrics_coverage(client: &SigiroClient) -> Result<Vec<Verdict>> {
    // Derived-table subqueries rather than CTEs: `validate_read_only` rejects
    // any `WITH` clause (CTE-name allowlisting was abandoned as too fragile a
    // security boundary), and this query routes through `/v1/query`.
    let sql = r#"
        SELECT s.service_name
        FROM (
            SELECT DISTINCT service_name
            FROM lake.sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        ) s
        WHERE s.service_name NOT IN (
            SELECT service_name FROM lake.sigiro_metrics_sum
                WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
            UNION
            SELECT service_name FROM lake.sigiro_metrics_gauge
                WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
            UNION
            SELECT service_name FROM lake.sigiro_metrics_histogram
                WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        )
        ORDER BY s.service_name
    "#;
    sample_check(
        client,
        sql,
        "metrics coverage query failed",
        "all span-emitting services also send metrics",
        |n| format!("{n} services send spans but no metrics — incomplete instrumentation"),
        |row| format!("  - {}", str_at(row, "service_name", "unknown")),
        5,
    )
    .await
}

/// eBPF auto-instrumentation status, as written to the admin status table by
/// `write_ebpf_status`. Read-only — we just report what the server recorded.
async fn check_ebpf_status(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let resp = client
        .admin_status()
        .await
        .context("admin/status call failed")?;
    let Some(status) = resp.status.get("ebpf_status") else {
        return Ok(vec![Verdict::warn(
            "status not initialized — server may be a fresh install or pre-restart",
        )]);
    };
    let reason = || {
        resp.status
            .get("ebpf_reason")
            .cloned()
            .unwrap_or_else(|| "no reason provided".to_string())
    };
    Ok(vec![match status.as_str() {
        "ready" => Verdict::pass("ready (obi installed, capabilities ok)"),
        "not_installed" | "disabled" => Verdict::warn(format!("disabled — {}", reason())),
        other => Verdict::warn(format!("disabled — unexpected status: {other}")),
    }])
}

/// A single service holding most of the last minute's spans is pressure on
/// the governor bucket — it can starve everyone else.
async fn check_rate_pressure(client: &SigiroClient) -> Result<Vec<Verdict>> {
    let sql = r#"
        SELECT service_name, COUNT(*) AS cnt
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '1 minute'
        GROUP BY service_name
        ORDER BY cnt DESC
        LIMIT 5
    "#;
    let rows = query_with_retry(client, sql)
        .await
        .context("rate-pressure query failed")?;
    let Some(top) = rows.first() else {
        return Ok(vec![Verdict::pass("no recent spans to analyze")]);
    };
    let total: i64 = rows.iter().map(|row| i64_at(row, "cnt")).sum();
    let spans = i64_at(top, "cnt");
    let dominant_pct = (spans as f64 / total.max(1) as f64) * 100.0;

    if dominant_pct < 50.0 {
        return Ok(vec![Verdict::pass(format!(
            "{dominant_pct:.0}% from top service, no dominance"
        ))]);
    }
    Ok(vec![
        Verdict::warn(format!(
            "1 service accounts for {dominant_pct:.0}% of last 1 min — risk of governor starvation"
        ))
        .hints(Some(format!(
            "  {}: {spans} spans ({dominant_pct:.0}%)",
            str_at(top, "service_name", "unknown")
        ))),
    ])
}

/// Flag runtime config values that hurt durability, correctness, or
/// performance.
///
/// Note: this reads the **CLI process**'s env, not the server's. In typical
/// deployments (single host, docker compose, shared `.env`) the two match. A
/// future improvement would expose the server's effective config via
/// `/admin/config` so the check is exact regardless of how the CLI launched.
fn check_config_drift() -> Result<Vec<Verdict>> {
    use crate::config;

    let mut findings = Vec::new();

    // FLUSH_INTERVAL_SECS — too high means a large data-loss window on crash.
    // 0 is fatal (timer never fires; rows only flush on threshold).
    match config::FLUSH_INTERVAL_SECS.value() {
        Ok(0) => findings.push(
            "SIGIRO_FLUSH_INTERVAL_SECS=0 — flush timer disabled; rows will only flush on row/byte threshold (data loss on crash)".to_string(),
        ),
        Ok(s) if s > 60 => findings.push(format!(
            "SIGIRO_FLUSH_INTERVAL_SECS={s} — large data-loss window on crash; recommend ≤ 10"
        )),
        _ => {}
    }

    // FLUSH_ROW_THRESHOLD — too high means the in-memory buffer can grow
    // unbounded for low-volume projects, then dump a huge batch on next flush.
    if let Ok(n) = config::FLUSH_ROW_THRESHOLD.value() {
        if n > 1_000_000 {
            findings.push(format!(
                "SIGIRO_FLUSH_ROW_THRESHOLD={n} — very high; in-memory buffer can grow large between flushes"
            ));
        }
    }

    // FLUSH_BYTE_THRESHOLD — too low causes constant small flushes (I/O thrash).
    if let Ok(b) = config::FLUSH_BYTE_THRESHOLD.value() {
        if b < 1_000_000 {
            findings.push(format!(
                "SIGIRO_FLUSH_BYTE_THRESHOLD={b} — very low; expect frequent small flushes (I/O overhead)"
            ));
        }
    }

    // BACKPRESSURE_POLICY — must be one of the recognized values, otherwise
    // startup fails fast.
    if let Ok(p) = config::BACKPRESSURE_POLICY.value() {
        if p != "block" && p != "drop-newest" {
            findings.push(format!(
                "SIGIRO_BACKPRESSURE_POLICY={p:?} — unknown; expected \"block\" or \"drop-newest\""
            ));
        }
    }

    if findings.is_empty() {
        return Ok(vec![Verdict::pass(
            "all runtime config values within known-good range",
        )]);
    }
    Ok(vec![
        Verdict::warn(format!(
            "{} suspicious runtime config value(s)",
            findings.len()
        ))
        .hints(findings.into_iter().map(|f| format!("  - {f}"))),
    ])
}

/// Run /v1/investigate on the worst error hotspot and surface the operations
/// investigate flagged as anomalous. This is the next click after the
/// svc-errors list: which specific operations are the actual cause.
async fn check_investigate_deep_dive(client: &SigiroClient) -> Result<Vec<Verdict>> {
    // Re-query the hotspot rather than threading it through from svc-errors:
    // sharing state across checks would make `run` order-dependent, which is
    // exactly the coupling that grew the old god function.
    let rows = query_with_retry(client, &service_error_sql(1))
        .await
        .context("deep-dive candidate query failed")?;
    let worst = rows
        .first()
        .filter(|row| error_rate(row) > ELEVATED_ERROR_PCT)
        .and_then(|row| opt_str_at(row, "service_name"));
    let Some(svc) = worst else {
        return Ok(vec![Verdict::pass("no hotspot to deep-dive")]);
    };

    // investigate() wants from/to in microseconds.
    let now_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    let from_us = now_us - 15 * 60 * 1_000_000;

    let block = investigate_with_retry(client, &svc, from_us, now_us)
        .await
        .context("investigate call failed")?;

    let anomalies: Vec<String> = block
        .operation_summaries
        .iter()
        .filter(|op| op.is_anomalous)
        .map(|op| {
            format!(
                "  {} — {}",
                op.span_name,
                op.anomaly_reason
                    .as_deref()
                    .unwrap_or("flagged anomalous (no reason given)")
            )
        })
        .collect();

    if anomalies.is_empty() {
        return Ok(vec![Verdict::pass(format!(
            "{svc}: investigated last 15 min, no anomalous operations"
        ))]);
    }
    Ok(vec![
        Verdict::warn(format!(
            "{svc}: {} anomalous operation(s) in last 15 min",
            anomalies.len()
        ))
        .hints(anomalies.into_iter().take(5)),
    ])
}

/// DuckDB memory headroom via `pragma_database_size()`.
///
/// This used to also report thread count via `SELECT current_setting`, but
/// `current_setting` reads ANY DuckDB setting by name
/// (`extension_directory`, `home_directory`, ...) with no way to restrict it
/// to just `'threads'` at the AST level, so it is deliberately excluded from
/// `server::SCALAR_FUNCTIONS` — allowing it back for this one check would
/// reopen the exact settings-leak hole that allowlist closes.
async fn check_duckdb_health(client: &SigiroClient) -> Result<Vec<Verdict>> {
    // pragma_database_size() returns human-readable strings like "244.1 MiB";
    // DuckDB exposes no numeric form via this pragma, so we parse in Rust.
    let sql = r#"
        SELECT memory_limit, memory_usage
        FROM pragma_database_size()
        WHERE database_name = 'lake'
    "#;
    let rows = query_with_retry(client, sql)
        .await
        .context("duckdb pragma query failed")?;
    let row = rows
        .first()
        .context("no row returned for database_name='lake'")?;
    let mem_limit = row
        .get("memory_limit")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_size_str)
        .context("memory_limit missing or unparseable")?;
    let mem_usage = row
        .get("memory_usage")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_size_str)
        .context("memory_usage missing or unparseable")?;

    let used_pct = if mem_limit > 0 {
        (mem_usage as f64 / mem_limit as f64) * 100.0
    } else {
        0.0
    };
    let summary = format!(
        "{:.1} / {:.1} MiB ({used_pct:.0}%)",
        bytes_to_mib(mem_usage),
        bytes_to_mib(mem_limit)
    );

    if used_pct >= 80.0 {
        return Ok(vec![Verdict::warn(format!("memory pressure — {summary}"))]);
    }
    Ok(vec![Verdict::pass(summary)])
}

/// Parse a DuckDB size string like "244.1 MiB" or "11.4 KiB" into bytes.
/// Returns None on unrecognised input — caller treats that as "unknown".
fn parse_size_str(s: &str) -> Option<u64> {
    let s = s.trim();
    // Split on whitespace: ["244.1", "MiB"]
    let mut parts = s.split_whitespace();
    let num: f64 = parts.next()?.parse().ok()?;
    let unit = parts.next().unwrap_or("B");
    let multiplier: u64 = match unit {
        "B" => 1,
        "KiB" | "KB" => 1024,
        "MiB" | "MB" => 1024 * 1024,
        "GiB" | "GB" => 1024 * 1024 * 1024,
        "TiB" | "TB" => 1024 * 1024 * 1024 * 1024,
        _ => return None,
    };
    Some((num * multiplier as f64) as u64)
}

fn bytes_to_mib(b: u64) -> f64 {
    b as f64 / (1024.0 * 1024.0)
}

// ---------------------------------------------------------------------------
// Report formatting
// ---------------------------------------------------------------------------

/// One line of the report, as produced by a check. The check decides pass vs
/// warn — it is the only code that knows what its numbers mean. `run` only
/// labels the line with a category.
struct Verdict {
    status: Status,
    message: String,
    hints: Vec<String>,
}

impl Verdict {
    fn pass(message: impl Into<String>) -> Self {
        Self::new(Status::Pass, message)
    }

    fn warn(message: impl Into<String>) -> Self {
        Self::new(Status::Warn, message)
    }

    fn fail(message: impl Into<String>) -> Self {
        Self::new(Status::Fail, message)
    }

    fn new(status: Status, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            hints: Vec::new(),
        }
    }

    /// Indented detail lines printed under the verdict. Takes any iterator so
    /// callers can pass a `Vec`, a `.take(3)`, or an `Option` for the
    /// at-most-one case without building a collection first.
    fn hints(mut self, hints: impl IntoIterator<Item = String>) -> Self {
        self.hints.extend(hints);
        self
    }
}

struct DiagnosisReport {
    checks: Vec<CheckResult>,
}

struct CheckResult {
    category: String,
    status: Status,
    message: String,
    hints: Vec<String>,
}

#[derive(Clone, Copy)]
enum Status {
    Pass,
    Warn,
    Fail,
}

impl DiagnosisReport {
    fn new() -> Self {
        Self { checks: Vec::new() }
    }

    fn push(&mut self, category: &str, verdict: Verdict) {
        self.checks.push(CheckResult {
            category: category.to_string(),
            status: verdict.status,
            message: verdict.message,
            hints: verdict.hints,
        });
    }

    /// Record a check's verdicts, turning a failed check into a warning.
    ///
    /// A diagnose run is worth more with 19 of 20 checks than with none, so a
    /// broken check degrades to one warning line instead of aborting. This is
    /// the only place in the module that inspects a check's `Result`.
    fn absorb(&mut self, category: &str, verdicts: Result<Vec<Verdict>>) {
        let verdicts =
            verdicts.unwrap_or_else(|e| vec![Verdict::warn(format!("check failed: {e:#}"))]);
        for verdict in verdicts {
            self.push(category, verdict);
        }
    }

    fn print(&self) {
        println!("\n╔══════════════════════════════════════════════════════════════╗");
        println!("║           sigiro self-diagnosis report                       ║");
        println!("╚══════════════════════════════════════════════════════════════╝\n");

        let (pass, warn, fail) = self.counts();
        for check in &self.checks {
            let icon = match check.status {
                Status::Pass => "✓",
                Status::Warn => "!",
                Status::Fail => "✗",
            };
            println!("  {icon}  {:12}  {}", check.category, check.message);
            for hint in &check.hints {
                println!("      → {hint}");
            }
        }

        println!();
        println!("  {pass} passed  {warn} warned  {fail} failed");
        println!();

        if fail > 0 {
            println!("  Action: fix failed checks before continuing.");
        } else if warn > 0 {
            println!("  Action: review warnings — may indicate degradation.");
        } else {
            println!("  All checks passed. sigiro is healthy.");
        }
        println!();
    }

    fn counts(&self) -> (usize, usize, usize) {
        let mut counts = (0, 0, 0);
        for check in &self.checks {
            match check.status {
                Status::Pass => counts.0 += 1,
                Status::Warn => counts.1 += 1,
                Status::Fail => counts.2 += 1,
            }
        }
        counts
    }
}
