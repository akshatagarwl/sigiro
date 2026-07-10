use anyhow::{Context, Result};
use axum::{Extension, Json, Router, extract::State, http::HeaderMap, middleware as axum_mw};
use axum_anyhow::{ApiResult, IntoApiError, ResultExt, bad_request};
use axum_extra::routing::RouterExt;
use axum_server::{Handle, tls_rustls::RustlsConfig};
use parking_lot::Mutex;
use rusqlite::Connection;
use sqlparser::ast::{
    Expr, ObjectName, Query as SqlQuery, SetExpr, Statement, TableFactor, Visit, Visitor,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};
use std::net::SocketAddr;
use std::ops::ControlFlow;
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
use crate::ingest::OtlpGrpcService;
use crate::inserter::MsgRouter;
use crate::investigate;
use crate::pool::{ProjectPool, ReaderPool};
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
        create_tenant_handler,
        create_key_handler,
        revoke_key_handler,
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
            CreateTenantRequest,
            CreateTenantResponse,
            CreateKeyRequest,
            AdminStatusResponse,
        )
    ),
    tags(
        (name = "query", description = "SQL query endpoints"),
        (name = "admin", description = "Admin endpoints"),
            )
)]
pub struct ApiDoc;

/// Rate-limit key: the Bearer token when present, otherwise the peer IP.
/// Keying unauthenticated requests by peer IP (instead of a single shared
/// placeholder) stops one anonymous client's burst from throttling every
/// other anonymous client — the bug that made open-mode queries appear to
/// intermittently "lose" data. ConnectInfo is populated by the connect-info
/// make-service in `serve_http_app`.
#[derive(Clone)]
struct RateLimitKeyExtractor;

impl KeyExtractor for RateLimitKeyExtractor {
    type Key = String;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        if let Some(token) = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            return Ok(format!("tok:{token}"));
        }
        let ip = req
            .extensions()
            .get::<axum::extract::ConnectInfo<SocketAddr>>()
            .map(|ci| ci.0.ip());
        Ok(match ip {
            Some(ip) => format!("ip:{ip}"),
            None => "ip:unknown".to_string(),
        })
    }
}

/// Render a `GovernorError` as a JSON body matching the shape of every other
/// API error (`{status, title, detail}`) so a rate-limited client cannot
/// mistake the plain-text default ("Too Many Requests!") for query output.
fn rate_limit_error_response(err: tower_governor::GovernorError) -> axum::response::Response {
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use tower_governor::GovernorError;

    let (status, detail, extra_headers) = match err {
        GovernorError::TooManyRequests { wait_time, headers } => (
            StatusCode::TOO_MANY_REQUESTS,
            format!("rate limit exceeded — retry after {wait_time}s"),
            headers,
        ),
        GovernorError::UnableToExtractKey => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "unable to extract rate-limit key".to_string(),
            None,
        ),
        GovernorError::Other { code, msg, headers } => (
            code,
            msg.unwrap_or_else(|| "rate limit error".to_string()),
            headers,
        ),
    };
    let body = serde_json::json!({
        "status": status.as_u16(),
        "title": "rate limited",
        "detail": detail,
    });
    let mut response = (status, Json(body)).into_response();
    if let Some(headers) = extra_headers {
        response.headers_mut().extend(headers);
    }
    response
}

pub struct AppState {
    pub pool: Arc<ProjectPool>,
    /// Admin SQLite for instance status (and, historically, worker registry).
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
    /// created). Replaces the previous `.ready` sentinel file — typed
    /// in-memory state is checked at request time instead of a file
    /// existence check, which is faster and impossible to forget to set.
    /// --- DI pattern: production code calls `mark_init_complete()`; tests call `.store(true)` directly. ---
    pub init_complete: Arc<AtomicBool>,
    /// Which cross-project query surface (if any) this node exposes.
    /// Currently inert — set from `SIGIRO_QUERY_SURFACE` at startup, read by
    /// a future feature (see `crate::config::QuerySurface`).
    pub query_surface: crate::config::QuerySurface,
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
/// This SQL-layer denylist is defense-in-depth: in local mode the engine-level
/// filesystem lockdown (pool::lock_filesystem) is the real, name-independent
/// guard, but distributed mode has no engine lock, and the denylist also shrinks
/// the in-data_dir cross-project read surface. A denylist can never be complete
/// against DuckDB's reader family; keep the engine lockdown as the primary guard.
const BLOCKED_TABLE_FUNCTIONS: &[&str] = &[
    "DROP",
    "READ_CSV",
    "READ_CSV_AUTO",
    "READ_PARQUET",
    "PARQUET_SCAN",
    "READ_JSON",
    "READ_JSON_AUTO",
    "READ_NDJSON",
    "READ_NDJSON_AUTO",
    "READ_TEXT",
    "READ_BLOB",
    "GLOB",
    "SNIFF_CSV",
];

/// DuckLake time-travel qualifier, e.g. `AT (VERSION => 3)` or
/// `AT (TIMESTAMP => '2026-07-09 19:55:00')`. sqlparser's GenericDialect can't
/// parse the `=>` form, so it is stripped before the SELECT-only structural
/// check. The keyword denylist below still scans the ORIGINAL sql, so this
/// widens reads without weakening the write/DDL/file-access guards.
static TIME_TRAVEL_AT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"(?i)\bAT\s*\([^)]*=>[^)]*\)").expect("valid time-travel regex")
});

