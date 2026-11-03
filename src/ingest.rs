use anyhow::Result;
use arrow::record_batch::RecordBatch;
use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    http::{StatusCode, header::CONTENT_TYPE},
    response::{IntoResponse, Response as AxumResponse},
    routing::post,
};
use headers::HeaderMapExt;
use multer::Multipart;
use opentelemetry_proto::tonic::collector::{
    logs::v1::{
        ExportLogsPartialSuccess, ExportLogsServiceRequest, ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    },
    metrics::v1::{
        ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        metrics_service_server::{MetricsService, MetricsServiceServer},
    },
    profiles::v1development::{
        ExportProfilesPartialSuccess, ExportProfilesServiceRequest, ExportProfilesServiceResponse,
        profiles_service_server::{ProfilesService, ProfilesServiceServer},
    },
    trace::v1::{
        ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
        trace_service_server::{TraceService, TraceServiceServer},
    },
};
use prost::Message;
use serde::Deserialize;
use sigiro_segments::{InputFormat, MetricBatches};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tracing_attributes::instrument;

use crate::inserter::{MsgRouter, RouterError};
use crate::types::IngestMsg;

/// Convenience helper for `Status::internal` to avoid repeating `format!` at call sites.
fn status_internal(msg: impl std::fmt::Display) -> Status {
    Status::internal(msg.to_string())
}

pub struct OtlpGrpcService {
    router: Arc<MsgRouter>,
    auth_mode: crate::auth::AuthMode,
    /// `SIGIRO_MAX_INGEST_BODY_BYTES` — resolved once at construction (see
    /// `crate::config::max_ingest_body_bytes`) and applied to every gRPC
    /// service via `max_decoding_message_size` below, matching the cap the
    /// OTLP HTTP router applies via `DefaultBodyLimit` (`otlp_http_router`).
    max_body_bytes: usize,
}

impl OtlpGrpcService {
    pub(crate) fn new(
        router: Arc<MsgRouter>,
        auth_mode: crate::auth::AuthMode,
        max_body_bytes: usize,
    ) -> Self {
        Self {
            router,
            auth_mode,
            max_body_bytes,
        }
    }

    /// Route a batch to the inserter.
    #[instrument(skip_all, fields(project_id))]
    async fn route_batch(&self, msg: IngestMsg) -> Result<(), Status> {
        let project_id = msg.project_id().to_string();
        if let Err(e) = self.router.send(msg).await {
            tracing::error!(project_id, "ingest router error: {e:?}");
            return Err(match e {
                RouterError::DiskCapacityExceeded => {
                    Status::resource_exhausted("disk capacity exceeded")
                }
                RouterError::MaintenanceStalled { .. } | RouterError::Admission(_) => {
                    Status::resource_exhausted(e.to_string())
                }
                _ => Status::internal("ingest failed"),
            });
        }
        Ok(())
    }

    fn clone_inner(&self) -> Self {
        Self {
            router: Arc::clone(&self.router),
            auth_mode: self.auth_mode.clone(),
            max_body_bytes: self.max_body_bytes,
        }
    }

    pub(crate) fn trace_service(&self) -> TraceServiceServer<Self> {
        TraceServiceServer::new(self.clone_inner()).max_decoding_message_size(self.max_body_bytes)
    }

    pub(crate) fn logs_service(&self) -> LogsServiceServer<Self> {
        LogsServiceServer::new(self.clone_inner()).max_decoding_message_size(self.max_body_bytes)
    }

    pub(crate) fn metrics_service(&self) -> MetricsServiceServer<Self> {
        MetricsServiceServer::new(self.clone_inner()).max_decoding_message_size(self.max_body_bytes)
    }

    pub(crate) fn profiles_service(&self) -> ProfilesServiceServer<Self> {
        ProfilesServiceServer::new(self.clone_inner())
            .max_decoding_message_size(self.max_body_bytes)
    }
}

/// Extract project_id from gRPC metadata.
/// Uses the explicit AuthMode — Open mode skips auth, JWT mode validates Bearer or AgentAuth.
pub async fn extract_project_id_grpc(
    auth_mode: &crate::auth::AuthMode,
    metadata: &tonic::metadata::MetadataMap,
) -> Result<String, Status> {
    match auth_mode {
        crate::auth::AuthMode::Open => Ok("default".to_string()),
        crate::auth::AuthMode::Jwt { .. } => {
            let authorization = metadata
                .get("authorization")
                .ok_or_else(|| Status::unauthenticated("Missing Authorization metadata"))?
                .to_str()
                .map_err(|_| Status::unauthenticated("Invalid Authorization encoding"))?;
            let credential =
                crate::auth::parse_api_credential(authorization).map_err(|error| match error {
                    crate::auth::CredentialParseError::MissingScheme
                    | crate::auth::CredentialParseError::UnsupportedScheme => {
                        Status::unauthenticated("Expected Bearer or AgentAuth credential")
                    }
                    crate::auth::CredentialParseError::InvalidToken => {
                        Status::unauthenticated("Invalid Authorization metadata")
                    }
                })?;

            auth_mode
                .authenticate(
                    &credential.token,
                    crate::auth::TELEMETRY_INGEST_SCOPE,
                    credential.scheme,
                )
                .await
                .map(|context| context.tenant_id.as_ref().to_owned())
                .map_err(|error| {
                    let failure = IngestAuthFailure::from(error);
                    Status::new(failure.code, failure.message)
                })
        }
    }
}

