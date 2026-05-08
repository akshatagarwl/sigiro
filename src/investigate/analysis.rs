//! Pure functions for investigation analysis — no DB access, no I/O.
//! These are testable in isolation and have no side effects.

use anyhow::Result;
use serde::{Deserialize, Serialize};


use crate::types::TimestampMicros;

// ---------------------------------------------------------------------------
// Coverage — signal availability report
// ---------------------------------------------------------------------------

/// Signal availability for a given service/time range.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct SignalCoverage {
    /// Signal type (spans, logs, metrics, profiles).
    pub signal_type: String,
    /// Whether this signal type has data in the window.
    pub available: bool,
    /// Severity: "high", "medium", "low", "none"
    pub severity: String,
    /// Human-readable reason if not available, or summary if available.
    pub reason: String,
}

/// Computed coverage for the investigation window.
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
    /// Signal coverage details instead of flat gap strings.
    pub signals: Vec<SignalCoverage>,
}

impl Coverage {
    /// Returns true if any high-severity signals are missing.
    pub fn has_critical_gaps(&self) -> bool {
        self.signals
            .iter()
            .any(|s| s.available && s.severity == "high")
    }
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
            severity: if has_spans { "high".to_string() } else { "none".to_string() },
            reason: if has_spans {
                format!("{} spans found", spans_total)
            } else {
                "no spans found — service may not be instrumented for tracing".to_string()
            },
        },
        SignalCoverage {
            signal_type: "logs".to_string(),
            available: has_logs,
            severity: if has_logs { "high".to_string() } else { "none".to_string() },
            reason: if has_logs {
                format!("{} log templates found", logs_total)
            } else {
                "no logs found — consider enabling log collection".to_string()
            },
        },
        SignalCoverage {
            signal_type: "metrics".to_string(),
            available: has_metrics,
            severity: if has_metrics { "medium".to_string() } else { "low".to_string() },
            reason: if has_metrics {
                format!("{} metric series found", metrics_count)
            } else {
                "no metrics found — no gauge/sum/histogram data in this window".to_string()
            },
        },
        SignalCoverage {
            signal_type: "profiles".to_string(),
            available: has_profiles,
            severity: if has_profiles { "medium".to_string() } else { "low".to_string() },
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
// AdaptiveConfig — vary detail level based on window size
// ---------------------------------------------------------------------------

/// | Window    | Metric buckets | Span exemplars | Log limit |
/// |-----------|----------------|----------------|-----------|
/// | < 15m     | 1 minute       | 50             | 50        |
/// | 15m - 2h  | 5 minutes      | 20             | 40        |
/// | > 2h      | 1 hour         | 0 (summaries)  | 30        |
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
            log_limit: 50,
        }
    } else if window_us <= two_hours {
        AdaptiveConfig {
            bucket_interval: "5 minutes",
            span_limit: 20,
            log_limit: 40,
        }
    } else {
        AdaptiveConfig {
            bucket_interval: "1 hour",
            span_limit: 0,
            log_limit: 30,
        }
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

// ---------------------------------------------------------------------------
// Span types — full span data with all attributes
// ---------------------------------------------------------------------------

/// OpenTelemetry span kind enum values.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpanKind {
    #[serde(rename = "server")]
    Server = 1,
    Client = 2,
    Producer = 3,
    Consumer = 4,
    Internal = 5,
}

impl SpanKind {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            1 => Some(Self::Server),
            2 => Some(Self::Client),
            3 => Some(Self::Producer),
            4 => Some(Self::Consumer),
            5 => Some(Self::Internal),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> i32 {
        *self as i32
    }
}

impl std::fmt::Display for SpanKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpanKind::Server => write!(f, "server"),
            SpanKind::Client => write!(f, "client"),
            SpanKind::Producer => write!(f, "producer"),
            SpanKind::Consumer => write!(f, "consumer"),
            SpanKind::Internal => write!(f, "internal"),
        }
    }
}

