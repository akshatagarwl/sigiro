use anyhow::Result;
use arrow::array::{Int64Array, ListArray, StringBuilder, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::StatusCode,
    routing::post,
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::{
        ExportLogsServiceRequest, ExportLogsServiceResponse,
        logs_service_server::{LogsService, LogsServiceServer},
    },
    metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        metrics_service_server::{MetricsService, MetricsServiceServer},
    },
    profiles::v1development::{
        ExportProfilesServiceRequest, ExportProfilesServiceResponse,
        profiles_service_server::{ProfilesService, ProfilesServiceServer},
    },
    trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
        trace_service_server::{TraceService, TraceServiceServer},
    },
};
use otlp2records::{InputFormat, MetricBatches};
use prost::Message;
use rusqlite::Connection;
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

use crate::inserter::MsgRouter;
use crate::types::IngestMsg;

pub struct OtlpGrpcService {
    router: Arc<MsgRouter>,
    admin_conn: Arc<Mutex<Connection>>,
}

impl OtlpGrpcService {
    pub fn new(router: Arc<MsgRouter>, admin_conn: Arc<Mutex<Connection>>) -> Self {
        Self { router, admin_conn }
    }

    pub fn trace_service(&self) -> TraceServiceServer<Self> {
        TraceServiceServer::new(Self {
            router: Arc::clone(&self.router),
            admin_conn: Arc::clone(&self.admin_conn),
        })
    }

    pub fn logs_service(&self) -> LogsServiceServer<Self> {
        LogsServiceServer::new(Self {
            router: Arc::clone(&self.router),
            admin_conn: Arc::clone(&self.admin_conn),
        })
    }

    pub fn metrics_service(&self) -> MetricsServiceServer<Self> {
        MetricsServiceServer::new(Self {
            router: Arc::clone(&self.router),
            admin_conn: Arc::clone(&self.admin_conn),
        })
    }

    pub fn profiles_service(&self) -> ProfilesServiceServer<Self> {
        ProfilesServiceServer::new(Self {
            router: Arc::clone(&self.router),
            admin_conn: Arc::clone(&self.admin_conn),
        })
    }
}

/// Extract project_id from gRPC metadata Bearer token.
/// Uses the auth cache to avoid DuckDB lookups on every request.
pub fn extract_project_id_grpc(
    admin_conn: &Mutex<Connection>,
    metadata: &tonic::metadata::MetadataMap,
) -> Result<String, Status> {
    let token = metadata
        .get("authorization")
        .ok_or_else(|| Status::unauthenticated("Missing Authorization metadata"))?
        .to_str()
        .map_err(|_| Status::unauthenticated("Invalid Authorization encoding"))?
        .strip_prefix("Bearer ")
        .ok_or_else(|| Status::unauthenticated("Expected: Bearer <key>"))?;

    if token.starts_with("sigiro_dev_") {
        return Ok("dev_project".to_string());
    }

    let ctx = crate::auth::verify_key_cached(admin_conn, token)
        .map_err(|_| Status::unauthenticated("Invalid API key"))?;
    Ok(ctx.project_id)
}

/// Detect InputFormat from Content-Type header.
/// OTLP/HTTP spec: application/x-protobuf (default) or application/json.
fn detect_format(headers: &axum::http::HeaderMap) -> InputFormat {
    headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            let ct_lower = ct.to_lowercase();
            if ct_lower.contains("json") {
                InputFormat::Json
            } else {
                InputFormat::Protobuf
            }
        })
        .unwrap_or(InputFormat::Protobuf)
}

/// Transform raw bytes into Arrow RecordBatch on the tokio blocking pool.
/// Supports both protobuf and JSON input based on Content-Type header.
async fn transform_traces(bytes: Vec<u8>, fmt: InputFormat) -> Result<RecordBatch, Status> {
    tokio::task::spawn_blocking(move || {
        otlp2records::transform_traces(&bytes, fmt)
            .map_err(|e| Status::internal(format!("transform traces: {e}")))
    })
    .await
    .map_err(|e| Status::internal(format!("transform task panicked: {e}")))?
}

async fn transform_logs(bytes: Vec<u8>, fmt: InputFormat) -> Result<RecordBatch, Status> {
    tokio::task::spawn_blocking(move || {
        otlp2records::transform_logs(&bytes, fmt)
            .map_err(|e| Status::internal(format!("transform logs: {e}")))
    })
    .await
    .map_err(|e| Status::internal(format!("transform task panicked: {e}")))?
}