/// Detect InputFormat from Content-Type header.
/// OTLP/HTTP spec: application/x-protobuf (default) or application/json.
fn detect_format(headers: &axum::http::HeaderMap) -> InputFormat {
    headers
        .typed_get::<headers::ContentType>()
        .map(|ct| {
            let mime: mime::Mime = ct.into();
            if mime.type_() == mime::APPLICATION && mime.subtype() == mime::JSON {
                InputFormat::Json
            } else {
                InputFormat::Protobuf
            }
        })
        .unwrap_or_else(|| {
            tracing::debug!("no Content-Type header, assuming OTLP protobuf (OTLP/HTTP default)");
            InputFormat::Protobuf
        })
}

/// Run a CPU-bound transform on the Tokio blocking pool.
/// Payload errors retain their status; only blocking-task failure is internal.
async fn spawn_transform<T>(
    workloads: crate::workload::WorkloadBudget,
    name: &'static str,
    f: impl FnOnce() -> Result<T, Status> + Send + 'static,
) -> Result<T, Status>
where
    T: Send + 'static,
{
    let permit = workloads
        .try_acquire(crate::workload::WorkloadClass::Transform)
        .map_err(|error| Status::resource_exhausted(error.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f()
    })
    .await
    .map_err(|e| status_internal(format!("{name} task panicked: {e}")))?
}

/// Transform raw bytes into Arrow RecordBatch on the tokio blocking pool.
/// Supports both protobuf and JSON input based on Content-Type header.
#[instrument(skip_all, fields(endpoint = "/v1/traces"))]
async fn transform_traces(
    workloads: crate::workload::WorkloadBudget,
    bytes: Bytes,
    fmt: InputFormat,
) -> Result<RecordBatch, Status> {
    spawn_transform(workloads, "transform traces", move || {
        sigiro_segments::transform_traces(&bytes, fmt)
            .map_err(|error| Status::invalid_argument(error.to_string()))
    })
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/logs"))]
async fn transform_logs(
    workloads: crate::workload::WorkloadBudget,
    bytes: Bytes,
    fmt: InputFormat,
) -> Result<(RecordBatch, u64), Status> {
    spawn_transform(workloads, "transform logs", move || {
        sigiro_segments::transform_logs(&bytes, fmt)
            .map_err(|error| Status::invalid_argument(error.to_string()))
    })
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/metrics"))]
async fn transform_metrics(
    workloads: crate::workload::WorkloadBudget,
    bytes: Bytes,
    fmt: InputFormat,
) -> Result<MetricBatches, Status> {
    spawn_transform(workloads, "transform metrics", move || {
        sigiro_segments::transform_metrics(&bytes, fmt)
            .map_err(|error| Status::invalid_argument(error.to_string()))
    })
    .await
}

#[tonic::async_trait]
impl TraceService for OtlpGrpcService {
    #[instrument(skip_all, fields(project_id))]
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata()).await?;
        // Transform on tokio blocking pool — no double decode.
        let bytes = Bytes::from(request.into_inner().encode_to_vec());
        let batch = transform_traces(self.router.workloads(), bytes, InputFormat::Protobuf).await?;
        if batch.num_rows() == 0 {
            tracing::warn!(project_id, "export traces: empty batch after transform");
            return Ok(Response::new(ExportTraceServiceResponse {
                partial_success: Some(ExportTracePartialSuccess {
                    rejected_spans: 1,
                    error_message: "empty batch after transform".to_string(),
                }),
            }));
        }
        self.route_batch(IngestMsg::Traces { project_id, batch })
            .await?;
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl LogsService for OtlpGrpcService {
    #[instrument(skip_all, fields(project_id))]
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata()).await?;
        let bytes = Bytes::from(request.into_inner().encode_to_vec());
        let (batch, rejected) =
            transform_logs(self.router.workloads(), bytes, InputFormat::Protobuf).await?;
        if batch.num_rows() == 0 {
            tracing::warn!(
                project_id,
                rejected,
                "export logs: empty batch after transform"
            );
            return Ok(Response::new(ExportLogsServiceResponse {
                partial_success: Some(ExportLogsPartialSuccess {
                    rejected_log_records: rejected.max(1) as i64,
                    error_message: "no usable log records after transform".to_string(),
                }),
            }));
        }
        if rejected > 0 {
            tracing::warn!(
                project_id,
                rejected,
                "export logs: skipped unusable records"
            );
        }
        self.route_batch(IngestMsg::Logs { project_id, batch })
            .await?;
        let partial_success = (rejected > 0).then(|| ExportLogsPartialSuccess {
            rejected_log_records: rejected as i64,
            error_message: "some log records had an unusable service.name and were skipped"
                .to_string(),
        });
        Ok(Response::new(ExportLogsServiceResponse { partial_success }))
    }
}

