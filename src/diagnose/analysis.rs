//! Pure functions for diagnosis analysis — no DB access, no I/O.
//! These are testable in isolation and have no side effects.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::types::{TimestampMicros, timestamp_micros_to_rfc3339};

// NOTE: changepoint detection (augurs BOCPD) no longer lives here — it moved
// to the scheduled anomaly pass in `crate::anomaly`, which persists results
// into the `sigiro_anomalies` table that diagnose now merely reads
// (docs/plans/01-continuous-anomaly-subsystem.md).

/// µs since epoch → "HH:MM" UTC, for anomaly_reason prose.
pub(crate) fn hhmm_utc(us: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(us) * 1000)
        .map(|dt| format!("{:02}:{:02}", dt.hour(), dt.minute()))
        .unwrap_or_else(|_| "?".to_string())
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

/// Computed coverage for the diagnosis window.
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
    timestamp_micros_to_rfc3339(us)
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
// Window — time range for diagnosis
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
        let from_ts = i64::from(from);
        let to_ts = i64::from(to);
        Ok(Self {
            from_ts,
            to_ts,
            from_iso: us_to_iso(from_ts)?,
            to_iso: us_to_iso(to_ts)?,
        })
    }
}
