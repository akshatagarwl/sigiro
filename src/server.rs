use anyhow::{Context, Result};
use axum::{Extension, Json, Router, extract::State, middleware as axum_mw};
use axum_anyhow::{ApiResult, IntoApiError, OptionExt, ResultExt, bad_request};
use axum_extra::routing::RouterExt;
use rusqlite::Connection;
use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::sync::{Arc, Mutex};
use tonic::transport::Server as TonicServer;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::KeyExtractor};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::admin;
use crate::auth::{self, AuthContext};
use crate::federation;
use crate::ingest::OtlpGrpcService;
use crate::inserter::MsgRouter;
use crate::investigate;
use crate::pool::ProjectPool;
use crate::responses::*;
use crate::types::*;
use tracing_attributes::instrument;

#[derive(OpenApi)]
#[openapi(
    paths(
        query_handler,
        services_handler,
        trace_detail_handler,
        investigate_handler,
        admin_create_key,
        admin_revoke_key,
        admin_status,
        register_worker_handler,
        deregister_worker_handler,
        list_workers_handler,
            ),
    components(
        schemas(
            ServicesResponse,
            ServiceEntry,
            TraceDetailResponse,
            SpanDetail,
            ServiceBreakdownEntry,
            CreateKeyResponse,
            RevokeKeyResponse,
            AdminStatusResponse,
            RegisterWorkerResponse,
            DeregisterWorkerResponse,
            ListWorkersResponse,
            crate::federation::WorkerInfo,
            crate::federation::PlatformInfo,
            crate::federation::WorkerCapabilities,
        )
    ),
    tags(
        (name = "query", description = "SQL query endpoints"),
        (name = "admin", description = "Admin endpoints"),
        (name = "federation", description = "Federation worker management"),
            )
)]
pub struct ApiDoc;

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
            .unwrap_or("Bearer missing_auth_header");
        Ok(key.to_string())
    }
}

pub struct AppState {
    pub pool: Arc<ProjectPool>,
    /// Admin SQLite for API key storage (separate from per-project data)
    pub admin_conn: Arc<Mutex<Connection>>,
    pub http_client: reqwest::Client,
}

// DuckDB's access_mode is database-level, not connection-level:
// - `SET access_mode = 'read_only'` fails with "Cannot change access_mode setting while database is running"
// - `Connection::open_with_flags(path, ReadOnly)` creates a separate database instance that
//   doesn't share in-memory state with the write connection (ingested data invisible)
//   Therefore we use AST-based SQL validation (sqlparser) + `prepare()` single-statement guarantee.
//   `conn.prepare()` only handles one statement — multi-statement injection via semicolons
//   is already prevented by DuckDB itself.

/// DuckDB file-reading table functions blocked even in SELECT queries
/// (e.g., read_csv('/etc/passwd') is a valid SELECT but reads arbitrary files).
const BLOCKED_TABLE_FUNCTIONS: &[&str] = &["DROP", "READ_CSV", "READ_PARQUET", "READ_JSON"];

