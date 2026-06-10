use crate::otlp::{InputFormat, MetricBatches};
use anyhow::Result;
use arrow::array::{Int64Array, ListArray, StringBuilder, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use headers::{Authorization, HeaderMapExt, authorization::Bearer};
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
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing_attributes::instrument;

use crate::inserter::MsgRouter;
use crate::types::IngestMsg;

/// Wrap an Arrow/record-batch construction error into a tonic Status.
fn arrow_status<E: std::fmt::Display>(e: E) -> Status {
    status_internal(format!("arrow error: {e}"))
}

/// Convenience helper for `Status::internal` to avoid repeating `format!` at call sites.
fn status_internal(msg: impl std::fmt::Display) -> Status {
    Status::internal(msg.to_string())
}

pub struct OtlpGrpcService {
    router: Arc<MsgRouter>,
    auth_mode: crate::auth::AuthMode,
}

impl OtlpGrpcService {
    pub fn new(router: Arc<MsgRouter>, auth_mode: crate::auth::AuthMode) -> Self {
        Self { router, auth_mode }
    }

    /// Route a batch to the inserter: register flusher then send.
    /// Consolidates the repeated register_flusher+send pattern across all 4 signal handlers.
    #[instrument(skip_all, fields(project_id))]
    async fn route_batch(&self, project_id: String, msg: IngestMsg) -> Result<(), Status> {
        if let Err(e) = self.router.register_flusher(&project_id) {
            tracing::error!(project_id, "failed to register flusher: {e:?}");
            return Err(Status::internal("flusher init failed"));
        }
        if let Err(e) = self.router.send(msg).await {
            tracing::error!(project_id, "ingest router error: {e:?}");
            return Err(Status::internal("ingest failed"));
        }
        Ok(())
    }

    pub fn trace_service(&self) -> TraceServiceServer<Self> {
        TraceServiceServer::new(Self {
            router: Arc::clone(&self.router),
            auth_mode: self.auth_mode.clone(),
        })
    }

    pub fn logs_service(&self) -> LogsServiceServer<Self> {
        LogsServiceServer::new(Self {
            router: Arc::clone(&self.router),
            auth_mode: self.auth_mode.clone(),
        })
    }

    pub fn metrics_service(&self) -> MetricsServiceServer<Self> {
        MetricsServiceServer::new(Self {
            router: Arc::clone(&self.router),
            auth_mode: self.auth_mode.clone(),
        })
    }

    pub fn profiles_service(&self) -> ProfilesServiceServer<Self> {
        ProfilesServiceServer::new(Self {
            router: Arc::clone(&self.router),
            auth_mode: self.auth_mode.clone(),
        })
    }
}

/// Extract project_id from gRPC metadata.
/// Uses the explicit AuthMode — Open mode skips auth, Key mode validates Bearer token.
pub fn extract_project_id_grpc(
    auth_mode: &crate::auth::AuthMode,
    metadata: &tonic::metadata::MetadataMap,
) -> Result<String, Status> {
    match auth_mode {
        crate::auth::AuthMode::Open => Ok("default".to_string()),
        crate::auth::AuthMode::Key { .. } => {
            let token = metadata
                .get("authorization")
                .ok_or_else(|| Status::unauthenticated("Missing Authorization metadata"))?
                .to_str()
                .map_err(|_| Status::unauthenticated("Invalid Authorization encoding"))?
                .strip_prefix("Bearer ")
                .ok_or_else(|| Status::unauthenticated("Expected: Bearer <key>"))?;

            let ctx = auth_mode
                .verify(token)
                .map_err(|_| Status::unauthenticated("Invalid API key"))?;
            Ok(ctx.project_id)
        }
    }
}

/// Detect InputFormat from Content-Type header.
/// OTLP/HTTP spec: application/x-protobuf (default) or application/json.
fn detect_format(headers: &axum::http::HeaderMap) -> InputFormat {
    headers
        .typed_get::<headers::ContentType>()
        .map(|ct| {
            if ct == headers::ContentType::json() {
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

/// Run a CPU-bound transform on the tokio blocking pool.
/// Maps both transform errors and task-panic errors to `Status::internal`.
async fn spawn_transform<T, E>(
    name: &'static str,
    f: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, Status>
where
    T: Send + 'static,
    E: std::fmt::Display,
{
    tokio::task::spawn_blocking(move || f().map_err(|e| status_internal(format!("{name}: {e}"))))
        .await
        .map_err(|e| status_internal(format!("{name} task panicked: {e}")))?
}

/// Transform raw bytes into Arrow RecordBatch on the tokio blocking pool.
/// Supports both protobuf and JSON input based on Content-Type header.
#[instrument(skip_all, fields(endpoint = "/v1/traces"))]
async fn transform_traces(bytes: Vec<u8>, fmt: InputFormat) -> Result<RecordBatch, Status> {
    spawn_transform("transform traces", move || {
        crate::otlp::transform_traces(&bytes, fmt)
    })
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/logs"))]
async fn transform_logs(bytes: Vec<u8>, fmt: InputFormat) -> Result<RecordBatch, Status> {
    spawn_transform("transform logs", move || {
        crate::otlp::transform_logs(&bytes, fmt)
    })
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/metrics"))]
async fn transform_metrics(bytes: Vec<u8>, fmt: InputFormat) -> Result<MetricBatches, Status> {
    spawn_transform("transform metrics", move || {
        crate::otlp::transform_metrics(&bytes, fmt)
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
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata())?;
        // Transform on tokio blocking pool — no double decode.
        // Encode once (prost), decode once (direct Arrow builder, src/otlp.rs).
        let bytes = request.into_inner().encode_to_vec();
        let batch = transform_traces(bytes, InputFormat::Protobuf).await?;
        if batch.num_rows() == 0 {
            tracing::warn!(project_id, "export traces: empty batch after transform");
            return Ok(Response::new(ExportTraceServiceResponse {
                partial_success: Some(ExportTracePartialSuccess {
                    rejected_spans: 1,
                    error_message: "empty batch after transform".to_string(),
                }),
            }));
        }
        self.route_batch(project_id.clone(), IngestMsg::Traces { project_id, batch })
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
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata())?;
        let bytes = request.into_inner().encode_to_vec();
        let batch = transform_logs(bytes, InputFormat::Protobuf).await?;
        if batch.num_rows() == 0 {
            tracing::warn!(project_id, "export logs: empty batch after transform");
            return Ok(Response::new(ExportLogsServiceResponse {
                partial_success: Some(ExportLogsPartialSuccess {
                    rejected_log_records: 1,
                    error_message: "empty batch after transform".to_string(),
                }),
            }));
        }
        self.route_batch(project_id.clone(), IngestMsg::Logs { project_id, batch })
            .await?;
        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl MetricsService for OtlpGrpcService {
    #[instrument(skip_all, fields(project_id))]
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata())?;
        let bytes = request.into_inner().encode_to_vec();
        let batches = transform_metrics(bytes, InputFormat::Protobuf).await?;
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
        self.route_batch(
            project_id.clone(),
            IngestMsg::Metrics {
                project_id,
                batches,
            },
        )
        .await?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

/// Resolve pprofile dictionary encoding into flat profile rows as a RecordBatch.
/// Resolution chain:
///   Sample.stack_index → Dictionary.stack_table[].location_indices[]
///   → Dictionary.location_table[].line[].function_index
///   → Dictionary.function_table[].name_strindex
///   → Dictionary.string_table[]
fn profile_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("profile_id", DataType::Utf8, true),
        Field::new("service_name", DataType::Utf8, false),
        Field::new(
            "timestamp",
            DataType::Timestamp(TimeUnit::Microsecond, None),
            false,
        ),
        Field::new("profile_type", DataType::Utf8, true),
        Field::new(
            "stack_trace",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            true,
        ),
        Field::new("value", DataType::Int64, true),
        Field::new("unit", DataType::Utf8, true),
        Field::new("resource_attributes", DataType::Utf8, true),
    ]))
}

fn transform_profiles(request: ExportProfilesServiceRequest) -> Result<RecordBatch, Status> {
    let dict = match request.dictionary {
        Some(d) => d,
        None => {
            // No dictionary = no samples can be resolved. Return empty batch.
            tracing::debug!("profiles request has no dictionary, returning empty batch");
            return RecordBatch::try_new(
                profile_schema(),
                vec![
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(
                        ListArray::try_new(
                            Arc::new(Field::new("item", DataType::Utf8, true)),
                            arrow::buffer::OffsetBuffer::new(vec![0i32].into()),
                            Arc::new(StringBuilder::new().finish()),
                            None,
                        )
                        .map_err(arrow_status)?,
                    ),
                    Arc::new(Int64Array::from(Vec::<i64>::new())),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                ],
            )
            .map_err(arrow_status);
        }
    };
    let string_table = &dict.string_table;

    let schema = profile_schema();

    let mut profile_ids = StringBuilder::new();
    let mut service_names = StringBuilder::new();
    let mut timestamps = Vec::new();
    let mut profile_types = StringBuilder::new();
    let mut stack_offsets: Vec<i32> = vec![0]; // list offsets
    let mut stack_values = StringBuilder::new();
    let mut values = Vec::new();
    let mut units = StringBuilder::new();
    let mut resource_attrs = StringBuilder::new();

    let resolve_str = |idx: i32| -> Result<&str, Status> {
        string_table
            .get(idx as usize)
            .map(String::as_str)
            .ok_or_else(|| {
                Status::invalid_argument(format!(
                    "string_table index {idx} out of bounds (table has {} entries)",
                    string_table.len()
                ))
            })
    };

    /// Resolve a stack from sample.stack_index through the dictionary tables.
    /// Returns function names from root to leaf, capped at 128 frames.
    /// Returns Err on any index out of bounds — malformed profiles are rejected entirely.
    fn resolve_stack<'a>(
        stack_index: i32,
        dict: &'a opentelemetry_proto::tonic::profiles::v1development::ProfilesDictionary,
        string_table: &'a [String],
    ) -> Result<Vec<&'a str>, Status> {
        let stack = dict.stack_table.get(stack_index as usize).ok_or_else(|| {
            Status::invalid_argument(format!(
                "stack_table index {stack_index} out of bounds (table has {} entries)",
                dict.stack_table.len()
            ))
        })?;
        let mut frames = Vec::with_capacity(stack.location_indices.len().min(128));
        for (frame_pos, &loc_idx) in stack.location_indices.iter().take(128).enumerate() {
            let loc = dict.location_table.get(loc_idx as usize).ok_or_else(|| {
                Status::invalid_argument(format!(
                    "location_table index {loc_idx} out of bounds at stack {stack_index} frame {frame_pos} (table has {} entries)",
                    dict.location_table.len()
                ))
            })?;
            let line = loc.line.first().ok_or_else(|| {
                Status::invalid_argument(format!(
                    "location index {loc_idx} has no line entries at stack {stack_index} frame {frame_pos}"
                ))
            })?;
            let func = dict.function_table.get(line.function_index as usize).ok_or_else(|| {
                Status::invalid_argument(format!(
                    "function_table index {} out of bounds at stack {stack_index} frame {frame_pos} (table has {} entries)",
                    line.function_index, dict.function_table.len()
                ))
            })?;
            let name = string_table.get(func.name_strindex as usize).ok_or_else(|| {
                Status::invalid_argument(format!(
                    "string_table index {} out of bounds for function name at stack {stack_index} frame {frame_pos} (table has {} entries)",
                    func.name_strindex, string_table.len()
                ))
            })?;
            frames.push(name.as_str());
        }
        Ok(frames)
    }

    for rp in &request.resource_profiles {
        let service_name = extract_service_name(rp.resource.as_ref())
            .ok_or_else(|| Status::invalid_argument(
                "resource_profiles entry missing service.name attribute — every profile must identify its service",
            ))?;

        let res_attrs_json = if let Some(r) = &rp.resource {
            if r.attributes.is_empty() {
                None
            } else {
                Some(format!(
                    "{{{}}}",
                    r.attributes
                        .iter()
                        .map(|a| format!(
                            "\"{}\":\"{}\"",
                            a.key,
                            a.value
                                .as_ref()
                                .map(|v| format!("{v:?}"))
                                .unwrap_or_else(|| format!("<MISSING VALUE for key '{}'>", a.key))
                        ))
                        .collect::<Vec<_>>()
                        .join(",")
                ))
            }
        } else {
            None
        };

        for sp in &rp.scope_profiles {
            for profile in &sp.profiles {
                let ts_us = (profile.time_unix_nano / 1000) as i64;
                let pid_hex;
                let pid = if profile.profile_id.is_empty() {
                    ""
                } else {
                    pid_hex = hex::encode(&profile.profile_id);
                    pid_hex.as_str()
                };

                // Resolve profile_type and unit from sample_type
                let (ptype, punit) = match profile.sample_type.as_ref() {
                    Some(st) => (
                        resolve_str(st.type_strindex)?,
                        resolve_str(st.unit_strindex)?,
                    ),
                    None => ("", ""),
                };

                for sample in &profile.sample {
                    let stack = resolve_stack(sample.stack_index, &dict, string_table)?;
                    let sample_values = if sample.values.is_empty() {
                        vec![0i64]
                    } else {
                        sample.values.clone()
                    };

                    // For multi-value profiles, write one row per value
                    for &val in &sample_values {
                        profile_ids.append_value(pid);
                        service_names.append_value(service_name);
                        timestamps.push(ts_us);
                        profile_types.append_value(ptype);

                        for frame in &stack {
                            stack_values.append_value(frame);
                        }
                        let prev_offset = *stack_offsets
                            .last()
                            .expect("invariant: stack_offsets always has at least one element");
                        stack_offsets.push(prev_offset + stack.len() as i32);

                        values.push(val);
                        units.append_value(punit);
                        match &res_attrs_json {
                            Some(j) => resource_attrs.append_value(j),
                            None => resource_attrs.append_null(),
                        }
                    }
                }
            }
        }
    }

    if timestamps.is_empty() {
        return RecordBatch::try_new(
            schema,
            vec![
                Arc::new(profile_ids.finish()),
                Arc::new(service_names.finish()),
                Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                Arc::new(profile_types.finish()),
                Arc::new(
                    ListArray::try_new(
                        Arc::new(Field::new("item", DataType::Utf8, true)),
                        arrow::buffer::OffsetBuffer::new(vec![0i32].into()),
                        Arc::new(stack_values.finish()),
                        None,
                    )
                    .map_err(arrow_status)?,
                ),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(units.finish()),
                Arc::new(resource_attrs.finish()),
            ],
        )
        .map_err(arrow_status);
    }

    let stack_list = ListArray::try_new(
        Arc::new(Field::new("item", DataType::Utf8, true)),
        arrow::buffer::OffsetBuffer::new(stack_offsets.into()),
        Arc::new(stack_values.finish()),
        None,
    )
    .map_err(arrow_status)?;

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(profile_ids.finish()),
            Arc::new(service_names.finish()),
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(profile_types.finish()),
            Arc::new(stack_list),
            Arc::new(Int64Array::from(values)),
            Arc::new(units.finish()),
            Arc::new(resource_attrs.finish()),
        ],
    )
    .map_err(arrow_status)
}

#[tonic::async_trait]
impl ProfilesService for OtlpGrpcService {
    #[instrument(skip_all, fields(project_id))]
    async fn export(
        &self,
        request: Request<ExportProfilesServiceRequest>,
    ) -> Result<Response<ExportProfilesServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata())?;
        let req = request.into_inner();
        let batch = spawn_transform("transform profiles", move || transform_profiles(req)).await?;
        if batch.num_rows() == 0 {
            tracing::warn!(project_id, "export profiles: empty batch after transform");
            return Ok(Response::new(ExportProfilesServiceResponse {
                partial_success: Some(ExportProfilesPartialSuccess {
                    rejected_profiles: 1,
                    error_message: "empty batch after transform".to_string(),
                }),
            }));
        }
        self.route_batch(
            project_id.clone(),
            IngestMsg::Profiles { project_id, batch },
        )
        .await?;
        Ok(Response::new(ExportProfilesServiceResponse {
            partial_success: None,
        }))
    }
}

