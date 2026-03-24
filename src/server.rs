use anyhow::{Context, Result};
use axum::{
    Extension, Json, Router,
    extract::State,
    http::StatusCode,
    middleware as axum_mw,
    routing::{get, post},
};
use crossbeam_channel::Sender;
use duckdb::Connection;
use problem_details::ProblemDetails;
use std::sync::{Arc, Mutex};
use tonic::transport::Server as TonicServer;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::KeyExtractor};

use crate::auth::{self, AuthContext};
use crate::ingest::{IngestMsg, OtlpGrpcService};
use crate::investigate;
use crate::pool::ProjectPool;

/// Rate limit key: extract Bearer token from Authorization header.
/// Falls back to "anonymous" if missing — auth middleware will reject it anyway.
#[derive(Clone)]
struct BearerKeyExtractor;

impl KeyExtractor for BearerKeyExtractor {
    type Key = String;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        let key = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            // TRADEOFF: rate limiter needs a key for every request, including unauthenticated ones.
            // "anonymous" groups all unauthenticated requests under one rate limit bucket.
            .unwrap_or("anonymous");
        Ok(key.to_string())
    }
}

/// RFC 9457 problem details constructors for sigiro API errors.
/// Content-Type: application/problem+json — standard format agents can rely on.
pub fn query_rejected(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new()
        .with_status(StatusCode::BAD_REQUEST)
        .with_title("Query Rejected")
        .with_detail(detail)
}

pub fn internal_error(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new()
        .with_status(StatusCode::INTERNAL_SERVER_ERROR)
        .with_title("Internal Error")
        .with_detail(detail)
}

pub fn invalid_request(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new()
        .with_status(StatusCode::BAD_REQUEST)
        .with_title("Invalid Request")
        .with_detail(detail)
}

pub struct AppState {
    pub pool: Arc<ProjectPool>,
    /// Admin DuckDB for API key storage (separate from per-project data)
    pub admin_conn: Arc<Mutex<Connection>>,
    pub http_client: reqwest::Client,
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
    pool: Arc<ProjectPool>,
    admin_conn: Arc<Mutex<Connection>>,
    tx: Sender<IngestMsg>,
) -> Result<()> {
    // gRPC server on :4317 — authenticated via Bearer token in metadata
    let grpc_tx = tx.clone();
    let grpc_admin = Arc::clone(&admin_conn);
    let grpc_handle = tokio::spawn(async move {
        let addr = "[::]:4317".parse().expect("valid addr");
        let svc = OtlpGrpcService::new(grpc_tx, grpc_admin);
        tracing::info!("gRPC listening on {addr}");
        TonicServer::builder()
            .add_service(svc.trace_service())
            .add_service(svc.logs_service())
            .add_service(svc.metrics_service())
            .serve(addr)
            .await
            .context("gRPC server failed")
    });

    tracing::info!("dev mode — use Bearer sigiro_dev_<anything> to authenticate");

    let app_state = Arc::new(AppState {
        pool,
        admin_conn: Arc::clone(&admin_conn),
        http_client: reqwest::Client::new(),
    });

    // Rate limit: 100 requests/minute per API key (GCRA algorithm, in-memory)
    let governor_conf = GovernorConfigBuilder::default()
        .key_extractor(BearerKeyExtractor)
        .per_second(2)
        .burst_size(10)
        .finish()
        .expect("valid governor config");

    // v1 API routes — all authenticated + rate limited
    // Layer order: request → governor → auth → handler (last .layer() runs first)
    let v1 = Router::new()
        .route("/query", post(query_handler))
        .route("/investigate", post(investigate_handler))
        .route("/services", get(services_handler))
        .layer(axum_mw::from_fn_with_state(
            app_state.clone(),
            auth::auth_middleware,
        ))
        .layer(GovernorLayer::new(governor_conf));

    // Admin routes — no bearer auth (chicken-and-egg: need admin to create first key).
    // These are on :9999 which should only be exposed internally.
    let admin = Router::new().route("/keys", post(admin_create_key)).route(
        "/keys/{short_token}",
        axum::routing::delete(admin_revoke_key),
    );

    // Public routes + v1 API + admin
    let http_app = Router::new()
        .route("/health", get(health))
        .nest("/v1", v1)
        .nest("/admin", admin)
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(app_state);

    let http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:9999").await?;
        tracing::info!("HTTP listening on 0.0.0.0:9999");
        axum::serve(listener, http_app)
            .await
            .context("HTTP server failed")
    });

    // OTLP HTTP on :4318 — authenticated via Authorization header
    let http_otlp_tx = tx;
    let otlp_admin = Arc::clone(&admin_conn);
    let otlp_http_app = crate::ingest::otlp_http_router(http_otlp_tx, otlp_admin);
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
        // TRADEOFF: DuckDB ValueRef has 20+ variants including Map, List, Enum, Union, etc.
        // Exhaustive matching would break on every duckdb-rs update. Debug-format is safe
        // because this is user-facing query output, not internal data flow.
        val => serde_json::Value::String(format!("{val:?}")),
    }
}

