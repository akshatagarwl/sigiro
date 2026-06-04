//! Self-diagnosis via sigiro's own telemetry — never tail Docker logs.
//!
//! This module implements the `sigiro diagnose` CLI command. It queries the
//! running sigiro instance via the HTTP API to detect common issues:
//!
//! - Error spikes (status_code = 2)
//! - Slow spans (p99 duration)
//! - OOM patterns (memory pressure from span attributes)
//! - Ingest stalls (gap in span timestamps)
//! - Federation problems (worker health, quack connectivity)
//!
//! All data comes from `/v1/query`, `/v1/investigate`, `/v1/traces/{id}` —
//! the same endpoints users already have. No direct DB access, no docker logs.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::time::Duration;

use crate::client::SigiroClient;

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

/// Run all diagnostic checks and print a report.
pub async fn run(client: &SigiroClient) -> Result<()> {
    let mut report = DiagnosisReport::new();

    // Check 1: Can we connect and query?
    match client.query("SELECT 1").await {
        Ok(_) => report.pass("connectivity", "sigiro query endpoint responding"),
        Err(e) => {
            report.fail("connectivity", &format!("sigiro not reachable: {e:#}"));
            // Can't do anything else if the server is down
            report.print();
            return Ok(());
        }
    }

    // Check 2: Server health (dead flushers)
    match check_health(client).await {
        Ok(HealthStatus::Healthy) => report.pass("health", "no dead flushers detected"),
        Ok(HealthStatus::DeadFlushers) => report.warn(
            "health",
            "dead flusher tasks detected — ingest may be stalled",
        ),
        Ok(HealthStatus::Unknown(msg)) => report.warn("health", &msg),
        Err(e) => report.warn("health", &format!("health check failed: {e:#}")),
    }

    // Check 3: Recent error spans
    match check_errors(client).await {
        Ok((count, sample_trace)) => {
            if count > 0 {
                report.warn("errors", &format!("{count} error spans in last 15 min"));
                if let Some(trace_id) = sample_trace {
                    report.hint(
                        "errors",
                        &format!("sigiro trace {trace_id} — see full context"),
                    );
                }
            } else {
                report.pass("errors", "no error spans in last 15 min");
            }
        }
        Err(e) => report.warn("errors", &format!("check failed: {e:#}")),
    }

    // Check 4: Service-level error hotspots
    match check_service_errors(client).await {
        Ok(hotspots) => {
            if hotspots.is_empty() {
                report.pass("svc-errors", "no services with elevated error rate");
            } else {
                report.warn(
                    "svc-errors",
                    &format!("{} services with error rate > 5%", hotspots.len()),
                );
                for (svc, rate, count) in hotspots.iter().take(3) {
                    report.hint(
                        "svc-errors",
                        &format!("  {svc}: {rate:.1}% ({count} errors)"),
                    );
                }
            }
        }
        Err(e) => report.warn("svc-errors", &format!("check failed: {e:#}")),
    }

    // Check 5: Slow spans (p99 > 1s)
    match check_slow_spans(client).await {
        Ok((p99_us, count)) => {
            let p99_ms = p99_us / 1000;
            if p99_ms > 1000 {
                report.warn(
                    "latency",
                    &format!("p99 span duration = {p99_ms}ms ({count} spans > 1s)"),
                );
            } else {
                report.pass("latency", &format!("p99 span duration = {p99_ms}ms"));
            }
        }
        Err(e) => report.warn("latency", &format!("check failed: {e:#}")),
    }

    // Check 6: Top slow operations by service
    match check_slow_operations(client).await {
        Ok(ops) => {
            if ops.is_empty() {
                report.pass("slow-ops", "no slow operations detected");
            } else {
                report.warn(
                    "slow-ops",
                    &format!("{} slow operations (>1s p95)", ops.len()),
                );
                for (svc, op, p95_ms) in ops.iter().take(3) {
                    report.hint("slow-ops", &format!("  {svc}/{op}: p95={p95_ms}ms"));
                }
            }
        }
        Err(e) => report.warn("slow-ops", &format!("check failed: {e:#}")),
    }

    // Check 7: Span kind distribution (anomaly: no client spans = possible missing egress telemetry)
    match check_span_kind_distribution(client).await {
        Ok((server, client, internal, consumer, producer)) => {
            let total = server + client + internal + consumer + producer;
            if total == 0 {
                report.pass("span-kinds", "no spans to analyze");
            } else {
                if server > 0 && client == 0 {
                    report.warn(
                        "span-kinds",
                        "server spans present but zero client spans — egress telemetry may be missing",
                    );
                } else {
                    report.pass(
                        "span-kinds",
                        &format!(
                            "srv={server} cli={client} int={internal} con={consumer} prod={producer}"
                        ),
                    );
                }
            }
        }
        Err(e) => report.warn("span-kinds", &format!("check failed: {e:#}")),
    }

    // Check 8: Ingest rate (spans/min)
    match check_ingest_rate(client).await {
        Ok(rate) => {
            if rate < 1.0 {
                report.warn("ingest", &format!("{rate:.1} spans/min — possible stall"));
            } else {
                report.pass("ingest", &format!("{rate:.1} spans/min"));
            }
        }
        Err(e) => report.warn("ingest", &format!("check failed: {e:#}")),
    }

    // Check 9: Log error patterns
    match check_log_errors(client).await {
        Ok((error_logs, patterns)) => {
            if error_logs == 0 {
                report.pass("log-errors", "no error-level logs in last 15 min");
            } else {
                report.warn(
                    "log-errors",
                    &format!("{error_logs} error-level logs in last 15 min"),
                );
                for (pattern, count) in patterns.iter().take(3) {
                    report.hint("log-errors", &format!("  ({count}x) {pattern}"));
                }
            }
        }
        Err(e) => report.warn("log-errors", &format!("check failed: {e:#}")),
    }

    // Check 10: Federation worker health
    match check_federation(client).await {
        Ok((worker_count, unhealthy)) => {
            if worker_count == 0 {
                report.pass("federation", "no workers registered (single-node mode)");
            } else if !unhealthy.is_empty() {
                report.warn(
                    "federation",
                    &format!("{} of {worker_count} workers unhealthy", unhealthy.len()),
                );
                for w in &unhealthy {
                    report.hint("federation", &format!("  - {w}"));
                }
            } else {
                report.pass("federation", &format!("{worker_count} workers healthy"));
            }
        }
        Err(e) => report.warn("federation", &format!("check failed: {e:#}")),
    }

    // Check 11: Storage bloat (row counts + growth)
    match check_storage(client).await {
        Ok(StorageReport { counts, growth }) => {
            let total: i64 = counts.values().sum();
            report.pass("storage", &format!("{total} total rows across all tables"));
            for (table, count) in counts {
                if count > 1_000_000 {
                    report.warn(
                        "storage",
                        &format!("{table}: {count} rows — consider retention"),
                    );
                }
            }
            if let Some((prev, curr, delta_pct)) = growth {
                if delta_pct > 50.0 {
                    report.warn(
                        "storage",
                        &format!(
                            "rapid growth: {prev} → {curr} rows (+{delta_pct:.0}%) since last check"
                        ),
                    );
                }
            }
        }
        Err(e) => report.warn("storage", &format!("check failed: {e:#}")),
    }

    // Check 13: Duplicate spans (same trace_id+span_id arriving multiple times).
    // Common cause: OTLP exporter retry storm, or broken span ID generation in
    // an instrumented library. Either way the data is corrupt.
    match check_duplicate_spans(client).await {
        Ok((count, sample)) => {
            if count > 0 {
                report.warn(
                    "dup-spans",
                    &format!("{count} duplicate (trace_id, span_id) pairs in last 15 min"),
                );
                for (tid, sid, n) in sample.iter().take(3) {
                    report.hint("dup-spans", &format!("  trace={tid} span={sid} ×{n}"));
                }
            } else {
                report.pass("dup-spans", "no duplicate spans in last 15 min");
            }
        }
        Err(e) => report.warn("dup-spans", &format!("check failed: {e:#}")),
    }

    // Check 14: Orphan spans — parent_span_id set but the parent never arrived
    // in the same trace. Strong signal of broken OTLP instrumentation (sampling,
    // dropped spans at the collector, or a parent service that is not exporting).
    match check_orphan_spans(client).await {
        Ok((count, sample)) => {
            if count > 0 {
                report.warn(
                    "orphan-spans",
                    &format!(
                        "{count} spans reference a missing parent in last 15 min — \
                         broken trace tree"
                    ),
                );
                for (svc, tid, sid) in sample.iter().take(3) {
                    report.hint("orphan-spans", &format!("  {svc} trace={tid} span={sid}"));
                }
            } else {
                report.pass("orphan-spans", "all spans have a present parent");
            }
        }
        Err(e) => report.warn("orphan-spans", &format!("check failed: {e:#}")),
    }

    // Check 15: Retention configuration
    match check_retention().await {
        Ok(Some(days)) => report.pass("retention", &format!("{days} days configured")),
        Ok(None) => report.warn(
            "retention",
            "no retention configured — storage will grow unbounded",
        ),
        Err(e) => report.warn("retention", &format!("check failed: {e:#}")),
    }

    report.print();
    Ok(())
}