// OTLP HTTP endpoints on :4318

struct OtlpHttpState {
    router: Arc<MsgRouter>,
    auth_mode: crate::auth::AuthMode,
}

pub fn otlp_http_router(router: Arc<MsgRouter>, auth_mode: crate::auth::AuthMode) -> Router {
    let state = Arc::new(OtlpHttpState { router, auth_mode });
    Router::new()
        .route("/", post(otlp_http_pyroscope)) // Pyroscope RS SDK posts here
        .route("/push.v1.PusherService/Push", post(otlp_http_pyroscope)) // pyroscope-rs SDK push path
        .route("/v1/traces", post(otlp_http_traces))
        .route("/v1/logs", post(otlp_http_logs))
        .route("/v1/metrics", post(otlp_http_metrics))
        .route("/v1/profiles", post(otlp_http_profiles))
        .route("/ingest", post(otlp_http_pyroscope))
        .route("/ingest/pyroscope", post(otlp_http_pyroscope))
        // F-06: a GET endpoint that documents the OTLP routes and reflects
        // back what the client sent, so a user can `curl /v1/otlp-info` to
        // verify the path is reachable and content-type handling is right.
        .route("/v1/otlp-info", get(otlp_http_info).post(otlp_http_info))
        .with_state(state)
}

/// GET/POST /v1/otlp-info — returns the OTLP endpoint manifest. Useful for
/// `curl http://sigiro:4318/v1/otlp-info` to verify the OTLP port is open
/// and reachable. POSTs the request body back in the response so users can
/// see exactly what the server received.
async fn otlp_http_info(
    State(_state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "otlp": {
            "traces":   "POST /v1/traces    Content-Type: application/x-protobuf",
            "logs":     "POST /v1/logs      Content-Type: application/x-protobuf",
            "metrics":  "POST /v1/metrics   Content-Type: application/x-protobuf",
            "profiles": "POST /v1/profiles  Content-Type: application/x-protobuf",
        },
        "received": {
            "method": "GET/POST",
            "body_bytes": body.len(),
            "content_type": headers.get("content-type").and_then(|v| v.to_str().ok()),
            "user_agent": headers.get("user-agent").and_then(|v| v.to_str().ok()),
        }
    }))
}

