//! Continuous, threshold-free anomaly detection over persisted telemetry.
//!
//! This is the scheduled half of the anomaly subsystem
//! (docs/plans/01-continuous-anomaly-subsystem.md): a periodic pass — driven
//! from the per-project inserter timer (src/inserter.rs) — that runs augurs
//! BOCPD changepoint detection over continuous 5-minute series of every
//! detected signal and persists the results into the `sigiro_anomalies`
//! table. `/v1/diagnose` and `/v1/anomalies` READ that table; nothing is
//! recomputed inline per request.
//!
//! Signals detected, and the entity each series is keyed by (stored in the
//! table's `operation` column):
//! - `error_rate`     — per (service, operation): % of error spans per bucket.
//! - `latency_p95`    — per (service, operation): p95 span duration (µs).
//! - `log_volume`     — per service (logs have no operation; per-template
//!   series would emit N anomalies for one flood): log count per bucket.
//!   `operation` holds the service name.
//! - `error_log_rate` — per service: % of ERROR-class logs per bucket.
//! - `profile_cost`   — per service (per-function series proved pathological
//!   in production: hundreds of noisy per-function series → ~880 anomaly
//!   rows and a ~10x slower pass, all profile noise; the per-function
//!   breakdown already lives in diagnose's `profiles` section): the
//!   service's TOTAL profiled cost — sum of sample values across all
//!   functions — per bucket. `operation` holds the service name.
//!
//! The pass is split in two so the expensive half never touches the single
//! DuckLake writer (which it would otherwise block for seconds, stalling
//! ingest flushes):
//! - [`detect`] — the multi-day series scans + BOCPD, run on a READER
//!   connection from the project's `ReaderPool`;
//! - [`persist`] — the brief DELETE+INSERT of the results, the only part
//!   that runs on the writer connection (DuckLake single-writer semantics).
//!
//! Threshold-free by construction: detection is Bayesian online changepoint
//! detection (`NormalGammaDetector`) on the z-scored series. The ONLY
//! structural choice is a direction — a shift is persisted when the series
//! mean AFTER the changepoint exceeds the mean BEFORE it ("worsening": more
//! errors, slower, louder, costlier) — a comparison, not a magic number.
//! Constant or too-short series produce no changepoints by construction, so
//! there is no minimum-points gate either. Nothing is filtered by size:
//! readers rank shifts by the UNIT-FREE symmetric relative change
//! (after − before) / (after + before) so shifts of different signals are
//! comparable (raw |after − before| would let a µs latency shift dwarf every
//! percentage shift), and every detected shift persists.

use anyhow::{Context, Result};
use augurs_changepoint::{Detector, NormalGammaDetector};
use duckdb::Connection;

use crate::diagnose::{
    is_error_expr, is_error_log_expr, operation_drill_filter, operation_key_expr,
};

/// How far back the pass looks when building each entity's series. Matches
/// the raw retention default (7 days) — a coverage window for the series,
/// not a detection threshold: detection itself has no tunables beyond the
/// BOCPD prior.
const SERIES_LOOKBACK_DAYS: i64 = 7;

/// The detection resolution: every series this pass detects on is bucketed
/// at this grain (the `INTERVAL '{DETECTION_BUCKET_MINUTES} minutes'` in the
/// three series scans below). 5-minute buckets are deliberate — BOCPD should
/// flag SUSTAINED regime shifts, not 1-minute blips — and this constant is
/// the single place that grain is defined.
pub(crate) const DETECTION_BUCKET_MINUTES: i64 = 5;
/// [`DETECTION_BUCKET_MINUTES`] in µs — the unit `ts_shift` is stored in.
pub(crate) const DETECTION_BUCKET_US: i64 = DETECTION_BUCKET_MINUTES * 60 * 1_000_000;

