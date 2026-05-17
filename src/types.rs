//! Shared types that span multiple modules, avoiding circular dependencies.

use crate::otlp::MetricBatches;
use arrow::record_batch::RecordBatch;

// --------------------------------------------------------------------------
// SupervisorHandle — process supervisor lifetime guard
// --------------------------------------------------------------------------

/// Handle wrapping a supervisor and optional cleanup path.
/// The cleanup runs on drop to remove temp config files.
#[derive(Debug)]
pub struct SupervisorHandle {
    #[allow(dead_code)]
    supervisor: crate::supervisor::Supervisor, // intentionally held to keep the job alive
    cleanup_path: Option<String>,
}

impl SupervisorHandle {
    pub fn new(supervisor: crate::supervisor::Supervisor, cleanup_path: Option<String>) -> Self {
        Self {
            supervisor,
            cleanup_path,
        }
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        if let Some(path) = &self.cleanup_path {
            let _ = std::fs::remove_file(path).ok();
            tracing::debug!("cleaned up config file: {}", path);
        }
    }
}

// --------------------------------------------------------------------------
// Supervisor registry — shared across federation.rs and ebpf.rs
// --------------------------------------------------------------------------

use parking_lot::Mutex;
use std::collections::HashMap;

/// Registry mapping supervisor names to their handles.
/// Passed as a constructor parameter to avoid global mutable state.
#[derive(Debug)]
pub struct SupervisorRegistry {
    pub(crate) inner: Mutex<HashMap<String, SupervisorHandle>>,
}

impl SupervisorRegistry {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    /// Register a supervisor with a name and optional cleanup path.
    pub fn register(
        &self,
        name: &str,
        supervisor: crate::supervisor::Supervisor,
        cleanup_path: Option<String>,
    ) -> anyhow::Result<()> {
        let mut guard = self.inner.lock();
        let handle = SupervisorHandle::new(supervisor, cleanup_path);
        guard.insert(name.to_string(), handle);
        tracing::info!("registered supervisor: {name}");
        Ok(())
    }
}

impl Default for SupervisorRegistry {
    fn default() -> Self {
        Self::new()
    }
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
    pub fn project_id(&self) -> &str {
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
    pub fn table_name(self, prefix: &str) -> String {
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
    /// Parse from the i32 value stored in DuckDB.
    /// Unknown values are mapped to Unset (the OTel default).
    pub fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Unset,
            1 => Self::Ok,
            2 => Self::Error,
            _ => Self::Unset,
        }
    }

    /// Convert back to i32 for SQL queries.
    pub fn as_i32(self) -> i32 {
        self as i32
    }

    /// True if this status represents an error.
    pub fn is_error(self) -> bool {
        self == Self::Error
    }
}

// ---------------------------------------------------------------------------
// TimestampMicros — prevents sec/µs confusion at API boundaries
// ---------------------------------------------------------------------------

/// Microsecond-precision timestamp. All internal timestamps are in microseconds.
/// CLI accepts seconds and converts via `from_seconds()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimestampMicros(pub i64);

impl TimestampMicros {
    /// From epoch seconds (CLI input boundary).
    pub fn from_seconds(sec: i64) -> Self {
        Self(sec * 1_000_000)
    }

    /// From epoch microseconds (internal storage).
    pub fn from_micros(us: i64) -> Self {
        Self(us)
    }

    /// Current time in microseconds.
    pub fn now() -> Self {
        let odt = time::OffsetDateTime::now_utc();
        Self(odt.unix_timestamp() * 1_000_000 + odt.microsecond() as i64)
    }

    /// Raw microseconds value (for SQL parameters).
    pub fn as_micros(self) -> i64 {
        self.0
    }

    /// Duration between two timestamps in microseconds.
    pub fn duration_since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
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

/// Route: GET /investigate
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/investigate")]
pub struct InvestigatePath;

/// Route: GET /services
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/services")]
pub struct ServicesPath;

/// Route: GET /federation/workers
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/federation/workers")]
pub struct FederationWorkersPath;

/// Route: GET /federation/workers/{id}
#[derive(Debug, Clone, Deserialize, TypedPath)]
#[typed_path("/federation/workers/{id}")]
pub struct FederationWorkerPath {
    pub id: String,
}

/// Route: GET /keys
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/keys")]
pub struct AdminKeysPath;

/// Route: GET /keys/{short_token}
#[derive(Debug, Clone, Deserialize, TypedPath)]
#[typed_path("/keys/{short_token}")]
pub struct AdminKeyPath {
    pub short_token: String,
}

/// Route: GET /status
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/status")]
pub struct AdminStatusPath;

/// Route: GET /health
#[derive(Debug, Clone, TypedPath)]
#[typed_path("/health")]
pub struct HealthPath;

/// Route: GET /traces/{trace_id}
#[derive(Debug, Clone, Deserialize, TypedPath)]
#[typed_path("/traces/{trace_id}")]
pub struct TraceDetailPath {
    pub trace_id: String,
}
