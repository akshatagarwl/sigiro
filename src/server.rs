use anyhow::{Context, Result};
use axum::{Extension, Json, Router, extract::State, middleware as axum_mw};
use axum_anyhow::{ApiResult, IntoApiError, OptionExt, ResultExt};
use axum_extra::routing::{RouterExt, TypedPath};
use rusqlite::Connection;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tonic::transport::Server as TonicServer;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::KeyExtractor};

use crate::admin;
use crate::auth::{self, AuthContext};
use crate::federation;
use crate::ingest::OtlpGrpcService;
use crate::inserter::MsgRouter;
use crate::investigate;
use crate::pool::ProjectPool;

// --- Typed path structs for compile-time route checking ---

#[derive(Debug, Clone, TypedPath)]
#[typed_path("/query")]
struct QueryPath;

#[derive(Debug, Clone, TypedPath)]
#[typed_path("/investigate")]
struct InvestigatePath;

#[derive(Debug, Clone, TypedPath)]
#[typed_path("/services")]
struct ServicesPath;

#[derive(Debug, Clone, TypedPath)]
#[typed_path("/federation/workers")]
struct FederationWorkersPath;

#[derive(Debug, Clone, Deserialize, TypedPath)]
#[typed_path("/federation/workers/{id}")]
struct FederationWorkerPath {
    id: String,
}

#[derive(Debug, Clone, TypedPath)]
#[typed_path("/keys")]
struct AdminKeysPath;

#[derive(Debug, Clone, Deserialize, TypedPath)]
#[typed_path("/keys/{short_token}")]
struct AdminKeyPath {
    short_token: String,
}

#[derive(Debug, Clone, TypedPath)]
#[typed_path("/status")]
struct AdminStatusPath;

#[derive(Debug, Clone, TypedPath)]
#[typed_path("/health")]
struct HealthPath;

/// Rate limit key: extract Bearer token from Authorization header.
/// Returns Err for requests without a parseable Bearer token — these will be
/// rejected by auth middleware anyway, so rate-limiting them is pointless noise.
#[derive(Clone)]
struct BearerKeyExtractor;

impl KeyExtractor for BearerKeyExtractor {
    type Key = String;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        // Use a placeholder when auth is missing/malformed so the request
        // reaches the auth middleware, which returns a proper 401 instead of
        // the governor's generic 500 "Unable To Extract Key".
        let key = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("__missing_auth__");
        Ok(key.to_string())
    }
}

pub struct AppState {
    pub pool: Arc<ProjectPool>,
    /// Admin SQLite for API key storage (separate from per-project data)
    pub admin_conn: Arc<Mutex<Connection>>,
    pub http_client: reqwest::Client,
}

/// DuckDB's access_mode is database-level, not connection-level:
/// - `SET access_mode = 'read_only'` fails with "Cannot change access_mode setting while database is running"
/// - `Connection::open_with_flags(path, ReadOnly)` creates a separate database instance that
///   doesn't share in-memory state with the write connection (ingested data invisible)
///   Therefore we use a keyword blocklist + `prepare()` single-statement guarantee.
///   `conn.prepare()` only handles one statement — multi-statement injection via semicolons
///   is already prevented by DuckDB itself.
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

/// Build the HTTP Router for the main API (port 9999).
/// Extracted from `run_servers` so integration tests can build the same
/// router with test state without starting TCP listeners.
pub fn build_http_router(app_state: Arc<AppState>) -> Router {
    // Rate limit: 100 requests/minute per API key (GCRA algorithm, in-memory)
    let governor_conf = GovernorConfigBuilder::default()
        .key_extractor(BearerKeyExtractor)
        .per_second(2)
        .burst_size(10)
        .finish()
        .expect("valid governor config");

    // v1 API routes — all authenticated + rate limited
    let v1 = Router::new()
        .typed_post(query_handler)
        .typed_post(investigate_handler)
        .typed_get(services_handler)
        .typed_post(register_worker_handler)
        .typed_get(list_workers_handler)
        .typed_delete(deregister_worker_handler)
        .layer(axum_mw::from_fn_with_state(
            app_state.clone(),
            auth::auth_middleware,
        ))
        .layer(GovernorLayer::new(governor_conf));

    // Admin routes — no bearer auth (chicken-and-egg: need admin to create first key).
    // These are on :9999 which should only be exposed internally.
    let admin = Router::new()
        .typed_post(admin_create_key)
        .typed_delete(admin_revoke_key)
        .typed_get(admin_status);

    // Public routes + v1 API + admin
    Router::new()
        .typed_get(health)
        .nest("/v1", v1)
        .nest("/admin", admin)
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(app_state)
}