/// The entire allowed FROM-target surface for user queries: every table
/// sigiro itself creates (src/schema.rs CREATE TABLE names) plus the
/// read-only `sigiro_anomalies` view it also creates there. Matched
/// case-insensitively against the FINAL identifier — see `is_allowed_table`
/// for how the schema-qualifier prefix (`lake.` / `main.`) is handled.
const ALLOWED_TABLES: &[&str] = &[
    "SIGIRO_SPANS",
    "SIGIRO_LOGS",
    "SIGIRO_LOG_TEMPLATES",
    "SIGIRO_METRICS_GAUGE",
    "SIGIRO_METRICS_SUM",
    "SIGIRO_METRICS_HISTOGRAM",
    "SIGIRO_METRICS_EXP_HISTOGRAM",
    "SIGIRO_PROFILES",
    "SIGIRO_ANOMALIES",
];

/// Table-valued / pragma functions sigiro's OWN code calls through this same
/// `/v1/query` guard — allowlist EXACTLY these, sourced from real call sites:
/// - `PRAGMA_DATABASE_SIZE` — src/diagnose.rs `check_duckdb_health` (DuckDB
///   memory-limit/usage self-check via `FROM pragma_database_size()`).
/// - `RANGE` — tests/api_integration_test.rs
///   `test_query_midstream_error_never_returns_partial_rows` (`FROM range(5) t(i)`,
///   a pure row generator with no filesystem/network access).
const SAFE_FUNCTIONS: &[&str] = &["PRAGMA_DATABASE_SIZE", "RANGE"];

/// Schema qualifiers accepted ahead of an allowed table name. DuckLake attaches
/// its catalog `AS lake` (src/storage.rs); the same data reads as schema `main`
/// under a local-file catalog and `lake` under a PostgreSQL-backed one (see
/// tests/e2e/docker/run.sh's `PROFILE_SCHEMA` detection). `information_schema`
/// is DuckDB's static, file-system-free catalog metadata — inherently safe to
/// expose in full (used by the same e2e script to detect the schema above).
fn is_allowed_table(name: &ObjectName) -> bool {
    let idents = &name.0;
    match idents.len() {
        1 => ALLOWED_TABLES.contains(&idents[0].value.to_uppercase().as_str()),
        2 => {
            let schema = idents[0].value.to_uppercase();
            let table = idents[1].value.to_uppercase();
            if schema == "INFORMATION_SCHEMA" {
                return true;
            }
            matches!(schema.as_str(), "LAKE" | "MAIN") && ALLOWED_TABLES.contains(&table.as_str())
        }
        // Fail closed: no legitimate sigiro query uses 3+-part relation names.
        _ => false,
    }
}

/// A table-valued-function / pivot-style relation is safe only if its
/// (possibly qualified) name is on `SAFE_FUNCTIONS` — matched on the final
/// identifier so `main.pragma_database_size()` and `pragma_database_size()`
/// are treated the same.
fn is_safe_function(name: &ObjectName) -> bool {
    name.0
        .last()
        .is_some_and(|ident| SAFE_FUNCTIONS.contains(&ident.value.to_uppercase().as_str()))
}

/// Reject the Postgres/DuckDB `TABLE <name>` query-shorthand (equivalent to
/// `SELECT * FROM <name>`). It is a distinct `SetExpr::Table` AST node whose
/// `table_name`/`schema_name` fields are plain `String`s, not an `ObjectName`
/// routed through sqlparser's relation/table-factor visitor hooks — so
/// without this explicit check it would bypass `AllowlistVisitor` entirely.
/// Not used anywhere in sigiro's own queries or tests, so fail closed.
fn reject_table_shorthand(expr: &SetExpr) -> ControlFlow<String> {
    match expr {
        SetExpr::SetOperation { left, right, .. } => match reject_table_shorthand(left) {
            ControlFlow::Continue(()) => reject_table_shorthand(right),
            brk => brk,
        },
        SetExpr::Table(_) => {
            ControlFlow::Break("query rejected: TABLE shorthand not allowed".to_string())
        }
        _ => ControlFlow::Continue(()),
    }
}