pub fn validate_read_only(sql: &str) -> Result<(), String> {
    let dialect = GenericDialect {};
    let statements = Parser::parse_sql(&dialect, sql)
        .map_err(|e| format!("query rejected: parse error: {e}"))?;

    for stmt in &statements {
        if !matches!(stmt, Statement::Query(_)) {
            return Err("query rejected: only SELECT queries are allowed".to_string());
        }
    }

    // Strip SQL comments before checking blocked keywords
    let stripped = sql
        .split_once("/*")
        .map(|(before, after)| {
            after
                .split_once("*/")
                .map(|(_, after)| format!("{before}{after}"))
                .unwrap_or(before.to_string())
        })
        .unwrap_or_else(|| sql.to_string())
        .split_once("--")
        .map(|(before, _)| before.to_string())
        .unwrap_or_else(|| sql.to_string());
    let upper = stripped.to_uppercase();
    for kw in BLOCKED_TABLE_FUNCTIONS {
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
        .typed_get(trace_detail_handler)
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

    // Public routes + v1 API + admin + Swagger UI
    Router::new()
        .typed_get(health)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .nest("/v1", v1)
        .nest("/admin", admin)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .layer(tower_http::compression::CompressionLayer::new())
        .with_state(app_state)
}

/// Server port configuration — all configurable via environment variables.
/// Uses typed-env for fail-fast parsing: malformed values error at startup.
#[derive(Clone)]
pub struct ServerPorts {
    pub http: u16,
    pub grpc: u16,
    pub otlp_http: u16,
}

impl ServerPorts {
    /// Load ports from environment. Fails fast on malformed values.
    /// Unset vars default to well-known OTLP ports.
    pub fn load() -> anyhow::Result<Self> {
        use crate::config;
        Ok(Self {
            http: config::HTTP_PORT
                .value()
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            grpc: config::GRPC_PORT
                .value()
                .map_err(|e| anyhow::anyhow!("{e}"))?,
            otlp_http: config::OTLP_HTTP_PORT
                .value()
                .map_err(|e| anyhow::anyhow!("{e}"))?,
        })
    }
}

pub async fn run_servers(
    pool: Arc<ProjectPool>,
    admin_conn: Arc<Mutex<Connection>>,
    router: Arc<MsgRouter>,
) -> Result<()> {
    let ports = ServerPorts::load()?;

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

#[instrument]
async fn health(_path: HealthPath) -> &'static str {
    // Create explicit OTEL span using global tracer
    use opentelemetry::trace::{Span, Tracer};
    let tracer = opentelemetry::global::tracer("sigiro");
    let mut span = tracer.start("health_check");
    span.add_event("health_handler_start", vec![]);

    tracing::info!("health check called");

    span.add_event("health_handler_end", vec![]);
    span.end();
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
/// Single connection handles both reads and writes — no snapshot refresh needed.
fn project_conn(state: &AppState, auth: &AuthContext) -> ApiResult<Arc<Mutex<duckdb::Connection>>> {
    // Explicit DI: get_project returns only existing connections.
    // Callers must call create_project() first to initialize a project.
    state
        .pool
        .get_project(&auth.project_id)
        .map_err(|e| anyhow::anyhow!(e).context_internal("Project DB", "connection not found"))
}

#[utoipa::path(
    post,
    path = "/v1/query",
    request_body = String,
    responses(
        (status = 200, description = "Query results", body = Vec<serde_json::Value>)
    ),
    tag = "query"
)]
#[instrument(skip_all, fields(sql_len = body.len()))]
async fn query_handler(
    _path: QueryPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    body: String,
) -> ApiResult<Json<Vec<serde_json::Value>>> {
    validate_read_only(&body).map_err(|e| bad_request("query rejected", &e))?;
    let conn = project_conn(&state, &auth)?;
    let conn = conn
        .lock()
        .map_err(|e| anyhow::anyhow!("DuckDB connection mutex poisoned: {e}"))?;
    let mut stmt = conn
        .prepare(&body)
        .map_err(|e| bad_request("query rejected", &format!("SQL error: {e}")))?;
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

    tracing::info!(rows = results.len(), "query completed");

    Ok(Json(results))
}

/// GET /v1/services — discover what services are sending telemetry.
/// Agents call this before /investigate to know what services exist.
#[utoipa::path(
    get,
    path = "/v1/services",
    responses(
        (status = 200, description = "List of services", body = ServicesResponse)
    ),
    tag = "query"
)]
#[instrument(skip_all)]
async fn services_handler(
    _path: ServicesPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<ServicesResponse>> {
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
        services.push(ServiceEntry {
            name,
            first_seen,
            last_seen,
            span_count,
        });
    }

    Ok(Json(ServicesResponse { services }))
}

/// GET /v1/traces/{trace_id} -- return a single trace with full detail including critical path and service breakdown.
#[utoipa::path(
     get,
     path = "/v1/traces/{trace_id}",
     params(
         ("trace_id" = String, Path, description = "Trace ID")
     ),
     responses(
         (status = 200, description = "Trace detail", body = TraceDetailResponse)
     ),
     tag = "query"
 )]