/// Cross-signal correlation: group shift timestamps into incidents.
///
/// Two shifts belong to the same incident when their `ts_shift` fall in the
/// SAME or ADJACENT detection bucket, transitively (a chain of shifts each
/// ≤ 1 bucket apart is one evolving incident). Returns groups of input
/// indices; every input lands in exactly one group — correlation never
/// drops a shift, it only decides which ones travel together.
///
/// Threshold-free by construction: `ts_shift` is only known to the pass's
/// own bucket resolution ([`DETECTION_BUCKET_US`]) — BOCPD places every
/// changepoint on a bucket boundary, so two series shifting at the same
/// wall-clock moment can legitimately land in adjacent buckets (the moment
/// falls differently within each series' buckets, and per-series noise can
/// lag the changepoint by one point). Same-or-adjacent bucket is therefore
/// the FINEST alignment the detector's own resolution supports — a window
/// derived from the system's structure, not a picked constant. Callers
/// correlate within one service (shifts of unrelated services are never
/// merged).
pub(crate) fn correlate_shifts(ts_shift_us: &[i64]) -> Vec<Vec<usize>> {
    let mut order: Vec<usize> = (0..ts_shift_us.len()).collect();
    order.sort_by_key(|&i| ts_shift_us[i]);
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut last_bucket: Option<i64> = None;
    for i in order {
        let bucket = ts_shift_us[i].div_euclid(DETECTION_BUCKET_US);
        match (last_bucket, groups.last_mut()) {
            (Some(prev), Some(group)) if bucket - prev <= 1 => group.push(i),
            _ => groups.push(vec![i]),
        }
        last_bucket = Some(bucket);
    }
    groups
}

// The signals this pass detects (the `signal` column of `sigiro_anomalies`).
#[derive(Debug, Clone, Copy)]
enum AnomalySignal {
    ErrorRate,
    LatencyP95,
    LogVolume,
    ErrorLogRate,
    ProfileCost,
}

impl AnomalySignal {
    const fn as_str(self) -> &'static str {
        match self {
            Self::ErrorRate => "error_rate",
            Self::LatencyP95 => "latency_p95",
            Self::LogVolume => "log_volume",
            Self::ErrorLogRate => "error_log_rate",
            Self::ProfileCost => "profile_cost",
        }
    }
}

/// One worsening regime shift found in a series.
#[derive(Debug)]
struct Shift {
    /// First bucket of the NEW regime, µs since epoch.
    ts_shift_us: i64,
    /// Series mean before the shift (raw signal units).
    before: f64,
    /// Series mean from the shift onward.
    after: f64,
}

/// A fully-resolved anomaly row: produced by [`detect`] on a reader
/// connection, carried across to the writer thread, and written by
/// [`persist`]. Opaque outside this module — callers only move it between
/// the two phases.
#[derive(Debug)]
pub struct AnomalyInsert {
    service: String,
    /// The entity the series was keyed by: operation name (span signals)
    /// or service name (log and profile signals).
    operation: String,
    signal: AnomalySignal,
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
/// augurs runs inside `catch_unwind`: a detector panic on one pathological
/// series must never abort the whole detection pass — it is logged and that
/// one series is skipped, the rest still detect.
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

/// The detection half of the scheduled pass — the expensive part. Builds
/// every entity's 5-minute series over the last [`SERIES_LOOKBACK_DAYS`]
/// (one scan per telemetry table: spans → error_rate + latency_p95, logs →
/// log_volume + error_log_rate, profiles → profile_cost) and runs BOCPD over
/// every series. Read-only: run it on a READER connection from the project's
/// `ReaderPool`, never on the writer — these scans take seconds and would
/// stall ingest flushes. Hand the returned rows to [`persist`] on the writer.
///
/// Fails loud: any signal's scan failing errors the WHOLE pass (the caller —
/// `spawn_anomaly_detection` — isolates it from ingest/flush).
pub fn detect(conn: &Connection, prefix: &str) -> Result<Vec<AnomalyInsert>> {
    let started = std::time::Instant::now();
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + i64::from(odt.microsecond())
    };
    let cutoff_us = now_us - SERIES_LOOKBACK_DAYS * 86_400 * 1_000_000;

    let mut inserts = Vec::new();
    detect_span_signals(conn, prefix, cutoff_us, &mut inserts)?;
    detect_log_signals(conn, prefix, cutoff_us, &mut inserts)?;
    detect_profile_cost(conn, prefix, cutoff_us, &mut inserts)?;