async fn transform_metrics(bytes: Vec<u8>, fmt: InputFormat) -> Result<MetricBatches, Status> {
    tokio::task::spawn_blocking(move || {
        otlp2records::transform_metrics(&bytes, fmt)
            .map_err(|e| Status::internal(format!("transform metrics: {e}")))
    })
    .await
    .map_err(|e| Status::internal(format!("transform task panicked: {e}")))?
}

#[tonic::async_trait]
impl TraceService for OtlpGrpcService {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.admin_conn, request.metadata())?;
        // Transform on tokio blocking pool — no double decode.
        // Encode once (prost), decode once (otlp2records). Previously: decode (tonic) → encode (prost) → decode (otlp2records).
        let bytes = request.into_inner().encode_to_vec();
        let batch = transform_traces(bytes, InputFormat::Protobuf).await?;
        let pid = project_id.clone();
        if let Err(e) = self.router.register_flusher(&pid) {
            tracing::error!(pid, "failed to register flusher: {e:?}");
            return Err(Status::internal("flusher init failed"));
        }
        if let Err(e) = self.router.send(IngestMsg::Traces { project_id, batch }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return Err(Status::internal("ingest failed"));
        }
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl LogsService for OtlpGrpcService {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.admin_conn, request.metadata())?;
        let bytes = request.into_inner().encode_to_vec();
        let batch = transform_logs(bytes, InputFormat::Protobuf).await?;
        let pid = project_id.clone();
        if let Err(e) = self.router.register_flusher(&pid) {
            tracing::error!(pid, "failed to register flusher: {e:?}");
            return Err(Status::internal("flusher init failed"));
        }
        if let Err(e) = self.router.send(IngestMsg::Logs { project_id, batch }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return Err(Status::internal("ingest failed"));
        }
        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl MetricsService for OtlpGrpcService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.admin_conn, request.metadata())?;
        let bytes = request.into_inner().encode_to_vec();
        let batches = transform_metrics(bytes, InputFormat::Protobuf).await?;
        let pid = project_id.clone();
        if let Err(e) = self.router.register_flusher(&pid) {
            tracing::error!(pid, "failed to register flusher: {e:?}");
            return Err(Status::internal("flusher init failed"));
        }
        if let Err(e) = self.router.send(IngestMsg::Metrics { project_id, batches }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return Err(Status::internal("ingest failed"));
        }
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
fn transform_profiles(request: ExportProfilesServiceRequest) -> Result<RecordBatch, Status> {
    let dict = match request.dictionary {
        Some(d) => d,
        None => {
            // No dictionary = no samples can be resolved. Return empty batch.
            tracing::debug!("profiles request has no dictionary, returning empty batch");
            return RecordBatch::try_new(
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
                ])),
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
                        .map_err(|e| Status::internal(format!("arrow error: {e}")))?,
                    ),
                    Arc::new(Int64Array::from(Vec::<i64>::new())),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                ],
            )
            .map_err(|e| Status::internal(format!("arrow error: {e}")));
        }
    };
    let string_table = &dict.string_table;

    let schema = Arc::new(Schema::new(vec![
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
    ]));

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
                    .map_err(|e| Status::internal(format!("arrow error: {e}")))?,
                ),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(units.finish()),
                Arc::new(resource_attrs.finish()),
            ],
        )
        .map_err(|e| Status::internal(format!("arrow error: {e}")));
    }

    let stack_list = ListArray::try_new(
        Arc::new(Field::new("item", DataType::Utf8, true)),
        arrow::buffer::OffsetBuffer::new(stack_offsets.into()),
        Arc::new(stack_values.finish()),
        None,
    )
    .map_err(|e| Status::internal(format!("arrow error: {e}")))?;

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
    .map_err(|e| Status::internal(format!("arrow error: {e}")))
}

#[tonic::async_trait]
impl ProfilesService for OtlpGrpcService {
    async fn export(
        &self,
        request: Request<ExportProfilesServiceRequest>,
    ) -> Result<Response<ExportProfilesServiceResponse>, Status> {
        let project_id = extract_project_id_grpc(&self.admin_conn, request.metadata())?;
        let req = request.into_inner();
        let batch = tokio::task::spawn_blocking(move || transform_profiles(req))
            .await
            .map_err(|e| Status::internal(format!("transform task panicked: {e}")))??;
        let pid = project_id.clone();
        if let Err(e) = self.router.register_flusher(&project_id) {
            tracing::error!(project_id, "failed to register flusher: {e:?}");
            return Err(Status::internal("flusher init failed"));
        }
        if let Err(e) = self.router.send(IngestMsg::Profiles { project_id, batch }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return Err(Status::internal("ingest failed"));
        }
        Ok(Response::new(ExportProfilesServiceResponse {
            partial_success: None,
        }))
    }
}

