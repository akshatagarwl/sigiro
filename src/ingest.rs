use anyhow::Result;
use arrow::array::{Int64Array, ListArray, StringBuilder, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use axum::{Router, body::Bytes, extract::State, http::StatusCode, routing::post};
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
        if batch.num_rows() > 0 {
            self.router
                .send(IngestMsg::Traces { project_id, batch })
                .await;
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
        if batch.num_rows() > 0 {
            self.router
                .send(IngestMsg::Logs { project_id, batch })
                .await;
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
        self.router
            .send(IngestMsg::Metrics {
                project_id,
                batches,
            })
            .await;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

/// Resolve pprofile dictionary encoding into flat profile rows as a RecordBatch.
///
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
        let service_name = rp
            .resource
            .as_ref()
            .and_then(|r| {
                r.attributes.iter().find_map(|a| {
                    if a.key == "service.name" {
                        a.value.as_ref().and_then(|v| {
                            v.value.as_ref().and_then(|val| match val {
                                opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) => Some(s.as_str()),
                                _ => None,
                            })
                        })
                    } else {
                        None
                    }
                })
            })
            .ok_or_else(|| {
                Status::invalid_argument(
                    "resource_profiles entry missing service.name attribute — every profile must identify its service",
                )
            })?;

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
        if batch.num_rows() > 0 {
            self.router
                .send(IngestMsg::Profiles { project_id, batch })
                .await;
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
        .route("/v1/traces", post(otlp_http_traces))
        .route("/v1/logs", post(otlp_http_logs))
        .route("/v1/metrics", post(otlp_http_metrics))
        .route("/v1/profiles", post(otlp_http_profiles))
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
    let project_id = match extract_project_id_http(&state.admin_conn, &headers) {
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
    if batch.num_rows() > 0 {
        state
            .router
            .send(IngestMsg::Traces { project_id, batch })
            .await;
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
        state
            .router
            .send(IngestMsg::Logs { project_id, batch })
            .await;
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
    state
        .router
        .send(IngestMsg::Metrics {
            project_id,
            batches,
        })
        .await;
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
        state
            .router
            .send(IngestMsg::Profiles { project_id, batch })
            .await;
    }
    StatusCode::OK
}
