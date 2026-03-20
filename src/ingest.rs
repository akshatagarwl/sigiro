use anyhow::Result;
use axum::{Router, body::Bytes, extract::State, http::StatusCode, routing::post};
use crossbeam_channel::Sender;
use opentelemetry_proto::tonic::{
    collector::{
        logs::v1::{
            logs_service_server::{LogsService, LogsServiceServer},
            ExportLogsServiceRequest, ExportLogsServiceResponse,
        },
        metrics::v1::{
            metrics_service_server::{MetricsService, MetricsServiceServer},
            ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        },
        trace::v1::{
            trace_service_server::{TraceService, TraceServiceServer},
            ExportTraceServiceRequest, ExportTraceServiceResponse,
        },
    },
};
use prost::Message;
use tonic::{Request, Response, Status};

pub enum IngestMsg {
    Traces(Vec<u8>),
    Logs(Vec<u8>),
    Metrics(Vec<u8>),
}

pub struct OtlpGrpcService {
    tx: Sender<IngestMsg>,
}

impl OtlpGrpcService {
    pub fn new(tx: Sender<IngestMsg>) -> Self {
        Self { tx }
    }

    pub fn trace_service(&self) -> TraceServiceServer<Self> {
        TraceServiceServer::new(Self { tx: self.tx.clone() })
    }

    pub fn logs_service(&self) -> LogsServiceServer<Self> {
        LogsServiceServer::new(Self { tx: self.tx.clone() })
    }

    pub fn metrics_service(&self) -> MetricsServiceServer<Self> {
        MetricsServiceServer::new(Self { tx: self.tx.clone() })
    }
}

#[tonic::async_trait]
impl TraceService for OtlpGrpcService {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        let msg = request.into_inner();
        let bytes = msg.encode_to_vec();
        self.tx
            .send(IngestMsg::Traces(bytes))
            .map_err(|e| Status::internal(e.to_string()))?;
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
        let msg = request.into_inner();
        let bytes = msg.encode_to_vec();
        self.tx
            .send(IngestMsg::Logs(bytes))
            .map_err(|e| Status::internal(e.to_string()))?;
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
        let msg = request.into_inner();
        let bytes = msg.encode_to_vec();
        self.tx
            .send(IngestMsg::Metrics(bytes))
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

// OTLP HTTP endpoints on :4318
pub fn otlp_http_router(tx: Sender<IngestMsg>) -> Router {
    Router::new()
        .route("/v1/traces", post(otlp_http_traces))
        .route("/v1/logs", post(otlp_http_logs))
        .route("/v1/metrics", post(otlp_http_metrics))
        .with_state(tx)
}

async fn otlp_http_traces(
    State(tx): State<Sender<IngestMsg>>,
    body: Bytes,
) -> StatusCode {
    if let Err(e) = tx.send(IngestMsg::Traces(body.to_vec())) {
        tracing::error!("failed to enqueue traces: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn otlp_http_logs(
    State(tx): State<Sender<IngestMsg>>,
    body: Bytes,
) -> StatusCode {
    if let Err(e) = tx.send(IngestMsg::Logs(body.to_vec())) {
        tracing::error!("failed to enqueue logs: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}

async fn otlp_http_metrics(
    State(tx): State<Sender<IngestMsg>>,
    body: Bytes,
) -> StatusCode {
    if let Err(e) = tx.send(IngestMsg::Metrics(body.to_vec())) {
        tracing::error!("failed to enqueue metrics: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR;
    }
    StatusCode::OK
}
