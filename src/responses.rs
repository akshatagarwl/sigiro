//! Typed response structs for API endpoints.
//!
//! Replaces `serde_json::Value` return types with compile-time verified structs,
//! enabling accurate OpenAPI schema generation via utoipa.

use serde::{Deserialize, Serialize};
use sigiro_segments::{SpanId, TraceId};
use std::collections::HashMap;
use utoipa::ToSchema;

/// Time in responses (#67): every point in time is a pair, `<name>_ts` in
/// microseconds since the Unix epoch (the unit `/v1/query` returns and
/// `TIMESTAMP` columns store) and `<name>_iso` in RFC 3339, UTC, with six
/// fractional digits and `Z`.
pub(crate) fn iso_from_micros(micros: i64) -> String {
    let Ok(at) = time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(micros) * 1_000) else {
        return String::new();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}Z",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        at.microsecond()
    )
}

/// GET /v1/services — complete result only; exceeding SIGIRO_MAX_RESULT_ROWS
/// returns an error rather than a partial list.
#[derive(Serialize, Deserialize, ToSchema)]
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
pub struct ServiceEntry {
    pub name: String,
    /// Earliest telemetry timestamp, microseconds since the Unix epoch.
    pub first_seen_ts: Option<i64>,
    /// `first_seen_ts` in RFC 3339 (UTC).
    pub first_seen_iso: Option<String>,
    /// Latest telemetry timestamp, microseconds since the Unix epoch.
    pub last_seen_ts: Option<i64>,
    /// `last_seen_ts` in RFC 3339 (UTC).
    pub last_seen_iso: Option<String>,
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
    /// Init complete, no dead writers. Background state that needs an
    /// operator but does not make the server unready is reported alongside.
    Ready {
        /// Alert webhook outbox across every project; absent when delivery is off.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alert_webhooks: Option<crate::alerts::DeliverySummary>,
        /// Present while the disk-capacity gate rejects ingest: its state
        /// (`tripped`, `healing`, or `stuck` with a `reason`) and the numbers.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        disk_gate: Option<crate::disk_gate::DiskGateHealth>,
        /// DuckLake maintenance per project: the last `CHECKPOINT`, failures
        /// since the last success, and current data files and inlined rows.
        /// A failing checkpoint does not stop ingest; absent while no project
        /// has a writer yet.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        storage: std::collections::BTreeMap<String, crate::storage_health::ProjectStorage>,
    },
    /// HTTP listener is up but init is still in progress (cold start). Probe
    /// should retry; no operator action required.
    Starting,
    /// Init complete but a writer thread has panicked. Its project's ingest
    /// fails (with a retryable error) until the next request replaces it.
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
    /// Span start, microseconds since the Unix epoch.
    pub(crate) start_ts: i64,
    /// `start_ts` in RFC 3339 (UTC).
    pub(crate) start_iso: String,
    /// Span end, microseconds since the Unix epoch.
    pub(crate) end_ts: i64,
    /// `end_ts` in RFC 3339 (UTC).
    pub(crate) end_iso: String,
    pub(crate) span_kind: Option<i32>,
    /// The span's attributes as a JSON object, values of 1 KiB or more included.
    #[schema(value_type = Option<Object>)]
    pub(crate) span_attributes: Option<serde_json::Value>,
    #[schema(value_type = Option<Object>)]
    pub(crate) resource_attributes: Option<serde_json::Value>,
    #[schema(value_type = Option<Vec<Object>>)]
    pub(crate) events_json: Option<serde_json::Value>,
    #[schema(value_type = Option<Vec<Object>>)]
    pub(crate) links_json: Option<serde_json::Value>,
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
            storage: Default::default(),
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
                oldest_queued_ts: None,
                oldest_queued_iso: None,
                last_dead_error: Some("http_401".into()),
            }),
            disk_gate: None,
            storage: Default::default(),
        };
        let json = serde_json::to_string(&dead).unwrap();
        assert_eq!(
            json,
            r#"{"status":"ready","alert_webhooks":{"queued":0,"dead":2,"last_dead_error":"http_401"}}"#
        );
        assert_eq!(serde_json::from_str::<HealthResponse>(&json).unwrap(), dead);
    }
}
