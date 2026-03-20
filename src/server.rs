use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    routing::{get, post},
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
        r = grpc_handle => { tracing::error!("gRPC server exited unexpectedly"); r?? },
        r = http_handle => { tracing::error!("HTTP server exited unexpectedly"); r?? },
        r = otlp_http_handle => { tracing::error!("OTLP HTTP server exited unexpectedly"); r?? },
    }

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

const BLOCKED_KEYWORDS: &[&str] = &[
    "DROP",
    "DELETE",
    "INSERT",
    "UPDATE",
    "ALTER",
    "CREATE",
    "COPY",
    "ATTACH",
    "DETACH",
    "EXPORT",
    "IMPORT",
    "PRAGMA",
    "LOAD",
    "INSTALL",
    "SET",
    "CALL",
    "TRUNCATE",
    "CHECKPOINT",
];

fn validate_read_only(sql: &str) -> Result<(), String> {
    let upper = sql.to_uppercase();
    for kw in BLOCKED_KEYWORDS {
        // Check for keyword as whole word (preceded by start/whitespace/semicolon, followed by whitespace/paren/semicolon/end)
        for (i, _) in upper.match_indices(kw) {
            let before_ok = i == 0
                || upper.as_bytes()[i - 1].is_ascii_whitespace()
                || upper.as_bytes()[i - 1] == b';'
                || upper.as_bytes()[i - 1] == b'(';
            let end = i + kw.len();
            let after_ok = end == upper.len()
                || upper.as_bytes()[end].is_ascii_whitespace()
                || upper.as_bytes()[end] == b'('
                || upper.as_bytes()[end] == b';';
            if before_ok && after_ok {
                return Err(format!("query rejected: {kw} statements not allowed"));
            }
        }
    }
    Ok(())
}

async fn query_handler(
    State(state): State<Arc<AppState>>,
    body: String,
) -> Result<Json<Vec<serde_json::Value>>, String> {
    validate_read_only(&body)?;
    let conn = state.conn.lock().map_err(|e| {
        tracing::error!("DuckDB connection mutex poisoned");
        e.to_string()
    })?;
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
            let val: serde_json::Value = if let Ok(s) = row.get::<_, String>(i) {
                serde_json::Value::String(s)
            } else if let Ok(n) = row.get::<_, i64>(i) {
                serde_json::json!(n)
            } else if let Ok(f) = row.get::<_, f64>(i) {
                serde_json::json!(f)
            } else if let Ok(b) = row.get::<_, bool>(i) {
                serde_json::json!(b)
            } else {
                serde_json::Value::Null
            };
            obj.insert(name.clone(), val);
        }
        results.push(serde_json::Value::Object(obj));
    }

    Ok(Json(results))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_drop_table() {
        assert!(validate_read_only("DROP TABLE sigiro_spans").is_err());
    }

    #[test]
    fn rejects_delete() {
        assert!(validate_read_only("DELETE FROM sigiro_spans WHERE 1=1").is_err());
    }

    #[test]
    fn rejects_insert() {
        assert!(validate_read_only("INSERT INTO sigiro_spans VALUES (1)").is_err());
    }

    #[test]
    fn rejects_update() {
        assert!(validate_read_only("UPDATE sigiro_spans SET status_code = 0").is_err());
    }

    #[test]
    fn rejects_case_insensitive() {
        assert!(validate_read_only("drop table sigiro_spans").is_err());
        assert!(validate_read_only("Drop Table sigiro_spans").is_err());
    }

    #[test]
    fn rejects_attach() {
        assert!(validate_read_only("ATTACH '/tmp/evil.db' AS evil").is_err());
    }

    #[test]
    fn rejects_copy() {
        assert!(validate_read_only("COPY sigiro_spans TO '/tmp/data.csv'").is_err());
    }

    #[test]
    fn rejects_load_extension() {
        assert!(validate_read_only("LOAD httpfs").is_err());
        assert!(validate_read_only("INSTALL httpfs").is_err());
    }

    #[test]
    fn rejects_set() {
        assert!(validate_read_only("SET access_mode = 'read_write'").is_err());
    }

    #[test]
    fn allows_select() {
        assert!(validate_read_only("SELECT * FROM sigiro_spans").is_ok());
    }

    #[test]
    fn allows_select_with_where() {
        assert!(
            validate_read_only("SELECT count(*) FROM sigiro_spans WHERE status_code = 2").is_ok()
        );
    }

    #[test]
    fn allows_keyword_in_column_name() {
        // "updated_at" contains "UPDATE" substring but is not a keyword at word boundary
        assert!(validate_read_only("SELECT updated_at FROM sigiro_spans").is_ok());
    }

    #[test]
    fn allows_describe() {
        assert!(validate_read_only("DESCRIBE sigiro_spans").is_ok());
    }

    #[test]
    fn allows_show_tables() {
        assert!(validate_read_only("SHOW TABLES").is_ok());
    }

    #[test]
    fn rejects_multi_statement_with_dml() {
        assert!(validate_read_only("SELECT 1; DROP TABLE sigiro_spans").is_err());
    }

    #[test]
    fn rejects_call() {
        assert!(validate_read_only("CALL pragma_version()").is_err());
    }

    #[test]
    fn rejects_truncate() {
        assert!(validate_read_only("TRUNCATE sigiro_spans").is_err());
    }

    #[test]
    fn rejects_checkpoint() {
        assert!(validate_read_only("CHECKPOINT").is_err());
    }

    #[test]
    fn rejects_keyword_after_paren() {
        assert!(validate_read_only("(DELETE FROM sigiro_spans)").is_err());
    }
}

async fn investigate_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> Result<Json<investigate::InvestigationBlock>, String> {
    let block = investigate::investigate(&state.conn, &req).map_err(|e| format!("{e:#}"))?;
    Ok(Json(block))
}
