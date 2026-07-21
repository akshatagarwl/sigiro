//! Pure functions for investigation analysis — no DB access, no I/O.
//! These are testable in isolation and have no side effects.

use anyhow::Result;
use augurs_changepoint::{Detector, NormalGammaDetector};
use serde::{Deserialize, Serialize};

use crate::types::TimestampMicros;

// ---------------------------------------------------------------------------
// Anomaly detection — augurs BOCPD changepoints on per-bucket health series
// ---------------------------------------------------------------------------

/// One bucket of an operation's health series over the full
/// [baseline_from .. to_ts] range — the input to changepoint detection.
/// Produced by the `op_series` CTE.
#[derive(Deserialize, Debug)]
pub(crate) struct SeriesBucket {
    /// Bucket start, µs since epoch.
    pub(crate) bucket_ts: i64,
    /// Error percentage within the bucket (0..100).
    pub(crate) err_rate: f64,
    /// p95 span duration within the bucket, µs.
    pub(crate) p95_us: f64,
}

/// Minimum series length to attempt changepoint detection — BOCPD on fewer
/// points has no run to establish a regime against, so we abstain
/// (is_anomalous = None) instead of guessing.
const MIN_SERIES_POINTS: usize = 8;

/// The verdict for one operation: tri-state flag + prose naming the shift.
pub(crate) struct AnomalyVerdict {
    pub(crate) is_anomalous: Option<bool>,
    pub(crate) reason: Option<String>,
}

fn mean(y: &[f64]) -> f64 {
    if y.is_empty() {
        return f64::NAN;
    }
    y.iter().sum::<f64>() / y.len() as f64
}

/// z-score the series. REQUIRED before handing it to `NormalGammaDetector`:
/// its default NormalGamma prior assumes standardized data and misbehaves on
/// raw scales (µs values in the hundreds of thousands). A constant series
/// (σ = 0) maps to all-zeros — no changepoints, by construction.
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

/// The LAST regime shift that starts inside the investigation window
/// (shifts in the baseline lead-in are old news), with the series mean
/// before and after it. None when the series has no in-window shift.
///
/// Index convention (verified empirically against augurs 0.10.2): the
/// detector reports the LAST index of the OLD regime — a step at index 30
/// yields cp = 29 — so the new regime starts at `cp + 1`, and that is the
/// bucket checked against the window and named in the reason.
fn last_in_window_shift(
    buckets: &[SeriesBucket],
    y: &[f64],
    window_from_us: i64,
) -> Option<(i64, f64, f64)> {
    let start = changepoints(y)
        .into_iter()
        .filter_map(|cp| {
            let s = cp + 1;
            (s < buckets.len() && buckets[s].bucket_ts >= window_from_us).then_some(s)
        })
        .next_back()?;
    Some((
        buckets[start].bucket_ts,
        mean(&y[..start]),
        mean(&y[start..]),
    ))
}

/// µs since epoch → "HH:MM" UTC, for anomaly_reason prose.
fn hhmm_utc(us: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(us) * 1000)
        .map(|dt| format!("{:02}:{:02}", dt.hour(), dt.minute()))
        .unwrap_or_else(|_| "?".to_string())
}

/// µs duration → prose ("2.1s" / "82.3ms") for anomaly_reason.
fn fmt_duration_us(v: f64) -> String {
    if v >= 1_000_000.0 {
        format!("{:.1}s", v / 1_000_000.0)
    } else {
        format!("{:.1}ms", v / 1_000.0)
    }
}

