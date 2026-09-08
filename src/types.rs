//! Shared types that span multiple modules, avoiding circular dependencies.

use crate::otlp::MetricBatches;
use arrow::record_batch::RecordBatch;

// --------------------------------------------------------------------------
// IngestMsg — carried across modules for routing
// --------------------------------------------------------------------------

/// Pre-transformed ingest message carrying Arrow RecordBatches, not raw bytes.
/// Transform happens on the tokio blocking pool (parallel) BEFORE the channel,
/// so the inserter threads only do DuckDB writes — no CPU-bound protobuf decoding.
#[derive(Debug)]
pub enum IngestMsg {
    Traces {
        project_id: String,
        batch: RecordBatch,
    },
    Logs {
        project_id: String,
        batch: RecordBatch,
    },
    Metrics {
        project_id: String,
        batches: MetricBatches,
    },
    Profiles {
        project_id: String,
        batch: RecordBatch,
    },
}

impl IngestMsg {
    pub(crate) fn project_id(&self) -> &str {
        match self {
            IngestMsg::Traces { project_id, .. }
            | IngestMsg::Logs { project_id, .. }
            | IngestMsg::Metrics { project_id, .. }
            | IngestMsg::Profiles { project_id, .. } => project_id,
        }
    }
}

// ---------------------------------------------------------------------------
// SignalTable — which DuckDB table a message targets
// ---------------------------------------------------------------------------

/// Signal types that map to DuckDB tables. Adding a new signal type
/// (e.g. Profiles) requires only adding a variant here and handling it
/// in the IngestMsg match — the flush path iterates the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SignalTable {
    Spans,
    Logs,
    MetricsGauge,
    MetricsSum,
    MetricsHistogram,
    MetricsExpHistogram,
    Profiles,
}

impl SignalTable {
    /// All signal table variants.
    pub const ALL: &'static [SignalTable] = &[
        SignalTable::Spans,
        SignalTable::Logs,
        SignalTable::MetricsGauge,
        SignalTable::MetricsSum,
        SignalTable::MetricsHistogram,
        SignalTable::MetricsExpHistogram,
        SignalTable::Profiles,
    ];

    /// DuckDB table name without prefix (e.g. "sigiro_spans").
    pub fn table_suffix(self) -> &'static str {
        match self {
            Self::Spans => "sigiro_spans",
            Self::Logs => "sigiro_logs",
            Self::MetricsGauge => "sigiro_metrics_gauge",
            Self::MetricsSum => "sigiro_metrics_sum",
            Self::MetricsHistogram => "sigiro_metrics_histogram",
            Self::MetricsExpHistogram => "sigiro_metrics_exp_histogram",
            Self::Profiles => "sigiro_profiles",
        }
    }

    /// DuckDB table name (with lake. prefix).
    pub(crate) fn table_name(self, prefix: &str) -> String {
        format!("{}{}", prefix, self.table_suffix())
    }
}

// ---------------------------------------------------------------------------
// StatusCode — OTel span status (replaces bare i32 magic numbers)
// ---------------------------------------------------------------------------

/// OTel span status code. Values match the OpenTelemetry specification:
///   Unset = 0, Ok = 1, Error = 2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(i32)]
pub enum StatusCode {
    Unset = 0,
    Ok = 1,
    Error = 2,
}

impl StatusCode {
    /// Convert back to i32 for SQL queries.
    pub(crate) fn as_i32(self) -> i32 {
        self as i32
    }
}

// ---------------------------------------------------------------------------
// TimestampMicros — prevents sec/µs confusion at API boundaries
// ---------------------------------------------------------------------------

/// Microsecond-precision timestamp. All internal timestamps are in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimestampMicros(pub i64);

impl TimestampMicros {
    /// From epoch microseconds (internal storage).
    pub(crate) fn from_micros(us: i64) -> Self {
        Self(us)
    }

    /// Raw microseconds value (for SQL parameters).
    pub(crate) fn as_micros(self) -> i64 {
        self.0
    }
}

impl From<i64> for TimestampMicros {
    fn from(us: i64) -> Self {
        Self(us)
    }
}

// ---------------------------------------------------------------------------
// Typed path structs — compile-time route checking for axum router
// ---------------------------------------------------------------------------
// Used with axum_extra::routing::TypedPath for compile-time route verification.

use axum_extra::routing::TypedPath;
use serde::{Deserialize, Serialize};

/// Route: GET /query
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/query")]
pub struct QueryPath;

/// Route: GET /diagnose
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/diagnose")]
pub struct DiagnosePath;

/// Route: GET /services
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/services")]
pub struct ServicesPath;

/// Route: GET /anomalies
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/anomalies")]
pub struct AnomaliesPath;

/// Route: GET /alerts
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/alerts")]
pub struct AlertsPath;

/// Route: GET /keys
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/keys")]
pub struct AdminKeysPath;

/// Route: GET /tenants
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/tenants")]
pub struct AdminTenantsPath;

/// Route: GET /keys/{short_token}
#[derive(Debug, Clone, Deserialize, TypedPath)]
#[typed_path("/keys/{short_token}")]
pub struct AdminKeyPath {
    pub(crate) short_token: String,
}

/// Route: GET /status
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/status")]
pub struct AdminStatusPath;

/// Route: GET /health
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/health")]
pub struct HealthPath;

/// Route: GET /v1/health
/// Alias of /health, kept for compatibility with the
/// "all OTLP HTTP endpoints live under /v1/..." convention.
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/v1/health")]
pub struct V1HealthPath;

/// Route: GET /healthz
/// Alias of /health, for Kubernetes liveness/readiness probes.
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/healthz")]
pub struct HealthzPath;

/// Route: GET /traces/{trace_id}
#[derive(Debug, Clone, Deserialize, TypedPath)]
#[typed_path("/traces/{trace_id}")]
pub struct TraceDetailPath {
    pub(crate) trace_id: String,
}
