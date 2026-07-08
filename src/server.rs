use anyhow::{Context, Result};
use axum::{Extension, Json, Router, extract::State, middleware as axum_mw};
use axum_anyhow::{ApiResult, IntoApiError, ResultExt, bad_request};
use axum_extra::routing::RouterExt;
use axum_server::{Handle, tls_rustls::RustlsConfig};
use parking_lot::Mutex;
use rusqlite::Connection;
use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
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
    /// Admin SQLite for federation worker storage and instance status
    pub admin_conn: Arc<Mutex<Connection>>,
    pub http_client: reqwest::Client,
    pub router: Arc<crate::inserter::MsgRouter>,
    /// Explicit auth mode — Open or Key. Never implicit.
    pub auth_mode: crate::auth::AuthMode,
    /// Resolved data directory.
    /// --- DI pattern: injected at construction; never read from env at request time. ---
    /// The previous design re-derived data_dir from the SIGIRO_DATA_DIR env var
    /// on every health check, which raced with parallel tests (each test
    /// called `std::env::set_var` to set up its own data dir, and the last
    /// writer won for all concurrent tests). Carrying data_dir through
    /// AppState makes the value request-scoped by construction — there is
    /// no shared mutable state to race on.
    pub data_dir: camino::Utf8PathBuf,
    /// True after `run_serve` finishes init (DuckLake attached, schema
    /// created, federation started). Replaces the previous `.ready`
    /// sentinel file — typed in-memory state is checked at request time
    /// instead of a file existence check, which is faster and impossible
    /// to forget to set.
    /// --- DI pattern: production code calls `mark_init_complete()`; tests call `.store(true)` directly. ---
    pub init_complete: Arc<AtomicBool>,
}

impl AppState {
    /// Mark init as complete. Called by `run_serve` at the tail of init.
    /// Idempotent — safe to call more than once.
    pub fn mark_init_complete(&self) {
        self.init_complete.store(true, Ordering::Release);
    }
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

    // Admin routes — no auth required. Self-hosted: expose only internally.
    let admin = Router::new().typed_get(admin_status);

    // Public routes + v1 API + admin + Swagger UI
    Router::new()
        .typed_get(health)
        .typed_get(v1_health)
        .typed_get(healthz)
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
            http: config::HTTP_PORT.value().map_err(anyhow::Error::from)?,
            grpc: config::GRPC_PORT.value().map_err(anyhow::Error::from)?,
            otlp_http: config::OTLP_HTTP_PORT
                .value()
                .map_err(anyhow::Error::from)?,
        })
    }
}