#[tonic::async_trait]
impl MetricsService for OtlpGrpcService {
    #[instrument(skip_all, fields(project_id))]
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata()).await?;
        let bytes = Bytes::from(request.into_inner().encode_to_vec());
        let batches =
            transform_metrics(self.router.workloads(), bytes, InputFormat::Protobuf).await?;
        let has_data = batches.gauge.as_ref().is_some_and(|b| b.num_rows() > 0)
            || batches.sum.as_ref().is_some_and(|b| b.num_rows() > 0)
            || batches.histogram.as_ref().is_some_and(|b| b.num_rows() > 0)
            || batches
                .exp_histogram
                .as_ref()
                .is_some_and(|b| b.num_rows() > 0);
        if !has_data {
            tracing::warn!(project_id, "export metrics: empty batch after transform");
            return Ok(Response::new(ExportMetricsServiceResponse {
                partial_success: Some(ExportMetricsPartialSuccess {
                    rejected_data_points: 1,
                    error_message: "empty batch after transform".to_string(),
                }),
            }));
        }
        self.route_batch(IngestMsg::Metrics {
            project_id,
            batches,
        })
        .await?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl ProfilesService for OtlpGrpcService {
    #[instrument(skip_all, fields(project_id))]
    async fn export(
        &self,
        request: Request<ExportProfilesServiceRequest>,
    ) -> Result<Response<ExportProfilesServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata()).await?;
        let req = request.into_inner();
        let batch = spawn_transform(self.router.workloads(), "transform profiles", move || {
            sigiro_segments::transform_otlp_profiles(&req.encode_to_vec()).map_err(decode_status)
        })
        .await?;
        if batch.num_rows() == 0 {
            tracing::warn!(project_id, "export profiles: empty batch after transform");
            return Ok(Response::new(ExportProfilesServiceResponse {
                partial_success: Some(ExportProfilesPartialSuccess {
                    rejected_profiles: 1,
                    error_message: "empty batch after transform".to_string(),
                }),
            }));
        }
        self.route_batch(IngestMsg::Profiles { project_id, batch })
            .await?;
        Ok(Response::new(ExportProfilesServiceResponse {
            partial_success: None,
        }))
    }
}

// OTLP HTTP endpoints on :4318

pub(crate) struct OtlpHttpState {
    router: Arc<MsgRouter>,
    auth_mode: crate::auth::AuthMode,
    max_body_bytes: usize,
}

/// `max_body_bytes` is `SIGIRO_MAX_INGEST_BODY_BYTES` (see
/// `crate::config::max_ingest_body_bytes`), resolved once by the caller.
/// `DefaultBodyLimit` only marks the request (an extension flag consulted by
/// the `Bytes` extractor each handler uses); the actual cap is enforced when
/// that extractor collects the body, which happens *after*
/// `RequestDecompressionLayer` (applied by the caller around this whole
/// router — see `run_servers`) has already un-gzipped it. So this bounds the
/// DECOMPRESSED size regardless of which layer is nested outside the other —
/// closing the gzip-bomb hole, not just capping wire size.
pub fn otlp_http_router(
    router: Arc<MsgRouter>,
    auth_mode: crate::auth::AuthMode,
    max_body_bytes: usize,
) -> Router {
    let rate_limit = auth_mode.requires_auth();
    let state = Arc::new(OtlpHttpState {
        router,
        auth_mode,
        max_body_bytes,
    });
    let app = Router::new()
        .route("/", post(pyroscope_root)) // Pyroscope RS SDK posts here
        .route("/push.v1.PusherService/Push", post(pyroscope_push)) // pyroscope-rs SDK push path
        .route("/v1/traces", post(otlp_http_traces))
        .route("/v1/logs", post(otlp_http_logs))
        .route("/v1/metrics", post(otlp_http_metrics))
        .route("/v1development/profiles", post(otlp_http_profiles))
        .route("/ingest", post(pyroscope_ingest))
        .route("/ingest/pyroscope", post(pyroscope_ingest_pyroscope))
        .layer(axum::extract::DefaultBodyLimit::max(max_body_bytes))
        .with_state(state);

    if rate_limit {
        let governor = GovernorConfigBuilder::default()
            .key_extractor(crate::server::SourceRateLimitKeyExtractor)
            .per_millisecond(20)
            .burst_size(100)
            .finish()
            .expect("valid OTLP/HTTP source governor config");
        app.layer(
            GovernorLayer::new(governor).error_handler(crate::server::rate_limit_error_response),
        )
    } else {
        app
    }
}

