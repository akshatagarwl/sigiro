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
    pub(crate) services: Vec<ServiceEntry>,
}

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceEntry {
    pub(crate) name: String,
    pub(crate) first_seen: Option<String>,
    pub(crate) last_seen: Option<String>,
    pub(crate) span_count: i64,
}

/// GET /v1/anomalies — precomputed regime shifts of every signal from the
/// continuous anomaly pass (the `sigiro_anomalies` table), biggest shift
/// first by the unit-free magnitude (after − before) / (after + before)
/// (comparable across signals; raw |after − before| breaks zero-baseline
/// ties, then newest first). Ranking, not filtering: the reader judges
/// significance from `before`/`after`. `truncated` reports the query row cap.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AnomaliesResponse {
    pub(crate) anomalies: Vec<AnomalyEntry>,
    /// True when the query row cap omitted shifts. This is an incomplete
    /// candidate snapshot: automatic submission must wait for a complete one.
    pub(crate) truncated: bool,
}

/// One row of the `sigiro_anomalies` table: a worsening regime shift found
/// by BOCPD changepoint detection over an entity's continuous 5-minute
/// series. No thresholds are involved anywhere — `direction` is always
/// "worsening" because only shifts whose mean after exceeds the mean
/// before are persisted.
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AnomalyEntry {
    /// When the scheduled pass wrote this row (ISO timestamp).
    pub(crate) detected_at: String,
    pub(crate) service_name: String,
    /// The entity the shifted series is keyed by: the derived operation
    /// identity (error_rate / latency_p95 — matches
    /// `operation_summaries[].operation` in /v1/diagnose) or the service
    /// name (log_volume / error_log_rate / profile_cost).
    pub(crate) operation: String,
    /// Which health series shifted: "error_rate", "latency_p95",
    /// "log_volume", "error_log_rate", or "profile_cost".
    pub(crate) signal: String,
    /// Detection kind, e.g. "changepoint" (BOCPD).
    pub(crate) kind: String,
    /// Always "worsening" — improvements are never persisted.
    pub(crate) direction: String,
    /// First bucket of the NEW regime (ISO timestamp).
    pub(crate) ts_shift: String,
    /// `ts_shift` as µs since the Unix epoch — the value incident
    /// correlation buckets on.
    pub(crate) ts_shift_us: i64,
    /// Cross-signal correlation id. Rows sharing an id shifted in the same
    /// or adjacent 5-minute detection bucket on the same service — one
    /// incident, not N independent anomalies (the window is the detection
    /// pass's own bucket resolution, not a threshold). `null` when the row
    /// correlates with no other returned row. Computed over the rows this
    /// request returns, so `service`/window filters scope the grouping.
    #[serde(default)]
    pub(crate) incident_id: Option<String>,
    /// Series mean before the shift, raw signal units (error %, p95 µs,
    /// logs per bucket, summed profile cost per bucket).
    pub(crate) before: f64,
    /// Series mean from the shift onward.
    pub(crate) after: f64,
    /// Ready-to-run SQL for POST /v1/query: the raw telemetry behind the
    /// shift (spans, logs, or profile samples, per the signal).
    pub(crate) drill_down_sql: Option<String>,
}

/// POST /admin/keys
#[derive(Serialize, Deserialize, ToSchema)]
pub struct CreateKeyResponse {
    pub(crate) key: String,
    pub(crate) key_hash: String,
    pub(crate) tenant_id: String,
}

/// DELETE /admin/keys/{short_token}
#[derive(Serialize, ToSchema)]
pub struct RevokeKeyResponse {
    pub(crate) revoked: bool,
    pub(crate) short_token: String,
}

/// POST /admin/tenants
#[derive(Serialize, Deserialize, ToSchema)]
pub struct CreateTenantResponse {
    pub(crate) id: String,
    pub(crate) status: String,
}

#[derive(Deserialize, ToSchema, validator::Validate)]
pub struct CreateTenantRequest {
    #[validate(custom(function = "validate_tenant_id"))]
    pub(crate) id: String,
}

#[derive(Deserialize, ToSchema, validator::Validate)]
pub struct CreateKeyRequest {
    #[validate(custom(function = "validate_tenant_id"))]
    pub(crate) tenant_id: String,
}

/// Validate a tenant id at the API boundary (fail loud early), reusing the
/// canonical rules from `storage::sanitize_tenant_id` so the API and the
/// storage layer never disagree on what a valid tenant id is.
fn validate_tenant_id(id: &str) -> Result<(), validator::ValidationError> {
    crate::storage::sanitize_tenant_id(id)
        .map(|_| ())
        .map_err(|e| {
            let mut err = validator::ValidationError::new("tenant_id");
            err.message = Some(e.to_string().into());
            err
        })
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
    pub(crate) trace_id: String,
    pub(crate) total_duration_us: i64,
    pub(crate) root_span: SpanDetail,
    pub(crate) span_count: i64,
    pub(crate) error_count: i64,
    pub(crate) services: Vec<String>,
    pub(crate) spans: Vec<SpanDetail>,
    pub(crate) critical_path: Vec<String>,
    pub(crate) service_breakdown: Vec<ServiceBreakdownEntry>,
}

/// Individual span detail in trace detail response.
#[derive(Serialize, Deserialize, ToSchema, Clone)]
pub struct SpanDetail {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    pub(crate) parent_span_id: Option<String>,
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
    pub(crate) duration_us: i64,
    pub(crate) percentage: f64,
}