/// Extract project_id from HTTP Authorization header.
/// Uses the explicit AuthMode — Open mode skips auth, Key mode validates Bearer token.
pub fn extract_project_id_http(
    auth_mode: &crate::auth::AuthMode,
    headers: &axum::http::HeaderMap,
) -> Result<String, StatusCode> {
    match auth_mode {
        crate::auth::AuthMode::Open => Ok("default".to_string()),
        crate::auth::AuthMode::Key { .. } => {
            let auth: Authorization<Bearer> = headers.typed_get().ok_or_else(|| {
                tracing::warn!("OTLP HTTP: missing or malformed Authorization header");
                StatusCode::UNAUTHORIZED
            })?;
            let token = auth.token();

            let ctx = auth_mode.verify(token).map_err(|_| {
                tracing::warn!("OTLP HTTP: invalid API key");
                StatusCode::UNAUTHORIZED
            })?;
            Ok(ctx.project_id)
        }
    }
}

/// Common helper: register flusher and send ingest message.
/// Returns 500 on failure, 200 on success.
async fn route_ingest_msg(state: &OtlpHttpState, project_id: &str, msg: IngestMsg) -> StatusCode {
    if let Err(e) = state.router.register_flusher(project_id) {
        tracing::error!(project_id, "failed to register flusher: {e:?}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    if let Err(e) = state.router.send(msg).await {
        tracing::error!(project_id, "ingest router error: {e:?}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

#[instrument(skip_all, fields(endpoint = "/v1/traces"))]
async fn otlp_http_traces(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let fmt = detect_format(&headers);
    let batch = match transform_traces(body.to_vec(), fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform traces: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    // F-05: empty batch is a valid no-op (e.g. OTel SDK shutdown sends an
    // empty final flush). Return 200 OK with no body — not 400 with WARN
    // "trace data lost" — so the sender doesn't log an error.
    if batch.num_rows() == 0 {
        return StatusCode::OK;
    }
    route_ingest_msg(
        &state,
        &project_id,
        IngestMsg::Traces {
            project_id: project_id.clone(),
            batch,
        },
    )
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/logs"))]
async fn otlp_http_logs(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let fmt = detect_format(&headers);
    let batch = match transform_logs(body.to_vec(), fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform logs: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    // F-05: empty batch is a valid no-op (see otlp_http_traces).
    if batch.num_rows() == 0 {
        return StatusCode::OK;
    }
    route_ingest_msg(
        &state,
        &project_id,
        IngestMsg::Logs {
            project_id: project_id.clone(),
            batch,
        },
    )
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/metrics"))]
async fn otlp_http_metrics(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let fmt = detect_format(&headers);
    let batches = match transform_metrics(body.to_vec(), fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform metrics: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    let has_data = batches.gauge.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches.sum.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches.histogram.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches
            .exp_histogram
            .as_ref()
            .is_some_and(|b| b.num_rows() > 0);
    if !has_data {
        // F-05: empty batch is a valid no-op (see otlp_http_traces).
        return StatusCode::OK;
    }
    route_ingest_msg(
        &state,
        &project_id,
        IngestMsg::Metrics {
            project_id: project_id.clone(),
            batches,
        },
    )
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/profiles"))]
async fn otlp_http_profiles(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers) {
        Ok(id) => id,
        Err(status) => return status,
    };
    let req = match ExportProfilesServiceRequest::decode(body) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("failed to decode profiles request: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    let batch = match tokio::task::spawn_blocking(move || transform_profiles(req)).await {
        Ok(Ok(b)) => b,
        Ok(Err(e)) => {
            tracing::error!("failed to transform profiles: {e}");
            return StatusCode::BAD_REQUEST;
        }
        Err(e) => {
            tracing::error!("transform profiles task panicked: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    };
    if batch.num_rows() == 0 {
        tracing::warn!(
            project_id,
            "batch empty after transform — profile data lost"
        );
        return StatusCode::BAD_REQUEST;
    }
    route_ingest_msg(
        &state,
        &project_id,
        IngestMsg::Profiles {
            project_id: project_id.clone(),
            batch,
        },
    )
    .await
}

// ─── Pyroscope native format ingest ──────────────────────────────────────────
//
// The pyroscope Rust crate pushes profiles in Pyroscope's own wire format:
//   PushRequest (prost protobuf) → gzip → HTTP POST
// Sigiro's OTLP /v1/profiles accepts only OTLP protobuf. This endpoint bridges
// the two by decoding Pyroscope's native format and converting to the same
// Arrow RecordBatch that transform_profiles produces.
//
// Note: "gen" is a reserved keyword in Rust 2021+, so we re-export the module
// under a safe name via the `pyroscope::encode::gen` module alias below.

/// HTTP handler for Pyroscope's native push format.
///
/// Accepts two formats:
/// - `application/x-protobuf` (gzip-compressed PushRequest) — direct push
/// - `multipart/form-data` (Pyroscope Go SDK format) — form file "profile" contains gzip-compressed PushRequest
async fn otlp_http_pyroscope(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.auth_mode, &headers) {
        Ok(id) => id,
        Err(status) => {
            tracing::warn!("pyroscope: auth failed: {status}");
            return status;
        }
    };

    let ct = headers
        .typed_get::<headers::ContentType>()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "(none)".to_string());
    tracing::debug!(
        "otlp_http_pyroscope: project_id={project_id} body_len={} ct={ct}",
        body.len()
    );

    // Detect format: multipart/form-data (SDK) vs raw protobuf
    let body_bytes = match parse_pyroscope_multipart(&headers, &body).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("pyroscope parse failed: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };

    let batch = match transform_pyroscope_profiles(body_bytes).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("transform pyroscope profiles failed: {e}");
            return StatusCode::BAD_REQUEST;
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
    if let Err(e) = state.router.register_flusher(&project_id) {
        tracing::error!(project_id, "failed to register flusher: {e:?}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    if let Err(e) = state
        .router
        .send(IngestMsg::Profiles { project_id, batch })
        .await
    {
        tracing::error!(pid, "ingest router error: {e:?}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

/// Extract service.name from an optional Resource's attributes.
/// Returns None if the resource is absent or lacks the attribute.
fn extract_service_name(
    resource: Option<&opentelemetry_proto::tonic::resource::v1::Resource>,
) -> Option<&str> {
    let resource = resource?;
    for attr in &resource.attributes {
        if attr.key == "service.name" {
            let val = attr.value.as_ref()?;
            let inner = val.value.as_ref()?;
            if let opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) = inner
            {
                return Some(s.as_str());
            }
        }
    }
    None
}

/// Parse the request body, handling both raw protobuf and Pyroscope SDK multipart/form-data.
///
/// The Pyroscope Go SDK sends `multipart/form-data` with a `profile` form file
/// containing the gzip-compressed PushRequest protobuf. This function detects the
/// content type and extracts/wraps the body appropriately.
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
        || content_type.contains("binary/octet-stream")
    {
        if body.is_empty() {
            return Err("empty body".to_string());
        }
        Ok(body.to_vec())
    } else {
        Err(format!(
            "unimplemented: Content-Type '{}' is not supported for Pyroscope ingest; \
             supported: 'multipart/form-data' (Go/Python SDK), 'application/proto' (gzip-compressed PushRequest)",
            content_type
        ))
    }
}

/// Decode a Pyroscope PushRequest (gzip-compressed protobuf) into an Arrow
/// RecordBatch using the same schema as transform_profiles.
pub async fn transform_pyroscope_profiles(body: Vec<u8>) -> Result<RecordBatch, Status> {
    // Stage 1: gzip decompression
    let decompressed = {
        let mut decoder = flate2::read::GzDecoder::new(&body[..]);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut out)
            .map_err(|e| status_internal(format!("gzip decompression failed: {e}")))?;
        out
    };
    tracing::debug!("pyroscope: decompressed {} bytes", decompressed.len());

    // Stage 2: PushRequest protobuf decode
    let push_request = pyroscope::encode::r#gen::push::PushRequest::decode(&decompressed[..])
        .map_err(|e| {
            tracing::debug!("pyroscope PushRequest decode failed: {:?}", e);
            Status::invalid_argument(format!("push request decode failed: {e}"))
        })?;
    tracing::debug!("pyroscope: series count={}", push_request.series.len());

    let schema = profile_schema();

    let mut profile_ids = StringBuilder::new();
    let mut service_names = StringBuilder::new();
    let mut timestamps = Vec::<i64>::new();
    let mut profile_types = StringBuilder::new();
    let mut stack_offsets: Vec<i32> = vec![0];
    let mut stack_values = StringBuilder::new();
    let mut values = Vec::<i64>::new();
    let mut units = StringBuilder::new();
    let mut resource_attrs = StringBuilder::new();
    let mut num_rows = 0usize;

    for series in &push_request.series {
        // Extract service.name from labels
        let service_name = series
            .labels
            .iter()
            .find(|l| l.name == "service_name")
            .map(|l| l.value.as_str())
            .unwrap_or("");

        for sample in &series.samples {
            // Decode the pprof Profile from raw_profile bytes.
            // Clone bytes first so we can move them into spawn_blocking without
            // borrowing from push_request (which would create a self-referential struct).
            let raw_bytes = sample.raw_profile.clone();
            let sample_id = sample.id.clone();
            let pprof_profile = tokio::task::spawn_blocking(move || {
                pyroscope::encode::r#gen::google::Profile::decode(&raw_bytes[..])
            })
            .await
            .map_err(|e| status_internal(format!("task join error: {e}")))?
            .map_err(|e| Status::invalid_argument(format!("pprof profile decode failed: {e}")))?;

            // Time and duration in nanoseconds → microseconds
            let ts_us = (pprof_profile.time_nanos / 1000) as i64;

            // Profile type and unit from sample_type[0]
            let (ptype_idx, punit_idx) = pprof_profile
                .sample_type
                .first()
                .map(|st| (st.r#type, st.unit))
                .unwrap_or((0, 0));

            let ptype = pprof_profile
                .string_table
                .get(ptype_idx as usize)
                .map(std::string::String::as_str)
                .unwrap_or("");
            let punit = pprof_profile
                .string_table
                .get(punit_idx as usize)
                .map(std::string::String::as_str)
                .unwrap_or("");

            // Build location_id → Location map
            let locations = &pprof_profile.location;
            let functions = &pprof_profile.function;
            let string_table = &pprof_profile.string_table;

            for s in &pprof_profile.sample {
                let stack: Vec<&str> = s
                    .location_id
                    .iter()
                    .filter_map(|&loc_id| {
                        let loc = locations.iter().find(|l| l.id == loc_id)?;
                        let line = loc.line.first()?;
                        let func = functions.iter().find(|f| f.id == line.function_id)?;
                        let name = string_table.get(func.name as usize)?;
                        Some(name.as_str())
                    })
                    .take(128)
                    .collect();

                let sample_values = if s.value.is_empty() {
                    vec![0i64]
                } else {
                    s.value.clone()
                };

                for &val in &sample_values {
                    profile_ids.append_value(&sample_id);
                    service_names.append_value(service_name);
                    timestamps.push(ts_us);
                    profile_types.append_value(ptype);

                    for frame in &stack {
                        stack_values.append_value(frame);
                    }
                    let prev_offset = *stack_offsets
                        .last()
                        .expect("stack_offsets always has an element");
                    stack_offsets.push(prev_offset + stack.len() as i32);

                    values.push(val);
                    units.append_value(punit);
                    resource_attrs.append_null();
                    num_rows += 1;
                }
            }
        }
    }

    if num_rows == 0 {
        return RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(
                    ListArray::try_new(
                        Arc::new(Field::new("item", DataType::Utf8, true)),
                        arrow::buffer::OffsetBuffer::new(vec![0i32].into()),
                        Arc::new(StringBuilder::new().finish()),
                        None,
                    )
                    .map_err(arrow_status)?,
                ),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
            ],
        )
        .map_err(arrow_status);
    }

    let stack_list = ListArray::try_new(
        Arc::new(Field::new("item", DataType::Utf8, true)),
        arrow::buffer::OffsetBuffer::new(stack_offsets.into()),
        Arc::new(stack_values.finish()),
        None,
    )
    .map_err(arrow_status)?;

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(profile_ids.finish()),
            Arc::new(service_names.finish()),
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(profile_types.finish()),
            Arc::new(stack_list),
            Arc::new(Int64Array::from(values)),
            Arc::new(units.finish()),
            Arc::new(resource_attrs.finish()),
        ],
    )
    .map_err(arrow_status)
}