/// Walks the full AST (via sqlparser's `visitor` feature — CTEs, set
/// operations, every FROM/JOIN, derived tables, and subqueries nested in
/// expressions/WHERE/scalar positions are all covered by the crate's own
/// recursive Visit derive, not hand-enumerated here) and rejects any
/// relation that is not an allowed sigiro table or safe function.
///
/// This closes the replacement-scan hole: `SELECT * FROM 's3://...'` or
/// `FROM '/etc/passwd'` parses as `TableFactor::Table` with a
/// single-element, unquoted-string `ObjectName` — it is not on
/// `ALLOWED_TABLES`, so `is_allowed_table` rejects it regardless of what
/// DuckDB's replacement-scan would have done with it.
#[derive(Default)]
struct AllowlistVisitor {
    /// Every `WITH <alias> AS (...)` name seen so far. `pre_visit_query`
    /// registers a query's own CTE aliases before the derived Visit impl
    /// recurses into its `with`/`body` children, so a CTE is always known by
    /// the time anything (a sibling CTE, the main body, or the CTE's own
    /// RECURSIVE self-reference) tries to select FROM it — a CTE alias is
    /// not a real table and would otherwise be rejected by `is_allowed_table`.
    cte_names: std::collections::HashSet<String>,
}

impl Visitor for AllowlistVisitor {
    type Break = String;

    fn pre_visit_query(&mut self, query: &SqlQuery) -> ControlFlow<String> {
        if let Some(with) = &query.with {
            for cte in &with.cte_tables {
                self.cte_names.insert(cte.alias.name.value.to_uppercase());
            }
        }
        reject_table_shorthand(&query.body)
    }

    fn pre_visit_table_factor(&mut self, table_factor: &TableFactor) -> ControlFlow<String> {
        match table_factor {
            TableFactor::Table { name, args, .. } => {
                let allowed = if args.is_some() {
                    is_safe_function(name)
                } else {
                    self.is_known_cte(name) || is_allowed_table(name)
                };
                if allowed {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(format!("query rejected: {name} not allowed"))
                }
            }
            // Subqueries / parenthesized joins: no relation of their own,
            // recurse into their children (handled automatically by the
            // derived Visit impl once we return Continue here).
            TableFactor::Derived { .. } | TableFactor::NestedJoin { .. } => {
                ControlFlow::Continue(())
            }
            TableFactor::Function { name, .. } => {
                if is_safe_function(name) {
                    ControlFlow::Continue(())
                } else {
                    ControlFlow::Break(format!("query rejected: function {name} not allowed"))
                }
            }
            TableFactor::TableFunction { expr, .. } => match expr {
                Expr::Function(f) if is_safe_function(&f.name) => ControlFlow::Continue(()),
                _ => ControlFlow::Break("query rejected: table function not allowed".to_string()),
            },
            // UNNEST, PIVOT, UNPIVOT, JSON_TABLE, OPENJSON, MATCH_RECOGNIZE, etc.
            // — fail closed, none of sigiro's own queries or tests use them.
            other => ControlFlow::Break(format!(
                "query rejected: relation kind not allowed: {other}"
            )),
        }
    }

    fn pre_visit_statement(&mut self, _statement: &Statement) -> ControlFlow<String> {
        // The only Statement this visitor ever walks into is the top-level
        // Statement::Query we matched on before calling `.visit()` — any
        // OTHER Statement node reachable here means one snuck in as a nested
        // AST node (e.g. a data-modifying CTE arm parsed as
        // `SetExpr::Insert(Statement)`/`SetExpr::Update(Statement)`). Reject
        // it; a read-only query tree never legitimately contains one.
        ControlFlow::Break("query rejected: nested statement not allowed".to_string())
    }
}

impl AllowlistVisitor {
    /// A bare (unqualified, single-part) name matching a CTE alias seen so
    /// far is a reference to that CTE, not a real table.
    fn is_known_cte(&self, name: &ObjectName) -> bool {
        match name.0.as_slice() {
            [ident] => self.cte_names.contains(&ident.value.to_uppercase()),
            _ => false,
        }
    }
}

pub fn validate_read_only(sql: &str) -> Result<(), String> {
    let dialect = GenericDialect {};
    let parseable = TIME_TRAVEL_AT.replace_all(sql, " ");
    let statements = Parser::parse_sql(&dialect, &parseable)
        .map_err(|e| format!("query rejected: parse error: {e}"))?;

    for stmt in &statements {
        let Statement::Query(query) = stmt else {
            return Err("query rejected: only SELECT queries are allowed".to_string());
        };
        if let ControlFlow::Break(msg) = query.visit(&mut AllowlistVisitor::default()) {
            return Err(msg);
        }
    }

    let tokens = Tokenizer::new(&dialect, sql)
        .tokenize()
        .map_err(|e| format!("query rejected: tokenize error: {e}"))?;
    for token in &tokens {
        if let Token::Word(w) = token {
            let word = w.value.to_uppercase();
            if BLOCKED_TABLE_FUNCTIONS.contains(&word.as_str()) {
                return Err(format!("query rejected: {word} not allowed"));
            }
        }
    }
    Ok(())
}