/// Extract project_id from HTTP Authorization header.
/// Uses the explicit AuthMode — Open mode skips auth, JWT mode validates Bearer or AgentAuth.
pub(crate) async fn extract_project_id_http(
    auth_mode: &crate::auth::AuthMode,
    headers: &axum::http::HeaderMap,
) -> Result<String, IngestAuthFailure> {
    match auth_mode {
        crate::auth::AuthMode::Open => Ok("default".to_string()),
        crate::auth::AuthMode::Jwt { .. } => {
            let value = headers
                .get(axum::http::header::AUTHORIZATION)
                .ok_or_else(|| {
                    tracing::warn!("OTLP HTTP: missing Authorization header");
                    IngestAuthFailure::unauthenticated(MISSING_AUTHORIZATION)
                })?
                .to_str()
                .map_err(|_| IngestAuthFailure::unauthenticated(MALFORMED_AUTHORIZATION))?;
            let credential = crate::auth::parse_api_credential(value).map_err(|_| {
                tracing::warn!("OTLP HTTP: malformed Authorization header");
                IngestAuthFailure::unauthenticated(MALFORMED_AUTHORIZATION)
            })?;

            auth_mode
                .authenticate(
                    &credential.token,
                    crate::auth::TELEMETRY_INGEST_SCOPE,
                    credential.scheme,
                )
                .await
                .map(|context| context.tenant_id.as_ref().to_owned())
                .map_err(|error| {
                    tracing::warn!("OTLP HTTP: authentication failed: {error}");
                    IngestAuthFailure::from(error)
                })
        }
    }
}

const MISSING_AUTHORIZATION: &str = "Missing Authorization header. Send `Authorization: Bearer <ingest key>`; create a key with `sigiro ingest-keys create`.";
const MALFORMED_AUTHORIZATION: &str = "Malformed Authorization header. Expected `Authorization: Bearer <ingest key or access token>`.";

/// Why an ingest request was not authenticated, in a form the sender can log.
///
/// A bare 401 left a service's exporter logging "401" with nothing to act on,
/// and the most common cause, a 15-minute token that has expired, looked the
/// same as a typo.
#[derive(Debug)]
pub(crate) struct IngestAuthFailure {
    pub(crate) status: StatusCode,
    pub(crate) code: tonic::Code,
    pub(crate) message: String,
}