pub async fn run_servers(
    pool: Arc<ProjectPool>,
    admin_conn: Arc<Mutex<Connection>>,
    router: Arc<MsgRouter>,
    data_dir: camino::Utf8PathBuf,
    init_complete: Arc<AtomicBool>,
) -> Result<()> {
    let ports = ServerPorts::load()?;

    let configured_key = crate::config::API_KEY.value().unwrap_or_default();
    let auth_mode = crate::auth::AuthMode::from_config(configured_key.clone());

    // TLS configuration
    let tls_config = match (
        crate::config::TLS_CERT.value(),
        crate::config::TLS_KEY.value(),
    ) {
        (Ok(cert), Ok(key)) => {
            tracing::info!("TLS enabled — cert={cert}, key={key}");
            ensure_rustls_provider()?;
            Some(
                RustlsConfig::from_pem_file(cert, key)
                    .await
                    .context("failed to load TLS config")?,
            )
        }
        (Ok(_), Err(_)) => {
            anyhow::bail!("SIGIRO_TLS_CERT is set but SIGIRO_TLS_KEY is missing");
        }
        (Err(_), Ok(_)) => {
            anyhow::bail!("SIGIRO_TLS_KEY is set but SIGIRO_TLS_CERT is missing");
        }
        (Err(_), Err(_)) => {
            tracing::info!("TLS not configured — serving plain TCP");
            None
        }
    };

    // gRPC server — authenticated via Bearer token in metadata
    let grpc_router = Arc::clone(&router);
    let grpc_auth_mode = auth_mode.clone();
    let grpc_port = ports.grpc;
    let mut servers = JoinSet::new();
    let (grpc_shutdown_tx, grpc_shutdown_rx) = oneshot::channel();
    servers.spawn(async move {
        let addr = format!("[::]:{grpc_port}").parse().expect("valid addr");
        let svc = OtlpGrpcService::new(grpc_router, grpc_auth_mode);
        tracing::info!("gRPC listening on {addr}");
        let result = TonicServer::builder()
            .add_service(svc.trace_service())
            .add_service(svc.logs_service())
            .add_service(svc.metrics_service())
            .add_service(svc.profiles_service())
            .serve_with_shutdown(addr, async move {
                if grpc_shutdown_rx.await.is_err() {
                    tracing::debug!("gRPC shutdown sender dropped");
                }
            })
            .await
            .context("gRPC server failed");
        ("gRPC", result)
    });

    let app_state = Arc::new(AppState {
        pool: Arc::clone(&pool),
        admin_conn: Arc::clone(&admin_conn),
        http_client: reqwest::Client::new(),
        router: Arc::clone(&router),
        auth_mode: auth_mode.clone(),
        data_dir,
        init_complete: Arc::clone(&init_complete),
    });

    {
        let guard = admin_conn.lock();
        federation::create_workers_table(&guard)
            .context("failed to create federation workers table")?;
    }

    let http_app = build_http_router(app_state);

    let http_port = ports.http;
    let http_tls_config = tls_config.clone();
    let http_handle = Handle::new();
    let http_server_handle = http_handle.clone();
    servers.spawn(async move {
        let result = serve_http_app(
            "HTTP",
            http_port,
            http_app,
            http_tls_config,
            http_server_handle,
        )
        .await;
        ("HTTP", result)
    });

    // OTLP HTTP — authenticated via Authorization header
    let otlp_http_app = crate::ingest::otlp_http_router(Arc::clone(&router), auth_mode)
        .layer(tower_http::decompression::RequestDecompressionLayer::new());
    let otlp_http_port = ports.otlp_http;
    let otlp_tls_config = tls_config;
    let otlp_http_handle = Handle::new();
    let otlp_http_server_handle = otlp_http_handle.clone();
    servers.spawn(async move {
        let result = serve_http_app(
            "OTLP HTTP",
            otlp_http_port,
            otlp_http_app,
            otlp_tls_config,
            otlp_http_server_handle,
        )
        .await;
        ("OTLP HTTP", result)
    });

    // Graceful shutdown: wait for SIGTERM or SIGINT, then drain.
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

    // Data retention background task (only if retention is configured).
    let retention_handle = spawn_retention_task(Arc::clone(&pool));

    let server_error = tokio::select! {
        result = servers.join_next() => {
            let error = match result {
                Some(Ok((name, Ok(())))) => anyhow::anyhow!("{name} server exited unexpectedly"),
                Some(Ok((name, Err(error)))) => error.context(format!("{name} server exited")),
                Some(Err(error)) => anyhow::Error::new(error).context("server task panicked"),
                None => anyhow::anyhow!("server task set became empty unexpectedly"),
            };
            tracing::error!("{error:#}");
            Some(error)
        },
        _ = sigterm.recv() => {
            tracing::info!("SIGTERM received — starting graceful shutdown");
            None
        },
        _ = sigint.recv() => {
            tracing::info!("SIGINT received — starting graceful shutdown");
            None
        },
    };

    http_handle.graceful_shutdown(None);
    otlp_http_handle.graceful_shutdown(None);
    let shutdown_send = grpc_shutdown_tx
        .send(())
        .map_err(|_| anyhow::anyhow!("gRPC shutdown receiver dropped unexpectedly"));
    let server_cleanup = match tokio::time::timeout(Duration::from_secs(10), async {
        let mut first_error = None;
        while let Some(result) = servers.join_next().await {
            match result {
                Ok((_name, Ok(()))) => {}
                Ok((name, Err(error))) => {
                    tracing::error!("{name} server failed while stopping: {error:#}");
                    first_error.get_or_insert_with(|| {
                        error.context(format!("{name} server failed while stopping"))
                    });
                }
                Err(error) => {
                    tracing::error!("server task failed while stopping: {error}");
                    first_error.get_or_insert_with(|| {
                        anyhow::Error::new(error).context("server task failed while stopping")
                    });
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    })
    .await
    {
        Ok(result) => result,
        Err(_) => {
            servers.abort_all();
            while servers.join_next().await.is_some() {}
            Err(anyhow::anyhow!(
                "server connections did not drain within 10 seconds"
            ))
        }
    };

    let retention_cleanup = if let Some(handle) = retention_handle {
        handle.abort();
        match handle.await {
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(anyhow::Error::new(error).context("retention task failed to stop")),
            Ok(()) => Ok(()),
        }
    } else {
        Ok(())
    };

    // Network tasks are gone, so no request can create a new flusher while the
    // existing channels are closed and drained.
    tracing::info!("draining flusher tasks...");
    let router_cleanup = router
        .shutdown_and_wait()
        .await
        .context("failed to drain flusher tasks");

    if let Some(error) = server_error {
        return Err(error);
    }
    shutdown_send?;
    server_cleanup?;
    retention_cleanup?;
    router_cleanup?;

    Ok(())
}

fn ensure_rustls_provider() -> Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return Ok(());
    }

    match rustls::crypto::aws_lc_rs::default_provider().install_default() {
        Ok(()) => Ok(()),
        Err(_) if rustls::crypto::CryptoProvider::get_default().is_some() => Ok(()),
        Err(_) => anyhow::bail!("failed to install Rustls AWS-LC crypto provider"),
    }
}

async fn serve_http_app(
    name: &'static str,
    port: u16,
    app: Router,
    tls_config: Option<RustlsConfig>,
    handle: Handle<SocketAddr>,
) -> Result<()> {
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    if let Some(config) = tls_config {
        tracing::info!(server = name, protocol = "HTTPS", "listening on {addr}");
        axum_server::bind_rustls(addr, config)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .with_context(|| format!("{name} HTTPS server failed"))
    } else {
        tracing::info!(server = name, protocol = "HTTP", "listening on {addr}");
        axum_server::bind(addr)
            .handle(handle)
            .serve(app.into_make_service())
            .await
            .with_context(|| format!("{name} HTTP server failed"))
    }
}

/// Spawn a background task that periodically deletes old data based on
/// SIGIRO_RETENTION_DAYS. Runs once per hour.
fn spawn_retention_task(pool: Arc<ProjectPool>) -> Option<tokio::task::JoinHandle<()>> {
    let days = match crate::config::RETENTION_DAYS.value() {
        Ok(d) if d > 0 => d,
        _ => return None,
    };

    tracing::info!(retention_days = days, "data retention enabled");

    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if let Err(e) = run_retention_cleanup(&pool, days).await {
                tracing::error!("retention cleanup failed: {e:#}");
            }
        }
    }))
}