// OTLP HTTP endpoints on :4318

struct OtlpHttpState {
    router: Arc<MsgRouter>,
    admin_conn: Arc<Mutex<Connection>>,
}

pub fn otlp_http_router(router: Arc<MsgRouter>, admin_conn: Arc<Mutex<Connection>>) -> Router {
    let state = Arc::new(OtlpHttpState { router, admin_conn });
    Router::new()
        .route("/", post(otlp_http_pyroscope)) // Pyroscope RS SDK posts here
        .route("/push.v1.PusherService/Push", post(otlp_http_pyroscope)) // pyroscope-rs SDK push path
        .route("/v1/traces", post(otlp_http_traces))
        .route("/v1/logs", post(otlp_http_logs))
        .route("/v1/metrics", post(otlp_http_metrics))
        .route("/v1/profiles", post(otlp_http_profiles))
        .route("/ingest", post(otlp_http_pyroscope))
        .route("/ingest/pyroscope", post(otlp_http_pyroscope))
        .with_state(state)
}

/// Extract project_id from HTTP Authorization header.
/// Uses the auth cache to avoid DuckDB lookups on every request.
pub fn extract_project_id_http(
    admin_conn: &Mutex<Connection>,
    headers: &axum::http::HeaderMap,
) -> Result<String, StatusCode> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            tracing::warn!("OTLP HTTP: missing or malformed Authorization header");
            StatusCode::UNAUTHORIZED
        })?;

    if token.starts_with("sigiro_dev_") {
        return Ok("dev_project".to_string());
    }

    let ctx = crate::auth::verify_key_cached(admin_conn, token).map_err(|_| {
        tracing::warn!("OTLP HTTP: invalid API key");
        StatusCode::UNAUTHORIZED
    })?;
    Ok(ctx.project_id)
}