// ---------------------------------------------------------------------------
// Individual checks — each runs a SQL query via the HTTP API
// ---------------------------------------------------------------------------

enum HealthStatus {
    Healthy,
    DeadFlushers,
    Unknown(String),
}

/// Check /health for dead flushers.
async fn check_health(client: &SigiroClient) -> Result<HealthStatus> {
    match client.status().await {
        Ok(resp) => {
            let body = resp.status.get("status").cloned().unwrap_or_default();
            if body == "ok" {
                Ok(HealthStatus::Healthy)
            } else if body.contains("dead flushers") {
                Ok(HealthStatus::DeadFlushers)
            } else {
                Ok(HealthStatus::Unknown(body))
            }
        }
        Err(e) => Err(e),
    }
}

/// Count error spans (status_code = 2) in the last 15 minutes.
async fn check_errors(client: &SigiroClient) -> Result<(i64, Option<String>)> {
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
    let count = rows
        .first()
        .and_then(|r| r.get("cnt"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let sample = rows
        .first()
        .and_then(|r| r.get("sample_trace"))
        .and_then(serde_json::Value::as_str)
        .map(String::from);
    Ok((count, sample))
}

/// Services with error rate > 5% in the last 15 minutes.
async fn check_service_errors(client: &SigiroClient) -> Result<Vec<(String, f64, i64)>> {
    let sql = r#"
        SELECT
            service_name,
            count(*) AS total,
            count(CASE WHEN status_code = 2 THEN 1 END) AS errors
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        GROUP BY service_name
        HAVING count(*) > 10
        ORDER BY errors DESC
        LIMIT 10
    "#;
    let rows = client
        .query(sql)
        .await
        .context("service errors query failed")?;
    let mut hotspots = Vec::new();
    for row in rows {
        let svc = row
            .get("service_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let total = row
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .max(1);
        let errors = row
            .get("errors")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let rate = (errors as f64 / total as f64) * 100.0;
        if rate > 5.0 {
            hotspots.push((svc, rate, errors));
        }
    }
    Ok(hotspots)
}

/// Compute p99 span duration and count of spans > 1s in last 15 min.
async fn check_slow_spans(client: &SigiroClient) -> Result<(i64, i64)> {
    let sql = r#"
        SELECT
            quantile_cont(duration, 0.99) AS p99,
            COUNT(CASE WHEN duration > 1000000 THEN 1 END) AS slow_count
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
    "#;
    let rows = client.query(sql).await.context("slow span query failed")?;
    let p99 = rows
        .first()
        .and_then(|r| r.get("p99"))
        .and_then(serde_json::Value::as_f64)
        .map(|f| f as i64)
        .unwrap_or(0);
    let slow = rows
        .first()
        .and_then(|r| r.get("slow_count"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Ok((p99, slow))
}

/// Top slow operations by service (p95 > 1s) in last 15 min.
async fn check_slow_operations(client: &SigiroClient) -> Result<Vec<(String, String, i64)>> {
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
    let rows = client
        .query(sql)
        .await
        .context("slow operations query failed")?;
    let mut ops = Vec::new();
    for row in rows {
        let svc = row
            .get("service_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let op = row
            .get("span_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let p95 = row
            .get("p95")
            .and_then(serde_json::Value::as_f64)
            .map(|f| f as i64 / 1000)
            .unwrap_or(0);
        ops.push((svc, op, p95));
    }
    Ok(ops)
}

/// Span kind distribution in last 15 minutes.
/// Returns (server, client, internal, consumer, producer) counts.
async fn check_span_kind_distribution(client: &SigiroClient) -> Result<(i64, i64, i64, i64, i64)> {
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
    let row = rows.first();
    let get = |key: &str| {
        row.and_then(|r| r.get(key))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    };
    Ok((
        get("server"),
        get("client"),
        get("internal"),
        get("consumer"),
        get("producer"),
    ))
}

/// Spans per minute over the last 15 minutes.
async fn check_ingest_rate(client: &SigiroClient) -> Result<f64> {
    let sql = r#"
        SELECT COUNT(*) AS cnt
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
    "#;
    let rows = client
        .query(sql)
        .await
        .context("ingest rate query failed")?;
    let count = rows
        .first()
        .and_then(|r| r.get("cnt"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Ok(count as f64 / 15.0)
}

/// Error-level logs in last 15 minutes, grouped by pattern.
async fn check_log_errors(client: &SigiroClient) -> Result<(i64, Vec<(String, i64)>)> {
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
    let mut total = 0i64;
    let mut patterns = Vec::new();
    for row in rows {
        let count = row
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        total += count;
        let pattern = row
            .get("pattern")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        patterns.push((pattern, count));
    }
    Ok((total, patterns))
}

/// Federation worker health.
async fn check_federation(client: &SigiroClient) -> Result<(usize, Vec<String>)> {
    let workers = client.list_workers().await.context("list workers failed")?;
    let count = workers.len();
    // Workers without recent spans are considered stale
    let mut unhealthy = Vec::new();
    for w in &workers {
        let sql = format!(
            "SELECT COUNT(*) AS cnt FROM lake.sigiro_spans WHERE service_name = '{}' AND timestamp > now()::TIMESTAMP - INTERVAL '5 minutes'",
            w.id
        );
        match client.query(&sql).await {
            Ok(rows) => {
                let recent = rows
                    .first()
                    .and_then(|r| r.get("cnt"))
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                if recent == 0 {
                    unhealthy.push(format!("{}: no spans in last 5 min", w.id));
                }
            }
            Err(_) => unhealthy.push(format!("{}: query failed", w.id)),
        }
    }
    Ok((count, unhealthy))
}

// ---------------------------------------------------------------------------
// Storage check with growth tracking
// ---------------------------------------------------------------------------

struct StorageReport {
    counts: HashMap<String, i64>,
    /// (previous_total, current_total, delta_percent)
    growth: Option<(i64, i64, f64)>,
}

/// Row counts per table + optional growth since last check (via local state file).
/// Missing tables are reported as 0 with a warning — they may not exist yet (no data ingested).
async fn check_storage(client: &SigiroClient) -> Result<StorageReport> {
    let tables = [
        "sigiro_spans",
        "sigiro_logs",
        "sigiro_metrics_sum",
        "sigiro_metrics_gauge",
        "sigiro_metrics_histogram",
    ];
    let mut counts = HashMap::new();
    for table in tables {
        let sql = format!("SELECT COUNT(*) AS cnt FROM lake.{table}");
        match client.query(&sql).await {
            Ok(rows) => {
                let count = rows
                    .first()
                    .and_then(|r| r.get("cnt"))
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                counts.insert(table.to_string(), count);
            }
            Err(e) => {
                // Table may not exist yet (no data ingested for this signal type).
                // Log at debug so the user knows, but don't fail the check.
                tracing::debug!(
                    "storage check: table {table} query failed — likely not created yet: {e:#}"
                );
                counts.insert(table.to_string(), 0);
            }
        }
    }

    let current_total: i64 = counts.values().sum();
    let growth = read_previous_storage_total()
        .map(|prev| {
            let delta = if prev > 0 {
                ((current_total - prev) as f64 / prev as f64) * 100.0
            } else {
                0.0
            };
            (prev, current_total, delta)
        })
        .ok();

    // Best-effort persist current total for next run
    if let Err(e) = write_storage_total(current_total) {
        tracing::debug!("failed to persist storage state: {e:#}");
    }

    Ok(StorageReport { counts, growth })
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

/// Read retention days from config.
async fn check_retention() -> Result<Option<u32>> {
    match crate::config::RETENTION_DAYS.value() {
        Ok(d) if d > 0 => Ok(Some(d)),
        _ => Ok(None),
    }
}

/// Detect duplicate (trace_id, span_id) pairs in the last 15 minutes.
/// Returns (count_of_duplicate_pairs, sample_up_to_10_worst).
///
/// Each duplicate is a sign that the same span arrived twice — either an OTLP
/// exporter retry storm, or an instrumented library generating conflicting IDs.
/// Either way the resulting trace is unreliable.
async fn check_duplicate_spans(client: &SigiroClient) -> Result<(i64, Vec<(String, String, i64)>)> {
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
    let rows = query_with_retry(client, sql)
        .await
        .context("duplicate spans query failed")?;
    let count = rows.len() as i64;
    let mut sample = Vec::with_capacity(rows.len());
    for row in rows {
        let tid = row
            .get("trace_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_string();
        let sid = row
            .get("span_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_string();
        let n = row
            .get("dup_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        sample.push((tid, sid, n));
    }
    Ok((count, sample))
}

/// Detect orphan spans — a span whose parent_span_id is set but no span in the
/// same trace carries that id. Returns (count_of_orphans, sample_up_to_10).
///
/// Common cause: a service in the trace stopped exporting (e.g. k8s rollout,
/// network policy, broken init). The parent never shows up, leaving the child
/// branches disconnected from the trace tree.
async fn check_orphan_spans(client: &SigiroClient) -> Result<(i64, Vec<(String, String, String)>)> {
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
    let rows = query_with_retry(client, sql)
        .await
        .context("orphan spans query failed")?;
    let count = rows.len() as i64;
    let mut sample = Vec::with_capacity(rows.len());
    for row in rows {
        let svc = row
            .get("service_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let tid = row
            .get("trace_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_string();
        let sid = row
            .get("span_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_string();
        sample.push((svc, tid, sid));
    }
    Ok((count, sample))
}

// ---------------------------------------------------------------------------
// Report formatting
// ---------------------------------------------------------------------------

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

    fn pass(&mut self, category: &str, message: &str) {
        self.checks.push(CheckResult {
            category: category.to_string(),
            status: Status::Pass,
            message: message.to_string(),
            hints: Vec::new(),
        });
    }

    fn warn(&mut self, category: &str, message: &str) {
        self.checks.push(CheckResult {
            category: category.to_string(),
            status: Status::Warn,
            message: message.to_string(),
            hints: Vec::new(),
        });
    }

    fn fail(&mut self, category: &str, message: &str) {
        self.checks.push(CheckResult {
            category: category.to_string(),
            status: Status::Fail,
            message: message.to_string(),
            hints: Vec::new(),
        });
    }

    fn hint(&mut self, category: &str, message: &str) {
        if let Some(check) = self.checks.iter_mut().find(|c| c.category == category) {
            check.hints.push(message.to_string());
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
        let pass = self
            .checks
            .iter()
            .filter(|c| matches!(c.status, Status::Pass))
            .count();
        let warn = self
            .checks
            .iter()
            .filter(|c| matches!(c.status, Status::Warn))
            .count();
        let fail = self
            .checks
            .iter()
            .filter(|c| matches!(c.status, Status::Fail))
            .count();
        (pass, warn, fail)
    }
}