/// Execute retention cleanup: delete spans/logs/metrics older than N days.
async fn run_retention_cleanup(pool: &ProjectPool, days: u32) -> anyhow::Result<()> {
    let cutoff = format!("now() - INTERVAL '{days} days'");
    let tables = [
        "lake.sigiro_spans",
        "lake.sigiro_logs",
        "lake.sigiro_metrics_sum",
        "lake.sigiro_metrics_gauge",
        "lake.sigiro_metrics_histogram",
        "lake.sigiro_metrics_exp_histogram",
        "lake.sigiro_profiles",
    ];

    for project_entry in pool.connections.iter() {
        let project_id = project_entry.key().clone();
        let conn = Arc::clone(&project_entry.conn);
        for table in &tables {
            let sql = format!("DELETE FROM {table} WHERE timestamp < {cutoff}");
            let c = Arc::clone(&conn);
            let t = table.to_string();
            match tokio::task::spawn_blocking(move || {
                let guard = c.lock();
                guard.execute_batch(&sql)
            })
            .await
            {
                Ok(Ok(_)) => {
                    tracing::debug!(project = %project_id, table = %t, "retention cleanup completed")
                }
                Ok(Err(e)) => {
                    tracing::warn!(project = %project_id, table = %t, "retention cleanup error: {e}")
                }
                Err(e) => {
                    tracing::warn!(project = %project_id, table = %t, "retention cleanup task failed: {e}")
                }
            }
        }
    }
    Ok(())
}