#[instrument(skip_all)]
async fn trace_detail_handler(
    _path: TraceDetailPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<Json<TraceDetailResponse>> {
    let conn = project_conn(&state, &auth)?;
    let conn = conn
        .lock()
        .map_err(|e| anyhow::anyhow!("DuckDB connection mutex poisoned: {e}"))?;
    let prefix = ProjectPool::prefix();
    let trace_id = _path.trace_id.clone();

    let sql = format!(
        "SELECT
            trace_id, span_id, parent_span_id, span_name, duration,
            status_code, service_name, span_kind,
            span_attributes, resource_attributes,
            timestamp::VARCHAR AS start_timestamp,
            end_timestamp,
            events_json, links_json,
            scope_name, scope_version,
            trace_state
         FROM {}sigiro_spans
         WHERE trace_id = ?
         ORDER BY timestamp",
        prefix
    );
    let mut stmt = conn
        .prepare(&sql)
        .context_internal("TraceDetail", "failed to prepare query")?;
    let mut rows = stmt
        .query([&trace_id])
        .context_internal("TraceDetail", "query execution failed")?;

    let mut spans: Vec<SpanDetail> = Vec::new();
    let mut span_map: std::collections::HashMap<String, SpanDetail> =
        std::collections::HashMap::new();
    let mut parent_to_children: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut root_span_id: Option<String> = None;
    let mut total_duration_ns: i64 = 0;
    let mut error_count: i64 = 0;
    let mut services_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut max_duration_span: Option<SpanDetail> = None;

    while let Some(row) = rows
        .next()
        .context_internal("TraceDetail", "row iteration failed")?
    {
        let span_id: String = row
            .get::<_, String>(1)
            .context_internal("TraceDetail", "failed to read span_id")?;
        let parent_span_id: String = row.get::<_, String>(2).unwrap_or_default();
        let duration_ns: i64 = row
            .get::<_, i64>(4)
            .context_internal("TraceDetail", "failed to read duration")?;
        let status_code: i32 = row
            .get::<_, i32>(5)
            .context_internal("TraceDetail", "failed to read status_code")?;
        let service_name: String = row
            .get::<_, String>(6)
            .context_internal("TraceDetail", "failed to read service_name")?;

        let span = SpanDetail {
            trace_id: row
                .get::<_, String>(0)
                .context_internal("TraceDetail", "failed to read trace_id")?,
            span_id: span_id.clone(),
            parent_span_id: if parent_span_id.is_empty() {
                None
            } else {
                Some(parent_span_id.clone())
            },
            span_name: row
                .get::<_, String>(3)
                .context_internal("TraceDetail", "failed to read span_name")?,
            duration_ns,
            status_code,
            service_name: service_name.clone(),
            timestamp: row
                .get::<_, String>(10)
                .context_internal("TraceDetail", "failed to read timestamp")?,
            span_kind: row.get::<_, Option<i32>>(7).ok().flatten(),
            span_attributes: row.get::<_, Option<String>>(8).ok().flatten(),
            resource_attributes: row.get::<_, Option<String>>(9).ok().flatten(),
            events_json: row.get::<_, Option<String>>(12).ok().flatten(),
            links_json: row.get::<_, Option<String>>(13).ok().flatten(),
            scope_name: row.get::<_, Option<String>>(14).ok().flatten(),
            scope_version: row.get::<_, Option<String>>(15).ok().flatten(),
            trace_state: row.get::<_, Option<String>>(16).ok().flatten(),
        };
        total_duration_ns = total_duration_ns.saturating_add(duration_ns);
        if status_code == 2 {
            error_count += 1;
        }
        services_set.insert(service_name);

        if parent_span_id.is_empty() {
            root_span_id = Some(span_id.clone());
        } else {
            parent_to_children
                .entry(parent_span_id)
                .or_default()
                .push(span_id.clone());
        }

        let is_longer = max_duration_span
            .as_ref()
            .map(|s| duration_ns > s.duration_ns)
            .unwrap_or(true);
        if is_longer {
            max_duration_span = Some(span.clone());
        }

        span_map.insert(span_id.clone(), span.clone());
        spans.push(span);
    }

    drop(rows);
    drop(stmt);

    if spans.is_empty() {
        return Err(axum_anyhow::not_found(
            "Trace Not Found",
            &format!("No trace found with id: {}", trace_id),
        ));
    }

    // Determine root span
    let root_span = if let Some(root_id) = root_span_id {
        span_map
            .get(&root_id)
            .cloned()
            .unwrap_or_else(|| max_duration_span.clone().unwrap())
    } else {
        max_duration_span.unwrap()
    };

    // Compute critical path (longest path from root to leaf by duration sum)
    let critical_path = compute_critical_path(&root_span.span_id, &parent_to_children, &span_map);

    // Compute service breakdown
    let service_breakdown = compute_service_breakdown(&spans, total_duration_ns);

    Ok(Json(TraceDetailResponse {
        trace_id,
        total_duration_ns,
        root_span,
        span_count: spans.len() as i64,
        error_count,
        services: services_set.into_iter().collect(),
        spans,
        critical_path,
        service_breakdown,
    }))
}

/// Compute the critical path: longest duration-sum path from root to any leaf.
fn compute_critical_path(
    root_id: &str,
    parent_to_children: &std::collections::HashMap<String, Vec<String>>,
    span_map: &std::collections::HashMap<String, SpanDetail>,
) -> Vec<String> {
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        span_id: &str,
        parent_to_children: &std::collections::HashMap<String, Vec<String>>,
        span_map: &std::collections::HashMap<String, SpanDetail>,
        current_path: &[String],
        current_duration: i64,
        best_path: &mut Vec<String>,
        best_duration: &mut i64,
    ) {
        let span = match span_map.get(span_id) {
            Some(s) => s,
            None => return,
        };
        let mut path = current_path.to_vec();
        path.push(span_id.to_string());
        let new_duration = current_duration.saturating_add(span.duration_ns);

        // Leaf node: check if this path is the critical path
        if let Some(children) = parent_to_children.get(span_id) {
            for child_id in children {
                dfs(
                    child_id,
                    parent_to_children,
                    span_map,
                    &path,
                    new_duration,
                    best_path,
                    best_duration,
                );
            }
        } else {
            // Leaf node
            if new_duration > *best_duration {
                *best_duration = new_duration;
                *best_path = path;
            }
        }
    }

    let mut best_path = Vec::new();
    let mut best_duration = i64::MIN;
    dfs(
        root_id,
        parent_to_children,
        span_map,
        &[],
        0,
        &mut best_path,
        &mut best_duration,
    );
    best_path
}