impl IngestAuthFailure {
    fn unauthenticated(message: &str) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: tonic::Code::Unauthenticated,
            message: message.to_owned(),
        }
    }

    /// OTLP/HTTP failure response: a `google.rpc.Status` in the request's own
    /// encoding, which SDKs decode and log, so a JSON request reads
    /// `{"code":16,"message":"…"}`.
    pub(crate) fn otlp_response(&self, format: InputFormat) -> AxumResponse {
        metrics_http_error_response(format, self.status, self.code, &self.message)
    }

    /// Plain-text response for the Pyroscope paths, which have no error schema.
    fn text_response(&self) -> AxumResponse {
        (
            self.status,
            [(CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("{}\n", self.message),
        )
            .into_response()
    }
}

impl From<crate::auth::AuthError> for IngestAuthFailure {
    fn from(error: crate::auth::AuthError) -> Self {
        let status = error.status();
        let code = match status {
            StatusCode::FORBIDDEN => tonic::Code::PermissionDenied,
            StatusCode::SERVICE_UNAVAILABLE => tonic::Code::Unavailable,
            _ => tonic::Code::Unauthenticated,
        };
        Self {
            status,
            code,
            message: error.reason(),
        }
    }
}

/// Common helper: send ingest message.
/// Returns 500 on failure, 200 on success.
async fn route_ingest_msg(state: &OtlpHttpState, msg: IngestMsg) -> StatusCode {
    let project_id = msg.project_id().to_string();
    if let Err(e) = state.router.send(msg).await {
        tracing::error!(project_id, "ingest router error: {e:?}");
        return match e {
            RouterError::DiskCapacityExceeded | RouterError::MaintenanceStalled { .. } => {
                StatusCode::INSUFFICIENT_STORAGE
            }
            RouterError::Admission(_) => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
    }
    StatusCode::OK
}

fn decode_status(error: sigiro_segments::DecodeError) -> Status {
    match error {
        sigiro_segments::DecodeError::TooLarge(detail) => Status::resource_exhausted(detail),
        sigiro_segments::DecodeError::Arrow(error) => {
            Status::internal(format!("arrow error: {error}"))
        }
        other => Status::invalid_argument(other.to_string()),
    }
}

fn transform_http_status(status: &Status) -> StatusCode {
    match status.code() {
        tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
        tonic::Code::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn metrics_http_response(
    format: InputFormat,
    status: StatusCode,
    partial_success: Option<ExportMetricsPartialSuccess>,
) -> AxumResponse {
    let response = ExportMetricsServiceResponse { partial_success };
    let (content_type, body) = match format {
        InputFormat::Json => {
            let body = match &response.partial_success {
                Some(partial) => serde_json::to_vec(&serde_json::json!({
                    "partialSuccess": {
                        "rejectedDataPoints": partial.rejected_data_points.to_string(),
                        "errorMessage": partial.error_message,
                    }
                }))
                .expect("OTLP metrics response must serialize"),
                None => b"{}".to_vec(),
            };
            ("application/json", body)
        }
        InputFormat::Protobuf => ("application/x-protobuf", response.encode_to_vec()),
    };
    (status, [(CONTENT_TYPE, content_type)], body).into_response()
}

fn metrics_http_error_response(
    format: InputFormat,
    status: StatusCode,
    code: tonic::Code,
    message: &str,
) -> AxumResponse {
    let code = code as i32;
    let (content_type, body) = match format {
        InputFormat::Json => (
            "application/json",
            serde_json::to_vec(&serde_json::json!({
                "code": code,
                "message": message,
            }))
            .expect("OTLP error response must serialize"),
        ),
        InputFormat::Protobuf => (
            "application/x-protobuf",
            tonic_types::pb::Status {
                code,
                message: message.to_string(),
                details: Vec::new(),
            }
            .encode_to_vec(),
        ),
    };
    (status, [(CONTENT_TYPE, content_type)], body).into_response()
}

#[instrument(skip_all, fields(endpoint = "/v1/traces"))]
async fn otlp_http_traces(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> AxumResponse {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers).await {
        Ok(id) => id,
        Err(failure) => return failure.otlp_response(detect_format(&headers)),
    };
    ingest_traces(&state, project_id, &headers, body)
        .await
        .into_response()
}

async fn ingest_traces(
    state: &OtlpHttpState,
    project_id: String,
    headers: &axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let fmt = detect_format(headers);
    let batch = match transform_traces(state.router.workloads(), body, fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform traces: {e}");
            return transform_http_status(&e);
        }
    };
    // F-05: empty batch is a valid no-op (e.g. OTel SDK shutdown sends an
    // empty final flush). Return 200 OK with no body — not 400 with WARN
    // "trace data lost" — so the sender doesn't log an error.
    if batch.num_rows() == 0 {
        return StatusCode::OK;
    }
    route_ingest_msg(state, IngestMsg::Traces { project_id, batch }).await
}

#[instrument(skip_all, fields(endpoint = "/v1/logs"))]
async fn otlp_http_logs(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> AxumResponse {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers).await {
        Ok(id) => id,
        Err(failure) => return failure.otlp_response(detect_format(&headers)),
    };
    ingest_logs(&state, project_id, &headers, body)
        .await
        .into_response()
}

async fn ingest_logs(
    state: &OtlpHttpState,
    project_id: String,
    headers: &axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let fmt = detect_format(headers);
    let (batch, rejected) = match transform_logs(state.router.workloads(), body, fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform logs: {e}");
            return transform_http_status(&e);
        }
    };
    // Per-record failures (e.g. unusable service.name) skip that record rather
    // than rejecting the batch; valid siblings are still routed.
    if rejected > 0 {
        tracing::warn!(project_id, rejected, "otlp logs: skipped unusable records");
    }
    // F-05: empty batch is a valid no-op (see otlp_http_traces).
    if batch.num_rows() == 0 {
        return StatusCode::OK;
    }
    route_ingest_msg(state, IngestMsg::Logs { project_id, batch }).await
}

#[instrument(skip_all, fields(endpoint = "/v1/metrics"))]
async fn otlp_http_metrics(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> AxumResponse {
    let fmt = detect_format(&headers);
    let project_id = match extract_project_id_http(&state.auth_mode, &headers).await {
        Ok(id) => id,
        Err(failure) => return failure.otlp_response(fmt),
    };
    let batches = match transform_metrics(state.router.workloads(), body, fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform metrics: {e}");
            return metrics_http_error_response(
                fmt,
                transform_http_status(&e),
                e.code(),
                e.message(),
            );
        }
    };
    let has_data = batches.gauge.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches.sum.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches.histogram.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches
            .exp_histogram
            .as_ref()
            .is_some_and(|b| b.num_rows() > 0);
    let rejected = batches.skipped.total();
    if fmt == InputFormat::Json && rejected > 0 {
        return metrics_http_error_response(
            fmt,
            StatusCode::BAD_REQUEST,
            tonic::Code::InvalidArgument,
            &format!(
                "rejected {rejected} metric data points: {}",
                batches.skipped.message()
            ),
        );
    }
    let partial_success = (rejected > 0).then(|| ExportMetricsPartialSuccess {
        rejected_data_points: rejected as i64,
        error_message: batches.skipped.message(),
    });
    if !has_data {
        // F-05: empty batch is a valid no-op (see otlp_http_traces).
        return metrics_http_response(fmt, StatusCode::OK, partial_success);
    }
    let status = route_ingest_msg(
        &state,
        IngestMsg::Metrics {
            project_id,
            batches,
        },
    )
    .await;
    if status != StatusCode::OK {
        let code = if status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::INSUFFICIENT_STORAGE
        {
            tonic::Code::ResourceExhausted
        } else {
            tonic::Code::Internal
        };
        return metrics_http_error_response(fmt, status, code, "failed to ingest metrics");
    }
    metrics_http_response(fmt, status, partial_success)
}

#[instrument(skip_all, fields(endpoint = "/v1development/profiles"))]
async fn otlp_http_profiles(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> AxumResponse {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers).await {
        Ok(id) => id,
        Err(failure) => return failure.otlp_response(detect_format(&headers)),
    };
    ingest_profiles(&state, project_id, body)
        .await
        .into_response()
}

async fn ingest_profiles(state: &OtlpHttpState, project_id: String, body: Bytes) -> StatusCode {
    let batch = match spawn_transform(state.router.workloads(), "transform profiles", move || {
        sigiro_segments::transform_otlp_profiles(&body).map_err(decode_status)
    })
    .await
    {
        Ok(batch) => batch,
        Err(error) => {
            tracing::error!("failed to transform profiles: {error}");
            return transform_http_status(&error);
        }
    };
    if batch.num_rows() == 0 {
        tracing::warn!(
            project_id,
            "batch empty after transform — profile data lost"
        );
        return StatusCode::BAD_REQUEST;
    }
    route_ingest_msg(state, IngestMsg::Profiles { project_id, batch }).await
}

// ─── Pyroscope native format ingest ──────────────────────────────────────────
//
// The pyroscope Rust crate pushes profiles in Pyroscope's own wire format:
//   PushRequest (prost protobuf) → gzip → HTTP POST
// Sigiro's OTLP /v1development/profiles accepts only OTLP protobuf. This endpoint bridges
// the two by decoding Pyroscope's native format and converting to the same
// Arrow RecordBatch that transform_profiles produces.
//
// Note: "gen" is a reserved keyword in Rust 2021+, so we re-export the module
// under a safe name via the `pyroscope::encode::gen` module alias below.

/// HTTP handler for Pyroscope's native push format.
///
/// Accepts these formats (and binary payloads without a Content-Type):
/// - `application/proto`, `application/octet-stream`, or `binary/octet-stream` —
///   PushRequest, or raw pprof with `name`
/// - `multipart/form-data` — `profile` contains PushRequest, or raw pprof with `name`
#[derive(Default, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct PyroscopeIngestQuery {
    /// When present, the body is a raw pprof profile and the service name is the
    /// part before the first `{`. When absent, the body is a Pyroscope
    /// PushRequest and each series must contain a `service_name` label.
    name: Option<String>,
}

struct PyroscopeProfileBody;

impl utoipa::PartialSchema for PyroscopeProfileBody {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        utoipa::openapi::ObjectBuilder::new()
            .schema_type(utoipa::openapi::schema::Type::String)
            .format(Some(utoipa::openapi::SchemaFormat::KnownFormat(
                utoipa::openapi::KnownFormat::Binary,
            )))
            .into()
    }
}

