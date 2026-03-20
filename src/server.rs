use anyhow::{Context, Result};
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use crossbeam_channel::Sender;
use duckdb::Connection;
use std::sync::{Arc, Mutex};
use tonic::transport::Server as TonicServer;

use crate::ingest::{IngestMsg, OtlpGrpcService};
use crate::investigate;

pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
}

pub async fn run_servers(conn: Arc<Mutex<Connection>>, tx: Sender<IngestMsg>) -> Result<()> {
    // gRPC server on :4317
    let grpc_tx = tx.clone();
    let grpc_handle = tokio::spawn(async move {
        let addr = "[::]:4317".parse().expect("valid addr");
        let svc = OtlpGrpcService::new(grpc_tx);
        tracing::info!("gRPC listening on {addr}");
        TonicServer::builder()
            .add_service(svc.trace_service())
            .add_service(svc.logs_service())
            .add_service(svc.metrics_service())
            .serve(addr)
            .await
            .context("gRPC server failed")
    });

    // HTTP server on :9999 (query + health)
    let app_state = Arc::new(AppState { conn });
    let http_app = Router::new()
        .route("/health", get(health))
        .route("/query", post(query_handler))
        .route("/investigate", post(investigate_handler))
        .with_state(app_state);

    let http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
        tracing::info!("HTTP listening on 0.0.0.0:9999");
        axum::serve(listener, http_app)
            .await
            .context("HTTP server failed")
    });

    // OTLP HTTP on :4318
    let http_otlp_tx = tx;
    let otlp_http_app = crate::ingest::otlp_http_router(http_otlp_tx);
    let otlp_http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:4318").await?;
        tracing::info!("OTLP HTTP listening on 0.0.0.0:4318");
        axum::serve(listener, otlp_http_app)
            .await
            .context("OTLP HTTP server failed")
    });

    tokio::select! {
        r = grpc_handle => r??,
        r = http_handle => r??,
        r = otlp_http_handle => r??,
    }

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn query_handler(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<Json<Vec<serde_json::Value>>, String> {
    let conn = state.conn.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare(&body).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;

    let stmt_ref = rows.as_ref().ok_or("no result set")?;
    let column_count = stmt_ref.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| {
            stmt_ref
                .column_name(i)
                .map(|s| s.to_string())
                .unwrap_or_else(|_| "?".to_string())
        })
        .collect();

    let mut results = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut obj = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row
                .get::<_, String>(i)
                .or_else(|_| row.get::<_, i64>(i).map(|v| v.to_string()))
                .or_else(|_| row.get::<_, f64>(i).map(|v| v.to_string()))
                .unwrap_or_else(|_| "null".to_string());
            obj.insert(name.clone(), serde_json::Value::String(val));
        }
        results.push(serde_json::Value::Object(obj));
    }

    Ok(Json(results))
}

async fn investigate_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> Result<Json<investigate::InvestigationBlock>, String> {
    let block = investigate::investigate(&state.conn, &req).map_err(|e| format!("{e:#}"))?;
    Ok(Json(block))
}