/// Compute service duration breakdown with percentages.
fn compute_service_breakdown(
    spans: &[SpanDetail],
    total_duration_ns: i64,
) -> Vec<ServiceBreakdownEntry> {
    let mut service_durations: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for span in spans {
        *service_durations
            .entry(span.service_name.clone())
            .or_insert(0) += span.duration_ns;
    }

    let total = if total_duration_ns > 0 {
        total_duration_ns as f64
    } else {
        1.0
    };
    let mut breakdown: Vec<ServiceBreakdownEntry> = service_durations
        .into_iter()
        .map(|(service, duration_ns)| ServiceBreakdownEntry {
            service,
            duration_ns,
            percentage: (duration_ns as f64 / total) * 100.0,
        })
        .collect();
    breakdown.sort_by(|a, b| b.duration_ns.cmp(&a.duration_ns));
    breakdown
}

#[utoipa::path(
    post,
    path = "/v1/investigate",
    responses(
        (status = 200, description = "Investigation results")
    ),
    tag = "query"
)]
#[instrument(skip_all)]
async fn investigate_handler(
    _path: InvestigatePath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<AuthContext>,
    Json(req): Json<investigate::InvestigateRequest>,
) -> ApiResult<Json<std::collections::BTreeMap<String, investigate::InvestigationBlock>>> {
    // Validate mandatory timestamp parameters.
    if req.from_ts == 0 {
        return Err(bad_request(
            "Invalid Request",
            "from_ts is required and must be a positive Unix timestamp in microseconds",
        ));
    }
    if req.to_ts == 0 {
        return Err(bad_request(
            "Invalid Request",
            "to_ts is required and must be a positive Unix timestamp in microseconds",
        ));
    }
    if req.to_ts <= req.from_ts {
        return Err(bad_request(
            "Invalid Request",
            "to_ts must be greater than from_ts",
        ));
    }

    // This gives a clear 404 instead of "failed to get read connection" when no data is ingested.
    let data_dir = crate::data_dir().context_internal("Investigate", "SIGIRO_DATA_DIR not set")?;
    let project_data_path = data_dir.join(&auth.project_id);
    if !project_data_path.exists()
        || project_data_path
            .read_dir()
            .map(|mut i| i.next().is_none())
            .unwrap_or(true)
    {
        return Err(axum_anyhow::not_found(
            "No Data Found",
            &format!(
                "No data found for project {}. Please ingest data before calling investigate.",
                auth.project_id
            ),
        ));
    }
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

#[utoipa::path(
    post,
    path = "/admin/keys",
    responses(
        (status = 200, description = "Created API key", body = CreateKeyResponse)
    ),
    tag = "admin"
)]
async fn admin_create_key(
    _path: AdminKeysPath,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateKeyRequest>,
) -> ApiResult<Json<CreateKeyResponse>> {
    let raw_key = admin::create_key(&state.admin_conn, &req.project, &req.plan, req.rate_limit)
        .context_internal("Admin", "failed to create API key")?;
    Ok(Json(CreateKeyResponse { key: raw_key }))
}

