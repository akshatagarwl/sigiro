use crate::otlp::{InputFormat, MetricBatches};
use anyhow::Result;
use arrow::array::{Int64Array, ListArray, ListBuilder, StringBuilder, TimestampMicrosecondArray};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use axum::{
    Router,
    body::Bytes,
    extract::{Query, State},
    http::StatusCode,
    routing::post,
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
use serde::Deserialize;
use std::io::Read;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tracing_attributes::instrument;

use crate::inserter::{MsgRouter, RouterError};
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
/// Uses the explicit AuthMode — Open mode skips auth, TenantKeys mode validates Bearer token.
pub fn extract_project_id_grpc(
    auth_mode: &crate::auth::AuthMode,
    metadata: &tonic::metadata::MetadataMap,
) -> Result<String, Status> {
    match auth_mode {
        crate::auth::AuthMode::Open => Ok("default".to_string()),
        crate::auth::AuthMode::TenantKeys { .. } => {
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
            Ok(ctx.tenant_id)
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
async fn transform_traces(bytes: Bytes, fmt: InputFormat) -> Result<RecordBatch, Status> {
    spawn_transform("transform traces", move || {
        crate::otlp::transform_traces(&bytes, fmt)
    })
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/logs"))]
async fn transform_logs(bytes: Bytes, fmt: InputFormat) -> Result<(RecordBatch, u64), Status> {
    spawn_transform("transform logs", move || {
        crate::otlp::transform_logs(&bytes, fmt)
    })
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1/metrics"))]
async fn transform_metrics(bytes: Bytes, fmt: InputFormat) -> Result<MetricBatches, Status> {
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
        let bytes = Bytes::from(request.into_inner().encode_to_vec());
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
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata())?;
        let bytes = Bytes::from(request.into_inner().encode_to_vec());
        let (batch, rejected) = transform_logs(bytes, InputFormat::Protobuf).await?;
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
        let project_id = extract_project_id_grpc(&self.auth_mode, request.metadata())?;
        let bytes = Bytes::from(request.into_inner().encode_to_vec());
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

/// Resolve pprofile dictionary encoding into flat profile rows as a RecordBatch.
/// Resolution chain:
///   Sample.stack_index → Dictionary.stack_table[].location_indices[]
///   → Dictionary.location_table[].line[].function_index
///   → Dictionary.function_table[].name_strindex
///   → Dictionary.string_table[]
fn profile_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("profile_id", DataType::Utf8, true),
        Field::new("trace_id", DataType::Utf8, true),
        Field::new("span_id", DataType::Utf8, true),
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

fn stack_trace_builder() -> ListBuilder<StringBuilder> {
    ListBuilder::new(StringBuilder::new()).with_field(Arc::new(Field::new(
        "item",
        DataType::Utf8,
        true,
    )))
}

fn append_stack_trace<S: AsRef<str>>(builder: &mut ListBuilder<StringBuilder>, stack: &[S]) {
    for frame in stack {
        builder.values().append_value(frame.as_ref());
    }
    builder.append(true);
}

fn empty_stack_trace_array() -> ListArray {
    stack_trace_builder().finish()
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
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                    Arc::new(StringBuilder::new().finish()),
                    Arc::new(empty_stack_trace_array()),
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
    let mut trace_ids = StringBuilder::new();
    let mut span_ids = StringBuilder::new();
    let mut service_names = StringBuilder::new();
    let mut timestamps = Vec::new();
    let mut profile_types = StringBuilder::new();
    let mut stack_traces = stack_trace_builder();
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

    fn resolve_link(
        link_index: i32,
        dict: &opentelemetry_proto::tonic::profiles::v1development::ProfilesDictionary,
    ) -> Result<Option<(String, String)>, Status> {
        if link_index == 0 {
            return Ok(None);
        }

        let link_index = usize::try_from(link_index).map_err(|_| {
            Status::invalid_argument(format!(
                "link_table index {link_index} must not be negative"
            ))
        })?;
        let link = dict.link_table.get(link_index).ok_or_else(|| {
            Status::invalid_argument(format!(
                "link_table index {link_index} out of bounds (table has {} entries)",
                dict.link_table.len()
            ))
        })?;
        if link.trace_id.len() != 16 {
            return Err(Status::invalid_argument(format!(
                "profile link trace_id at index {link_index} must be 16 bytes, got {}",
                link.trace_id.len()
            )));
        }
        if link.span_id.len() != 8 {
            return Err(Status::invalid_argument(format!(
                "profile link span_id at index {link_index} must be 8 bytes, got {}",
                link.span_id.len()
            )));
        }
        if link.trace_id.iter().all(|b| *b == 0) {
            return Err(Status::invalid_argument(format!(
                "profile link trace_id at index {link_index} must not be all zeros"
            )));
        }
        if link.span_id.iter().all(|b| *b == 0) {
            return Err(Status::invalid_argument(format!(
                "profile link span_id at index {link_index} must not be all zeros"
            )));
        }

        Ok(Some((
            hex::encode(&link.trace_id),
            hex::encode(&link.span_id),
        )))
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
                    let link = resolve_link(sample.link_index, &dict)?;
                    let sample_values = if sample.values.is_empty() {
                        vec![0i64]
                    } else {
                        sample.values.clone()
                    };

                    // For multi-value profiles, write one row per value
                    for &val in &sample_values {
                        profile_ids.append_value(pid);
                        match &link {
                            Some((trace_id, span_id)) => {
                                trace_ids.append_value(trace_id);
                                span_ids.append_value(span_id);
                            }
                            None => {
                                trace_ids.append_null();
                                span_ids.append_null();
                            }
                        }
                        service_names.append_value(service_name);
                        timestamps.push(ts_us);
                        profile_types.append_value(ptype);
                        append_stack_trace(&mut stack_traces, &stack);

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
                Arc::new(trace_ids.finish()),
                Arc::new(span_ids.finish()),
                Arc::new(service_names.finish()),
                Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                Arc::new(profile_types.finish()),
                Arc::new(stack_traces.finish()),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(units.finish()),
                Arc::new(resource_attrs.finish()),
            ],
        )
        .map_err(arrow_status);
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(profile_ids.finish()),
            Arc::new(trace_ids.finish()),
            Arc::new(span_ids.finish()),
            Arc::new(service_names.finish()),
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(profile_types.finish()),
            Arc::new(stack_traces.finish()),
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
        self.route_batch(IngestMsg::Profiles { project_id, batch })
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
        .route("/", post(otlp_http_pyroscope)) // Pyroscope RS SDK posts here
        .route("/push.v1.PusherService/Push", post(otlp_http_pyroscope)) // pyroscope-rs SDK push path
        .route("/v1/traces", post(otlp_http_traces))
        .route("/v1/logs", post(otlp_http_logs))
        .route("/v1/metrics", post(otlp_http_metrics))
        .route("/v1development/profiles", post(otlp_http_profiles))
        .route("/ingest", post(otlp_http_pyroscope))
        .route("/ingest/pyroscope", post(otlp_http_pyroscope))
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
/// Uses the explicit AuthMode — Open mode skips auth, TenantKeys mode validates Bearer token.
pub fn extract_project_id_http(
    auth_mode: &crate::auth::AuthMode,
    headers: &axum::http::HeaderMap,
) -> Result<String, StatusCode> {
    match auth_mode {
        crate::auth::AuthMode::Open => Ok("default".to_string()),
        crate::auth::AuthMode::TenantKeys { .. } => {
            let auth: Authorization<Bearer> = headers.typed_get().ok_or_else(|| {
                tracing::warn!("OTLP HTTP: missing or malformed Authorization header");
                StatusCode::UNAUTHORIZED
            })?;
            let token = auth.token();

            let ctx = auth_mode.verify(token).map_err(|_| {
                tracing::warn!("OTLP HTTP: invalid API key");
                StatusCode::UNAUTHORIZED
            })?;
            Ok(ctx.tenant_id)
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
            RouterError::DiskCapacityExceeded => StatusCode::INSUFFICIENT_STORAGE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
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
    let batch = match transform_traces(body, fmt).await {
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
    route_ingest_msg(&state, IngestMsg::Traces { project_id, batch }).await
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
    let (batch, rejected) = match transform_logs(body, fmt).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to transform logs: {e}");
            return StatusCode::BAD_REQUEST;
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
    route_ingest_msg(&state, IngestMsg::Logs { project_id, batch }).await
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
    let batches = match transform_metrics(body, fmt).await {
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
        IngestMsg::Metrics {
            project_id,
            batches,
        },
    )
    .await
}

#[instrument(skip_all, fields(endpoint = "/v1development/profiles"))]
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
    route_ingest_msg(&state, IngestMsg::Profiles { project_id, batch }).await
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
/// Accepts two formats:
/// - `application/proto` (gzip-compressed PushRequest) — Pyroscope Rust SDK
/// - `multipart/form-data` (Pyroscope Node SDK format) — form file "profile" contains raw pprof
#[derive(Default, Deserialize)]
struct PyroscopeIngestQuery {
    name: Option<String>,
}

async fn otlp_http_pyroscope(
    State(state): State<Arc<OtlpHttpState>>,
    Query(query): Query<PyroscopeIngestQuery>,
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

    let batch =
        match transform_pyroscope_profiles(body_bytes, query.name.as_deref(), state.max_body_bytes)
            .await
        {
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
    if let Err(e) = state
        .router
        .send(IngestMsg::Profiles { project_id, batch })
        .await
    {
        tracing::error!(pid, "ingest router error: {e:?}");
        return match e {
            RouterError::DiskCapacityExceeded => StatusCode::INSUFFICIENT_STORAGE,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
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
        || content_type.contains("binary/octet-stream")
    {
        if body.is_empty() {
            return Err("empty body".to_string());
        }
        Ok(body.to_vec())
    } else {
        Err(format!(
            "unimplemented: Content-Type '{}' is not supported for Pyroscope ingest; \
             supported: 'multipart/form-data' (SDK pprof), 'application/proto' (gzip-compressed PushRequest)",
            content_type
        ))
    }
}

/// Decode either a Pyroscope PushRequest or an SDK raw pprof profile into an
/// Arrow RecordBatch using the same schema as transform_profiles.
pub async fn transform_pyroscope_profiles(
    body: Vec<u8>,
    sdk_name: Option<&str>,
    max_profile_bytes: usize,
) -> Result<RecordBatch, Status> {
    let decompressed = decompress_profile(body, max_profile_bytes, "Pyroscope request")?;
    tracing::debug!("pyroscope: payload {} bytes", decompressed.len());

    // Stage 2: Node sends raw pprof with the application name in the query;
    // pyroscope-rs sends a PushRequest whose series already carries service_name.
    let push_request = if let Some(name) = sdk_name {
        let service_name = name.split_once('{').map_or(name, |(base, _)| base).trim();
        if service_name.is_empty() {
            return Err(Status::invalid_argument(
                "Pyroscope SDK name must contain a non-empty application name",
            ));
        }
        pyroscope::encode::r#gen::push::PushRequest {
            series: vec![pyroscope::encode::r#gen::push::RawProfileSeries {
                labels: vec![pyroscope::encode::r#gen::types::LabelPair {
                    name: "service_name".to_string(),
                    value: service_name.to_string(),
                }],
                samples: vec![pyroscope::encode::r#gen::push::RawSample {
                    raw_profile: decompressed,
                    id: String::new(),
                }],
            }],
        }
    } else {
        pyroscope::encode::r#gen::push::PushRequest::decode(&decompressed[..]).map_err(|e| {
            tracing::debug!("pyroscope PushRequest decode failed: {:?}", e);
            Status::invalid_argument(format!("push request decode failed: {e}"))
        })?
    };
    tracing::debug!("pyroscope: series count={}", push_request.series.len());

    let schema = profile_schema();

    let mut profile_ids = StringBuilder::new();
    let mut trace_ids = StringBuilder::new();
    let mut span_ids = StringBuilder::new();
    let mut service_names = StringBuilder::new();
    let mut timestamps = Vec::<i64>::new();
    let mut profile_types = StringBuilder::new();
    let mut stack_traces = stack_trace_builder();
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
            let pprof_profile = tokio::task::spawn_blocking(move || -> Result<_, Status> {
                let decompressed =
                    decompress_profile(raw_bytes, max_profile_bytes, "embedded pprof profile")?;
                pyroscope::encode::r#gen::google::Profile::decode(&decompressed[..]).map_err(|e| {
                    Status::invalid_argument(format!("pprof profile decode failed: {e}"))
                })
            })
            .await
            .map_err(|e| status_internal(format!("task join error: {e}")))??;

            // Time and duration in nanoseconds → microseconds
            let ts_us = (pprof_profile.time_nanos / 1000) as i64;

            if service_name.is_empty() {
                return Err(Status::invalid_argument(
                    "Pyroscope series is missing the service_name label",
                ));
            }

            let resolve_string = |index: i64, field: &str| -> Result<&str, Status> {
                let index = usize::try_from(index).map_err(|_| {
                    Status::invalid_argument(format!(
                        "pprof {field} string index {index} must not be negative"
                    ))
                })?;
                pprof_profile
                    .string_table
                    .get(index)
                    .map(String::as_str)
                    .ok_or_else(|| {
                        Status::invalid_argument(format!(
                            "pprof {field} string index {index} out of bounds (table has {} entries)",
                            pprof_profile.string_table.len()
                        ))
                    })
            };

            let sample_types = pprof_profile
                .sample_type
                .iter()
                .enumerate()
                .map(|(index, sample_type)| {
                    let profile_type =
                        resolve_string(sample_type.r#type, &format!("sample_type[{index}].type"))?;
                    let unit =
                        resolve_string(sample_type.unit, &format!("sample_type[{index}].unit"))?;
                    if profile_type.is_empty() || unit.is_empty() {
                        return Err(Status::invalid_argument(format!(
                            "pprof sample_type[{index}] must have non-empty type and unit"
                        )));
                    }
                    Ok((profile_type, unit))
                })
                .collect::<Result<Vec<_>, Status>>()?;

            let locations = &pprof_profile.location;
            let functions = &pprof_profile.function;
            let string_table = &pprof_profile.string_table;

            for (sample_index, pprof_sample) in pprof_profile.sample.iter().enumerate() {
                if pprof_sample.value.len() != sample_types.len() {
                    return Err(Status::invalid_argument(format!(
                        "pprof sample[{sample_index}] has {} values but the profile declares {} sample types",
                        pprof_sample.value.len(),
                        sample_types.len()
                    )));
                }

                let mut stack = Vec::with_capacity(pprof_sample.location_id.len().min(128));
                for (frame_index, &location_id) in
                    pprof_sample.location_id.iter().take(128).enumerate()
                {
                    let location = locations
                        .iter()
                        .find(|location| location.id == location_id)
                        .ok_or_else(|| {
                            Status::invalid_argument(format!(
                                "pprof sample[{sample_index}] frame[{frame_index}] references missing location id {location_id}"
                            ))
                        })?;
                    let Some(line) = location.line.first() else {
                        stack.push(format!("0x{:x}", location.address));
                        continue;
                    };
                    let function = functions
                        .iter()
                        .find(|function| function.id == line.function_id)
                        .ok_or_else(|| {
                            Status::invalid_argument(format!(
                                "pprof location id {location_id} references missing function id {}",
                                line.function_id
                            ))
                        })?;
                    let function_name_index = usize::try_from(function.name).map_err(|_| {
                        Status::invalid_argument(format!(
                            "pprof function id {} has negative name index {}",
                            function.id, function.name
                        ))
                    })?;
                    let function_name = string_table.get(function_name_index).ok_or_else(|| {
                        Status::invalid_argument(format!(
                            "pprof function id {} name index {function_name_index} out of bounds (table has {} entries)",
                            function.id,
                            string_table.len()
                        ))
                    })?;
                    if function_name.is_empty() {
                        return Err(Status::invalid_argument(format!(
                            "pprof function id {} has an empty name",
                            function.id
                        )));
                    }
                    stack.push(function_name.clone());
                }
                // pprof stores leaf-first; Sigiro stores caller-first for folded stacks.
                stack.reverse();

                for ((profile_type, unit), &value) in sample_types.iter().zip(&pprof_sample.value) {
                    profile_ids.append_value(&sample_id);
                    trace_ids.append_null();
                    span_ids.append_null();
                    service_names.append_value(service_name);
                    timestamps.push(ts_us);
                    profile_types.append_value(profile_type);
                    append_stack_trace(&mut stack_traces, &stack);

                    values.push(value);
                    units.append_value(unit);
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
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(TimestampMicrosecondArray::from(Vec::<i64>::new())),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(empty_stack_trace_array()),
                Arc::new(Int64Array::from(Vec::<i64>::new())),
                Arc::new(StringBuilder::new().finish()),
                Arc::new(StringBuilder::new().finish()),
            ],
        )
        .map_err(arrow_status);
    }

    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(profile_ids.finish()),
            Arc::new(trace_ids.finish()),
            Arc::new(span_ids.finish()),
            Arc::new(service_names.finish()),
            Arc::new(TimestampMicrosecondArray::from(timestamps)),
            Arc::new(profile_types.finish()),
            Arc::new(stack_traces.finish()),
            Arc::new(Int64Array::from(values)),
            Arc::new(units.finish()),
            Arc::new(resource_attrs.finish()),
        ],
    )
    .map_err(arrow_status)
}

fn decompress_profile(
    bytes: Vec<u8>,
    max_profile_bytes: usize,
    description: &str,
) -> Result<Vec<u8>, Status> {
    if bytes.len() > max_profile_bytes {
        return Err(Status::resource_exhausted(format!(
            "{description} exceeds the {max_profile_bytes}-byte ingest limit"
        )));
    }
    if !bytes.starts_with(&[0x1f, 0x8b]) {
        return Ok(bytes);
    }

    let read_limit = u64::try_from(max_profile_bytes)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or_else(|| status_internal("ingest body limit cannot be represented as u64"))?;
    let mut decompressed = Vec::new();
    flate2::read::GzDecoder::new(&bytes[..])
        .take(read_limit)
        .read_to_end(&mut decompressed)
        .map_err(|e| Status::invalid_argument(format!("{description} gzip is invalid: {e}")))?;
    if decompressed.len() > max_profile_bytes {
        return Err(Status::resource_exhausted(format!(
            "{description} exceeds the {max_profile_bytes}-byte ingest limit after decompression"
        )));
    }
    Ok(decompressed)
}
