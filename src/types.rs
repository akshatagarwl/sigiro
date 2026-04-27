//! Shared types that span multiple modules, avoiding circular dependencies.

use arrow::record_batch::RecordBatch;
use otlp2records::MetricBatches;

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
        if let Some(ref path) = self.cleanup_path {
            let _ = std::fs::remove_file(path).ok();
            tracing::debug!("cleaned up config file: {}", path);
        }
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
// Typed path structs — compile-time route checking for axum router
// ---------------------------------------------------------------------------
// Moved from server.rs to centralize route types that span modules.
// Used with axum_extra::routing::TypedPath for compile-time route verification.

use axum_extra::routing::TypedPath;
use serde::Deserialize;

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
