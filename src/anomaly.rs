//! Continuous, threshold-free anomaly detection over persisted telemetry.
//!
//! This is the scheduled half of the anomaly subsystem
//! (docs/plans/01-continuous-anomaly-subsystem.md): a periodic pass — driven
//! from the per-project inserter timer (src/inserter.rs) — that runs augurs
//! BOCPD changepoint detection over each operation's continuous 5-minute
//! error-rate series and persists the results into the `sigiro_anomalies`
//! table. `/v1/investigate` and `/v1/anomalies` READ that table; nothing is
//! recomputed inline per request.
//!
//! Threshold-free by construction: detection is Bayesian online changepoint
//! detection (`NormalGammaDetector`) on the z-scored series. The ONLY
//! structural choice is a direction — a shift is persisted when the series
//! mean AFTER the changepoint exceeds the mean BEFORE it ("worsening") — a
//! comparison, not a magic number. Constant or too-short series produce no
//! changepoints by construction, so there is no minimum-points gate either.
//!
//! This slice covers spans/error-rate only; other signals (latency, log
//! volume, profiles) and cross-signal correlation land in later phases.

use anyhow::{Context, Result};
use augurs_changepoint::{Detector, NormalGammaDetector};
use duckdb::Connection;

use crate::investigate::{operation_drill_filter, operation_key_expr};
use crate::types::StatusCode;

/// How far back the pass looks when building each operation's error-rate
/// series. Matches the raw-span retention default (7 days) — a coverage
/// window for the series, not a detection threshold: detection itself has
/// no tunables beyond the BOCPD prior.
const SERIES_LOOKBACK_DAYS: i64 = 7;

/// The signal this slice detects on. Later slices add more (p95, log volume).
const SIGNAL_ERROR_RATE: &str = "error_rate";

/// One worsening regime shift found in a series.
struct Shift {
    /// First bucket of the NEW regime, µs since epoch.
    ts_shift_us: i64,
    /// Series mean before the shift (raw units — error %).
    before: f64,
    /// Series mean from the shift onward.
    after: f64,
}

/// A fully-resolved anomaly row ready to insert.
struct AnomalyInsert {
    service: String,
    operation: String,
    shift: Shift,
    drill_down_sql: String,
}

fn mean(y: &[f64]) -> f64 {
    if y.is_empty() {
        return f64::NAN;
    }
    y.iter().sum::<f64>() / y.len() as f64
}

/// z-score the series. REQUIRED before handing it to `NormalGammaDetector`:
/// its default NormalGamma prior assumes standardized data and misbehaves on
/// raw scales. A constant series (σ = 0) maps to all-zeros — no
/// changepoints, by construction.
fn standardize(y: &[f64]) -> Vec<f64> {
    let m = mean(y);
    let var = y.iter().map(|v| (v - m).powi(2)).sum::<f64>() / y.len() as f64;
    let sd = var.sqrt();
    if sd == 0.0 {
        return vec![0.0; y.len()];
    }
    y.iter().map(|v| (v - m) / sd).collect()
}

/// BOCPD changepoints (augurs `NormalGammaDetector`) on the standardized
/// series. The detector always reports index 0 (the start of the first
/// regime) — dropped, it is not a shift.
fn changepoints(y: &[f64]) -> Vec<usize> {
    let z = standardize(y);
    let mut detector = NormalGammaDetector::default();
    let mut cps = detector.detect_changepoints(&z);
    cps.retain(|&i| i != 0);
    cps
}

/// All WORSENING regime shifts in one series: for each changepoint, the new
/// regime starts at `cp + 1` (verified augurs convention: the detector
/// reports the LAST index of the OLD regime), and the shift is kept only
/// when the mean after exceeds the mean before. Improvements never persist.
///
/// augurs runs inside `catch_unwind`: this pass shares a thread with ingest
/// flushing, and a detector panic must never take the flusher down with it.
fn worsening_shifts(bucket_ts: &[i64], y: &[f64]) -> Vec<Shift> {
    let cps = match std::panic::catch_unwind(|| changepoints(y)) {
        Ok(cps) => cps,
        Err(_) => {
            tracing::warn!(
                series_len = y.len(),
                "augurs changepoint detection panicked — skipping this series"
            );
            return Vec::new();
        }
    };
    cps.into_iter()
        .filter_map(|cp| {
            let start = cp + 1;
            if start >= y.len() {
                return None;
            }
            let before = mean(&y[..start]);
            let after = mean(&y[start..]);
            (after > before).then(|| Shift {
                ts_shift_us: bucket_ts[start],
                before,
                after,
            })
        })
        .collect()
}

