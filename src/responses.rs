//! Typed response structs for API endpoints.
//!
//! Replaces `serde_json::Value` return types with compile-time verified structs,
//! enabling accurate OpenAPI schema generation via utoipa.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// GET /v1/services
#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServicesResponse {
    pub services: Vec<ServiceEntry>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEntry {
    pub name: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
    pub span_count: i64,
}

/// POST /admin/keys
#[derive(Serialize, Deserialize, ToSchema)]
pub struct CreateKeyResponse {
    pub key: String,
}

/// DELETE /admin/keys/{short_token}
#[derive(Serialize, ToSchema)]
pub struct RevokeKeyResponse {
    pub revoked: bool,
    pub short_token: String,
}

/// GET /admin/status
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AdminStatusResponse {
    #[schema(value_type = HashMap<String, String>)]
    pub status: HashMap<String, String>,
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

/// POST /v1/federation/workers
#[derive(Serialize, ToSchema)]
pub struct RegisterWorkerResponse {
    pub registered: bool,
    pub id: String,
}

/// DELETE /v1/federation/workers/{id}
#[derive(Serialize, ToSchema)]
pub struct DeregisterWorkerResponse {
    pub deregistered: bool,
}

/// GET /v1/federation/workers
#[derive(Serialize, ToSchema)]
pub struct ListWorkersResponse {
    pub workers: Vec<crate::federation::WorkerInfo>,
}

// ---------------------------------------------------------------------------
// GET /v1/traces/{trace_id} — trace detail and tree responses
// ---------------------------------------------------------------------------

/// Complete trace detail response with critical path and service breakdown.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct TraceDetailResponse {
    pub trace_id: String,
    pub total_duration_us: i64,
    pub root_span: SpanDetail,
    pub span_count: i64,
    pub error_count: i64,
    pub services: Vec<String>,
    pub spans: Vec<SpanDetail>,
    pub critical_path: Vec<String>,
    pub service_breakdown: Vec<ServiceBreakdownEntry>,
}

/// Individual span detail in trace detail response.
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct SpanDetail {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub span_name: String,
    pub duration_us: i64,
    pub status_code: i32,
    pub service_name: String,
    pub timestamp: String,
    pub span_kind: Option<i32>,
    pub span_attributes: Option<String>,
    pub resource_attributes: Option<String>,
    pub events_json: Option<String>,
    pub links_json: Option<String>,
    pub scope_name: Option<String>,
    pub scope_version: Option<String>,
    pub trace_state: Option<String>,
}

/// Service duration breakdown with percentages.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct ServiceBreakdownEntry {
    pub service: String,
    pub duration_us: i64,
    pub percentage: f64,
}