/// Full span with all attributes and event data.
#[derive(Serialize, Deserialize, Clone)]
pub struct Span {
    /// 32-byte trace ID as lowercase hex.
    pub trace_id: String,
    /// 16-byte span ID as lowercase hex.
    pub span_id: String,
    /// 16-byte parent span ID as lowercase hex (empty if root span).
    pub parent_span_id: String,
    /// Span operation name.
    pub span_name: String,
    /// Span duration in nanoseconds.
    /// NOTE: The value is actually in microseconds. The name `duration_ns` is historical.
    pub duration_ns: i64,
    /// OpenTelemetry status code.
    pub status_code: crate::types::StatusCode,
    /// Service name.
    pub service_name: String,
    /// Span kind (server, client, producer, consumer, internal).
    pub span_kind: Option<SpanKind>,
    /// Span attributes as a JSON string.
    pub span_attributes: Option<String>,
    /// Resource attributes as a JSON string.
    pub resource_attributes: Option<String>,
    /// Span start timestamp in microseconds.
    pub start_timestamp: i64,
    /// Span end timestamp in microseconds.
    pub end_timestamp: i64,
    /// Events as a JSON array string.
    pub events_json: Option<String>,
    /// Links as a JSON array string.
    pub links_json: Option<String>,
    /// Instrumentation library name.
    pub instrumentation_library_name: Option<String>,
    /// Instrumentation library version.
    pub instrumentation_library_version: Option<String>,
    /// Trace state header values.
    pub trace_state: Option<String>,
    /// Scope span flag (1 if from scope span).
    pub scope_span: Option<i32>,
}

impl Span {
    /// Returns true if this is a root span (no parent).
    pub fn is_root(&self) -> bool {
        self.parent_span_id.is_empty()
    }

    /// Returns true if the span has an error status.
    pub fn is_error(&self) -> bool {
        self.status_code.is_error()
    }

    /// Returns the duration in milliseconds as a float.
    pub fn duration_ms(&self) -> f64 {
        self.duration_ns as f64 / 1_000_000.0
    }
}

/// A complete trace with root span and all child spans.
#[derive(Serialize, Deserialize, Clone)]
pub struct Trace {
    /// The trace ID.
    pub trace_id: String,
    /// The root span of this trace.
    pub root_span: Span,
    /// All spans in this trace (including root).
    pub spans: Vec<Span>,
    /// Logs linked to this trace (chronological, limited).
    pub linked_logs: Vec<LogEntry>,
    /// Number of metrics associated with this trace's time range.
    pub metric_count: i64,
    /// Total duration of the trace in nanoseconds (end of last span - start of first).
    /// NOTE: The value is actually in microseconds. The name `total_duration_ns` is historical.
    pub total_duration_ns: i64,
    /// Number of spans with error status in this trace.
    pub error_count: i64,
    /// Whether this trace contains errors.
    pub has_errors: bool,
    /// Total number of spans in this trace.
    pub span_count: i32,
}

impl Trace {
    /// Creates a new trace from the root span and all child spans.
    pub fn new(trace_id: String, root_span: Span, spans: Vec<Span>) -> Self {
        let error_count = spans.iter().filter(|s| s.is_error()).count() as i64;
        let total_duration_ns = if let (Some(first), Some(last)) = (
            spans.iter().map(|s| s.start_timestamp).min(),
            spans.iter().map(|s| s.end_timestamp).max(),
        ) {
            last - first
        } else {
            root_span.duration_ns
        };

        let span_count = (spans.len() + 1) as i32;
        Trace {
            trace_id,
            root_span,
            spans,
            linked_logs: Vec::new(),
            metric_count: 0,
            total_duration_ns,
            error_count,
            has_errors: error_count > 0,
            span_count,
        }
    }

    /// Returns the trace ID.
    pub fn trace_id_ref(&self) -> &str {
        &self.trace_id
    }
}

/// Service dependency graph node with aggregated stats.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServiceDependency {
    /// Name of the dependent service.
    pub service_name: String,
    /// Number of spans from this service in shared traces.
    pub span_count: i64,
    /// Number of error spans in shared traces.
    pub error_count: i64,
    /// Error rate as a percentage.
    pub error_rate: f64,
}

// ---------------------------------------------------------------------------
// Log types — structured log entries
// ---------------------------------------------------------------------------

/// A single log entry (chronological).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogEntry {
    /// Unix timestamp in microseconds.
    pub timestamp: i64,
    /// ISO-formatted timestamp string.
    pub timestamp_iso: String,
    /// Log body/content.
    pub body: String,
    /// Severity text (DEBUG, INFO, WARN, ERROR, etc.).
    pub severity_text: String,
    /// Trace ID if this log is linked to a trace.
    pub trace_id: Option<String>,
    /// Span ID if this log is linked to a span.
    pub span_id: Option<String>,
    /// Log attributes as a JSON string.
    pub attributes_json: Option<String>,
    /// Instrumentation library name.
    pub instrumentation_library_name: Option<String>,
    /// Instrumentation library version.
    pub instrumentation_library_version: Option<String>,
    /// Flags field.
    pub flags: Option<i32>,
    /// Resource attributes.
    pub resource_attributes_json: Option<String>,
}

