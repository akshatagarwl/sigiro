use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::State,
    middleware as axum_mw,
    routing::{get, post},
};
use crossbeam_channel::Sender;
use duckdb::Connection;
use std::sync::{Arc, Mutex};
use tonic::transport::Server as TonicServer;

use crate::auth;
use crate::ingest::{IngestMsg, OtlpGrpcService};
use crate::investigate;

pub struct AppState {
    pub conn: Arc<Mutex<Connection>>,
    /// "lake." when DuckLake is active, "" for local mode
    pub table_prefix: &'static str,
    /// Unkey auth cache — None in OSS/local mode
    pub auth_cache: Option<moka::future::Cache<String, crate::auth::AuthContext>>,
    /// HTTP client for Unkey/Lago calls — None in OSS/local mode
    pub http_client: Option<reqwest::Client>,
    /// Unkey API ID — None in OSS/local mode
    pub unkey_api_id: Option<String>,
}

/// DuckDB's access_mode is database-level, not connection-level:
/// - `SET access_mode = 'read_only'` fails with "Cannot change access_mode setting while database is running"
/// - `Connection::open_with_flags(path, ReadOnly)` creates a separate database instance that
///   doesn't share in-memory state with the write connection (ingested data invisible)
///
/// Therefore we use a keyword blocklist + `prepare()` single-statement guarantee.
/// `conn.prepare()` only handles one statement — multi-statement injection via semicolons
/// is already prevented by DuckDB itself.
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
    "CALL",
    "TRUNCATE",
    "CHECKPOINT",
    "LOAD",
    "INSTALL",
    "SET",
    "READ_CSV",
    "READ_PARQUET",
    "READ_JSON",
];

pub fn validate_read_only(sql: &str) -> Result<(), String> {
    let upper = sql.to_uppercase();
    for kw in BLOCKED_KEYWORDS {
        // Word-boundary check: keyword must be preceded and followed by non-alphanumeric/underscore
        // This prevents false positives like "updated_at" matching "UPDATE"
        for (pos, _) in upper.match_indices(kw) {
            let before_ok = pos == 0
                || !upper.as_bytes()[pos - 1].is_ascii_alphanumeric()
                    && upper.as_bytes()[pos - 1] != b'_';
            let end = pos + kw.len();
            let after_ok = end >= upper.len()
                || !upper.as_bytes()[end].is_ascii_alphanumeric() && upper.as_bytes()[end] != b'_';
            if before_ok && after_ok {
                return Err(format!("query rejected: {kw} not allowed"));
            }
        }
    }
    Ok(())
}

pub async fn run_servers(
    conn: Arc<Mutex<Connection>>,
    tx: Sender<IngestMsg>,
    table_prefix: &'static str,
) -> Result<()> {
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
    // Auth is opt-in: only active when UNKEY_API_ID is set
    let (auth_cache, http_client, unkey_api_id) = match std::env::var("UNKEY_API_ID") {
        Ok(api_id) => {
            let cache = moka::future::Cache::builder()
                .max_capacity(10_000)
                .time_to_live(std::time::Duration::from_secs(60))
                .build();
            tracing::info!("auth enabled — Unkey API ID: {api_id}");
            (Some(cache), Some(reqwest::Client::new()), Some(api_id))
        }
        Err(_) => (None, None, None),
    };
    let app_state = Arc::new(AppState {
        conn,
        table_prefix,
        auth_cache,
        http_client,
        unkey_api_id,
    });
    let http_app = Router::new()
        .route("/health", get(health))
        .route("/query", post(query_handler))
        .route("/investigate", post(investigate_handler))
        .route("/investigate/multi", post(investigate_multi_handler))
        .layer(axum_mw::from_fn_with_state(
            app_state.clone(),
            auth::auth_middleware,
        ))
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

    // Servers should run forever. Any exit is unexpected and fatal.
    // Match on the result to log the specific server and error before propagating.
    tokio::select! {
        r = grpc_handle => {
            let result = r?;
            tracing::error!("gRPC server exited: {result:?}");
            result?
        },
        r = http_handle => {
            let result = r?;
            tracing::error!("HTTP server exited: {result:?}");
            result?
        },
        r = otlp_http_handle => {
            let result = r?;
            tracing::error!("OTLP HTTP server exited: {result:?}");
            result?
        },
    }

    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

/// Convert a DuckDB ValueRef to a serde_json::Value.
/// Uses pattern matching on the actual type — no cascading type-try guessing.
/// Public for reuse by the MCP server.
pub fn value_ref_to_json(val: duckdb::types::ValueRef<'_>) -> serde_json::Value {
    use duckdb::types::ValueRef;
    match val {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Boolean(b) => serde_json::json!(b),
        ValueRef::TinyInt(n) => serde_json::json!(n),
        ValueRef::SmallInt(n) => serde_json::json!(n),
        ValueRef::Int(n) => serde_json::json!(n),
        ValueRef::BigInt(n) => serde_json::json!(n),
        ValueRef::HugeInt(n) => serde_json::json!(n.to_string()),
        ValueRef::UTinyInt(n) => serde_json::json!(n),
        ValueRef::USmallInt(n) => serde_json::json!(n),
        ValueRef::UInt(n) => serde_json::json!(n),
        ValueRef::UBigInt(n) => serde_json::json!(n),
        ValueRef::Float(f) => serde_json::json!(f),
        ValueRef::Double(f) => serde_json::json!(f),
        ValueRef::Text(bytes) => {
            let s = std::str::from_utf8(bytes).unwrap_or("<invalid utf8>");
            serde_json::Value::String(s.to_string())
        }
        ValueRef::Blob(bytes) => serde_json::json!(format!("<blob {} bytes>", bytes.len())),
        ValueRef::Timestamp(_, us) => serde_json::json!(us),
        ValueRef::Date32(d) => serde_json::json!(d),
        ValueRef::Time64(_, t) => serde_json::json!(t),
        // Compound types: render as string representation
        _ => serde_json::Value::String(format!("{val:?}")),
    }
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
                .map(ToString::to_string)
                .unwrap_or_else(|_| "?".to_string())
        })
        .collect();

    let mut results = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut obj = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row.get_ref(i).map_err(|e| e.to_string())?;
            obj.insert(name.clone(), value_ref_to_json(val));
        }
        results.push(serde_json::Value::Object(obj));
    }

    Ok(Json(results))
}

async fn investigate_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> Result<Json<investigate::InvestigationBlock>, String> {
    let block = investigate::investigate(&state.conn, &req, state.table_prefix)
        .map_err(|e| format!("{e:#}"))?;
    Ok(Json(block))
}

async fn investigate_multi_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<investigate::InvestigateMultiRequest>,
) -> Result<Json<std::collections::BTreeMap<String, investigate::InvestigationBlock>>, String> {
    let blocks = investigate::investigate_multi(&state.conn, &req, state.table_prefix)
        .map_err(|e| format!("{e:#}"))?;
    Ok(Json(blocks))
}