#[utoipa::path(
    delete,
    path = "/admin/keys/{short_token}",
    responses(
        (status = 200, description = "Key revoked", body = RevokeKeyResponse)
    ),
    tag = "admin"
)]
async fn admin_revoke_key(
    AdminKeyPath { short_token }: AdminKeyPath,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<RevokeKeyResponse>> {
    let revoked = admin::revoke_key(&state.admin_conn, &short_token)
        .context_internal("Admin", "failed to revoke API key")?;
    if revoked {
        Ok(Json(RevokeKeyResponse {
            revoked: true,
            short_token,
        }))
    } else {
        Err(axum_anyhow::not_found(
            "Key Not Found",
            &format!("No key found with short token: {short_token}"),
        ))
    }
}

/// GET /admin/status — returns all instance_status rows as JSON object.
#[utoipa::path(
    get,
    path = "/admin/status",
    responses(
        (status = 200, description = "Server status", body = AdminStatusResponse)
    ),
    tag = "admin"
)]
async fn admin_status(
    _path: AdminStatusPath,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<AdminStatusResponse>> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let map = admin::read_all_status(&guard).context_internal("Admin", "failed to read status")?;
    Ok(Json(AdminStatusResponse { status: map }))
}

// --- Federation endpoints (authenticated, under /v1/federation) ---

/// POST /v1/federation/workers -- register a federation worker.
#[utoipa::path(
    post,
    path = "/v1/federation/workers",
    responses(
        (status = 200, description = "Worker registered", body = RegisterWorkerResponse)
    ),
    tag = "federation"
)]
async fn register_worker_handler(
    _path: FederationWorkersPath,
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
    Json(worker): Json<federation::WorkerInfo>,
) -> ApiResult<Json<RegisterWorkerResponse>> {
    let id = worker.id.clone();
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    federation::register_worker(&guard, &worker)
        .context_internal("Federation", "failed to register worker")?;
    Ok(Json(RegisterWorkerResponse {
        registered: true,
        id,
    }))
}

/// DELETE /v1/federation/workers/{id} -- deregister a federation worker.
#[utoipa::path(
    delete,
    path = "/v1/federation/workers/{id}",
    responses(
        (status = 200, description = "Worker deregistered", body = DeregisterWorkerResponse)
    ),
    tag = "federation"
)]
async fn deregister_worker_handler(
    FederationWorkerPath { id }: FederationWorkerPath,
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<Json<DeregisterWorkerResponse>> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let deregistered = federation::deregister_worker(&guard, &id)
        .context_internal("Federation", "failed to deregister worker")?;
    if deregistered {
        Ok(Json(DeregisterWorkerResponse { deregistered: true }))
    } else {
        Err(axum_anyhow::not_found(
            "Worker Not Found",
            &format!("No worker found with id: {id}"),
        ))
    }
}

/// GET /v1/federation/workers -- list all federation workers.
#[utoipa::path(
    get,
    path = "/v1/federation/workers",
    responses(
        (status = 200, description = "List of workers", body = ListWorkersResponse)
    ),
    tag = "federation"
)]
async fn list_workers_handler(
    _path: FederationWorkersPath,
    State(state): State<Arc<AppState>>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<Json<ListWorkersResponse>> {
    let guard = state
        .admin_conn
        .lock()
        .map_err(|e| anyhow::anyhow!("admin DB mutex: {e}"))?;
    let workers = federation::list_workers(&guard)
        .context_internal("Federation", "failed to list workers")?;
    Ok(Json(ListWorkersResponse { workers }))
}