#[instrument(skip(state))]
async fn health(
    _path: HealthPath,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    // 1. Flusher health — most critical. A dead flusher silently drops
    //    telemetry, so we must report degraded even if init completed.
    if state.router.has_dead_flushers() {
        tracing::warn!("health check: dead flushers detected");
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse::Degraded {
                reason: "dead flushers".to_string(),
            }),
        );
    }

    // 2. Init completeness — `AppState::init_complete` is flipped at the
    //    tail of `run_serve`. While the HTTP listener is up before init
    //    finishes, the server is not actually serving traffic yet, so
    //    probes (k8s readiness, docker healthcheck) must see 503.
    if !state.init_complete.load(Ordering::Acquire) {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse::Starting),
        );
    }

    tracing::info!("health check called");
    (
        axum::http::StatusCode::OK,
        axum::Json(HealthResponse::Ready),
    )
}

#[instrument(skip(state))]
async fn v1_health(
    _path: V1HealthPath,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    health(HealthPath, State(state)).await
}

#[instrument(skip(state))]
async fn healthz(
    _path: HealthzPath,
    State(state): State<Arc<AppState>>,
) -> impl axum::response::IntoResponse {
    health(HealthPath, State(state)).await
}

/// Convert a DuckDB ValueRef to a serde_json::Value.
/// Uses pattern matching on the actual type — no cascading type-try guessing.
/// Public for reuse by HTTP query handling and tests.
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

/// Escape a string for safe use inside a SQL single-quoted string literal.
fn escape_sql_string(s: &str) -> String {
    s.replace("'", "''")
}

/// Filter out the current node from the worker list to avoid forwarding loops.
fn exclude_self_workers(
    workers: Vec<crate::federation::WorkerInfo>,
) -> Vec<crate::federation::WorkerInfo> {
    let advertise_endpoint = crate::config::ADVERTISE_ENDPOINT.value().ok();
    let endpoint = crate::config::ENDPOINT.value().ok();
    workers
        .into_iter()
        .filter(|w| {
            if w.id.is_empty() {
                return false;
            }
            if let Some(ref adv) = advertise_endpoint {
                if w.endpoint == *adv {
                    return false;
                }
            }
            if let Some(ref ep) = endpoint {
                if w.endpoint == *ep {
                    return false;
                }
            }
            // Also exclude obvious localhost self-references
            if w.endpoint.contains("://localhost:") || w.endpoint.contains("://127.0.0.1:") {
                return false;
            }
            true
        })
        .collect()
}

/// Execute SQL against DuckDB (local or federated).
async fn execute_sql(
    state: &AppState,
    auth: &AuthContext,
    sql: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let workers = exclude_self_workers(federation::list_workers(&state.admin_conn.lock())?);
    if !workers.is_empty() {
        let quack_port = crate::config::QUACK_PORT
            .value()
            .expect("SIGIRO_QUACK_PORT must be set");
        let quack_token = crate::config::QUACK_TOKEN
            .value()
            .expect("SIGIRO_QUACK_TOKEN must be set");
        // Federated queries don't need a local project connection;
        // each worker is queried via HTTP over the network.
        let workers_clone = workers.to_vec();
        let sql_clone = sql.to_string();
        let result = tokio::task::spawn_blocking(move || {
            // dummy connection to satisfy the signature (not actually used)
            let dummy_conn = duckdb::Connection::open_in_memory()
                .map_err(|e| anyhow::anyhow!("failed to open dummy connection: {e}"))?;
            crate::quack_federation::blocking_federated_query(
                &dummy_conn,
                &workers_clone,
                quack_port,
                &quack_token,
                &sql_clone,
            )
        })
        .await??;
        return Ok(result);
    }

    // No federation workers — execute locally against the project connection.
    let conn_raw =
        project_conn(state, auth).map_err(|e: axum_anyhow::ApiError| anyhow::anyhow!("{:?}", e))?;
    let sql_clone = sql.to_string();
    tokio::task::spawn_blocking(move || {
        let conn_guard = conn_raw.lock();
        let conn_ref: &duckdb::Connection = &conn_guard;
        let mut stmt = conn_ref
            .prepare(&sql_clone)
            .map_err(|e| anyhow::anyhow!("failed to prepare query: {e}"))?;
        let _dummy = stmt
            .query([])
            .map_err(|e| anyhow::anyhow!("failed to execute query: {e}"))?;
        drop(_dummy);
        let column_count = stmt.column_count();
        let column_names: Vec<String> = (0..column_count)
            .map(|i| {
                stmt.column_name(i)
                    .map_or("?".to_string(), std::string::ToString::to_string)
            })
            .collect();
        let mut rows = stmt.query([])?;
        let mut results = Vec::new();
        while let Ok(Some(row)) = rows.next() {
            let mut obj = serde_json::Map::new();
            for (i, name) in column_names.iter().enumerate() {
                let val = row
                    .get_ref(i)
                    .map_err(|e| anyhow::anyhow!("column read failed: {e}"))?;
                obj.insert(name.clone(), duckdb_value_to_json(val));
            }
            results.push(serde_json::Value::Object(obj));
        }
        Ok(results)
    })
    .await?
}