/// Build the HTTP Router for the main API (port 9999).
/// Extracted from `run_servers` so integration tests can build the same
/// router with test state without starting TCP listeners.
pub fn build_http_router(app_state: Arc<AppState>) -> Router {
    let rate_limit_v1 = app_state.auth_mode.requires_auth();

    // v1 API routes — authenticated in key mode; open mode injects the default project.
    let v1 = Router::new()
        .typed_post(query_handler)
        .typed_post(investigate_handler)
        .typed_get(services_handler)
        .typed_get(trace_detail_handler)
        .layer(axum_mw::from_fn_with_state(
            app_state.clone(),
            auth::auth_middleware,
        ));

    let v1 = if rate_limit_v1 {
        // Rate limit (GCRA, in-memory): ~20 req/s sustained, burst 100 — a sane
        // ceiling when the API is exposed behind an API key.
        let governor_conf = GovernorConfigBuilder::default()
            .key_extractor(RateLimitKeyExtractor)
            .per_second(20)
            .burst_size(100)
            .finish()
            .expect("valid governor config");
        v1.layer(GovernorLayer::new(governor_conf).error_handler(rate_limit_error_response))
    } else {
        v1
    };

    // All /admin routes, including status, are admin-token gated by their
    // handlers (see require_admin_token).
    let admin = Router::new()
        .typed_get(admin_status)
        .typed_post(create_tenant_handler)
        .typed_post(create_key_handler)
        .typed_delete(revoke_key_handler);

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

/// Refuse configurations where the quack query surface would serve without its
/// read-side backstop.
///
/// The quack authorization gate (`crate::quack`) is SELECT-only and does NOT
/// block file/URL reader functions (`read_csv`, `read_parquet('https://…')`,
/// `httpfs`, `glob`). The only thing that stops those is the filesystem
/// lockdown (`enable_external_access=false`), which `pool.rs` SKIPS whenever an
/// S3 bucket is configured. So serving quack with S3 configured would expose
/// arbitrary file read + SSRF/exfiltration on the read surface. Refuse it,
/// keyed on `s3_bucket` directly (the actual lockdown-skip condition) rather
/// than on `CATALOG_URL`, so the guard stays correct even if the distributed
/// restriction below is later relaxed. Lifting the S3 refusal requires adding a
/// reader/table allowlist to the quack gate first (see `crate::quack`).
pub(crate) fn ensure_quack_config_safe(
    storage_config: &Option<crate::storage::StorageConfig>,
) -> Result<()> {
    let Some(cfg) = storage_config else {
        return Ok(()); // local file catalog: lockdown always runs — safe.
    };
    anyhow::ensure!(
        cfg.s3_bucket.is_empty(),
        "SIGIRO_QUERY_SURFACE=quack refused: an S3-backed catalog disables the \
         filesystem lockdown that guards the quack read surface against \
         read_csv/read_parquet(url)/httpfs exfiltration — add a reader/table \
         allowlist to the quack authorization gate before enabling this"
    );
    anyhow::bail!(
        "SIGIRO_QUERY_SURFACE=quack is not supported in distributed \
         (SIGIRO_CATALOG_URL) mode yet — it currently serves only the single \
         non-distributed \"default\" project"
    )
}

pub async fn run_servers(
    pool: Arc<ProjectPool>,
    admin_conn: Arc<Mutex<Connection>>,
    router: Arc<MsgRouter>,
    data_dir: camino::Utf8PathBuf,
    init_complete: Arc<AtomicBool>,
) -> Result<()> {
    let ports = ServerPorts::load()?;
    let max_ingest_body_bytes = crate::config::max_ingest_body_bytes()?;

    let configured_key = crate::config::API_KEY.value().unwrap_or_default();
    let auth_mode = crate::auth::AuthMode::from_runtime_config(configured_key.clone(), &data_dir)?;

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
        let svc = OtlpGrpcService::new(grpc_router, grpc_auth_mode, max_ingest_body_bytes);
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

    let query_surface = crate::config::query_surface()?;
    let app_state = Arc::new(AppState {
        pool: Arc::clone(&pool),
        admin_conn: Arc::clone(&admin_conn),
        http_client: reqwest::Client::new(),
        router: Arc::clone(&router),
        auth_mode: auth_mode.clone(),
        data_dir,
        init_complete: Arc::clone(&init_complete),
        query_surface,
    });

    // Edge query-serving surface: served in-process, on a connection cloned
    // from the SAME DuckDB instance ingest writes to (a DuckDB-file catalog
    // is one-writer-XOR-many-readers, so a separate process cannot share the
    // lock — see crate::quack module docs). `query_surface == None` (the
    // default) changes nothing: no extension load, no server, no new port.
    let quack_shutdown_tx = if query_surface == crate::config::QuerySurface::Quack {
        ensure_quack_config_safe(&crate::storage::load_config()?)?;
        let bind = crate::config::QUACK_BIND
            .value()
            .map_err(anyhow::Error::from)?;
        anyhow::ensure!(
            bind != "0.0.0.0",
            "SIGIRO_QUACK_BIND must not be 0.0.0.0 — quack has no working TLS \
             of its own; bind an explicit loopback/private interface and rely \
             on a network perimeter (e.g. Tailscale) for anything wider"
        );
        let port = crate::config::QUACK_PORT
            .value()
            .map_err(anyhow::Error::from)?;
        let token = crate::config::QUACK_TOKEN
            .value()
            .map_err(anyhow::Error::from)?;
        let token = if token.is_empty() { None } else { Some(token) };

        let quack_conn = pool
            .clone_connection("default")
            .context("failed to open the quack query-surface connection")?;
        crate::quack::load_and_pin(&quack_conn)?;
        crate::quack::install_authorization_gate(&quack_conn)?;
        let listen = crate::quack::start_server(&quack_conn, &bind, port, token.as_deref())?;
        tracing::info!(
            bind,
            port,
            listen_uri = %listen.listen_uri,
            "quack query surface serving (in-process, read-only)"
        );

        let (tx, rx) = oneshot::channel::<()>();
        servers.spawn_blocking(move || {
            if rx.blocking_recv().is_err() {
                tracing::debug!("quack shutdown sender dropped");
            }
            let result = crate::quack::stop_server(&quack_conn, &bind, port)
                .context("failed to stop the quack query surface cleanly");
            if let Err(ref error) = result {
                tracing::warn!("{error:#}");
            }
            drop(quack_conn);
            ("quack", result)
        });
        Some(tx)
    } else {
        None
    };

    // OTLP HTTP ingest sub-router (POST /v1/traces|logs|metrics|profiles + the
    // pyroscope paths) — a composable Router with its own auth/DefaultBodyLimit/
    // state. RequestDecompressionLayer is applied to THIS sub-router only, not
    // the whole :9999 app, matching what the dedicated :4318 listener already
    // does. Merged into the :9999 app below so there is one public port; the
    // :4318/:4317 listeners keep running too (unexposed), reusing the same
    // router (axum's Router is cheap to clone — Arc-backed).
    let otlp_ingest_router =
        crate::ingest::otlp_http_router(Arc::clone(&router), auth_mode, max_ingest_body_bytes)
            .layer(tower_http::decompression::RequestDecompressionLayer::new());

    let http_app = build_http_router(app_state).merge(otlp_ingest_router.clone());

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
    let otlp_http_app = otlp_ingest_router;
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
    let retention_handle = spawn_retention_task(Arc::clone(&router));

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
    let quack_shutdown_send = quack_shutdown_tx.map(|tx| {
        tx.send(())
            .map_err(|_| anyhow::anyhow!("quack shutdown receiver dropped unexpectedly"))
    });
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
    if let Some(result) = quack_shutdown_send {
        result?;
    }
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
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .with_context(|| format!("{name} HTTPS server failed"))
    } else {
        tracing::info!(server = name, protocol = "HTTP", "listening on {addr}");
        axum_server::bind(addr)
            .handle(handle)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .with_context(|| format!("{name} HTTP server failed"))
    }
}