/// Judge one operation from its bucketed health series (error rate + p95),
/// entirely baseline-derived: augurs BOCPD finds regime changes; an
/// in-window changepoint marking a WORSENING shift (mean after > mean
/// before) flags the operation. Improvements do not flag. A chronic-but-
/// stable failure rate has no changepoint and is therefore NOT anomalous —
/// it IS the baseline; chronic health lives in error_pct.
pub(crate) fn detect_operation_anomaly(
    buckets: &[SeriesBucket],
    window_from_us: i64,
) -> AnomalyVerdict {
    if buckets.len() < MIN_SERIES_POINTS {
        // Too few points to establish a regime — abstain, don't guess.
        return AnomalyVerdict {
            is_anomalous: None,
            reason: None,
        };
    }
    let err_series: Vec<f64> = buckets.iter().map(|b| b.err_rate).collect();
    let p95_series: Vec<f64> = buckets.iter().map(|b| b.p95_us).collect();

    let mut reasons = Vec::new();
    let mut worsened = false;

    if let Some((ts, before, after)) = last_in_window_shift(buckets, &err_series, window_from_us)
        && after > before
    {
        worsened = true;
        reasons.push(format!(
            "error rate shifted at {} ({before:.1}% → {after:.1}%)",
            hhmm_utc(ts)
        ));
    }
    if let Some((ts, before, after)) = last_in_window_shift(buckets, &p95_series, window_from_us)
        && after > before
    {
        worsened = true;
        reasons.push(format!(
            "p95 shifted at {} ({} → {})",
            hhmm_utc(ts),
            fmt_duration_us(before),
            fmt_duration_us(after)
        ));
    }
    AnomalyVerdict {
        is_anomalous: Some(worsened),
        reason: (!reasons.is_empty()).then(|| reasons.join("; ")),
    }
}

// ---------------------------------------------------------------------------
// Coverage — signal availability report
// ---------------------------------------------------------------------------

/// Signal availability for a given service/time range.
#[derive(Serialize, Deserialize, Debug, PartialEq, utoipa::ToSchema)]
pub struct SignalCoverage {
    /// Signal type (spans, logs, metrics, profiles).
    pub(crate) signal_type: String,
    /// Whether this signal type has data in the window.
    pub available: bool,
    /// Severity: "high", "medium", "low", "none"
    pub severity: String,
    /// Human-readable reason if not available, or summary if available.
    pub reason: String,
}

/// Computed coverage for the investigation window.
#[derive(Serialize, Deserialize, Debug, PartialEq, utoipa::ToSchema)]
pub struct Coverage {
    pub has_spans: bool,
    pub has_logs: bool,
    pub has_metrics: bool,
    pub has_profiles: bool,
    pub span_count: i64,
    pub log_template_count: i64,
    pub metric_series_count: i64,
    pub(crate) profile_sample_count: i64,
    /// Signal coverage details instead of flat gap strings.
    pub signals: Vec<SignalCoverage>,
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

    let signals = vec![
        SignalCoverage {
            signal_type: "spans".to_string(),
            available: has_spans,
            severity: if has_spans {
                "high".to_string()
            } else {
                "none".to_string()
            },
            reason: if has_spans {
                format!("{} spans found", spans_total)
            } else {
                // F-08: the old message blamed the user when the real cause
                // was a sigiro crash on ingest. Distinguish the cases so the
                // message helps diagnose, not scold.
                if has_logs || has_metrics {
                    "no spans but logs/metrics present — check if OTLP trace endpoint is reachable and SDK is configured to export traces (not just logs/metrics)".to_string()
                } else {
                    "no spans, logs, or metrics found for this service in the time window — check service_name spelling, the time window (--from/--to), and that the service is actually sending OTLP".to_string()
                }
            },
        },
        SignalCoverage {
            signal_type: "logs".to_string(),
            available: has_logs,
            severity: if has_logs {
                "high".to_string()
            } else {
                "none".to_string()
            },
            reason: if has_logs {
                format!("{} log templates found", logs_total)
            } else {
                "no logs found — consider enabling log collection".to_string()
            },
        },
        SignalCoverage {
            signal_type: "metrics".to_string(),
            available: has_metrics,
            severity: if has_metrics {
                "medium".to_string()
            } else {
                "low".to_string()
            },
            reason: if has_metrics {
                format!("{} metric series found", metrics_count)
            } else {
                "no metrics found — no gauge/sum/histogram data in this window".to_string()
            },
        },
        SignalCoverage {
            signal_type: "profiles".to_string(),
            available: has_profiles,
            severity: if has_profiles {
                "medium".to_string()
            } else {
                "low".to_string()
            },
            reason: if has_profiles {
                format!("{} profile samples found", profile_samples)
            } else {
                "no profiles found — no CPU/memory profiling data in this window".to_string()
            },
        },
        SignalCoverage {
            signal_type: "correlated_signals".to_string(),
            available: has_spans && has_logs,
            severity: if has_spans && has_logs {
                "high".to_string()
            } else if has_spans || has_logs {
                "medium".to_string()
            } else {
                "none".to_string()
            },
            reason: if has_spans && has_logs {
                "spans and logs are both available for correlation".to_string()
            } else if has_spans {
                "spans exist but no logs — rely on span data for diagnosis".to_string()
            } else {
                "neither spans nor logs available".to_string()
            },
        },
    ];