fn duckdb_value_to_json(val: duckdb::types::ValueRef<'_>) -> serde_json::Value {
    use duckdb::types::ValueRef;
    match val {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Boolean(b) => serde_json::Value::Bool(b),
        ValueRef::TinyInt(b) => serde_json::json!(b),
        ValueRef::SmallInt(b) => serde_json::json!(b),
        ValueRef::Int(b) => serde_json::json!(b),
        ValueRef::BigInt(b) => serde_json::json!(b),
        ValueRef::Float(b) => serde_json::json!(b),
        ValueRef::Double(b) => serde_json::json!(b),
        ValueRef::Text(b) => serde_json::Value::String(String::from_utf8_lossy(b).to_string()),
        ValueRef::Blob(b) => serde_json::Value::String(format!("[blob {} bytes]", b.len())),
        _ => serde_json::Value::String(format!("{:?}", val)),
    }
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
    let results = execute_sql(&state, &auth, &body)
        .await
        .map_err(|e| bad_request("query rejected", &format!("{e:#}")))?;
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
    let results = execute_sql(&state, &auth, &sql)
        .await
        .map_err(|e| bad_request("services query failed", &format!("{e:#}")))?;

    let mut services = Vec::new();
    for row in results {
        let obj = row.as_object().ok_or_else(|| {
            bad_request("services query failed", "expected object row from query")
        })?;
        let name = obj
            .get("service_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let first_seen = obj
            .get("first_seen")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);
        let last_seen = obj
            .get("last_seen")
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);
        let span_count = obj
            .get("span_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
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
    let prefix = ProjectPool::prefix();
    let trace_id = _path.trace_id.clone();
    let safe_trace_id = escape_sql_string(&trace_id);

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
         WHERE trace_id = '{}'
         ORDER BY timestamp",
        prefix, safe_trace_id
    );
    let results = execute_sql(&state, &auth, &sql)
        .await
        .map_err(|e| bad_request("trace detail query failed", &format!("{e:#}")))?;

    let mut spans: Vec<SpanDetail> = Vec::new();
    let mut span_map: std::collections::HashMap<String, SpanDetail> =
        std::collections::HashMap::new();
    let mut parent_to_children: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut root_span_id: Option<String> = None;
    let mut total_duration_us: i64 = 0;
    let mut error_count: i64 = 0;
    let mut services_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut max_duration_span: Option<SpanDetail> = None;

    for row in results {
        let obj = row.as_object().ok_or_else(|| {
            bad_request(
                "trace detail query failed",
                "expected object row from query",
            )
        })?;

        let span_id = obj
            .get("span_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let parent_span_id = obj
            .get("parent_span_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let duration_us = obj
            .get("duration")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let status_code = obj
            .get("status_code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0) as i32;
        let service_name = obj
            .get("service_name")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let span = SpanDetail {
            trace_id: obj
                .get("trace_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            span_id: span_id.clone(),
            parent_span_id: if parent_span_id.is_empty() {
                None
            } else {
                Some(parent_span_id.clone())
            },
            span_name: obj
                .get("span_name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            duration_us,
            status_code,
            service_name: service_name.clone(),
            timestamp: obj
                .get("start_timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            span_kind: obj
                .get("span_kind")
                .and_then(serde_json::Value::as_i64)
                .map(|n| n as i32),
            span_attributes: obj
                .get("span_attributes")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            resource_attributes: obj
                .get("resource_attributes")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            events_json: obj
                .get("events_json")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            links_json: obj
                .get("links_json")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            scope_name: obj
                .get("scope_name")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            scope_version: obj
                .get("scope_version")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            trace_state: obj
                .get("trace_state")
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
        };
        total_duration_us = total_duration_us.saturating_add(duration_us);
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
            .map(|s| duration_us > s.duration_us)
            .unwrap_or(true);
        if is_longer {
            max_duration_span = Some(span.clone());
        }

        span_map.insert(span_id.clone(), span.clone());
        spans.push(span);
    }

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
    let service_breakdown = compute_service_breakdown(&spans, total_duration_us);

    Ok(Json(TraceDetailResponse {
        trace_id,
        total_duration_us,
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
        let new_duration = current_duration.saturating_add(span.duration_us);

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
    total_duration_us: i64,
) -> Vec<ServiceBreakdownEntry> {
    let mut service_durations: std::collections::HashMap<String, i64> =
        std::collections::HashMap::new();
    for span in spans {
        *service_durations
            .entry(span.service_name.clone())
            .or_insert(0) += span.duration_us;
    }

    let total = if total_duration_us > 0 {
        total_duration_us as f64
    } else {
        1.0
    };
    let mut breakdown: Vec<ServiceBreakdownEntry> = service_durations
        .into_iter()
        .map(|(service, duration_us)| ServiceBreakdownEntry {
            service,
            duration_us,
            percentage: (duration_us as f64 / total) * 100.0,
        })
        .collect();
    breakdown.sort_by(|a, b| b.duration_us.cmp(&a.duration_us));
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

    let workers = exclude_self_workers(
        federation::list_workers(&state.admin_conn.lock())
            .context_internal("Investigate", "failed to list workers")?,
    );

    if !workers.is_empty() {
        // Federated investigate: forward to all workers and merge results.
        let api_key = crate::config::API_KEY
            .value()
            .context_internal("Investigate", "SIGIRO_API_KEY not set")?;
        let mut merged: std::collections::BTreeMap<String, investigate::InvestigationBlock> =
            std::collections::BTreeMap::new();

        for worker in &workers {
            let url = format!("{}/v1/investigate", worker.endpoint);
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .context_internal("Investigate", "failed to build HTTP client")?;
            let response = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", api_key))
                .header("Content-Type", "application/json")
                .json(&req)
                .send()
                .await
                .with_context(|| format!("HTTP request failed for worker {}", worker.id))
                .context_internal("Investigate", "worker request failed")?;

            if !response.status().is_success() {
                let status = response.status();
                let text = response.text().await.unwrap_or_default();
                tracing::warn!(worker_id=%worker.id, status=%status, text=%text, "worker investigate failed");
                continue;
            }

            let blocks: std::collections::BTreeMap<String, investigate::InvestigationBlock> =
                response
                    .json()
                    .await
                    .with_context(|| format!("failed to parse JSON from worker {}", worker.id))
                    .context_internal("Investigate", "worker response parse failed")?;

            for (service, block) in blocks {
                match merged.entry(service) {
                    std::collections::btree_map::Entry::Vacant(e) => {
                        e.insert(block);
                    }
                    std::collections::btree_map::Entry::Occupied(mut e) => {
                        merge_investigation_blocks(e.get_mut(), block);
                    }
                }
            }
        }

        if merged.is_empty() {
            return Err(axum_anyhow::not_found(
                "No Data Found",
                "No investigation results from any worker.",
            ));
        }

        return Ok(Json(merged));
    }

    // Local investigate (no workers).
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

/// Merge `other` into `base` for the same service.
fn merge_investigation_blocks(
    base: &mut investigate::InvestigationBlock,
    other: investigate::InvestigationBlock,
) {
    // Merge truncation counts.
    base.truncation.spans_total += other.truncation.spans_total;
    base.truncation.spans_returned += other.truncation.spans_returned;
    base.truncation.logs_total += other.truncation.logs_total;
    base.truncation.logs_returned += other.truncation.logs_returned;
    base.truncation.error_traces_total += other.truncation.error_traces_total;
    base.truncation.error_traces_returned += other.truncation.error_traces_returned;
    base.truncation.linked_spans_total += other.truncation.linked_spans_total;
    base.truncation.linked_spans_returned += other.truncation.linked_spans_returned;

    // Merge coverage.
    base.coverage.has_spans |= other.coverage.has_spans;
    base.coverage.has_logs |= other.coverage.has_logs;
    base.coverage.has_metrics |= other.coverage.has_metrics;
    base.coverage.has_profiles |= other.coverage.has_profiles;
    base.coverage.span_count += other.coverage.span_count;
    base.coverage.log_template_count += other.coverage.log_template_count;
    base.coverage.metric_series_count += other.coverage.metric_series_count;
    base.coverage.profile_sample_count += other.coverage.profile_sample_count;

    // Merge context pod_count (take max since it's instance count per node).
    base.context.pod_count = base.context.pod_count.max(other.context.pod_count);
    base.context.dependencies.extend(other.context.dependencies);
    base.context.dependents.extend(other.context.dependents);
    base.context
        .dependencies
        .sort_by(|a, b| a.service_name.cmp(&b.service_name));
    base.context
        .dependencies
        .dedup_by(|a, b| a.service_name == b.service_name);
    base.context
        .dependents
        .sort_by(|a, b| a.service_name.cmp(&b.service_name));
    base.context
        .dependents
        .dedup_by(|a, b| a.service_name == b.service_name);

    // Merge vectors.
    base.events.extend(other.events);
    // Deduplicate operation summaries by span_name, summing counts.
    {
        let mut op_map: std::collections::BTreeMap<String, investigate::OperationSummary> =
            std::collections::BTreeMap::new();
        for op in base.operation_summaries.drain(..) {
            let key = op.span_name.clone();
            op_map.insert(key, op);
        }
        for op in other.operation_summaries {
            match op_map.entry(op.span_name.clone()) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(op);
                }
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    let existing = e.get_mut();
                    existing.count += op.count;
                    existing.error_count += op.error_count;
                    existing.error_pct = if existing.count > 0 {
                        (existing.error_count as f64 / existing.count as f64) * 100.0
                    } else {
                        0.0
                    };
                    existing.duration_p50_ns =
                        (existing.duration_p50_ns + op.duration_p50_ns) / 2.0;
                    existing.duration_p95_ns = existing.duration_p95_ns.max(op.duration_p95_ns);
                    existing.duration_p99_ns = existing.duration_p99_ns.max(op.duration_p99_ns);
                    if existing.exemplar_error_trace_id.is_none() {
                        existing.exemplar_error_trace_id = op.exemplar_error_trace_id;
                    }
                }
            }
        }
        base.operation_summaries = op_map.into_values().collect();
    }
    base.metrics.extend(other.metrics);
    base.traces.extend(other.traces);
    base.traces.sort_by(|a, b| a.trace_id.cmp(&b.trace_id));
    base.traces.dedup_by(|a, b| a.trace_id == b.trace_id);
    base.timeline.extend(other.timeline);
    base.log_patterns.extend(other.log_patterns);
    base.logs.extend(other.logs);
    base.linked_traces.extend(other.linked_traces);
    base.linked_traces
        .sort_by(|a, b| a.trace_id.cmp(&b.trace_id));
    base.linked_traces.dedup_by(|a, b| a.trace_id == b.trace_id);
    base.profiles.extend(other.profiles);

    // Merge signals: keep the one that's available if the other isn't.
    let mut signal_map: std::collections::BTreeMap<String, investigate::SignalCoverage> = base
        .coverage
        .signals
        .drain(..)
        .map(|s| (s.signal_type.clone(), s))
        .collect();
    for s in other.coverage.signals {
        match signal_map.entry(s.signal_type.clone()) {
            std::collections::btree_map::Entry::Vacant(e) => {
                e.insert(s);
            }
            std::collections::btree_map::Entry::Occupied(mut e) => {
                if s.available && !e.get().available {
                    e.insert(s);
                }
            }
        }
    }
    base.coverage.signals = signal_map.into_values().collect();
}

// --- Admin endpoints ---
// Self-hosted: admin endpoints are unauthenticated. Run sigiro behind a
// reverse proxy or firewall if you need to restrict access.

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
    let guard = state.admin_conn.lock();
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
    let guard = state.admin_conn.lock();
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
    let guard = state.admin_conn.lock();
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
    let guard = state.admin_conn.lock();
    let workers = federation::list_workers(&guard)
        .context_internal("Federation", "failed to list workers")?;
    Ok(Json(ListWorkersResponse { workers }))
}