/// One scheduled anomaly pass over a project's spans: build each
/// (service, operation)'s per-minute error-rate series over the last
/// [`SERIES_LOOKBACK_DAYS`], run BOCPD, and replace the project's
/// `sigiro_anomalies` rows with the worsening shifts found. Returns the
/// number of anomaly rows persisted.
///
/// Idempotent by full replace: the pass recomputes from the same continuous
/// history every time, so DELETE-then-INSERT inside one transaction keeps
/// repeated passes from accumulating duplicates (and a mid-pass failure
/// rolls back, leaving the previous pass's rows intact).
pub fn run_anomaly_pass(conn: &Connection, prefix: &str) -> Result<usize> {
    let started = std::time::Instant::now();
    let err = StatusCode::Error.as_i32();
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + i64::from(odt.microsecond())
    };
    let cutoff_us = now_us - SERIES_LOOKBACK_DAYS * 86_400 * 1_000_000;
    let op_key = operation_key_expr("s.");

    // 5-minute error-rate buckets per (service, operation), oldest first.
    // 5min (not 1min) is deliberate: BOCPD should flag SUSTAINED regime shifts,
    // not 1-minute blips — coarser buckets both denoise (fewer spurious shifts)
    // and cut the pass cost ~5x (it shares the writer thread with ingest flush,
    // so its runtime is bounded on purpose). Buckets only exist where spans
    // exist — gaps are simply absent. Same derived operation identity as
    // /v1/investigate (shared `operation_key_expr`), so investigate's LEFT JOIN
    // on (service, operation) always matches.
    let series_sql = format!(
        "SELECT s.service_name,
                {op_key} AS operation,
                any_value(s.span_name) AS span_name,
                epoch_us(time_bucket(INTERVAL '5 minutes', s.timestamp)) AS bucket_ts,
                count(CASE WHEN s.status_code = {err} THEN 1 END) * 100.0 / count(*) AS err_rate
         FROM {prefix}sigiro_spans s
         WHERE s.timestamp >= make_timestamp({cutoff_us})
           AND s.span_name NOT LIKE '%TraceService/Export%'
         GROUP BY s.service_name, operation, bucket_ts
         ORDER BY s.service_name, operation, bucket_ts"
    );

    let mut stmt = conn
        .prepare(&series_sql)
        .context("failed to prepare anomaly series query")?;
    let rows: Vec<(String, String, String, i64, f64)> = stmt
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .context("anomaly series query failed")?
        .collect::<std::result::Result<_, _>>()
        .context("anomaly series row read failed")?;

    // Group the ordered rows into per-(service, operation) series and detect.
    let mut inserts: Vec<AnomalyInsert> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let (service, operation, span_name) = (&rows[i].0, &rows[i].1, &rows[i].2);
        let mut bucket_ts = Vec::new();
        let mut err_rate = Vec::new();
        while i < rows.len() && rows[i].0 == *service && rows[i].1 == *operation {
            bucket_ts.push(rows[i].3);
            err_rate.push(rows[i].4);
            i += 1;
        }
        for shift in worsening_shifts(&bucket_ts, &err_rate) {
            let drill_down_sql = drill_sql(prefix, service, span_name, operation, &shift);
            inserts.push(AnomalyInsert {
                service: service.clone(),
                operation: operation.clone(),
                shift,
                drill_down_sql,
            });
        }
    }

    // Full replace inside one transaction — see the doc comment above.
    conn.execute_batch("BEGIN TRANSACTION")
        .context("failed to begin anomaly-pass transaction")?;
    let written = (|| -> Result<usize> {
        conn.execute(&format!("DELETE FROM {prefix}sigiro_anomalies"), [])
            .context("failed to clear sigiro_anomalies")?;
        let mut ins = conn
            .prepare(&format!(
                "INSERT INTO {prefix}sigiro_anomalies
                 (detected_at, service_name, operation, signal, kind, direction,
                  ts_shift, \"before\", \"after\", drill_down_sql)
                 VALUES (make_timestamp(?), ?, ?, '{SIGNAL_ERROR_RATE}', 'changepoint',
                         'worsening', make_timestamp(?), ?, ?, ?)"
            ))
            .context("failed to prepare sigiro_anomalies insert")?;
        for row in &inserts {
            ins.execute(duckdb::params![
                now_us,
                row.service,
                row.operation,
                row.shift.ts_shift_us,
                row.shift.before,
                row.shift.after,
                row.drill_down_sql,
            ])
            .context("failed to insert anomaly row")?;
        }
        Ok(inserts.len())
    })();
    match written {
        Ok(n) => {
            conn.execute_batch("COMMIT")
                .context("failed to commit anomaly-pass transaction")?;
            tracing::info!(
                anomalies = n,
                elapsed_ms = started.elapsed().as_millis() as u64,
                "anomaly pass completed"
            );
            Ok(n)
        }
        Err(e) => {
            if let Err(rb) = conn.execute_batch("ROLLBACK") {
                tracing::warn!("anomaly-pass rollback failed: {rb}");
            }
            Err(e)
        }
    }
}

/// Ready-to-run /v1/query SQL for the raw spans behind an anomaly: this
/// operation's spans from the shift onward (the new regime), errors first.
/// Uses only allowlisted scalar functions so it passes `validate_read_only`
/// verbatim.
fn drill_sql(
    prefix: &str,
    service: &str,
    span_name: &str,
    operation: &str,
    shift: &Shift,
) -> String {
    let err = StatusCode::Error.as_i32();
    let svc = service.replace('\'', "''");
    let filter = operation_drill_filter(span_name, operation);
    let ts = shift.ts_shift_us;
    format!(
        "SELECT * FROM {prefix}sigiro_spans \
         WHERE service_name = '{svc}' AND {filter} AND timestamp >= make_timestamp({ts}) \
         ORDER BY CASE WHEN status_code = {err} THEN 0 ELSE 1 END, duration DESC"
    )
}