    tracing::info!(
        anomalies = inserts.len(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "anomaly detection completed"
    );
    Ok(inserts)
}

/// error_rate + latency_p95, per (service, operation), from ONE span scan.
///
/// 5-minute buckets (not 1min) are deliberate across all signals: BOCPD
/// should flag SUSTAINED regime shifts, not 1-minute blips — coarser buckets
/// denoise (fewer spurious shifts) and cut the scan cost ~5x. Buckets only
/// exist where telemetry exists — gaps are simply absent. Same derived
/// operation identity as /v1/diagnose (shared `operation_key_expr`), so
/// diagnose's LEFT JOIN on (service, operation) always matches.
fn detect_span_signals(
    conn: &Connection,
    prefix: &str,
    cutoff_us: i64,
    inserts: &mut Vec<AnomalyInsert>,
) -> Result<()> {
    let is_err = is_error_expr("s.");
    let op_key = operation_key_expr("s.");
    let series_sql = format!(
        "SELECT s.service_name,
                {op_key} AS operation,
                any_value(s.span_name) AS span_name,
                epoch_us(time_bucket(INTERVAL '{DETECTION_BUCKET_MINUTES} minutes', s.timestamp)) AS bucket_ts,
                count(CASE WHEN {is_err} THEN 1 END) * 100.0 / count(*) AS err_rate,
                approx_quantile(s.duration, 0.95)::DOUBLE AS p95_us
         FROM {prefix}sigiro_spans s
         WHERE s.timestamp >= make_timestamp({cutoff_us})
           AND s.span_name NOT LIKE '%TraceService/Export%'
         GROUP BY s.service_name, operation, bucket_ts
         ORDER BY s.service_name, operation, bucket_ts"
    );

    let mut stmt = conn
        .prepare(&series_sql)
        .context("failed to prepare anomaly span-series query")?;
    let rows: Vec<(String, String, String, i64, f64, f64)> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .context("anomaly span-series query failed")?
        .collect::<std::result::Result<_, _>>()
        .context("anomaly span-series row read failed")?;

    // Group the ordered rows into per-(service, operation) series and detect
    // on BOTH signals of the shared scan.
    let mut i = 0;
    while i < rows.len() {
        let (service, operation, span_name) = (&rows[i].0, &rows[i].1, &rows[i].2);
        let mut bucket_ts = Vec::new();
        let mut err_rate = Vec::new();
        let mut p95_us = Vec::new();
        while i < rows.len() && rows[i].0 == *service && rows[i].1 == *operation {
            bucket_ts.push(rows[i].3);
            err_rate.push(rows[i].4);
            p95_us.push(rows[i].5);
            i += 1;
        }
        for shift in worsening_shifts(&bucket_ts, &err_rate) {
            let drill_down_sql =
                span_drill_sql(prefix, service, span_name, operation, &shift, true);
            inserts.push(AnomalyInsert {
                service: service.clone(),
                operation: operation.clone(),
                signal: AnomalySignal::ErrorRate,
                shift,
                drill_down_sql,
            });
        }
        for shift in worsening_shifts(&bucket_ts, &p95_us) {
            let drill_down_sql =
                span_drill_sql(prefix, service, span_name, operation, &shift, false);
            inserts.push(AnomalyInsert {
                service: service.clone(),
                operation: operation.clone(),
                signal: AnomalySignal::LatencyP95,
                shift,
                drill_down_sql,
            });
        }
    }
    Ok(())
}

/// log_volume + error_log_rate, per service, from ONE log scan. Logs carry
/// no operation, and per-template series would emit N anomalies for one
/// flood (and multiply the scan's group count by template cardinality) —
/// per-service is the useful, cheap entity; the drill (and diagnose's
/// log_patterns section) breaks the flood down by template. The table's
/// `operation` column holds the service name (the entity).
fn detect_log_signals(
    conn: &Connection,
    prefix: &str,
    cutoff_us: i64,
    inserts: &mut Vec<AnomalyInsert>,
) -> Result<()> {
    let is_err = is_error_log_expr("l.");
    let series_sql = format!(
        "SELECT l.service_name,
                epoch_us(time_bucket(INTERVAL '{DETECTION_BUCKET_MINUTES} minutes', l.timestamp)) AS bucket_ts,
                count(*)::DOUBLE AS volume,
                count(CASE WHEN {is_err} THEN 1 END)
                    * 100.0 / count(*) AS err_log_rate
         FROM {prefix}sigiro_logs l
         WHERE l.timestamp >= make_timestamp({cutoff_us})
         GROUP BY l.service_name, bucket_ts
         ORDER BY l.service_name, bucket_ts"
    );

    let mut stmt = conn
        .prepare(&series_sql)
        .context("failed to prepare anomaly log-series query")?;
    let rows: Vec<(String, i64, f64, f64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .context("anomaly log-series query failed")?
        .collect::<std::result::Result<_, _>>()
        .context("anomaly log-series row read failed")?;

    let mut i = 0;
    while i < rows.len() {
        let service = &rows[i].0;
        let mut bucket_ts = Vec::new();
        let mut volume = Vec::new();
        let mut err_log_rate = Vec::new();
        while i < rows.len() && rows[i].0 == *service {
            bucket_ts.push(rows[i].1);
            volume.push(rows[i].2);
            err_log_rate.push(rows[i].3);
            i += 1;
        }
        for shift in worsening_shifts(&bucket_ts, &volume) {
            let drill_down_sql = log_drill_sql(prefix, service, &shift, false);
            inserts.push(AnomalyInsert {
                service: service.clone(),
                operation: service.clone(),
                signal: AnomalySignal::LogVolume,
                shift,
                drill_down_sql,
            });
        }
        for shift in worsening_shifts(&bucket_ts, &err_log_rate) {
            let drill_down_sql = log_drill_sql(prefix, service, &shift, true);
            inserts.push(AnomalyInsert {
                service: service.clone(),
                operation: service.clone(),
                signal: AnomalySignal::ErrorLogRate,
                shift,
                drill_down_sql,
            });
        }
    }
    Ok(())
}

/// profile_cost, per SERVICE: the service's total profiled cost — sum of
/// sample values across ALL functions — per bucket. A sustained rise means
/// "this service's profiled cost regressed"; the drill (and diagnose's
/// `profiles` section) breaks it down per function. Deliberately NOT
/// per-function: on real data hundreds of functions each carry a noisy cost
/// series, and per-function BOCPD produced ~880 anomaly rows (drowning
/// every other signal and the cross-signal correlation) at ~10x the scan
/// cost — one low-cardinality series per service is the meaningful entity,
/// same as the log signals. The table's `operation` column holds the
/// service name.
fn detect_profile_cost(
    conn: &Connection,
    prefix: &str,
    cutoff_us: i64,
    inserts: &mut Vec<AnomalyInsert>,
) -> Result<()> {
    let series_sql = format!(
        "SELECT p.service_name,
                epoch_us(time_bucket(INTERVAL '{DETECTION_BUCKET_MINUTES} minutes', p.timestamp)) AS bucket_ts,
                COALESCE(sum(p.value), 0)::DOUBLE AS cost
         FROM {prefix}sigiro_profiles p
         WHERE p.timestamp >= make_timestamp({cutoff_us})
         GROUP BY p.service_name, bucket_ts
         ORDER BY p.service_name, bucket_ts"
    );

    let mut stmt = conn
        .prepare(&series_sql)
        .context("failed to prepare anomaly profile-series query")?;
    let rows: Vec<(String, i64, f64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .context("anomaly profile-series query failed")?
        .collect::<std::result::Result<_, _>>()
        .context("anomaly profile-series row read failed")?;

    let mut i = 0;
    while i < rows.len() {
        let service = &rows[i].0;
        let mut bucket_ts = Vec::new();
        let mut cost = Vec::new();
        while i < rows.len() && rows[i].0 == *service {
            bucket_ts.push(rows[i].1);
            cost.push(rows[i].2);
            i += 1;
        }
        for shift in worsening_shifts(&bucket_ts, &cost) {
            let drill_down_sql = profile_drill_sql(prefix, service, &shift);
            inserts.push(AnomalyInsert {
                service: service.clone(),
                operation: service.clone(),
                signal: AnomalySignal::ProfileCost,
                shift,
                drill_down_sql,
            });
        }
    }
    Ok(())
}

/// The persistence half of the scheduled pass — the only part that touches
/// the single DuckLake writer, and it holds it just long enough for one
/// DELETE+INSERT transaction over [`detect`]'s results (no scan, no BOCPD).
/// Returns the number of anomaly rows persisted.
///
/// Idempotent by full replace: the pass recomputes from the same continuous
/// history every time, so DELETE-then-INSERT inside one transaction keeps
/// repeated passes from accumulating duplicates (and a mid-pass failure
/// rolls back, leaving the previous pass's rows intact).
pub fn persist(conn: &Connection, prefix: &str, inserts: &[AnomalyInsert]) -> Result<usize> {
    let started = std::time::Instant::now();
    let now_us = {
        let odt = time::OffsetDateTime::now_utc();
        odt.unix_timestamp() * 1_000_000 + i64::from(odt.microsecond())
    };

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
                 VALUES (make_timestamp(?), ?, ?, ?, 'changepoint',
                         'worsening', make_timestamp(?), ?, ?, ?)"
            ))
            .context("failed to prepare sigiro_anomalies insert")?;
        for row in inserts {
            ins.execute(duckdb::params![
                now_us,
                row.service,
                row.operation,
                row.signal.as_str(),
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
                "anomaly pass persisted"
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

/// Ready-to-run /v1/query SQL for the raw spans behind a span-signal
/// anomaly: this operation's spans from the shift onward (the new regime).
/// `errors_first` orders errors ahead of the rest (error_rate shifts);
/// latency shifts order purely slowest-first. Uses only allowlisted scalar
/// functions so it passes `validate_read_only` verbatim.
///
/// Six arguments against a ceiling of five (`clippy.toml`), and deliberately not
/// refactored. The grouping that would fix the count does not exist: `prefix` and
/// `service` are loop-invariant at both call sites while `shift` is the loop
/// variable, so a context struct holding the three would be rebuilt on every
/// iteration to save nothing. Bundling `span_name`+`operation` instead — the pair
/// `operation_drill_filter` already takes together — would trade one clear pair of
/// `&str`s for a type with a single call path. Both are abstractions with no
/// reader. `log_drill_sql` and `profile_drill_sql` below are the same family with
/// fewer identity arguments, which is why only this one crosses the line.
#[allow(clippy::too_many_arguments)]
fn span_drill_sql(
    prefix: &str,
    service: &str,
    span_name: &str,
    operation: &str,
    shift: &Shift,
    errors_first: bool,
) -> String {
    let svc = service.replace('\'', "''");
    let filter = operation_drill_filter(span_name, operation);
    let ts = shift.ts_shift_us;
    let order = if errors_first {
        format!(
            "CASE WHEN {} THEN 0 ELSE 1 END, duration DESC",
            is_error_expr("")
        )
    } else {
        "duration DESC".to_string()
    };
    format!(
        "SELECT * FROM {prefix}sigiro_spans \
         WHERE service_name = '{svc}' AND {filter} AND timestamp >= make_timestamp({ts}) \
         ORDER BY {order}"
    )
}

/// Ready-to-run /v1/query SQL for the logs behind a log-signal anomaly: the
/// service's logs from the shift onward, newest first. `errors_only`
/// restricts to ERROR-class severities (error_log_rate shifts).
fn log_drill_sql(prefix: &str, service: &str, shift: &Shift, errors_only: bool) -> String {
    let svc = service.replace('\'', "''");
    let ts = shift.ts_shift_us;
    let severity = if errors_only {
        format!(" AND {}", is_error_log_expr(""))
    } else {
        String::new()
    };
    format!(
        "SELECT * FROM {prefix}sigiro_logs \
         WHERE service_name = '{svc}'{severity} AND timestamp >= make_timestamp({ts}) \
         ORDER BY timestamp DESC"
    )
}

/// Ready-to-run /v1/query SQL for the profile samples behind a profile_cost
/// anomaly: the service's samples (all functions — the series is the
/// service's total cost) from the shift onward, costliest first, so the
/// regressing functions lead. Uses only allowlisted scalar functions so it
/// passes `validate_read_only` verbatim.
fn profile_drill_sql(prefix: &str, service: &str, shift: &Shift) -> String {
    let svc = service.replace('\'', "''");
    let ts = shift.ts_shift_us;
    format!(
        "SELECT * FROM {prefix}sigiro_profiles \
         WHERE service_name = '{svc}' AND timestamp >= make_timestamp({ts}) \
         ORDER BY value DESC"
    )
}