/// Server port configuration — all configurable via environment variables.
/// Defaults match the original hardcoded values.
#[derive(Clone)]
pub struct ServerPorts {
    pub http: u16,
    pub grpc: u16,
    pub otlp_http: u16,
}

impl Default for ServerPorts {
    fn default() -> Self {
        Self {
            http: std::env::var("SIGIRO_HTTP_PORT")
                .unwrap_or_else(|_| "9999".to_string())
                .parse()
                .unwrap_or(9999),
            grpc: std::env::var("SIGIRO_GRPC_PORT")
                .unwrap_or_else(|_| "4317".to_string())
                .parse()
                .unwrap_or(4317),
            otlp_http: std::env::var("SIGIRO_OTLP_HTTP_PORT")
                .unwrap_or_else(|_| "4318".to_string())
                .parse()
                .unwrap_or(4318),
        }
    }
}

pub async fn run_servers(
    pool: Arc<ProjectPool>,
    admin_conn: Arc<Mutex<Connection>>,
    router: Arc<MsgRouter>,
) -> Result<()> {
    let ports = ServerPorts::default();

    // gRPC server — authenticated via Bearer token in metadata
    let grpc_admin = Arc::clone(&admin_conn);
    let grpc_router = Arc::clone(&router);
    let grpc_port = ports.grpc;
    let grpc_handle = tokio::spawn(async move {
        let addr = format!("[::]:{grpc_port}").parse().expect("valid addr");
        let svc = OtlpGrpcService::new(grpc_router, grpc_admin);
        tracing::info!("gRPC listening on {addr}");
        TonicServer::builder()
            .add_service(svc.trace_service())
            .add_service(svc.logs_service())
            .add_service(svc.metrics_service())
            .add_service(svc.profiles_service())
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

    {
        let guard = admin_conn
            .lock()
            .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
        federation::create_workers_table(&guard)
            .context("failed to create federation workers table")?;
    }

    let http_app = build_http_router(app_state);

    let http_port = ports.http;
    let http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{http_port}")).await?;
        tracing::info!("HTTP listening on 0.0.0.0:{http_port}");
        axum::serve(listener, http_app)
            .await
            .context("HTTP server failed")
    });

    // OTLP HTTP — authenticated via Authorization header
    let otlp_http_app = crate::ingest::otlp_http_router(router, Arc::clone(&admin_conn));
    let otlp_http_port = ports.otlp_http;
    let otlp_http_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{otlp_http_port}")).await?;
        tracing::info!("OTLP HTTP listening on 0.0.0.0:{otlp_http_port}");
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

async fn health(_path: HealthPath) -> &'static str {
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
fn project_conn(state: &AppState, auth: &AuthContext) -> ApiResult<Arc<Mutex<duckdb::Connection>>> {
    // Reads go through the dedicated read connection — never contended by writes.
    state
        .pool
        .get_read(&auth.project_id)
        .context_internal("Project DB", "failed to get read connection")
}

async fn query_handler(
    _path: QueryPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    body: String,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    validate_read_only(&body).map_err(|e| {
        anyhow::anyhow!(e).context_bad_request("Query Rejected", "query contains blocked keyword")
    })?;
    let conn = project_conn(&state, &auth)?;
    let conn = conn
        .lock()
        .map_err(|e| anyhow::anyhow!("DuckDB connection mutex poisoned: {e}"))?;
    let mut stmt = conn
        .prepare(&body)
        .context_bad_request("Query Rejected", "invalid SQL")?;
    let mut rows = stmt
        .query([])
        .context_internal("Query", "query execution failed")?;

    let stmt_ref = rows.as_ref().context_internal("Query", "no result set")?;
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
    while let Some(row) = rows
        .next()
        .context_internal("Query", "row iteration failed")?
    {
        let mut obj: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for (i, name) in column_names.iter().enumerate() {
            let val = row
                .get_ref(i)
                .context_internal("Query", "column read failed")?;
            obj.insert(name.clone(), value_ref_to_json(val));
        }
        results.push(serde_json::Value::Object(obj));
    }

    Ok(Json(results))
}

/// GET /v1/services — discover what services are sending telemetry.
/// Agents call this before /investigate to know what services exist.
async fn services_handler(
    _path: ServicesPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<serde_json::Value>> {
    let conn = project_conn(&state, &auth)?;
    let conn = conn
        .lock()
        .map_err(|e| anyhow::anyhow!("DuckDB connection mutex poisoned: {e}"))?;
    let prefix = ProjectPool::prefix();
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
        .context_internal("Services", "failed to prepare query")?;
    let mut rows = stmt
        .query([])
        .context_internal("Services", "query execution failed")?;

    let mut services = Vec::new();
    while let Some(row) = rows
        .next()
        .context_internal("Services", "row iteration failed")?
    {
        let name: String = row
            .get::<_, String>(0)
            .context_internal("Services", "failed to read service_name")?;
        let first_seen: Option<String> = row
            .get::<_, Option<String>>(1)
            .context_internal("Services", "failed to read first_seen")?;
        let last_seen: Option<String> = row
            .get::<_, Option<String>>(2)
            .context_internal("Services", "failed to read last_seen")?;
        let span_count: i64 = row
            .get::<_, i64>(3)
            .context_internal("Services", "failed to read span_count")?;
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
    _path: InvestigatePath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> ApiResult<Json<std::collections::BTreeMap<String, investigate::InvestigationBlock>>> {
    let conn = project_conn(&state, &auth)?;
    let blocks = investigate::investigate(&conn, &req, ProjectPool::prefix())
        .context_internal("Investigate", "investigation failed")?;
    Ok(Json(blocks))
}

// --- Admin endpoints ---
// No bearer auth required — :9999 should only be exposed internally.
// This solves the DuckDB single-writer lock problem: the CLI talks to the
// running server over HTTP instead of opening admin.sqlite directly.

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
    _path: AdminKeysPath,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateKeyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let raw_key = admin::create_key(&state.admin_conn, &req.project, &req.plan, req.rate_limit)
        .context_internal("Admin", "failed to create API key")?;
    Ok(Json(serde_json::json!({ "key": raw_key })))
}

async fn admin_revoke_key(
    AdminKeyPath { short_token }: AdminKeyPath,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let revoked = admin::revoke_key(&state.admin_conn, &short_token)
        .context_internal("Admin", "failed to revoke API key")?;
    if revoked {
        Ok(Json(
            serde_json::json!({ "revoked": true, "short_token": short_token }),
        ))
    } else {
        Err(axum_anyhow::not_found(
            "Key Not Found",
            &format!("No key found with short token: {short_token}"),
        ))
    }
}

/// GET /admin/status — returns all instance_status rows as JSON object.
async fn admin_status(
    _path: AdminStatusPath,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let map = admin::read_all_status(&guard).context_internal("Admin", "failed to read status")?;
    Ok(Json(serde_json::Value::Object(
        map.into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect(),
    )))
}

// --- Federation endpoints (authenticated, under /v1/federation) ---

/// POST /v1/federation/workers -- register a federation worker.
async fn register_worker_handler(
    _path: FederationWorkersPath,
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
    Json(worker): Json<federation::WorkerInfo>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = worker.id.clone();
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    federation::register_worker(&guard, &worker)
        .context_internal("Federation", "failed to register worker")?;
    Ok(Json(serde_json::json!({
        "registered": true,
        "id": id,
    })))
}

/// DELETE /v1/federation/workers/{id} -- deregister a federation worker.
async fn deregister_worker_handler(
    FederationWorkerPath { id }: FederationWorkerPath,
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<Json<serde_json::Value>> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let deregistered = federation::deregister_worker(&guard, &id)
        .context_internal("Federation", "failed to deregister worker")?;
    if deregistered {
        Ok(Json(serde_json::json!({ "deregistered": true })))
    } else {
        Err(axum_anyhow::not_found(
            "Worker Not Found",
            &format!("No worker found with id: {id}"),
        ))
    }
}

/// GET /v1/federation/workers -- list all federation workers.
async fn list_workers_handler(
    _path: FederationWorkersPath,
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<Json<serde_json::Value>> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let workers = federation::list_workers(&guard)
        .context_internal("Federation", "failed to list workers")?;
    Ok(Json(serde_json::json!({ "workers": workers })))
}
