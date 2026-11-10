//! Typed response structs for API endpoints.
//!
//! Replaces `serde_json::Value` return types with compile-time verified structs,
//! enabling accurate OpenAPI schema generation via utoipa.

use serde::{Deserialize, Serialize};
use sigiro_segments::{SpanId, TraceId};
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
    /// Webhook outbox for this project: queued and dead notifications.
    pub(crate) delivery: crate::alerts::DeliverySummary,
}

/// POST /v1/alerts/replay
#[derive(Serialize, Deserialize, ToSchema)]
pub struct AlertsReplayResponse {
    /// Dead notifications put back in the queue.
    pub(crate) replayed: u64,
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
    /// Init complete, no dead flushers. Background state that needs an
    /// operator but does not make the server unready is reported alongside.
    Ready {
        /// Alert webhook outbox across every project; absent when delivery is off.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alert_webhooks: Option<crate::alerts::DeliverySummary>,
        /// Present while the disk-capacity gate rejects ingest: its state
        /// (`tripped`, `healing`, or `stuck` with a `reason`) and the numbers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disk_gate: Option<crate::disk_gate::DiskGateHealth>,
        /// Buffered rows dropped after a failed append and its retry, since
        /// start (`sigiro_rows_dropped_total`); absent while zero.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rows_dropped_total: Option<u64>,
    },
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

#[cfg(test)]
mod health_response_tests {
    use super::HealthResponse;

    #[test]
    fn ready_stays_a_bare_status_until_there_is_something_to_report() {
        let bare = HealthResponse::Ready {
            alert_webhooks: None,
            disk_gate: None,
            rows_dropped_total: None,
        };
        assert_eq!(
            serde_json::to_string(&bare).unwrap(),
            r#"{"status":"ready"}"#
        );
        assert_eq!(
            serde_json::from_str::<HealthResponse>(r#"{"status":"ready"}"#).unwrap(),
            bare
        );
        let dead = HealthResponse::Ready {
            alert_webhooks: Some(crate::alerts::DeliverySummary {
                queued: 0,
                dead: 2,
                oldest_queued_at: None,
                last_dead_error: Some("http_401".into()),
            }),
            disk_gate: None,
            rows_dropped_total: None,
        };
        let json = serde_json::to_string(&dead).unwrap();
        assert_eq!(
            json,
            r#"{"status":"ready","alert_webhooks":{"queued":0,"dead":2,"last_dead_error":"http_401"}}"#
        );
        assert_eq!(serde_json::from_str::<HealthResponse>(&json).unwrap(), dead);
    }
}
