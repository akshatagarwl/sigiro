use anyhow::Result;
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
}

impl IngestMsg {
    pub fn project_id(&self) -> &str {
        match self {
            IngestMsg::Traces { project_id, .. }
            | IngestMsg::Logs { project_id, .. }
            | IngestMsg::Metrics { project_id, .. } => project_id,
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