impl utoipa::ToSchema for PyroscopeProfileBody {}

const PYROSCOPE_INGEST_DESCRIPTION: &str = "Accept a Pyroscope PushRequest or raw pprof profile. With `name`, the payload is raw pprof and the service name is the part before the first `{`; without `name`, the payload is a PushRequest whose series require a `service_name` label. Binary bodies may be gzip-compressed. Multipart bodies must carry the payload in the `profile` field. A missing Content-Type is also accepted as a binary body. See the [profile ingest guide](https://sigiro.com/docs/how-to/send-profiles).";

macro_rules! pyroscope_ingest_handler {
    ($handler:ident, $path:literal, $operation_id:literal, $summary:literal) => {
        #[utoipa::path(
            post,
            path = $path,
            operation_id = $operation_id,
            // Without an explicit tag utoipa uses the module path, and the
            // public reference showed a section called `crate::ingest`.
            tag = "Pyroscope ingest",
            summary = $summary,
            description = PYROSCOPE_INGEST_DESCRIPTION,
            params(PyroscopeIngestQuery),
            request_body(
                description = "Pyroscope PushRequest or raw pprof bytes. For multipart/form-data, put the binary payload in the `profile` field.",
                content(
                    (PyroscopeProfileBody = "application/proto"),
                    (PyroscopeProfileBody = "application/octet-stream"),
                    (PyroscopeProfileBody = "binary/octet-stream"),
                    (PyroscopeProfileBody = "multipart/form-data")
                )
            ),
            responses(
                (status = 200, description = "Profile accepted"),
                (status = 400, description = "Unsupported media type, malformed payload, missing service name, or empty profile"),
                (status = 401, description = "Missing, expired, revoked or invalid credential; the plain-text body says which"),
                (status = 403, description = "The credential lacks telemetry:ingest"),
                (status = 413, description = "Request body exceeds the configured ingest limit"),
                (status = 429, description = "Ingest rate limit exceeded"),
                (status = 500, description = "Ingest failed"),
                (status = 507, description = "Storage cannot currently accept ingest")
            ),
            security(("bearer" = [])),
        )]
        pub(crate) async fn $handler(
            state: State<Arc<OtlpHttpState>>,
            query: Query<PyroscopeIngestQuery>,
            headers: axum::http::HeaderMap,
            body: Bytes,
        ) -> AxumResponse {
            otlp_http_pyroscope(state, query, headers, body).await
        }
    };
}