/// Spawn a background task that periodically deletes old data based on
/// SIGIRO_RETENTION_DAYS. Runs once per hour.
fn spawn_retention_task(router: Arc<MsgRouter>) -> Option<tokio::task::JoinHandle<()>> {
    let days = match crate::config::RETENTION_DAYS.value() {
        Ok(d) if d > 0 => d,
        _ => return None,
    };

    tracing::info!(retention_days = days, "data retention enabled");

    Some(tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            interval.tick().await;
            if let Err(e) = router.run_retention_cleanup(days).await {
                tracing::error!("retention cleanup failed: {e:#}");
            }
        }
    }))
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

/// Get the per-project DuckDB reader pool using the AuthContext.
fn project_reader_pool(state: &AppState, auth: &AuthContext) -> ApiResult<ReaderPool> {
    // Explicit DI: get_project returns only existing projects.
    // Callers must call create_project() first to initialize a project.
    state
        .pool
        .get_project(&auth.tenant_id)
        .map_err(|e| anyhow::anyhow!(e).context_internal("Project DB", "connection not found"))
}

/// Escape a string for safe use inside a SQL single-quoted string literal.
fn escape_sql_string(s: &str) -> String {
    s.replace("'", "''")
}

/// Execute SQL against the local DuckDB project connection. Returns the
/// result rows plus whether the row cap (`SIGIRO_MAX_RESULT_ROWS`) truncated
/// them.
async fn execute_sql(
    state: &AppState,
    auth: &AuthContext,
    sql: &str,
) -> anyhow::Result<(Vec<serde_json::Value>, bool)> {
    let readers = project_reader_pool(state, auth)
        .map_err(|e: axum_anyhow::ApiError| anyhow::anyhow!("{:?}", e))?;
    let sql_clone = sql.to_string();
    let timeout_secs = crate::config::query_timeout_secs()?;
    let max_rows = crate::config::max_result_rows()?;
    let (handle_tx, handle_rx) = tokio::sync::oneshot::channel::<Arc<duckdb::InterruptHandle>>();

    let mut query_task = tokio::task::spawn_blocking(move || {
        let conn = readers.get();
        // Sent before `prepare`/`query` so the timeout branch below can
        // interrupt this exact connection even if it never gets a chance to
        // run (e.g. still waiting on `readers.get()` for a free connection).
        // An error here just means the async side gave up waiting — nothing
        // to interrupt for, so there is nothing to report.
        if handle_tx.send(conn.interrupt_handle()).is_err() {
            tracing::debug!("query timeout watchdog receiver dropped before send");
        }

        let mut stmt = conn
            .prepare(&sql_clone)
            .map_err(|e| anyhow::anyhow!("failed to prepare query: {e}"))?;
        // Execute ONCE. Column names come from the live statement behind the
        // Rows handle (owned Vec, so the borrow ends before we iterate).
        let mut rows = stmt
            .query([])
            .map_err(|e| anyhow::anyhow!("failed to execute query: {e}"))?;
        let column_names: Vec<String> = rows
            .as_ref()
            .map(duckdb::Statement::column_names)
            .unwrap_or_default();
        let mut results = Vec::new();
        let mut truncated = false;
        // Propagate a mid-stream error instead of silently truncating the
        // result and returning partial rows as HTTP 200. Row-cap truncation
        // (below) is the one deliberate, reported exception.
        while let Some(row) = rows
            .next()
            .map_err(|e| anyhow::anyhow!("row read failed: {e}"))?
        {
            if results.len() >= max_rows {
                truncated = true;
                break;
            }
            let mut obj = serde_json::Map::new();
            for (i, name) in column_names.iter().enumerate() {
                let val = row
                    .get_ref(i)
                    .map_err(|e| anyhow::anyhow!("column read failed: {e}"))?;
                obj.insert(name.clone(), duckdb_value_to_json(val));
            }
            results.push(serde_json::Value::Object(obj));
        }
        Ok((results, truncated))
    });

    // ponytail: timeout enforcement polls a shared oneshot handle rather than
    // tracking per-connection "busy" state, so there's a narrow window where
    // the deadline fires just as the query finishes and the now-idle pooled
    // connection gets a spurious interrupt(). DuckDB (this bundled version)
    // has no `statement_timeout` setting to enforce this engine-side instead;
    // revisit if spurious interrupts on adjacent queries are ever observed.
    tokio::select! {
        joined = &mut query_task => {
            joined
                .map_err(|e| anyhow::anyhow!("query task failed: {e}"))?
        }
        () = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            if let Ok(handle) = handle_rx.await {
                tracing::warn!(timeout_secs, "query exceeded timeout — interrupting");
                handle.interrupt();
            }
            (&mut query_task)
                .await
                .map_err(|e| anyhow::anyhow!("query task failed: {e}"))?
        }
    }
}

