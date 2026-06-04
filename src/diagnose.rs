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

    // Check 16: Cross-signal coverage — services sending spans but no metrics.
    // A service with telemetry but no metrics is a sign of incomplete
    // instrumentation (metrics exporter missing or not configured).
    match check_metrics_coverage(client).await {
        Ok(uncovered) => {
            if uncovered.is_empty() {
                report.pass(
                    "metrics-coverage",
                    "all span-emitting services also send metrics",
                );
            } else {
                report.warn(
                    "metrics-coverage",
                    &format!(
                        "{} services send spans but no metrics — incomplete instrumentation",
                        uncovered.len()
                    ),
                );
                for svc in uncovered.iter().take(5) {
                    report.hint("metrics-coverage", &format!("  - {svc}"));
                }
            }
        }
        Err(e) => report.warn("metrics-coverage", &format!("check failed: {e:#}")),
    }

    // Check 17: eBPF status — read /admin/status for the kernel-collector state.
    // "ready" means obi is on disk with the right capabilities; "not_installed"
    // or "disabled" means the user can install/configure eBPF, or accept the gap.
    match check_ebpf_status(client).await {
        Ok(EbpfState::Ready) => report.pass("ebpf", "ready (obi installed, capabilities ok)"),
        Ok(EbpfState::Disabled(reason)) => {
            report.warn("ebpf", &format!("disabled — {reason}"));
        }
        Ok(EbpfState::Unknown) => {
            report.warn(
                "ebpf",
                "status not initialized — server may be a fresh install or pre-restart",
            );
        }
        Err(e) => report.warn("ebpf", &format!("check failed: {e:#}")),
    }

    // Check 18: Rate-limit pressure — flag services that dominate the
    // recent ingest rate. A single service emitting most of the spans is
    // a signal that it is hammering the API and may soon be the only one
    // the governor bucket serves.
    match check_rate_pressure(client).await {
        Ok(PressureReport { top, dominant_pct }) => {
            if top.is_empty() {
                report.pass("rate-pressure", "no recent spans to analyze");
            } else if dominant_pct >= 50.0 {
                report.warn(
                    "rate-pressure",
                    &format!(
                        "1 service accounts for {dominant_pct:.0}% of last 1 min — risk of governor starvation"
                    ),
                );
                if let Some((svc, n, pct)) = top.first() {
                    report.hint("rate-pressure", &format!("  {svc}: {n} spans ({pct:.0}%)"));
                }
            } else {
                report.pass(
                    "rate-pressure",
                    &format!("{dominant_pct:.0}% from top service, no dominance"),
                );
            }
        }
        Err(e) => report.warn("rate-pressure", &format!("check failed: {e:#}")),
    }

    // Check 19: Config drift — read typed env vars and warn on suspicious
    // values that hurt durability (large flush windows), correctness
    // (unknown backpressure policy), or performance (tiny byte threshold).
    match check_config_drift().await {
        Ok(report_lines) => {
            if report_lines.is_empty() {
                report.pass(
                    "config-drift",
                    "all runtime config values within known-good range",
                );
            } else {
                report.warn(
                    "config-drift",
                    &format!("{} suspicious runtime config value(s)", report_lines.len()),
                );
                for line in report_lines {
                    report.hint("config-drift", &format!("  - {line}"));
                }
            }
        }
        Err(e) => report.warn("config-drift", &format!("check failed: {e:#}")),
    }

    // Check 20: Deep-dive on the worst service-error hotspot. If svc-errors
    // found a service with elevated errors, run /v1/investigate on it and
    // surface any operations flagged as anomalous (is_anomalous=true).
    // Failures here are non-fatal — fall back to the surface hotspot list.
    match check_investigate_deep_dive(client).await {
        Ok(DeepDiveReport { service, anomalies }) => match (service, anomalies.is_empty()) {
            (None, _) => {
                report.pass("deep-dive", "no hotspot to deep-dive");
            }
            (Some(svc), true) => {
                report.pass(
                    "deep-dive",
                    &format!("{svc}: investigated last 15 min, no anomalous operations"),
                );
            }
            (Some(svc), false) => {
                report.warn(
                    "deep-dive",
                    &format!(
                        "{svc}: {} anomalous operation(s) in last 15 min",
                        anomalies.len()
                    ),
                );
                for (op, reason) in anomalies.iter().take(5) {
                    report.hint("deep-dive", &format!("  {op} — {reason}"));
                }
            }
        },
        Err(e) => report.warn("deep-dive", &format!("check failed: {e:#}")),
    }

    // Check 21: DuckDB health — read memory usage and thread count via
    // pragma_database_size() + current_setting(). Warn on memory pressure
    // (usage > 80 % of limit) or single-threaded execution on a box that
    // clearly has cores to spare.
    match check_duckdb_health(client).await {
        Ok(DuckDbReport {
            mem_limit_bytes,
            mem_usage_bytes,
            mem_used_pct,
            threads,
        }) => {
            let mem_str = format!(
                "{:.1} / {:.1} MiB ({mem_used_pct:.0}%)",
                bytes_to_mib(mem_usage_bytes),
                bytes_to_mib(mem_limit_bytes)
            );
            if mem_used_pct >= 80.0 {
                report.warn("duckdb", &format!("memory pressure — {mem_str}"));
            } else if threads == 1 {
                report.warn(
                    "duckdb",
                    &format!("{mem_str}, threads=1 — consider raising thread_count"),
                );
            } else {
                report.pass("duckdb", &format!("{mem_str}, threads={threads}"));
            }
        }
        Err(e) => report.warn("duckdb", &format!("check failed: {e:#}")),
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

/// Find services that emitted spans in the last 15 minutes but did not emit
/// any metrics (sum/gauge/histogram) in the same window — a strong signal
/// of incomplete instrumentation (metrics exporter not wired up).
async fn check_metrics_coverage(client: &SigiroClient) -> Result<Vec<String>> {
    let sql = r#"
        WITH span_services AS (
            SELECT DISTINCT service_name
            FROM lake.sigiro_spans
            WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        ),
        metric_services AS (
            SELECT service_name FROM lake.sigiro_metrics_sum
                WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
            UNION
            SELECT service_name FROM lake.sigiro_metrics_gauge
                WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
            UNION
            SELECT service_name FROM lake.sigiro_metrics_histogram
                WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        )
        SELECT s.service_name
        FROM span_services s
        WHERE s.service_name NOT IN (SELECT service_name FROM metric_services)
        ORDER BY s.service_name
    "#;
    let rows = query_with_retry(client, sql)
        .await
        .context("metrics coverage query failed")?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let svc = row
            .get("service_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        out.push(svc);
    }
    Ok(out)
}

enum EbpfState {
    Ready,
    Disabled(String),
    /// /admin/status did not contain an `ebpf_status` key — likely a fresh
    /// install or the server hasn't run the eBPF readiness check yet.
    Unknown,
}

/// Surface the eBPF auto-instrumentation status from the server's admin
/// status table. The server writes the status via `write_ebpf_status`
/// (see src/ebpf.rs); we just read it back.
async fn check_ebpf_status(client: &SigiroClient) -> Result<EbpfState> {
    let resp = client
        .admin_status()
        .await
        .context("admin/status call failed")?;
    let status = match resp.status.get("ebpf_status") {
        Some(s) => s.as_str(),
        None => return Ok(EbpfState::Unknown),
    };
    match status {
        "ready" => Ok(EbpfState::Ready),
        "not_installed" | "disabled" => {
            let reason = resp
                .status
                .get("ebpf_reason")
                .cloned()
                .unwrap_or_else(|| "no reason provided".to_string());
            Ok(EbpfState::Disabled(reason))
        }
        other => Ok(EbpfState::Disabled(format!("unexpected status: {other}"))),
    }
}

struct PressureReport {
    /// Up to 5 top services in the last 1 min, ordered by span count desc.
    top: Vec<(String, i64, f64)>,
    /// The share (0–100) held by the top service, or 0.0 if there is no traffic.
    dominant_pct: f64,
}

/// Look at the last 1 minute of spans and report top services by volume.
/// A single service holding ≥ 50 % of the share is treated as pressure
/// on the governor bucket — it can starve the rest.
async fn check_rate_pressure(client: &SigiroClient) -> Result<PressureReport> {
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

    let mut top: Vec<(String, i64, f64)> = Vec::with_capacity(rows.len());
    let mut total: i64 = 0;
    let mut raw: Vec<(String, i64)> = Vec::with_capacity(rows.len());
    for row in &rows {
        let svc = row
            .get("service_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let n = row
            .get("cnt")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        total += n;
        raw.push((svc, n));
    }
    let denom = if total > 0 { total as f64 } else { 1.0 };
    for (svc, n) in raw {
        let pct = (n as f64 / denom) * 100.0;
        top.push((svc, n, pct));
    }
    let dominant_pct = top.first().map(|t| t.2).unwrap_or(0.0);
    Ok(PressureReport { top, dominant_pct })
}

/// Read typed env vars and flag values that hurt durability, correctness,
/// or performance. Each line is a self-contained explanation suitable for
/// a `hint` under the "config-drift" check.
///
/// Note: this reads the **CLI process**'s env, not the server's. In typical
/// deployments (single host, docker compose, shared `.env`) the two match
/// and the check is accurate. A future improvement would expose the
/// server's effective config via `/admin/config` so the check is exact
/// regardless of how the CLI was launched.
async fn check_config_drift() -> Result<Vec<String>> {
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
        Ok(_) => {}
        Err(_) => {}
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
    // MsgRouter falls back silently and the operator's intent is lost.
    if let Ok(p) = config::BACKPRESSURE_POLICY.value() {
        if p != "block" && p != "drop-oldest" {
            findings.push(format!(
                "SIGIRO_BACKPRESSURE_POLICY={p:?} — unknown; expected \"block\" or \"drop-oldest\""
            ));
        }
    }

    Ok(findings)
}

struct DeepDiveReport {
    /// The service that was investigated, if any hotspot was found.
    service: Option<String>,
    /// (operation_name, anomaly_reason) pairs flagged by investigate.
    anomalies: Vec<(String, String)>,
}

/// Run /v1/investigate on the worst svc-errors hotspot and surface the
/// operations that investigate flagged as anomalous. This is a *deep-dive*
/// on top of the surface hotspots from check_service_errors — it gives the
/// operator the next click: which specific operations are the actual cause.
async fn check_investigate_deep_dive(client: &SigiroClient) -> Result<DeepDiveReport> {
    // Re-query svc-errors locally to pick the worst hotspot. We don't pass
    // the result through because run() has already consumed the first batch.
    let sql = r#"
        SELECT service_name,
               count(*) AS total,
               count(CASE WHEN status_code = 2 THEN 1 END) AS errors
        FROM lake.sigiro_spans
        WHERE timestamp > now()::TIMESTAMP - INTERVAL '15 minutes'
        GROUP BY service_name
        HAVING count(*) > 10
        ORDER BY errors DESC
        LIMIT 1
    "#;
    let rows = query_with_retry(client, sql)
        .await
        .context("deep-dive candidate query failed")?;
    let worst = rows.first().and_then(|r| {
        let svc = r
            .get("service_name")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let total = r
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .max(1);
        let errors = r
            .get("errors")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let rate = (errors as f64 / total as f64) * 100.0;
        if rate > 5.0 { Some((svc, rate)) } else { None }
    });

    let (svc, _rate) = match worst {
        Some(t) => t,
        None => {
            return Ok(DeepDiveReport {
                service: None,
                anomalies: Vec::new(),
            });
        }
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

    let anomalies = block
        .operation_summaries
        .iter()
        .filter(|op| op.is_anomalous.unwrap_or(false))
        .map(|op| {
            let reason = op
                .anomaly_reason
                .clone()
                .unwrap_or_else(|| "flagged anomalous (no reason given)".to_string());
            (op.span_name.clone(), reason)
        })
        .collect();

    Ok(DeepDiveReport {
        service: Some(svc),
        anomalies,
    })
}

struct DuckDbReport {
    mem_limit_bytes: u64,
    mem_usage_bytes: u64,
    mem_used_pct: f64,
    threads: u64,
}

/// Query DuckDB's runtime state via `pragma_database_size()` (memory limit
/// and current usage for the project DB) plus `current_setting('threads')`.
/// Warn on memory pressure or single-threaded execution.
async fn check_duckdb_health(client: &SigiroClient) -> Result<DuckDbReport> {
    // Note: pragma_database_size() returns human-readable strings like
    // "244.1 MiB". We parse them in Rust (see parse_size_str) since DuckDB
    // doesn't expose a numeric form via this pragma.
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

    let threads_sql = "SELECT current_setting('threads') AS threads";
    let threads_rows = query_with_retry(client, threads_sql)
        .await
        .context("duckdb threads query failed")?;
    let threads: u64 = threads_rows
        .first()
        .and_then(|r| r.get("threads"))
        .and_then(serde_json::Value::as_str)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let mem_used_pct = if mem_limit > 0 {
        (mem_usage as f64 / mem_limit as f64) * 100.0
    } else {
        0.0
    };

    Ok(DuckDbReport {
        mem_limit_bytes: mem_limit,
        mem_usage_bytes: mem_usage,
        mem_used_pct,
        threads,
    })
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