pyroscope_ingest_handler!(
    pyroscope_root,
    "/",
    "ingestPyroscopeRoot",
    "Ingest Pyroscope profiles at the server root"
);
pyroscope_ingest_handler!(
    pyroscope_push,
    "/push.v1.PusherService/Push",
    "ingestPyroscopePush",
    "Ingest Pyroscope profiles over the Push API"
);
pyroscope_ingest_handler!(
    pyroscope_ingest,
    "/ingest",
    "ingestPyroscope",
    "Ingest Pyroscope profiles"
);
pyroscope_ingest_handler!(
    pyroscope_ingest_pyroscope,
    "/ingest/pyroscope",
    "ingestPyroscopeAlias",
    "Ingest Pyroscope profiles at the /ingest/pyroscope alias"
);

async fn otlp_http_pyroscope(
    State(state): State<Arc<OtlpHttpState>>,
    Query(query): Query<PyroscopeIngestQuery>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> AxumResponse {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers).await {
        Ok(id) => id,
        Err(failure) => {
            tracing::warn!("pyroscope: auth failed: {}", failure.message);
            return failure.text_response();
        }
    };
    ingest_pyroscope(&state, project_id, query, &headers, body)
        .await
        .into_response()
}

async fn ingest_pyroscope(
    state: &OtlpHttpState,
    project_id: String,
    query: PyroscopeIngestQuery,
    headers: &axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let ct = headers
        .typed_get::<headers::ContentType>()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "(none)".to_string());
    tracing::debug!(
        "otlp_http_pyroscope: project_id={project_id} body_len={} ct={ct}",
        body.len()
    );

    // Detect format: multipart/form-data (SDK) vs raw protobuf
    let body_bytes = match parse_pyroscope_multipart(headers, &body).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("pyroscope parse failed: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    let sdk_name = query.name;
    let max_profile_bytes = state.max_body_bytes;
    let batch = match spawn_transform(
        state.router.workloads(),
        "transform pyroscope profiles",
        move || {
            sigiro_segments::transform_pyroscope_profiles(
                body_bytes,
                sdk_name.as_deref(),
                max_profile_bytes,
            )
            .map_err(decode_status)
        },
    )
    .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("transform pyroscope profiles failed: {e}");
            return transform_http_status(&e);
        }
    };

    tracing::info!(
        "pyroscope ingest: project_id={project_id} rows={}",
        batch.num_rows()
    );

    if batch.num_rows() == 0 {
        tracing::warn!(
            project_id,
            "otlp_http_profiles: batch empty after transform — profile data lost"
        );
        return StatusCode::BAD_REQUEST;
    }
    let pid = project_id.clone();
    if let Err(e) = state
        .router
        .send(IngestMsg::Profiles { project_id, batch })
        .await
    {
        tracing::error!(pid, "ingest router error: {e:?}");
        return match e {
            RouterError::DiskCapacityExceeded | RouterError::MaintenanceStalled { .. } => {
                StatusCode::INSUFFICIENT_STORAGE
            }
            RouterError::Admission(_) => StatusCode::TOO_MANY_REQUESTS,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
    }
    StatusCode::OK
}

