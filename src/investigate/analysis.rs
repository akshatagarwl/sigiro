//! Pure functions for investigation analysis — no DB access, no I/O.
//! These are testable in isolation and have no side effects.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::types::TimestampMicros;

// ---------------------------------------------------------------------------
// Coverage — signal availability report
// ---------------------------------------------------------------------------

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
    if !has_profiles {
        gaps.push("no profiles found — no CPU/memory profiling data in this window".to_string());
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

// ---------------------------------------------------------------------------
// AdaptiveConfig — vary detail level based on window size
// ---------------------------------------------------------------------------

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
            span_limit: 0,
            log_limit: 20,
        }
    }
}

// ---------------------------------------------------------------------------
// Timestamp formatting
// ---------------------------------------------------------------------------

pub fn us_to_iso(us: i64) -> Result<String> {
    chrono::DateTime::from_timestamp_micros(us)
        .map(|dt| dt.to_rfc3339())
        .ok_or_else(|| anyhow::anyhow!("invalid timestamp_micros: {us}"))
}

// ---------------------------------------------------------------------------
// Span tree builder — parent → children mapping
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone)]
pub struct SpanRow {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub span_name: String,
    pub duration_ns: i64,
    pub status_code: crate::types::StatusCode,
    pub service_name: String,
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

// ---------------------------------------------------------------------------
// Window — time range for investigation
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct Window {
    pub from_ts: i64,
    pub to_ts: i64,
    pub from_iso: String,
    pub to_iso: String,
}

impl Window {
    pub fn new(from: TimestampMicros, to: TimestampMicros) -> Result<Self> {
        Ok(Self {
            from_ts: from.as_micros(),
            to_ts: to.as_micros(),
            from_iso: us_to_iso(from.as_micros())?,
            to_iso: us_to_iso(to.as_micros())?,
        })
    }
}
