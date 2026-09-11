//! Shared types that span multiple modules, avoiding circular dependencies.

use crate::otlp::MetricBatches;
use anyhow::{Context, Result};
use arrow::record_batch::RecordBatch;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Format a Unix timestamp in microseconds as RFC 3339.
pub(crate) fn timestamp_micros_to_rfc3339(timestamp_us: i64) -> Result<String> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp_us) * 1_000)
        .context("timestamp is out of range")?
        .format(&Rfc3339)
        .context("failed to format timestamp")
}

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
// TenantId — prevents invalid identifiers reaching storage and SQL boundaries
// ---------------------------------------------------------------------------

/// Canonical tenant identifier used for filesystem, schema, and control-plane keys.
///
/// The character and length rules are part of the type so deserialized API input
/// cannot bypass the storage identifier invariant.
#[nutype::nutype(
    validate(predicate = |id: &str| {
        !id.is_empty()
            && id.len() <= 56
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            && !matches!(id, "extensions" | "tenants")
    }),
    derive(Debug, Clone, PartialEq, Eq, Hash, Into, AsRef, Display, Serialize, Deserialize)
)]
pub struct TenantId(String);

// ---------------------------------------------------------------------------
// TimestampMicros — prevents sec/µs confusion at API boundaries
// ---------------------------------------------------------------------------

/// Plausible contemporary Unix timestamp in microseconds.
///
/// The 16-digit range deliberately rejects values expressed in seconds,
/// milliseconds, or nanoseconds. It is the same API invariant used by
/// diagnosis input validation: approximately 2001 through 2286.
#[nutype::nutype(
    validate(
        greater_or_equal = 1_000_000_000_000_000,
        less = 10_000_000_000_000_000
    ),
    derive(
        Debug,
        Clone,
        Copy,
        PartialEq,
        Eq,
        PartialOrd,
        Ord,
        Hash,
        Into,
        Serialize,
        Deserialize
    )
)]
pub struct TimestampMicros(i64);

// ---------------------------------------------------------------------------
// ApiKeyHashPrefix — validated identity for key revocation
// ---------------------------------------------------------------------------

/// Unambiguous hexadecimal prefix of a SHA-256 API-key hash.
///
/// Eight characters is the control-plane minimum for avoiding accidental broad
/// matches; 64 is the full hash width. Checked deserialization keeps malformed
/// path values out of the revocation operation.
#[nutype::nutype(
    validate(predicate = |prefix: &str| {
        (8..=64).contains(&prefix.len()) && prefix.bytes().all(|b| b.is_ascii_hexdigit())
    }),
    derive(Debug, Clone, PartialEq, Eq, Hash, Into, AsRef, Display, Serialize, Deserialize)
)]
pub struct ApiKeyHashPrefix(String);

#[nutype::nutype(
    validate(predicate = |id: &str| {
        id.len() == 32
            && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            && id.bytes().any(|b| b != b'0')
    }),
    derive(Debug, Clone, PartialEq, Eq, Hash, Into, AsRef, Display, Serialize, Deserialize)
)]
pub struct TraceId(String);

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
    pub(crate) short_token: ApiKeyHashPrefix,
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
    pub(crate) trace_id: TraceId,
}