/// Extract service.name from an optional Resource's attributes.
/// Returns None if the resource is absent or lacks the attribute.
/// Parse the request body, handling both raw protobuf and Pyroscope SDK multipart/form-data.
///
/// Pyroscope SDKs send `multipart/form-data` with a `profile` form file. The
/// profile may be raw pprof (Node SDK, identified by the `name` query parameter)
/// or a PushRequest protobuf. This function extracts the profile bytes; decoding
/// is handled by `transform_pyroscope_profiles`.
async fn parse_pyroscope_multipart(
    headers: &axum::http::HeaderMap,
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let content_type = headers
        .typed_get::<headers::ContentType>()
        .map(|c| c.to_string())
        .unwrap_or_default();

    if content_type.contains("multipart/form-data") {
        // Extract boundary from Content-Type header via typed mime parsing
        let mime: mime::Mime = headers
            .typed_get::<headers::ContentType>()
            .ok_or("missing Content-Type header")?
            .into();
        let boundary = mime
            .get_param("boundary")
            .map(|b| b.to_string())
            .ok_or("missing boundary in multipart Content-Type")?;

        let stream = futures_util::stream::once(std::future::ready(Ok::<_, std::io::Error>(
            Bytes::copy_from_slice(body),
        )));
        let mut multipart = Multipart::new(stream, boundary);

        while let Some(field) = multipart
            .next_field()
            .await
            .map_err(|e| format!("multipart parse error: {e}"))?
        {
            if field.name() == Some("profile") {
                let profile_bytes = field
                    .bytes()
                    .await
                    .map_err(|e| format!("multipart field read error: {e}"))?;
                tracing::debug!(
                    "pyroscope multipart: extracted {} profile bytes",
                    profile_bytes.len()
                );
                return Ok(profile_bytes.to_vec());
            }
        }

        Err("profile form field not found in multipart body".to_string())
    } else if content_type.is_empty()
        || content_type.contains("application/proto")
        || content_type.contains("application/octet-stream")
        || content_type.contains("binary/octet-stream")
    {
        if body.is_empty() {
            return Err("empty body".to_string());
        }
        Ok(body.to_vec())
    } else {
        Err(format!(
            "unimplemented: Content-Type '{}' is not supported for Pyroscope ingest; \
             supported: 'multipart/form-data' (SDK pprof), 'application/proto' or \
             'application/octet-stream' (binary profile payload)",
            content_type
        ))
    }
}

#[cfg(test)]
mod ingest_auth_reason_tests {
    use super::*;
    use crate::auth::test_support::{jwt_mode, user_token};
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        headers
    }

    async fn json_body(response: AxumResponse) -> (StatusCode, serde_json::Value) {
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn every_401_says_why() {
        let (mode, keys) = jwt_mode();
        let acme = crate::types::TenantId::try_new("acme".to_owned()).unwrap();
        let created = keys.create(&acme, "svc", "user-1").await.unwrap();

        assert_eq!(
            extract_project_id_http(&mode, &bearer(&created.key))
                .await
                .unwrap(),
            "acme"
        );

        let cases = [
            (HeaderMap::new(), "Missing Authorization header"),
            (
                bearer(&user_token("acme", "telemetry:ingest", -300)),
                "expired",
            ),
            (bearer("sigiro_abc_def"), "not valid"),
            (bearer("garbage"), "could not be verified"),
        ];
        for (headers, expected) in cases {
            let failure = extract_project_id_http(&mode, &headers).await.unwrap_err();
            let (status, body) = json_body(failure.otlp_response(InputFormat::Json)).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{expected}");
            assert_eq!(body["code"], 16, "{expected}");
            assert!(
                body["message"].as_str().unwrap().contains(expected),
                "{body} lacks {expected}"
            );
        }

        keys.revoke(&acme, &created.info.id).await.unwrap();
        let failure = extract_project_id_http(&mode, &bearer(&created.key))
            .await
            .unwrap_err();
        let (status, body) = json_body(failure.otlp_response(InputFormat::Json)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body["message"].as_str().unwrap().contains("revoked"));

        // Protobuf senders get a google.rpc.Status their SDK decodes.
        let response = failure.otlp_response(InputFormat::Protobuf);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap();
        let status = tonic_types::pb::Status::decode(bytes).unwrap();
        assert_eq!(status.code, tonic::Code::Unauthenticated as i32);
        assert!(status.message.contains("revoked"));
    }
}