/// Convert one DuckDB column value to a `serde_json::Value`. Shared with the
/// federated query coordinator (`crate::federate`), which reads rows off its
/// own local coordinator connection the same way this handler reads rows off
/// a project reader connection.
pub(crate) fn duckdb_value_to_json(val: duckdb::types::ValueRef<'_>) -> serde_json::Value {
    use duckdb::types::ValueRef;
    match val {
        ValueRef::Null => serde_json::Value::Null,
        ValueRef::Boolean(b) => serde_json::Value::Bool(b),
        ValueRef::TinyInt(b) => serde_json::json!(b),
        ValueRef::SmallInt(b) => serde_json::json!(b),
        ValueRef::Int(b) => serde_json::json!(b),
        ValueRef::BigInt(b) => serde_json::json!(b),
        // HUGEINT (int128) — produced e.g. by SUM over BIGINT (the federated
        // fold's sum(count)/sum(sum)). Emit as a JSON number when it fits i64,
        // else as a string to avoid silent precision loss. Without this arm it
        // fell through to the Debug catch-all ("HugeInt(3)").
        ValueRef::HugeInt(b) => match i64::try_from(b) {
            Ok(n) => serde_json::json!(n),
            Err(_) => serde_json::Value::String(b.to_string()),
        },
        // Unsigned integer variants — a passthrough `SELECT *` over a
        // sigiro_* table (e.g. a UBIGINT column) used to fall through to the
        // `{:?}` catch-all below and print e.g. `"UBigInt(5)"` instead of `5`.
        ValueRef::UTinyInt(b) => serde_json::json!(b),
        ValueRef::USmallInt(b) => serde_json::json!(b),
        ValueRef::UInt(b) => serde_json::json!(b),
        ValueRef::UBigInt(b) => serde_json::json!(b),
        ValueRef::Float(b) => serde_json::json!(b),
        ValueRef::Double(b) => serde_json::json!(b),
        // DECIMAL — emit a JSON number when the value round-trips through f64
        // exactly (checked by reformatting at the same scale and comparing
        // text, not by comparing floats), else a decimal string so a
        // high-precision DECIMAL never silently loses digits.
        ValueRef::Decimal(d) => decimal_to_json(&d.to_string()),
        // Timestamp/Date32/Time64 — raw ticks (matching `value_ref_to_json`'s
        // existing convention below for the same DuckDB types), not a
        // Debug-formatted enum tuple.
        ValueRef::Timestamp(_, us) => serde_json::json!(us),
        ValueRef::Date32(d) => serde_json::json!(d),
        ValueRef::Time64(_, t) => serde_json::json!(t),
        ValueRef::Text(b) => serde_json::Value::String(String::from_utf8_lossy(b).to_string()),
        ValueRef::Blob(b) => serde_json::Value::String(format!("[blob {} bytes]", b.len())),
        _ => serde_json::Value::String(format!("{:?}", val)),
    }
}