impl LogEntry {
    /// Returns true if this log has an associated trace.
    pub fn has_trace(&self) -> bool {
        self.trace_id.is_some()
    }

    /// Returns true if this is an error-level log.
    pub fn is_error(&self) -> bool {
        self.severity_text.eq_ignore_ascii_case("error")
            || self.severity_text.eq_ignore_ascii_case("err")
            || self.severity_text.eq_ignore_ascii_case("fatal")
            || self.severity_text.eq_ignore_ascii_case("critical")
    }

    /// Returns true if this is a warning or higher.
    pub fn is_warning(&self) -> bool {
        self.severity_text.eq_ignore_ascii_case("warn")
            || self.severity_text.eq_ignore_ascii_case("warning")
            || self.is_error()
    }
}

/// A log pattern with aggregated statistics.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogPattern {
    /// Template or pattern string.
    pub pattern: String,
    /// Example log body.
    pub example_body: Option<String>,
    /// Severity text.
    pub severity_text: String,
    /// Total count of logs matching this pattern.
    pub count: i64,
    /// First seen timestamp (ISO string).
    pub first_seen: Option<String>,
    /// Last seen timestamp (ISO string).
    pub last_seen: Option<String>,
    /// Number of error logs matching this pattern.
    pub error_count: i64,
}

// ---------------------------------------------------------------------------
// Timeline types — unified event timeline
// ---------------------------------------------------------------------------

/// A single entry in the unified timeline.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TimelineEntry {
    /// A span start/end event.
    Span {
        timestamp: i64,
        trace_id: String,
        span_id: String,
        span_name: String,
        service_name: String,
        span_kind: Option<String>,
        duration_ns: i64,
        is_error: bool,
    },
    /// A derived event (deploy, K8s event, etc.).
    Event {
        timestamp: i64,
        timestamp_iso: String,
        event_type: String,
        description: String,
    },
    /// A log entry.
    Log {
        timestamp: i64,
        severity_text: String,
        body: String,
        trace_id: Option<String>,
        span_id: Option<String>,
        is_error: bool,
    },
    /// A version change (deploy).
    Version {
        timestamp: i64,
        version: String,
        service_name: String,
    },
}

impl TimelineEntry {
    /// Returns the timestamp for sorting.
    pub fn timestamp(&self) -> i64 {
        match self {
            TimelineEntry::Span { timestamp, .. } => *timestamp,
            TimelineEntry::Event { timestamp, .. } => *timestamp,
            TimelineEntry::Log { timestamp, .. } => *timestamp,
            TimelineEntry::Version { timestamp, .. } => *timestamp,
        }
    }

    /// Returns the entry type name for display.
    pub fn type_name(&self) -> &'static str {
        match self {
            TimelineEntry::Span { .. } => "span",
            TimelineEntry::Event { .. } => "event",
            TimelineEntry::Log { .. } => "log",
            TimelineEntry::Version { .. } => "version",
        }
    }
}

/// Merges spans, events, and logs into a unified timeline sorted by timestamp.
pub fn build_timeline(
    spans: &[Span],
    derived_events: &[(String, String, String)], // (detected_at, event_type, description)
    logs: &[LogEntry],
) -> Vec<TimelineEntry> {
    let mut entries = Vec::new();

    // Add span entries (start of each span).
    for span in spans {
        entries.push(TimelineEntry::Span {
            timestamp: span.start_timestamp,
            trace_id: span.trace_id.clone(),
            span_id: span.span_id.clone(),
            span_name: span.span_name.clone(),
            service_name: span.service_name.clone(),
            span_kind: span.span_kind.map(|k| k.to_string()),
            duration_ns: span.duration_ns,
            is_error: span.is_error(),
        });
    }

    // Add derived event entries.
    for (detected_at, event_type, description) in derived_events {
        if let Ok(ts) = detected_at.parse::<i64>() {
            entries.push(TimelineEntry::Event {
                timestamp: ts,
                timestamp_iso: detected_at.clone(),
                event_type: event_type.clone(),
                description: description.clone(),
            });
        }
    }

    // Add log entries.
    for log in logs {
        entries.push(TimelineEntry::Log {
            timestamp: log.timestamp,
            severity_text: log.severity_text.clone(),
            body: log.body.clone(),
            trace_id: log.trace_id.clone(),
            span_id: log.span_id.clone(),
            is_error: log.is_error(),
        });
    }

    // Sort by timestamp.
    entries.sort_by_key(|e| e.timestamp());
    entries
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