/// Get the per-project DuckDB connection from the pool using the AuthContext.
#[allow(clippy::result_large_err)] // ProblemDetails is the axum error response type
fn project_conn(
    state: &AppState,
    auth: &AuthContext,
) -> Result<Arc<Mutex<Connection>>, ProblemDetails> {
    state
        .pool
        .get_or_create(&auth.project_id)
        .map_err(|e| internal_error(format!("project DB: {e:#}")))
}

async fn query_handler(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    body: String,
) -> Result<Json<Vec<serde_json::Value>>, ProblemDetails> {
    validate_read_only(&body).map_err(query_rejected)?;
    let conn = project_conn(&state, &auth)?;
    let conn = conn.lock().map_err(|e| {
        tracing::error!("DuckDB connection mutex poisoned");
        internal_error(e.to_string())
    })?;
    let mut stmt = conn
        .prepare(&body)
        .map_err(|e| query_rejected(e.to_string()))?;
    let mut rows = stmt.query([]).map_err(|e| internal_error(e.to_string()))?;

    let stmt_ref = rows
        .as_ref()
        .ok_or_else(|| internal_error("no result set"))?;
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
    while let Some(row) = rows.next().map_err(|e| internal_error(e.to_string()))? {
        let mut obj = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row.get_ref(i).map_err(|e| internal_error(e.to_string()))?;
            obj.insert(name.clone(), value_ref_to_json(val));
        }
        results.push(serde_json::Value::Object(obj));
    }

    Ok(Json(results))
}

/// GET /v1/services — discover what services are sending telemetry.
/// Agents call this before /investigate to know what services exist.
async fn services_handler(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> Result<Json<serde_json::Value>, ProblemDetails> {
    let conn = project_conn(&state, &auth)?;
    let conn = conn.lock().map_err(|e| {
        tracing::error!("DuckDB connection mutex poisoned");
        internal_error(e.to_string())
    })?;
    let prefix = state.pool.prefix();
    let sql = format!(
        "SELECT service_name, \
                min(timestamp)::VARCHAR AS first_seen, \
                max(timestamp)::VARCHAR AS last_seen, \
                count(*) AS span_count \
         FROM {}sigiro_spans \
         GROUP BY service_name \
         ORDER BY span_count DESC",
        prefix
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| internal_error(e.to_string()))?;
    let mut rows = stmt.query([]).map_err(|e| internal_error(e.to_string()))?;

    let mut services = Vec::new();
    while let Some(row) = rows.next().map_err(|e| internal_error(e.to_string()))? {
        let name: String = row.get(0).map_err(|e| internal_error(e.to_string()))?;
        let first_seen: String = row.get(1).map_err(|e| internal_error(e.to_string()))?;
        let last_seen: String = row.get(2).map_err(|e| internal_error(e.to_string()))?;
        let span_count: i64 = row.get(3).map_err(|e| internal_error(e.to_string()))?;
        services.push(serde_json::json!({
            "name": name,
            "first_seen": first_seen,
            "last_seen": last_seen,
            "span_count": span_count,
        }));
    }

    Ok(Json(serde_json::json!({ "services": services })))
}

async fn investigate_handler(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> Result<Json<std::collections::BTreeMap<String, investigate::InvestigationBlock>>, ProblemDetails>
{
    let conn = project_conn(&state, &auth)?;
    let blocks = investigate::investigate(&conn, &req, state.pool.prefix())
        .map_err(|e| internal_error(format!("{e:#}")))?;
    Ok(Json(blocks))
}

// --- Admin endpoints ---
// No bearer auth required — :9999 should only be exposed internally.
// This solves the DuckDB single-writer lock problem: the CLI talks to the
// running server over HTTP instead of opening admin.duckdb directly.

#[derive(serde::Deserialize)]
struct CreateKeyRequest {
    project: String,
    #[serde(default = "default_plan")]
    plan: String,
    #[serde(default = "default_rate_limit")]
    rate_limit: u32,
}

fn default_plan() -> String {
    "free".to_string()
}

fn default_rate_limit() -> u32 {
    100
}

async fn admin_create_key(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateKeyRequest>,
) -> Result<Json<serde_json::Value>, ProblemDetails> {
    let raw_key = auth::create_key(&state.admin_conn, &req.project, &req.plan, req.rate_limit)
        .map_err(|e| internal_error(format!("{e:#}")))?;
    Ok(Json(serde_json::json!({ "key": raw_key })))
}

async fn admin_revoke_key(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(short_token): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, ProblemDetails> {
    let revoked = auth::revoke_key(&state.admin_conn, &short_token)
        .map_err(|e| internal_error(format!("{e:#}")))?;
    if revoked {
        Ok(Json(
            serde_json::json!({ "revoked": true, "short_token": short_token }),
        ))
    } else {
        Err(ProblemDetails::new()
            .with_status(StatusCode::NOT_FOUND)
            .with_title("Key Not Found")
            .with_detail(format!("No key found with short token: {short_token}")))
    }
}