/// Render a DuckDB DECIMAL's exact text (`d.to_string()`, e.g. `"15.50"`) as a
/// JSON number when converting it to `f64` and back at the same scale
/// reproduces the identical text (i.e. no precision was lost), else as a
/// string. Pure text/parse round-trip — never trusts float equality.
fn decimal_to_json(exact: &str) -> serde_json::Value {
    let Ok(f) = exact.parse::<f64>() else {
        return serde_json::Value::String(exact.to_string());
    };
    let scale = exact.split_once('.').map_or(0, |(_, frac)| frac.len());
    if format!("{f:.scale$}") == exact {
        serde_json::json!(f)
    } else {
        serde_json::Value::String(exact.to_string())
    }
}

/// `x-sigiro-truncated` header + JSON rows — signals `SIGIRO_MAX_RESULT_ROWS`
/// truncation without changing the response body shape (still a bare array,
/// so existing callers like `SigiroClient::query` keep working unchanged).
type QueryResponse = (
    [(&'static str, &'static str); 1],
    Json<Vec<serde_json::Value>>,
);

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
) -> ApiResult<QueryResponse> {
    // Truncated SQL preview so every query outcome (reject / fail / complete)
    // leaves a server-side trace — a silently-throttled or errored query used
    // to leave none, which is what let a false "data loss" RCA take hold.
    let sql_preview: String = body.chars().take(200).collect();
    if let Err(e) = validate_read_only(&body) {
        tracing::warn!(sql = %sql_preview, "query rejected (validation): {e}");
        return Err(bad_request("query rejected", &e));
    }
    match execute_sql(&state, &auth, &body).await {
        Ok((results, truncated)) => {
            tracing::info!(
                rows = results.len(),
                truncated,
                sql = %sql_preview,
                "query completed"
            );
            // SIGIRO_MAX_RESULT_ROWS was hit — the client must not mistake a
            // capped result for the complete answer.
            let truncated_header = [(
                "x-sigiro-truncated",
                if truncated { "true" } else { "false" },
            )];
            Ok((truncated_header, Json(results)))
        }
        Err(e) => {
            tracing::warn!(sql = %sql_preview, "query failed: {e:#}");
            Err(bad_request("query rejected", &format!("{e:#}")))
        }
    }
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
    let (results, _truncated) = execute_sql(&state, &auth, &sql)
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
    let (results, _truncated) = execute_sql(&state, &auth, &sql)
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

    let data_dir = crate::data_dir().context_internal("Investigate", "SIGIRO_DATA_DIR not set")?;
    let project_data_path = match crate::storage::load_config()
        .context_internal("Investigate", "failed to load storage config")?
    {
        Some(_) => crate::storage::tenant_data_dir(&data_dir, &auth.tenant_id)
            .context_internal("Investigate", "invalid tenant id")?,
        None => data_dir.join(&auth.tenant_id),
    };
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
                auth.tenant_id
            ),
        ));
    }
    let readers = project_reader_pool(&state, &auth)?;
    let blocks = tokio::task::spawn_blocking(move || {
        let conn = readers.get();
        investigate::investigate(&conn, &req, ProjectPool::prefix())
    })
    .await
    .map_err(|e| anyhow::anyhow!("investigation task failed: {e}"))
    .context_internal("Investigate", "investigation task failed")?
    .context_internal("Investigate", "investigation failed")?;
    Ok(Json(blocks))
}