    Coverage {
        has_spans,
        has_logs,
        has_metrics,
        has_profiles,
        span_count: spans_total,
        log_template_count: logs_total,
        metric_series_count: metrics_count as i64,
        profile_sample_count: profile_samples,
        signals,
    }
}

// ---------------------------------------------------------------------------
// Metric bucket grain — the one query knob that scales with window size
// ---------------------------------------------------------------------------

/// The metric bucket grain, scaled to the window so a 10-minute look isn't one
/// bar and a 7-day look isn't 10k rows. This is the only genuinely
/// window-dependent knob: on real data, span/log payloads stay tiny (≤72KB)
/// regardless of width, so those use flat caps in the query, not tiers — and
/// wide windows return exemplar spans like any other (the old `span_limit: 0`
/// blanked exactly the surveys that most need them).
pub fn metric_bucket_interval(from_ts: i64, to_ts: i64) -> &'static str {
    let window_us = to_ts - from_ts;
    let fifteen_min = 15 * 60 * 1_000_000_i64;
    let two_hours = 2 * 3600 * 1_000_000_i64;

    if window_us <= fifteen_min {
        "1 minute"
    } else if window_us <= two_hours {
        "5 minutes"
    } else {
        "1 hour"
    }
}

// ---------------------------------------------------------------------------
// Timestamp formatting
// ---------------------------------------------------------------------------

pub fn us_to_iso(us: i64) -> Result<String> {
    let ns = us
        .checked_mul(1000)
        .ok_or_else(|| anyhow::anyhow!("timestamp overflow: {us}"))?;
    let dt = time::OffsetDateTime::from_unix_timestamp_nanos(ns.into())
        .map_err(|_| anyhow::anyhow!("invalid timestamp_micros: {us}"))?;
    dt.format(&time::format_description::well_known::Rfc3339)
        .map(|s| s.to_string())
        .map_err(|_| anyhow::anyhow!("format failed for timestamp_micros: {us}"))
}

/// Service dependency graph node with aggregated stats.
#[derive(Serialize, Deserialize, Debug, Clone, utoipa::ToSchema)]
pub struct ServiceDependency {
    /// Name of the dependent service.
    pub service_name: String,
    /// Number of spans from this service in shared traces.
    pub(crate) span_count: i64,
    /// Number of error spans in shared traces.
    pub(crate) error_count: i64,
    /// Error rate as a percentage.
    pub(crate) error_rate: f64,
}

// ---------------------------------------------------------------------------
// Window — time range for investigation
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct Window {
    pub from_ts: i64,
    pub to_ts: i64,
    pub from_iso: String,
    pub to_iso: String,
}

impl Window {
    pub(crate) fn new(from: TimestampMicros, to: TimestampMicros) -> Result<Self> {
        Ok(Self {
            from_ts: from.as_micros(),
            to_ts: to.as_micros(),
            from_iso: us_to_iso(from.as_micros())?,
            to_iso: us_to_iso(to.as_micros())?,
        })
    }
}