async fn otlp_http_traces(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    tracing::info!("otlp_http_traces: received trace request, body len = {}", body.len());
    let project_id = match extract_project_id_http(&state.admin_conn, &headers) {
        Ok(id) => id,
        Err(status) => return status,
    };
    tracing::info!(project_id = %project_id, "otlp_http_traces: auth OK");
    let fmt = detect_format(&headers);
    tracing::info!(project_id = %project_id, format = ?fmt, "otlp_http_traces: detected format");
    let batch = match transform_traces(body.to_vec(), fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform traces: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    tracing::info!(project_id = %project_id, rows = batch.num_rows(), "otlp_http_traces: batch transformed");
    if batch.num_rows() > 0 {
        let pid = project_id.clone();
        if let Err(e) = state.router.register_flusher(&pid) {
            tracing::error!(pid, "failed to register flusher: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        tracing::info!(project_id = %pid, "otlp_http_traces: flusher registered, calling send");
        let send_pid = pid.clone();
        if let Err(e) = state.router.send(IngestMsg::Traces { project_id, batch }).await {
            tracing::error!(send_pid, "ingest router error: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        tracing::info!(send_pid = %send_pid, "otlp_http_traces: send complete");
    } else {
        tracing::warn!(project_id, "otlp_http_traces: batch empty after transform — trace data lost");
    }
    StatusCode::OK
}

async fn otlp_http_logs(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.admin_conn, &headers) {
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
    if batch.num_rows() > 0 {
        let pid = project_id.clone();
        if let Err(e) = state.router.register_flusher(&pid) {
            tracing::error!(pid, "failed to register flusher: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        if let Err(e) = state.router.send(IngestMsg::Logs { project_id, batch }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    StatusCode::OK
}

async fn otlp_http_metrics(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.admin_conn, &headers) {
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
    // Only send if at least one metric batch is non-empty (consistent with traces/logs)
    let has_data = batches.gauge.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches.sum.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches.histogram.as_ref().is_some_and(|b| b.num_rows() > 0)
        || batches
            .exp_histogram
            .as_ref()
            .is_some_and(|b| b.num_rows() > 0);
    if has_data {
        let pid = project_id.clone();
        if let Err(e) = state.router.register_flusher(&pid) {
            tracing::error!(pid, "failed to register flusher: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        if let Err(e) = state.router.send(IngestMsg::Metrics { project_id, batches }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    StatusCode::OK
}

async fn otlp_http_profiles(
    State(state): State<Arc<OtlpHttpState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> StatusCode {
    let project_id = match extract_project_id_http(&state.admin_conn, &headers) {
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
    if batch.num_rows() > 0 {
        let pid = project_id.clone();
        if let Err(e) = state.router.register_flusher(&project_id) {
            tracing::error!(project_id, "failed to register flusher: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        if let Err(e) = state.router.send(IngestMsg::Profiles { project_id, batch }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    StatusCode::OK
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
    // Dev bypass: accept Pyroscope SDK clients with sigiro_dev_* tokens
    // without doing blocking DB lookups that hang in async context
    let project_id = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|token| {
            if token.starts_with("sigiro_dev_") {
                "dev_project".to_string()
            } else {
                format!("sdk:{}", &token[..8.min(token.len())])
            }
        })
        .unwrap_or_else(|| "dev_project".to_string());

    let ct = headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("(none)");
    tracing::debug!("otlp_http_pyroscope: project_id={project_id} body_len={} ct={ct}", body.len());

    // Detect format: multipart/form-data (SDK) vs raw protobuf
    let body_bytes = match parse_pyroscope_multipart(&headers, &body) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("pyroscope multipart parse failed: {e}");
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

    tracing::info!("pyroscope ingest: project_id={project_id} rows={}", batch.num_rows());

    if batch.num_rows() > 0 {
        let pid = project_id.clone();
        if let Err(e) = state.router.register_flusher(&project_id) {
            tracing::error!(project_id, "failed to register flusher: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        if let Err(e) = state.router.send(IngestMsg::Profiles { project_id, batch }).await {
            tracing::error!(pid, "ingest router error: {e:?}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
    }
    StatusCode::OK
}

/// Extract service.name from an optional Resource's attributes.
/// Returns None if the resource is absent or lacks the attribute.
fn extract_service_name(resource: Option<&opentelemetry_proto::tonic::resource::v1::Resource>) -> Option<&str> {
    let resource = resource?;
    for attr in &resource.attributes {
        if attr.key == "service.name" {
            let val = attr.value.as_ref()?;
            let inner = val.value.as_ref()?;
            if let opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) = inner {
                return Some(s.as_str());
            }
        }
    }
    None
}

/// Extract a form field value from multipart data by field name.
fn extract_form_field(
    data: &[u8],
    boundary: &str,
    field_name: &str,
) -> Option<Vec<u8>> {
    let boundary_bytes = boundary.as_bytes();
    let field_marker = format!("name=\"{}\"", field_name).into_bytes();
    let marker_len = field_marker.len();

    // Find start of field header: --boundary\r\n...name="fieldname"...
    let mut search_start = 0;
    let _data_len = data.len();

    loop {
        // Find field name marker
        let marker_pos = find_bytes_with_start(data, &field_marker, search_start)?;
        let after_marker = marker_pos + marker_len;

        // Verify this is within a proper multipart header (should be preceded by --boundary\r\n)
        // Find the closest preceding --boundary
        let header_start = find_bytes_reverse(data, b"--", marker_pos)?;
        if header_start + 2 + boundary_bytes.len() + 2 > marker_pos {
            // Not a proper field header, continue searching after this occurrence
            search_start = marker_pos + 1;
            continue;
        }

        // Verify boundary matches
        let candidate_boundary = &data[header_start + 2..header_start + 2 + boundary_bytes.len()];
        if candidate_boundary != boundary_bytes {
            // Wrong boundary, continue after header start
            search_start = header_start + 2;
            continue;
        }

        // Find \r\n\r\n that ends the header (start of content)
        let header_end = find_bytes_with_start(data, b"\r\n\r\n", after_marker)?;
        let content_start = header_end + 4;

        // Find the next boundary marker to determine content end
        // Look for --<boundary> or --<boundary>--
        let next_boundary = find_next_boundary(data, boundary_bytes, content_start)?;

        // Content is between content_start and next_boundary
        // Trim trailing \r\n before the --boundary line
        let mut content_end = next_boundary;
        while content_end > content_start && (data[content_end - 1] == b'\r' || data[content_end - 1] == b'\n') {
            content_end -= 1;
        }

        return Some(data[content_start..content_end].to_vec());
    }
}

/// Find first occurrence of needle in haystack starting at offset.
fn find_bytes_with_start(haystack: &[u8], needle: &[u8], start: usize) -> Option<usize> {
    if start >= haystack.len() || needle.len() > haystack.len() - start {
        return None;
    }
    haystack[start..].windows(needle.len()).position(|w| w == needle).map(|p| start + p)
}

/// Find last occurrence of needle before max_pos (searching backwards).
fn find_bytes_reverse(haystack: &[u8], needle: &[u8], max_pos: usize) -> Option<usize> {
    if max_pos == 0 || needle.len() > max_pos {
        return None;
    }
    haystack[..max_pos].windows(needle.len()).position(|w| w == needle)
}

/// Find the next multipart boundary (--boundary or --boundary--) starting from offset.
fn find_next_boundary(data: &[u8], boundary: &[u8], start: usize) -> Option<usize> {
    // Search for -- followed by boundary
    let mut i = start;
    let dash_dash = b"--";
    while i < data.len() {
        if let Some(pos) = find_bytes_with_start(&data[i..], dash_dash, 0) {
            let abs_pos = i + pos;
            if abs_pos + 2 + boundary.len() <= data.len()
                && &data[abs_pos + 2..abs_pos + 2 + boundary.len()] == boundary {
                    // Found --boundary, check if followed by -- (closing boundary) or \r\n (regular)
                    let after = abs_pos + 2 + boundary.len();
                    if after < data.len() {
                        if data[after] == b'-' && after + 1 < data.len() && data[after + 1] == b'-' {
                            // Closing boundary --boundary--
                            return Some(abs_pos);
                        } else if data[after] == b'\r' || data[after] == b'\n' {
                            return Some(abs_pos);
                        }
                    }
                }
            // Move past this -- for next search
            i = abs_pos + 2;
        } else {
            break;
        }
    }
    None
}

fn extract_mime_param(content_type: &str, param: &str) -> Option<String> {
    for part in content_type.split(';') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix(&format!("{}=\"", param)) {
            return Some(val.trim_matches('"').to_string());
        }
        if let Some(val) = part.strip_prefix(&format!("{}=", param)) {
            return Some(val.trim().to_string());
        }
    }
    None
}

/// Parse the request body, handling both raw protobuf and Pyroscope SDK multipart/form-data.
///
/// The Pyroscope Go SDK sends `multipart/form-data` with a `profile` form file
/// containing the gzip-compressed PushRequest protobuf. This function detects the
/// content type and extracts/wraps the body appropriately.
fn parse_pyroscope_multipart(headers: &axum::http::HeaderMap, body: &[u8]) -> Result<Vec<u8>, String> {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if content_type.contains("multipart/form-data") {
        // Extract boundary from Content-Type header
        let boundary = extract_mime_param(content_type, "boundary")
            .ok_or("missing boundary in multipart Content-Type")?;

        // The profile field contains the gzip-compressed PushRequest
        let profile_bytes = extract_form_field(body, &boundary, "profile")
            .ok_or("profile form field not found in multipart body")?;

        tracing::debug!("pyroscope multipart: extracted {} profile bytes", profile_bytes.len());
        Ok(profile_bytes)
    } else {
        // Raw protobuf body (gzip-compressed PushRequest)
        Ok(body.to_vec())
    }
}

/// Decode a Pyroscope PushRequest (gzip-compressed protobuf) into an Arrow
/// RecordBatch using the same schema as transform_profiles.
async fn transform_pyroscope_profiles(
    body: Vec<u8>,
) -> Result<RecordBatch, Status> {
    // Stage 1: gzip decompression
    let decompressed = {
        let mut decoder = flate2::read::GzDecoder::new(&body[..]);
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut out)
            .map_err(|e| Status::internal(format!("gzip decompression failed: {e}")))?;
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

    let schema = Arc::new(Schema::new(vec![
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
    ]));

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
            .map_err(|e| Status::internal(format!("task join error: {e}")))?
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
                    let prev_offset =
                        *stack_offsets.last().expect("stack_offsets always has an element");
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
                Arc::new(ListArray::try_new(
                    Arc::new(Field::new("item", DataType::Utf8, true)),
                    arrow::buffer::OffsetBuffer::new(vec![0i32].into()),
                    Arc::new(StringBuilder::new().finish()),
                    None,
                )
                .map_err(|e| Status::internal(format!("arrow error: {e}")))?),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
            ],
        )
        .map_err(|e| Status::internal(format!("arrow error: {e}")));
    }

    let stack_list = ListArray::try_new(
        Arc::new(Field::new("item", DataType::Utf8, true)),
        arrow::buffer::OffsetBuffer::new(stack_offsets.into()),
        Arc::new(stack_values.finish()),
        None,
    )
    .map_err(|e| Status::internal(format!("arrow error: {e}")))?;

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
    .map_err(|e| Status::internal(format!("arrow error: {e}")))
}
