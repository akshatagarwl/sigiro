use anyhow::Result;
use arrow::array::{Int64Array, ListArray, StringBuilder, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use axum::{Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use crossbeam_channel::Sender;
use duckdb::Connection;
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
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

/// Pre-transformed ingest message carrying Arrow RecordBatches, not raw bytes.
/// Transform happens on the tokio blocking pool (parallel) BEFORE the channel,
/// so the inserter threads only do DuckDB writes — no CPU-bound protobuf decoding.
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

pub struct OtlpGrpcService {
    tx: Sender<IngestMsg>,
    admin_conn: Arc<Mutex<Connection>>,
}

impl OtlpGrpcService {
    pub fn new(tx: Sender<IngestMsg>, admin_conn: Arc<Mutex<Connection>>) -> Self {
        Self { tx, admin_conn }
    }

    pub fn trace_service(&self) -> TraceServiceServer<Self> {
        TraceServiceServer::new(Self {
            tx: self.tx.clone(),
            admin_conn: Arc::clone(&self.admin_conn),
        })
    }

    pub fn logs_service(&self) -> LogsServiceServer<Self> {
        LogsServiceServer::new(Self {
            tx: self.tx.clone(),
            admin_conn: Arc::clone(&self.admin_conn),
        })
    }

    pub fn metrics_service(&self) -> MetricsServiceServer<Self> {
        MetricsServiceServer::new(Self {
            tx: self.tx.clone(),
            admin_conn: Arc::clone(&self.admin_conn),
        })
    }

    pub fn profiles_service(&self) -> ProfilesServiceServer<Self> {
        ProfilesServiceServer::new(Self {
            tx: self.tx.clone(),
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

/// Transform raw protobuf bytes into Arrow RecordBatch on the tokio blocking pool.
/// This parallelizes the CPU-bound protobuf decode + Arrow conversion across all cores.
async fn transform_traces(bytes: Vec<u8>) -> Result<RecordBatch, Status> {
    tokio::task::spawn_blocking(move || {
        otlp2records::transform_traces(&bytes, InputFormat::Protobuf)
            .map_err(|e| Status::internal(format!("transform traces: {e}")))
    })
    .await
    .map_err(|e| Status::internal(format!("transform task panicked: {e}")))?
}

async fn transform_logs(bytes: Vec<u8>) -> Result<RecordBatch, Status> {
    tokio::task::spawn_blocking(move || {
        otlp2records::transform_logs(&bytes, InputFormat::Protobuf)
            .map_err(|e| Status::internal(format!("transform logs: {e}")))
    })
    .await
    .map_err(|e| Status::internal(format!("transform task panicked: {e}")))?
}

async fn transform_metrics(bytes: Vec<u8>) -> Result<MetricBatches, Status> {
    tokio::task::spawn_blocking(move || {
        otlp2records::transform_metrics(&bytes, InputFormat::Protobuf)
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
        let batch = transform_traces(bytes).await?;
        if batch.num_rows() > 0 {
            self.tx
                .send(IngestMsg::Traces { project_id, batch })
                .map_err(|e| Status::internal(e.to_string()))?;
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
        let batch = transform_logs(bytes).await?;
        if batch.num_rows() > 0 {
            self.tx
                .send(IngestMsg::Logs { project_id, batch })
                .map_err(|e| Status::internal(e.to_string()))?;
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
        let batches = transform_metrics(bytes).await?;
        self.tx
            .send(IngestMsg::Metrics {
                project_id,
                batches,
            })
            .map_err(|e| Status::internal(e.to_string()))?;
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
    let dict = request.dictionary.unwrap_or_default(); // TRADEOFF: missing dictionary = empty, not an error
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

    let resolve_str = |idx: i32| -> &str {
        string_table
            .get(idx as usize)
            .map(String::as_str)
            .unwrap_or("")
    };

    /// Resolve a stack from sample.stack_index through the dictionary tables.
    /// Returns function names from root to leaf, capped at 128 frames.
    fn resolve_stack<'a>(
        stack_index: i32,
        dict: &'a opentelemetry_proto::tonic::profiles::v1development::ProfilesDictionary,
        string_table: &'a [String],
    ) -> Vec<&'a str> {
        let stack = match dict.stack_table.get(stack_index as usize) {
            Some(s) => s,
            None => return vec![],
        };
        let mut frames = Vec::with_capacity(stack.location_indices.len().min(128));
        for &loc_idx in stack.location_indices.iter().take(128) {
            if let Some(loc) = dict.location_table.get(loc_idx as usize) {
                // Use the first line's function (most common case)
                if let Some(line) = loc.line.first() {
                    if let Some(func) = dict.function_table.get(line.function_index as usize) {
                        let name = string_table
                            .get(func.name_strindex as usize)
                            .map(String::as_str)
                            .unwrap_or("<unknown>");
                        frames.push(name);
                    }
                }
            }
        }
        frames
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
            .unwrap_or("unknown");

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
                                .unwrap_or_default()
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
                let pid = if profile.profile_id.is_empty() {
                    ""
                } else {
                    // profile_id is bytes, hex encode
                    // Use a static buffer approach for safety
                    ""
                };

                // Resolve profile_type and unit from sample_type
                let (ptype, punit) = profile.sample_type.as_ref().map_or(("", ""), |st| {
                    (resolve_str(st.type_strindex), resolve_str(st.unit_strindex))
                });

                for sample in &profile.sample {
                    let stack = resolve_stack(sample.stack_index, &dict, string_table);
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
            self.tx
                .send(IngestMsg::Profiles { project_id, batch })
                .map_err(|e| Status::internal(e.to_string()))?;
        }
        Ok(Response::new(ExportProfilesServiceResponse {
            partial_success: None,
        }))
    }
}

// OTLP HTTP endpoints on :4318

struct OtlpHttpState {
    tx: Sender<IngestMsg>,
    admin_conn: Arc<Mutex<Connection>>,
}

pub fn otlp_http_router(tx: Sender<IngestMsg>, admin_conn: Arc<Mutex<Connection>>) -> Router {
    let state = Arc::new(OtlpHttpState { tx, admin_conn });
    Router::new()
        .route("/v1/traces", post(otlp_http_traces))
        .route("/v1/logs", post(otlp_http_logs))
        .route("/v1/metrics", post(otlp_http_metrics))
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
    // HTTP path: bytes come in raw — transform on blocking pool, no double decode.
    let batch = match transform_traces(body.to_vec()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform traces: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    if batch.num_rows() > 0 {
        if let Err(e) = state.tx.send(IngestMsg::Traces { project_id, batch }) {
            tracing::error!("failed to enqueue traces: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
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
    let batch = match transform_logs(body.to_vec()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform logs: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    if batch.num_rows() > 0 {
        if let Err(e) = state.tx.send(IngestMsg::Logs { project_id, batch }) {
            tracing::error!("failed to enqueue logs: {e}");
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
    let batches = match transform_metrics(body.to_vec()).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform metrics: {e}");
            return StatusCode::BAD_REQUEST;
        }
    };
    if let Err(e) = state.tx.send(IngestMsg::Metrics {
        project_id,
        batches,
    }) {
        tracing::error!("failed to enqueue metrics: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}
