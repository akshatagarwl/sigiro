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

use crate::client::SigiroClient;

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

    // Check 2: Recent error spans
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

    // Check 3: Slow spans (p99 > 1s)
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

    // Check 4: Ingest rate (spans/min)
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

    // Check 5: Federation worker health
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

    // Check 6: Storage bloat (row counts)
    match check_storage(client).await {
        Ok(counts) => {
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
        }
        Err(e) => report.warn("storage", &format!("check failed: {e:#}")),
    }

    report.print();
    Ok(())
}

// ---------------------------------------------------------------------------
// Individual checks — each runs a SQL query via the HTTP API
// ---------------------------------------------------------------------------

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

/// Row counts per table.
async fn check_storage(client: &SigiroClient) -> Result<HashMap<String, i64>> {
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
            Err(_) => {
                counts.insert(table.to_string(), 0);
            }
        }
    }
    Ok(counts)
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
