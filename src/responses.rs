//! Typed response structs for API endpoints.
//!
//! Replaces `serde_json::Value` return types with compile-time verified structs,
//! enabling accurate OpenAPI schema generation via utoipa.

use crate::types::{SpanId, TraceId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// GET /v1/services — complete result only; exceeding SIGIRO_MAX_RESULT_ROWS
/// returns an error rather than a partial list.
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServicesResponse {
    pub services: Vec<ServiceEntry>,
}

/// GET /v1/alerts
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AlertsResponse {
    pub(crate) alerts: Vec<crate::alerts::AlertInstance>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEntry {
    pub name: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub signals: ServiceSignals,
    pub total_count: i64,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct ServiceSignals {
    pub spans: i64,
    pub logs: i64,
    pub metrics: i64,
    pub profiles: i64,
}

/// GET /admin/status
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AdminStatusResponse {
    #[schema(value_type = HashMap<String, String>)]
    pub(crate) status: HashMap<String, String>,
}

/// GET /health — server liveness/readiness with the *reason* if not ready.
///
/// Returned for both 200 (Ready) and 503 (Starting/Degraded) so callers can
/// distinguish the two not-ready states without parsing the body shape.
#[derive(Serialize, Deserialize, ToSchema, PartialEq, Eq, Debug)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HealthResponse {
    /// Init complete, no dead flushers.
    Ready,
    /// HTTP listener is up but init is still in progress (cold start). Probe
    /// should retry; no operator action required.
    Starting,
    /// Init complete but a flusher task has panicked. Operator intervention
    /// required; ingest is silently dropping data.
    Degraded { reason: String },
}

// ---------------------------------------------------------------------------
// GET /v1/traces/{trace_id} — trace detail and tree responses
// ---------------------------------------------------------------------------

/// Complete trace detail response with critical path and service breakdown.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct TraceDetailResponse {
    #[schema(value_type = String)]
    pub(crate) trace_id: TraceId,
    /// Elapsed wall-clock time from the earliest span start to the latest span end, in microseconds.
    pub(crate) total_duration_us: i64,
    pub(crate) root_span: SpanDetail,
    pub(crate) span_count: i64,
    pub(crate) error_count: i64,
    pub(crate) services: Vec<String>,
    pub(crate) spans: Vec<SpanDetail>,
    #[schema(value_type = Vec<String>)]
    pub(crate) critical_path: Vec<SpanId>,
    pub(crate) service_breakdown: Vec<ServiceBreakdownEntry>,
}

/// Individual span detail in trace detail response.
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct SpanDetail {
    #[schema(value_type = String)]
    pub(crate) trace_id: TraceId,
    #[schema(value_type = String)]
    pub(crate) span_id: SpanId,
    #[schema(value_type = Option<String>)]
    pub(crate) parent_span_id: Option<SpanId>,
    pub(crate) span_name: String,
    pub(crate) duration_us: i64,
    pub(crate) status_code: i32,
    pub(crate) service_name: String,
    pub(crate) timestamp: String,
    pub(crate) span_kind: Option<i32>,
    pub(crate) span_attributes: Option<String>,
    pub(crate) resource_attributes: Option<String>,
    pub(crate) events_json: Option<String>,
    pub(crate) links_json: Option<String>,
    pub(crate) scope_name: Option<String>,
    pub(crate) scope_version: Option<String>,
    pub(crate) trace_state: Option<String>,
}

/// Service duration breakdown with percentages.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct ServiceBreakdownEntry {
    pub(crate) service: String,
    /// Cumulative inclusive duration of the service's spans, in microseconds.
    pub(crate) duration_us: i64,
    /// Percentage of cumulative inclusive span duration across all services.
    pub(crate) percentage: f64,
}