// --- Admin endpoints ---
// All endpoints, including status, are admin-token gated (see require_admin_token).

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
    headers: HeaderMap,
) -> ApiResult<Json<AdminStatusResponse>> {
    require_admin_token(&headers)?;
    let guard = state.admin_conn.lock();
    let mut map =
        admin::read_all_status(&guard).context_internal("Admin", "failed to read status")?;
    map.insert(
        "ingest_dropped_messages_total".to_string(),
        state.router.dropped_messages().to_string(),
    );
    Ok(Json(AdminStatusResponse { status: map }))
}

fn require_admin_token(headers: &HeaderMap) -> Result<(), axum_anyhow::ApiError> {
    let expected = crate::config::ADMIN_TOKEN.value().map_err(|_| {
        axum_anyhow::unauthorized(
            "Missing Admin Token",
            "SIGIRO_ADMIN_TOKEN is not configured",
        )
    })?;
    if expected.is_empty() {
        return Err(axum_anyhow::unauthorized(
            "Missing Admin Token",
            "SIGIRO_ADMIN_TOKEN is empty",
        ));
    }
    let token = auth::extract_bearer_token(headers)?;
    if token == expected {
        Ok(())
    } else {
        Err(axum_anyhow::unauthorized(
            "Invalid Admin Token",
            "The provided admin bearer token is invalid",
        ))
    }
}

fn control_plane_from_state(state: &AppState) -> ApiResult<Arc<auth::ControlPlane>> {
    state.auth_mode.control_plane().ok_or_else(|| {
        bad_request(
            "distributed auth required",
            "tenant provisioning requires SIGIRO_CATALOG_URL distributed mode",
        )
    })
}

/// POST /admin/tenants — create or reactivate a tenant in the distributed control plane.
#[utoipa::path(
    post,
    path = "/admin/tenants",
    request_body = CreateTenantRequest,
    responses(
        (status = 200, description = "Tenant created", body = CreateTenantResponse)
    ),
    tag = "admin"
)]
async fn create_tenant_handler(
    _path: AdminTenantsPath,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateTenantRequest>,
) -> ApiResult<Json<CreateTenantResponse>> {
    require_admin_token(&headers)?;
    let control = control_plane_from_state(&state)?;
    control
        .create_tenant(&req.id)
        .context_internal("Admin", "failed to create tenant")?;
    Ok(Json(CreateTenantResponse {
        id: req.id,
        status: "active".to_string(),
    }))
}

/// POST /admin/keys — mint an API key for an existing tenant.
#[utoipa::path(
    post,
    path = "/admin/keys",
    request_body = CreateKeyRequest,
    responses(
        (status = 200, description = "API key created", body = CreateKeyResponse)
    ),
    tag = "admin"
)]
async fn create_key_handler(
    _path: AdminKeysPath,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateKeyRequest>,
) -> ApiResult<Json<CreateKeyResponse>> {
    require_admin_token(&headers)?;
    let control = control_plane_from_state(&state)?;
    let minted = control
        .mint_key(&req.tenant_id)
        .context_internal("Admin", "failed to mint API key")?;
    Ok(Json(CreateKeyResponse {
        key: minted.key,
        key_hash: minted.key_hash,
        tenant_id: minted.tenant_id,
    }))
}

/// DELETE /admin/keys/{short_token} — revoke an API key by hash prefix.
#[utoipa::path(
    delete,
    path = "/admin/keys/{short_token}",
    params(
        ("short_token" = String, Path, description = "8-64 character key hash prefix")
    ),
    responses(
        (status = 200, description = "API key revoked", body = RevokeKeyResponse)
    ),
    tag = "admin"
)]
async fn revoke_key_handler(
    AdminKeyPath { short_token }: AdminKeyPath,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<RevokeKeyResponse>> {
    require_admin_token(&headers)?;
    let control = control_plane_from_state(&state)?;
    let revoked = control
        .revoke_key_hash_prefix(&short_token)
        .context_internal("Admin", "failed to revoke API key")?;
    Ok(Json(RevokeKeyResponse {
        revoked,
        short_token,
    }))
}

#[cfg(test)]
mod quack_config_guard_tests {
    use super::ensure_quack_config_safe;
    use crate::storage::StorageConfig;

    fn distributed(s3_bucket: &str) -> StorageConfig {
        StorageConfig {
            catalog_url: "postgres://host/db".to_string(),
            s3_bucket: s3_bucket.to_string(),
            s3_region: String::new(),
            s3_endpoint: String::new(),
            s3_key_id: String::new(),
            s3_secret: String::new(),
        }
    }

    #[test]
    fn local_mode_is_allowed() {
        assert!(ensure_quack_config_safe(&None).is_ok());
    }

    #[test]
    fn s3_backed_catalog_is_refused_for_exfiltration_reason() {
        let err = ensure_quack_config_safe(&Some(distributed("my-bucket")))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("exfiltration"),
            "S3 refusal must cite the read-side exfiltration risk, got: {err}"
        );
    }

    #[test]
    fn distributed_without_s3_is_still_refused() {
        assert!(ensure_quack_config_safe(&Some(distributed(""))).is_err());
    }
}
