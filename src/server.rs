use anyhow::{Context, Result};
use axum::{Extension, Json, Router, extract::State, middleware as axum_mw};
use axum_anyhow::{ApiResult, ResultExt, bad_request, internal_error};
use axum_extra::routing::RouterExt;
use axum_server::{Handle, tls_rustls::RustlsConfig};
use axum_valid::Valid;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tonic::transport::{Identity, Server as TonicServer, ServerTlsConfig};
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder, key_extractor::KeyExtractor};
use utoipa::OpenApi;

use crate::admin;
use crate::auth;
use crate::diagnose;
use crate::ingest::OtlpGrpcService;
use crate::inserter::{DiskCapacityState, MsgRouter};
use crate::pool::ProjectPool;
use crate::responses::*;
use crate::sql::{
    ALLOWED_TABLES, duckdb_row_value_to_json, escape_sql_string, validate_read_only_with,
};
use crate::types::*;
use tracing_attributes::instrument;

#[derive(OpenApi)]
#[openapi(
    /* Without this, utoipa defaults `info.title` to the crate name, "sigiro".
     * A renderer uses `info.title` for the reference's own heading and page
     * title, so the docs page came out as "sigiro - sigiro" — the product name
     * twice and the section name nowhere.
     *
     * "sigiro API Reference" is what Stripe and Resend call the same section.
     * Note this is the *document* title, not a per-operation summary: those stay
     * short verb-object phrases ("List services"), which is the same convention
     * and what a sidebar entry can hold. */
    info(
        title = "sigiro API Reference",
        description = "Send OpenTelemetry to sigiro. Ask what changed. Get ranked findings and the SQL that proves them."
    ),
    paths(
        query_handler,
        services_handler,
        alerts_handler,
        anomalies_handler,
        trace_detail_handler,
        diagnose_handler,
        crate::ingest::pyroscope_root,
        crate::ingest::pyroscope_push,
        crate::ingest::pyroscope_ingest,
        crate::ingest::pyroscope_ingest_pyroscope,
        admin_status,
        create_tenant_handler,
        create_key_handler,
        revoke_key_handler,
            ),
    components(
        schemas(
            ServicesResponse,
            ServiceEntry,
            AlertsResponse,
            crate::alerts::AlertInstance,
            AnomaliesResponse,
            AnomalyEntry,
            TraceDetailResponse,
            SpanDetail,
            ServiceBreakdownEntry,
            CreateKeyResponse,
            RevokeKeyResponse,
            CreateTenantRequest,
            CreateTenantResponse,
            CreateKeyRequest,
            AdminStatusResponse,
            diagnose::DiagnoseRequest,
            diagnose::DiagnoseResponse,
            diagnose::DiagnosisBlock,
            diagnose::Finding,
            diagnose::FindingKind,
            diagnose::FindingSeverity,
            diagnose::Incident,
            diagnose::IncidentShift,
            diagnose::AnomalySignal,
            diagnose::OperationSummary,
            diagnose::ErrorTypeCount,
            diagnose::GenAiModelSummary,
            diagnose::FinishReasonCount,
            diagnose::MetricBucket,
            diagnose::SpanRecord,
            diagnose::LogRecord,
            diagnose::LogPattern,
            diagnose::ProfileSummary,
            diagnose::DerivedEvent,
            diagnose::ServiceContext,
            diagnose::Coverage,
            diagnose::SignalCoverage,
            diagnose::ServiceDependency,
            diagnose::Window,
        )
    ),
    modifiers(&SecurityAddon),
    /* No `tags`, and no `tag` on any operation.
     *
     * There were two — `query` and `admin` — and a renderer groups operations by
     * tag, so the reference gained a level of nesting for them. Once `/admin` left
     * the public spec, `query` was one group holding every operation: a heading
     * that grouped nothing, showing up in the docs sidebar and again as an eyebrow
     * over each operation's title. Five operations read better as five operations.
     *
     * Worth tagging again if this API grows a second genuine area. Tagging by
     * transport ("query") was never that — it described how you reach the endpoint,
     * not what it is for. */
)]
pub(crate) struct ApiDoc;

/// Declares the bearer-token security scheme so agents reading /openapi.json
/// know every /v1 and /admin endpoint expects `Authorization: Bearer <key>`.
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
        let components = openapi
            .components
            .get_or_insert_with(utoipa::openapi::Components::new);
        components.add_security_scheme(
            "bearer",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("prefixed-api-key")
                    .build(),
            ),
        );
    }
}

/// Pre-auth rate-limit key. This must not key on the unverified Authorization
/// header: an attacker could rotate bogus tokens to evade the limiter and grow
/// its key map without bound. ConnectInfo is populated by `serve_http_app`.
#[derive(Clone)]
pub(crate) struct SourceRateLimitKeyExtractor;

impl KeyExtractor for SourceRateLimitKeyExtractor {
    type Key = String;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
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

/// Post-auth rate-limit key. Auth middleware inserts this trusted tenant
/// context before the limiter runs, so one tenant cannot consume another's
/// query allowance and raw API keys never live in the governor key map.
#[derive(Clone)]
struct TenantRateLimitKeyExtractor;

impl KeyExtractor for TenantRateLimitKeyExtractor {
    type Key = String;

    fn extract<T>(
        &self,
        req: &axum::http::Request<T>,
    ) -> Result<Self::Key, tower_governor::GovernorError> {
        req.extensions()
            .get::<TenantId>()
            .map(|tenant_id| format!("tenant:{tenant_id}"))
            .ok_or(tower_governor::GovernorError::UnableToExtractKey)
    }
}

/// Render a `GovernorError` as a JSON body matching the shape of every other
/// API error (`{status, title, detail}`) so a rate-limited client cannot
/// mistake the plain-text default ("Too Many Requests!") for query output.
pub(crate) fn rate_limit_error_response(
    err: tower_governor::GovernorError,
) -> axum::response::Response {
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
    pub alerts: crate::alerts::AlertStore,
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
//   is already prevented by DuckDB itself. The firewall itself lives in
//   `crate::sql` — see `crate::sql::validate_read_only`.

/// The public spec: everything `ApiDoc` describes, minus `/admin`.
///
/// `ApiDoc` stays complete on purpose — the admin handlers keep their
/// `#[utoipa::path]` attributes, so their request and response types are still
/// checked against the code and still documented for us. What changes is what a
/// stranger is handed.
///
/// `/openapi.json` is unauthenticated by design: it is the entire discoverability
/// surface, and an agent has to be able to read it before it has a key. That is
/// the right call for the query endpoints and the wrong one for
/// `/admin/tenants`, `/admin/keys` and `/admin/status` — those mint and revoke
/// credentials and enumerate tenants. Publishing their exact shape to anyone who
/// asks is free reconnaissance: it does not grant access, since every one of them
/// still requires the admin token, but it removes the need to guess.
///
/// It also fixes three things at once rather than one at a time. The docs site
/// generates its API reference from this document, so the admin pages disappear
/// from the sidebar; the sitemap is built from those routes, so they leave the
/// submitted sitemap too; and the spec itself stops advertising them. Filtering
/// downstream in the docs would have fixed only the first, and only there.
///
/// The admin schemas go with the paths. Left behind they would be unreferenced
/// entries that still spell out the request and response bodies, which is most of
/// what the paths were telling anyone.
///
/// Built once. The spec is static, and this is a public endpoint.
static PUBLIC_SPEC: std::sync::LazyLock<utoipa::openapi::OpenApi> =
    std::sync::LazyLock::new(|| {
        let mut spec = ApiDoc::openapi();

        spec.paths
            .paths
            .retain(|path, _| !path.starts_with("/admin"));

        if let Some(components) = spec.components.as_mut() {
            for schema in [
                "AdminStatusResponse",
                "CreateKeyRequest",
                "CreateKeyResponse",
                "CreateTenantRequest",
                "CreateTenantResponse",
                "RevokeKeyResponse",
            ] {
                components.schemas.remove(schema);
            }
        }

        spec
    });

/// GET /openapi.json — the single public discoverability surface: the
/// machine-readable OpenAPI contract. No Swagger UI or extra doc endpoints.
async fn openapi_json() -> axum::Json<&'static utoipa::openapi::OpenApi> {
    axum::Json(&*PUBLIC_SPEC)
}

/// Build the HTTP Router for the main API (port 9999).
/// Extracted from `run_servers` so integration tests can build the same
/// router with test state without starting TCP listeners.
pub fn build_http_router(app_state: Arc<AppState>) -> Router {
    let rate_limit_v1 = app_state.auth_mode.requires_auth();

    // v1 API routes. In hosted mode the source limiter runs before auth to
    // bound bogus-token work, then auth injects a trusted TenantId, then the
    // per-tenant limiter runs before the handler.
    let v1 = Router::new()
        .typed_post(query_handler)
        .typed_post(diagnose_handler)
        .typed_get(services_handler)
        .typed_get(alerts_handler)
        .typed_get(anomalies_handler)
        .typed_get(trace_detail_handler);

    let v1 = if rate_limit_v1 {
        let tenant_governor = GovernorConfigBuilder::default()
            .key_extractor(TenantRateLimitKeyExtractor)
            .per_millisecond(200)
            .burst_size(20)
            .finish()
            .expect("valid per-tenant governor config");
        let source_governor = GovernorConfigBuilder::default()
            .key_extractor(SourceRateLimitKeyExtractor)
            .per_millisecond(20)
            .burst_size(100)
            .finish()
            .expect("valid pre-auth source governor config");
        v1.layer(GovernorLayer::new(tenant_governor).error_handler(rate_limit_error_response))
            .layer(axum_mw::from_fn_with_state(
                app_state.clone(),
                auth::auth_middleware,
            ))
            .layer(GovernorLayer::new(source_governor).error_handler(rate_limit_error_response))
    } else {
        v1.layer(axum_mw::from_fn_with_state(
            app_state.clone(),
            auth::auth_middleware,
        ))
    };

    // All /admin routes, including status, are admin-token gated by a LAYER —
    // the same shape as /v1's auth_middleware above. Previously each handler
    // called require_admin_token() as its first line, so the gate depended on
    // whoever wrote the next handler remembering to. Enforcing it here means a
    // new admin route cannot be added unauthenticated by omission.
    let admin = Router::new()
        .typed_get(admin_status)
        .typed_post(create_tenant_handler)
        .typed_post(create_key_handler)
        .typed_delete(revoke_key_handler)
        .layer(axum_mw::from_fn(auth::admin_auth_middleware));

    // Public routes + v1 API + admin. Discoverability is the single
    // machine-readable OpenAPI contract at /openapi.json (public, no auth) —
    // no Swagger UI or extra doc endpoints, to minimize surface area.
    Router::new()
        .typed_get(health)
        .typed_get(v1_health)
        .typed_get(healthz)
        .route("/openapi.json", axum::routing::get(openapi_json))
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
    pub(crate) http: u16,
    pub(crate) grpc: u16,
    pub(crate) otlp_http: u16,
}

impl ServerPorts {
    /// Load ports from environment. Fails fast on malformed values.
    /// Unset vars default to well-known OTLP ports.
    pub(crate) fn load() -> anyhow::Result<Self> {
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

/// Reject a public/wildcard bind for the quack edge's mandatory tailnet
/// perimeter. In edge mode (`query_surface == Quack`), auth is Open and every
/// listener — quack itself plus, per `run_servers`, HTTP/OTLP-HTTP/gRPC too —
/// binds this SAME address, so it has to actually be a private/loopback
/// interface: quack speaks plain HTTP with no working TLS of its own, and an
/// Open-auth surface on a wildcard bind is reachable from anywhere. Parse the
/// address and reject any `is_unspecified()` (all-interfaces) form rather than
/// string-matching the two obvious spellings — the wildcard has many equivalent
/// textual forms (`0.0.0.0`, `::`, `0::0`, `0:0:0:0:0:0:0:0`,
/// `::ffff:0.0.0.0`) that all bind every interface, and matching only the
/// literals would leave exactly the same hole open under a different spelling.
pub fn validate_quack_bind_address(bind: std::net::IpAddr) -> Result<()> {
    anyhow::ensure!(
        !bind.to_canonical().is_unspecified(),
        "SIGIRO_QUACK_BIND must not be a wildcard address ({bind:?}) — quack has no working \
         TLS of its own; bind an explicit loopback/private interface and rely on a network \
         perimeter (e.g. Tailscale) for anything wider"
    );
    Ok(())
}

/// Resolve the bind IPs `run_servers` uses for its HTTP/OTLP-HTTP listener and
/// its gRPC listener, given the already-validated edge bind (`Some`, a quack
/// edge — see `validate_quack_bind_address`) or `None` (hosted mode). Pure
/// and unit-testable independent of `run_servers`'s side effects (real
/// listeners, DB pools, ...).
///
/// Hosted mode's pair is BYTE-IDENTICAL to before this fix: `0.0.0.0` for
/// HTTP/OTLP (kamal-proxy sits on the docker network — a loopback bind would
/// break proxy access) and `::` for gRPC (that listener's pre-existing,
/// unrelated default). Edge mode returns the SAME address for both, so
/// HTTP/OTLP-HTTP/gRPC all end up bound exactly where quack itself is.
pub fn resolve_listener_bind_ips(
    edge_bind: Option<std::net::IpAddr>,
) -> (std::net::IpAddr, std::net::IpAddr) {
    match edge_bind {
        Some(bind) => (bind, bind),
        None => (
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
            std::net::IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED),
        ),
    }
}

pub async fn run_servers(
    pool: Arc<ProjectPool>,
    alerts: crate::alerts::AlertStore,
    router: Arc<MsgRouter>,
    data_dir: camino::Utf8PathBuf,
    init_complete: Arc<AtomicBool>,
) -> Result<()> {
    let ports = ServerPorts::load()?;
    let max_ingest_body_bytes = crate::config::max_ingest_body_bytes()?;

    let query_surface = crate::config::query_surface()?;
    let auth_mode =
        crate::auth::AuthMode::from_runtime_config(&data_dir, query_surface, pool.storage_mode())?;

    // The bind address enforced for EVERY listener (HTTP, OTLP-HTTP, gRPC, and
    // quack itself) when this node is a quack edge — resolved once here so
    // the gRPC spawn below (which happens before the quack section further
    // down) already has it. `None` in hosted mode: every listener below then
    // falls back to today's wildcard bind, unchanged. See
    // `validate_quack_bind_address` for why a wildcard is rejected outright.
    let edge_bind = if query_surface == crate::config::QuerySurface::Quack {
        let raw_bind = crate::config::QUACK_BIND
            .value()
            .map_err(anyhow::Error::from)?;
        let bind = raw_bind
            .parse()
            .with_context(|| format!("SIGIRO_QUACK_BIND {raw_bind:?} is not a valid IP address"))?;
        validate_quack_bind_address(bind)?;
        Some(bind)
    } else {
        None
    };
    let (http_bind_ip, grpc_bind_ip) = resolve_listener_bind_ips(edge_bind);

    // TLS configuration
    let (otlp_http_tls_config, grpc_tls_config) = match (
        crate::config::TLS_CERT.value(),
        crate::config::TLS_KEY.value(),
    ) {
        (Ok(cert), Ok(key)) => {
            tracing::info!("direct-ingest TLS enabled — cert={cert}, key={key}");
            ensure_rustls_provider()?;
            let http_tls_config = RustlsConfig::from_pem_file(&cert, &key)
                .await
                .context("failed to load HTTP TLS config")?;
            let cert_pem = tokio::fs::read(&cert)
                .await
                .with_context(|| format!("failed to read TLS certificate {cert}"))?;
            let key_pem = tokio::fs::read(&key)
                .await
                .with_context(|| format!("failed to read TLS key {key}"))?;
            let grpc_tls_config = ServerTlsConfig::new()
                .identity(Identity::from_pem(cert_pem, key_pem))
                .timeout(Duration::from_secs(5));
            (Some(http_tls_config), Some(grpc_tls_config))
        }
        (Ok(_), Err(_)) => {
            anyhow::bail!("SIGIRO_TLS_CERT is set but SIGIRO_TLS_KEY is missing");
        }
        (Err(_), Ok(_)) => {
            anyhow::bail!("SIGIRO_TLS_KEY is set but SIGIRO_TLS_CERT is missing");
        }
        (Err(_), Err(_)) => {
            tracing::info!("TLS not configured — serving plain TCP");
            (None, None)
        }
    };

    // gRPC server — authenticated via Bearer token in metadata
    let grpc_server = TonicServer::builder()
        .concurrency_limit_per_connection(16)
        .load_shed(true)
        .timeout(Duration::from_secs(15));
    let mut grpc_server = match grpc_tls_config {
        Some(tls_config) => grpc_server
            .tls_config(tls_config)
            .context("failed to configure gRPC TLS")?,
        None => grpc_server,
    };
    let grpc_router = Arc::clone(&router);
    let grpc_auth_mode = auth_mode.clone();
    let grpc_port = ports.grpc;
    let mut servers = JoinSet::new();
    let (grpc_shutdown_tx, grpc_shutdown_rx) = oneshot::channel();
    servers.spawn(async move {
        let addr = SocketAddr::new(grpc_bind_ip, grpc_port);
        let svc = OtlpGrpcService::new(grpc_router, grpc_auth_mode, max_ingest_body_bytes);
        tracing::info!("gRPC listening on {addr}");
        let result = grpc_server
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

    // Clone before `data_dir` moves into AppState — the storage-maintenance
    // task samples this same directory for the disk cap.
    let maintenance_data_dir = data_dir.clone();

    let admin_conn = alerts.admin_connection();
    let app_state = Arc::new(AppState {
        pool: Arc::clone(&pool),
        admin_conn,
        alerts,
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
    //
    // Serving over an S3-backed distributed "default" project (SIGIRO_CATALOG_URL
    // set + SIGIRO_S3_BUCKET set) is supported here with NO extra guard: the
    // filesystem lockdown (`pool::lock_filesystem`) is what's skipped in that
    // mode, not the security boundary — `validate_read_only` (SELECT-only +
    // sigiro_* table allowlist + reader-function denylist + positive
    // scalar-function allowlist + WITH-ban) IS the boundary, in every storage
    // mode, and does not depend on the filesystem lockdown at all (see
    // `crate::quack::install_authorization_gate`, which wraps this same
    // validator). `enable_external_access` staying on in S3 mode is required
    // for DuckLake itself to reach S3, not a hole left open for user queries.
    let quack_shutdown_tx = if let Some(bind) = edge_bind {
        let port = crate::config::QUACK_PORT
            .value()
            .map_err(anyhow::Error::from)?;
        let token = crate::config::quack_token()?;

        let quack_conn = pool.clone_connection("default").context(
            "failed to open the quack query-surface connection — run_serve should have \
             pre-created the single-edge \"default\" project before run_servers starts \
             (see main.rs run_serve's quack pre-create, which fails loud on its own \
             create_project error rather than deferring the failure to here)",
        )?;
        crate::quack::load_and_pin(&quack_conn)?;
        crate::quack::install_authorization_gate(&quack_conn)?;
        // Derived once and moved into the shutdown task below, so the stop call
        // cannot address a different URI than the start call — see `QuackUri`.
        let quack_uri = crate::quack::QuackUri::new(bind, port);
        let listen = crate::quack::start_server(&quack_conn, &quack_uri, token.as_ref())?;
        tracing::info!(
            bind = %bind,
            port,
            listen_uri = %listen.listen_uri,
            "quack query surface serving (in-process, read-only)"
        );

        let (tx, rx) = oneshot::channel::<()>();
        servers.spawn_blocking(move || {
            if rx.blocking_recv().is_err() {
                tracing::debug!("quack shutdown sender dropped");
            }
            let result = crate::quack::stop_server(&quack_conn, &quack_uri)
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

    let http_addr = SocketAddr::new(http_bind_ip, ports.http);
    let http_handle = Handle::new();
    let http_server_handle = http_handle.clone();
    servers.spawn(async move {
        let result = serve_http_app(
            "HTTP",
            http_addr,
            http_app,
            // The main HTTP API stays plaintext inside the deployment network;
            // kamal-proxy terminates public TLS before forwarding to this port.
            None,
            http_server_handle,
        )
        .await;
        ("HTTP", result)
    });

    // Flip init-complete only once the HTTP listener is actually bound.
    //
    // This used to happen at the tail of `run_serve`, BEFORE this function ran,
    // which made `HealthResponse::Starting` unreachable: nothing could connect
    // to report 503 to. `Handle::listening()` resolves when `axum_server` has
    // bound the socket, which is the first instant a probe can arrive — so
    // flipping here is both the earliest correct point and the latest safe one.
    //
    // Spawned rather than awaited: `listening()` only resolves after the server
    // future above starts polling, and that cannot happen while this function
    // is still building the remaining servers. Awaiting it inline deadlocks.
    let init_complete_on_bind = Arc::clone(&init_complete);
    let http_listen_handle = http_handle.clone();
    tokio::spawn(async move {
        if http_listen_handle.listening().await.is_some() {
            init_complete_on_bind.store(true, Ordering::Release);
            tracing::info!("sigiro ready — HTTP listener bound, /health now reports ready");
        } else {
            tracing::error!("HTTP listener failed to bind — /health stays 503 Starting");
        }
    });

    // OTLP HTTP — authenticated via Authorization header
    let otlp_http_app = otlp_ingest_router;
    let otlp_http_addr = SocketAddr::new(http_bind_ip, ports.otlp_http);
    let otlp_tls_config = otlp_http_tls_config;
    let otlp_http_handle = Handle::new();
    let otlp_http_server_handle = otlp_http_handle.clone();
    servers.spawn(async move {
        let result = serve_http_app(
            "OTLP HTTP",
            otlp_http_addr,
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

    // Storage-maintenance background task: routine retention cleanup plus the
    // disk-byte cap sampler/gate (runs if either is configured). The disk gate
    // shares the router's `DiskCapacityState` so `send` rejects when tripped.
    let retention_handle = spawn_storage_maintenance_task(
        Arc::clone(&router),
        maintenance_data_dir,
        router.disk_state(),
    );

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

/// `addr`: `run_servers` composes the resolved edge/hosted bind with the port —
/// a quack edge binds its own private address (`SIGIRO_QUACK_BIND`), hosted mode
/// keeps the unchanged `0.0.0.0` (it sits behind kamal-proxy on the docker
/// network, where a loopback bind would break proxy access).
///
/// One `SocketAddr` rather than an `IpAddr` plus a `u16`: this function's only
/// use for the two halves was to rejoin them on its first line, and each caller
/// held a port local that existed solely to be passed here.
async fn serve_http_app(
    name: &'static str,
    addr: SocketAddr,
    app: Router,
    tls_config: Option<RustlsConfig>,
    handle: Handle<SocketAddr>,
) -> Result<()> {
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

/// Total bytes of regular files under `path` (recursive), plus whether the walk
/// was COMPLETE. Symlinks are not followed and are not counted (only regular
/// files). The bool is `false` if any entry/stat failed — the total is then an
/// UNDER-count, and the caller must not use it to CLEAR the cap (an
/// under-count must never re-open ingest while real usage may still be over);
/// an under-count that still exceeds the cap is safe to trip on.
fn dir_size_bytes(path: &std::path::Path) -> (u64, bool) {
    let mut total: u64 = 0;
    let mut reliable = true;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        match entry {
            Ok(e) if e.file_type().is_file() => match e.metadata() {
                Ok(meta) => total += meta.len(),
                Err(err) => {
                    reliable = false;
                    tracing::warn!("disk sampler: stat failed for {:?}: {err}", e.path());
                }
            },
            Ok(_) => {}
            Err(err) => {
                reliable = false;
                tracing::warn!("disk sampler: walk entry failed: {err}");
            }
        }
    }
    (total, reliable)
}

/// Resets `DiskCapacityState::cleanup_in_flight` to `false` on drop so the
/// single-flight self-heal flag is released on every exit of the spawned
/// cleanup task — normal completion, error, OR panic-unwind — instead of only
/// on the success path. Without this a panic in `run_retention_cleanup` would
/// leave the flag stuck `true` and disable immediate self-heal forever.
struct CleanupInFlightGuard(Arc<DiskCapacityState>);

impl Drop for CleanupInFlightGuard {
    fn drop(&mut self) {
        self.0.cleanup_in_flight.store(false, Ordering::Release);
    }
}

/// Spawn the storage-maintenance background task. Runs when retention is on
/// (`SIGIRO_RETENTION_DAYS > 0`) OR the disk cap is armed (`cap > 0`); returns
/// `None` when neither is configured. Drives two independent timers:
/// - every `SIGIRO_DISK_SAMPLE_SECONDS`: sample `DATA_DIR` bytes and, if the
///   cap is armed, trip/clear the gate with hysteresis. On the OFF→ON trip,
///   single-flight an immediate retention cleanup (self-heal); the next sample
///   tick re-measures and clears the gate — clearing stays on this one sampler.
/// - hourly: run the routine retention cleanup (as before).
fn spawn_storage_maintenance_task(
    router: Arc<MsgRouter>,
    data_dir: camino::Utf8PathBuf,
    disk: Arc<DiskCapacityState>,
) -> Option<tokio::task::JoinHandle<()>> {
    let retention_days = match crate::config::RETENTION_DAYS.value() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("invalid SIGIRO_RETENTION_DAYS, disabling retention: {e:#}");
            0
        }
    };
    let cap = disk.cap;
    if retention_days == 0 && cap == 0 {
        return None;
    }

    let sample_secs = match crate::config::disk_sample_seconds() {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("invalid SIGIRO_DISK_SAMPLE_SECONDS, using 30s: {e:#}");
            30
        }
    };

    tracing::info!(
        retention_days,
        disk_cap_bytes = cap,
        sample_secs,
        "storage maintenance enabled"
    );
    if cap > 0
        && !crate::config::S3_BUCKET
            .value()
            .unwrap_or_default()
            .is_empty()
    {
        tracing::warn!(
            "SIGIRO_MAX_DISK_BYTES caps only the LOCAL catalog/inline/temp footprint in S3 mode — cold Parquet in the bucket is not counted"
        );
    }

    Some(tokio::spawn(async move {
        let mut sample_interval = tokio::time::interval(Duration::from_secs(sample_secs));
        let mut retention_interval = tokio::time::interval(Duration::from_secs(3600));
        loop {
            tokio::select! {
                _ = sample_interval.tick() => {
                    let dir = data_dir.clone();
                    let (bytes, reliable) = match tokio::task::spawn_blocking(move || dir_size_bytes(dir.as_std_path())).await {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::error!("disk sampler task failed to join: {e}");
                            continue;
                        }
                    };
                    // `apply_sample` is only ever called here (one serialized
                    // sampler) — no concurrent caller can race the load/store,
                    // and `over_cap` is re-evaluated on the NEXT tick after any
                    // self-heal cleanup frees space, so we never resample off-loop.
                    match disk.apply_sample(bytes, reliable) {
                        Some(true) => {
                            tracing::warn!(
                                sampled_bytes = bytes,
                                cap,
                                "disk cap tripped — REJECTING ingest until usage falls below 90% of cap"
                            );
                            // Self-heal: kick one retention cleanup, single-flighted.
                            // The next sample tick re-measures and clears the gate.
                            if retention_days > 0
                                && disk
                                    .cleanup_in_flight
                                    .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                                    .is_ok()
                            {
                                let r = Arc::clone(&router);
                                let ds = Arc::clone(&disk);
                                tokio::spawn(async move {
                                    // RAII: reset the single-flight flag on EVERY exit —
                                    // normal, error, or panic — so a panicking cleanup
                                    // can't wedge self-heal off forever.
                                    let _reset = CleanupInFlightGuard(ds);
                                    if let Err(e) = r.run_retention_cleanup(retention_days).await {
                                        tracing::error!("disk-cap self-heal retention cleanup failed: {e:#}");
                                    }
                                });
                            }
                        }
                        Some(false) => {
                            tracing::info!(
                                sampled_bytes = bytes,
                                cap,
                                "disk usage back under 90% of cap — ingest re-enabled"
                            );
                        }
                        None => {}
                    }
                }

                _ = retention_interval.tick() => {
                    if retention_days > 0
                        && let Err(e) = router.run_retention_cleanup(retention_days).await
                    {
                        tracing::error!("retention cleanup failed: {e:#}");
                    }
                }
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

    // 2. DuckLake maintenance health — a project whose writer has failed
    //    `SIGIRO_MAX_CHECKPOINT_FAILURES` consecutive checkpoints is shedding
    //    ingest, because nothing it accepts can be materialised into Parquet.
    //    Reported before init for the same reason as dead flushers: the
    //    condition means telemetry is being rejected right now.
    let stalled = state.router.maintenance_health().stalled_projects();
    if !stalled.is_empty() {
        tracing::warn!(
            projects = ?stalled,
            "health check: DuckLake maintenance stalled — ingest is being shed"
        );
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(HealthResponse::Degraded {
                reason: format!(
                    "DuckLake maintenance stalled for {} project(s): {}",
                    stalled.len(),
                    stalled.join(", ")
                ),
            }),
        );
    }

    // 3. Init completeness — `AppState::init_complete` is flipped by
    //    `run_servers` once `axum_server` reports the HTTP listener bound.
    //    A probe that arrives between the socket accepting connections and
    //    the rest of startup finishing (quack extension load, the `default`
    //    project's DuckLake attach — which can be S3-backed) gets 503, so
    //    k8s readiness and docker healthchecks do not route traffic to a
    //    server that is listening but not yet serving.
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

/// Execute SQL against the local DuckDB project connection. Returns the
/// result rows plus whether the row cap (`SIGIRO_MAX_RESULT_ROWS`) truncated
/// them.
async fn execute_sql(
    state: &AppState,
    auth: &TenantId,
    sql: &str,
) -> anyhow::Result<(Vec<serde_json::Value>, bool)> {
    // Explicit DI boundary: the authenticated tenant identity selects the
    // idempotent project setup operation. The pool itself keeps strict
    // create/get APIs, while every process can recover its in-memory project
    // entry after a restart or when a request lands on another Machine.
    let pool = Arc::clone(&state.pool);
    let project_id = auth.as_ref().to_owned();
    let sql_clone = sql.to_string();
    let timeout_secs = crate::config::query_timeout_secs()?;
    let max_rows = crate::config::max_result_rows()?;
    let (handle_tx, mut handle_rx) =
        tokio::sync::oneshot::channel::<Arc<duckdb::InterruptHandle>>();

    let mut query_task = tokio::task::spawn_blocking(move || {
        // DuckLake ATTACH can perform network I/O and cold extension loading;
        // keep it on the blocking worker with the query, never on Tokio's
        // async runtime thread.
        let readers = pool
            .create_project(&project_id)
            .with_context(|| format!("failed to initialize project {project_id}"))?;
        let conn = readers.get();
        // Sent before `prepare`/`query` so the timeout branch below can
        // interrupt this exact connection even if it never gets a chance to
        // run (e.g. still waiting on `readers.get()` for a free connection).
        handle_tx
            .send(conn.interrupt_handle())
            .map_err(|_| anyhow::anyhow!("query timed out before execution"))?;

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
        let column_types = rows
            .as_ref()
            .map(|statement| {
                (0..statement.column_count())
                    .map(|idx| statement.column_type(idx))
                    .collect::<Vec<_>>()
            })
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
                let val = duckdb_row_value_to_json(row, i, &column_types[i])
                    .map_err(|e| anyhow::anyhow!("column read failed: {e}"))?;
                obj.insert(name.clone(), val);
            }
            results.push(serde_json::Value::Object(obj));
        }
        Ok((results, truncated))
    });

    tokio::select! {
        joined = &mut query_task => {
            joined
                .map_err(|e| anyhow::anyhow!("query task failed: {e}"))?
        }
        () = tokio::time::sleep(Duration::from_secs(timeout_secs)) => {
            let handle = handle_rx.try_recv().ok();
            tracing::warn!(
                timeout_secs,
                executing = handle.is_some(),
                "query exceeded timeout"
            );
            if let Some(handle) = handle {
                handle.interrupt();
            }
            anyhow::bail!("query exceeded {timeout_secs} second timeout")
        }
    }
}

/// `x-sigiro-truncated` header + JSON rows — signals `SIGIRO_MAX_RESULT_ROWS`
/// truncation without changing the response body shape (still a bare array,
/// so existing callers like `SigiroClient::query` keep working unchanged).
type QueryResponse = (
    [(&'static str, &'static str); 1],
    Json<Vec<serde_json::Value>>,
);

/// The SQL-subset contract, stated once and surfaced in BOTH places an agent
/// hits it: the /openapi.json operation description and every guard
/// rejection. Rejections that don't say what IS allowed force trial-and-error.
fn query_sql_contract() -> String {
    format!(
        "Accepts exactly one read-only SELECT statement (raw SQL as the request body). \
         Allowed tables (optionally qualified as lake.* or main.*): {}. \
         WITH/CTE clauses are not supported — rewrite as derived-table subqueries \
         (SELECT ... FROM (SELECT ...) t). Scalar functions are allowlisted \
         (aggregates, json_extract/json_extract_string, date/time, string, math, \
         coalesce/nullif, and list/array helpers such as unnest, list_filter and \
         list_extract for VARCHAR[] columns like sigiro_profiles.stack_trace); \
         file/network readers and settings introspection are blocked.",
        ALLOWED_TABLES
            .iter()
            .map(|t| t.to_lowercase())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Ask a question of your telemetry in SQL. Read-only.
///
/// Every `drill_down_sql` that `/v1/diagnose` returns can be pasted here
/// verbatim.
///
/// Readable tables, optionally qualified `lake.*` or `main.*`:
///
/// - `sigiro_spans`
/// - `sigiro_logs`
/// - `sigiro_log_templates`
/// - `sigiro_metrics_gauge`
/// - `sigiro_metrics_sum`
/// - `sigiro_metrics_histogram`
/// - `sigiro_metrics_exp_histogram`
/// - `sigiro_profiles`
/// - `sigiro_anomalies`
///
/// ## Bound `timestamp` with a literal, not `now()`
///
/// Statistics pruning only happens when the comparison value is a constant the
/// planner can fold. `now()` is volatile, so any predicate built from it is
/// evaluated per row and every file in the table is opened.
///
/// ```sql
/// -- prunes: the planner folds the constant and skips whole files
/// WHERE timestamp > TIMESTAMP '2026-08-31 08:00:00'
///
/// -- does not prune: opens every file, then filters
/// WHERE timestamp > (now() - INTERVAL 1 HOUR)::TIMESTAMP
/// ```
///
/// Compute the bound in your client and send it as a literal.
///
/// ## If you must use `now()`, cast it
///
/// The `timestamp` columns are `TIMESTAMP` (no timezone) and `now()` returns
/// `TIMESTAMPTZ`. Comparing them directly makes DuckDB cast the **column**,
/// which adds a per-row conversion on top of the missing pruning. Measured
/// against production on 2026-08-31, same table and window:
///
/// | predicate | result |
/// |---|---|
/// | `timestamp > TIMESTAMP '2026-08-31 08:00:00'` | 200 in 2.48s |
/// | `timestamp > (now() - INTERVAL 5 MINUTE)::TIMESTAMP` | 200 in 0.65s |
/// | `timestamp > now() - INTERVAL 5 MINUTE` | 400 `INTERRUPT` at 10.98s |
/// | `timestamp > TIMESTAMPTZ '2026-08-31 08:00:00+00'` | 400 `INTERRUPT` at 10.65s |
///
/// So the cast is worth having, but it is a mitigation and not the fix: the
/// literal is.
#[utoipa::path(
    post,
    path = "/v1/query",
    operation_id = "run-query",
    summary = "Run a SQL query",
    /* `request_body(...)` rather than `request_body = String`, for the two fields
     * that form carries: the wire format, and an example.
     *
     * `content_type` earns its place — `String` alone said nothing about whether to
     * send JSON or text, and it is text.
     *
     * The `description` here is deliberately ONE plain sentence. Markdown was tried
     * and reverted: Blume renders an operation's `description` as markdown but a
     * request body's as plain text, so a list here reached the page as literal
     * hyphens and backticks — worse than the prose it replaced. The table list and
     * the SQL restrictions therefore live in the operation description above, which
     * does render, and this field says only what the body is. */
    request_body(
        content = String,
        content_type = "text/plain",
        description = "One SELECT statement. No WITH/CTE clauses — rewrite them as \
derived-table subqueries. Scalar functions are allowlisted; file and network \
readers are blocked. Bound timestamp columns with a literal TIMESTAMP rather \
than an expression built from now(), or statistics pruning is lost and the \
query reads every file.",
        example = json!(
            "SELECT service_name, count(*) AS errors FROM sigiro_spans \
WHERE status_code = 2 AND timestamp >= TIMESTAMP '2026-09-11 12:00:00' \
AND timestamp < TIMESTAMP '2026-09-11 13:00:00' GROUP BY 1 ORDER BY 2 DESC"
        ),
    ),
    responses(
        // An example, because the body type is untyped JSON: without one, any
        // schema-driven renderer samples `Vec<Value>` to `[null]`, which reads as
        // a broken endpoint rather than "rows, shaped by your SELECT".
        (status = 200, description = "Query results — one object per row, keyed by \
your SELECT's column names", body = Vec<serde_json::Value>,
            example = json!([
                { "service_name": "checkout", "errors": 128 },
                { "service_name": "cart", "errors": 4 }
            ]))
    ),
)]
#[instrument(skip_all, fields(sql_len = body.len()))]
async fn query_handler(
    _path: QueryPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<TenantId>,
    body: String,
) -> ApiResult<QueryResponse> {
    // Truncated SQL preview so every query outcome (reject / fail / complete)
    // leaves a server-side trace — a silently-throttled or errored query used
    // to leave none, which is what let a false "data loss" RCA take hold.
    let sql_preview: String = body.chars().take(200).collect();
    // In hosted mode (shared PostgreSQL catalog) block information_schema
    // enumeration — it would expose the control plane + sibling tenant schema
    // names. Local/self-host keeps it (single-catalog, safe).
    let allow_information_schema = !state.pool.storage_mode().is_distributed();
    if let Err(e) = validate_read_only_with(&body, allow_information_schema) {
        tracing::warn!(sql = %sql_preview, "query rejected (validation): {e}");
        return Err(bad_request(
            "query rejected",
            &format!("{e}. {}", query_sql_contract()),
        ));
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
/// Agents call this before /diagnose to know what services exist.
#[derive(serde::Deserialize)]
struct ServiceQueryRow {
    service_name: String,
    first_seen: Option<String>,
    last_seen: Option<String>,
    span_count: i64,
    log_count: i64,
    metric_count: i64,
    total_count: i64,
}

#[utoipa::path(
    get,
    path = "/v1/services",
    operation_id = "list-services",
    summary = "List services",
    description = "Every service that has reported telemetry, with the first and \
last timestamp seen for each (microseconds since the Unix epoch). The entry point \
of an agent loop: pick a service here, then diagnose it.",
    responses(
        (status = 200, description = "List of services", body = ServicesResponse)
    ),
)]
#[instrument(skip_all)]
async fn services_handler(
    _path: ServicesPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<TenantId>,
) -> ApiResult<Json<ServicesResponse>> {
    let prefix = ProjectPool::prefix();
    let sql = format!(
        "SELECT service_name, \
                min(first_seen)::VARCHAR AS first_seen, \
                max(last_seen)::VARCHAR AS last_seen, \
                sum(span_count)::BIGINT AS span_count, \
                sum(log_count)::BIGINT AS log_count, \
                sum(metric_count)::BIGINT AS metric_count, \
                sum(span_count + log_count + metric_count)::BIGINT AS total_count \
         FROM ( \
             SELECT service_name, min(timestamp) AS first_seen, max(timestamp) AS last_seen, \
                    count(*) AS span_count, 0::BIGINT AS log_count, 0::BIGINT AS metric_count \
             FROM {prefix}sigiro_spans GROUP BY service_name \
             UNION ALL \
             SELECT service_name, min(timestamp), max(timestamp), 0, count(*), 0 \
             FROM {prefix}sigiro_logs GROUP BY service_name \
             UNION ALL \
             SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*) \
             FROM {prefix}sigiro_metrics_gauge GROUP BY service_name \
             UNION ALL \
             SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*) \
             FROM {prefix}sigiro_metrics_sum GROUP BY service_name \
             UNION ALL \
             SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*) \
             FROM {prefix}sigiro_metrics_histogram GROUP BY service_name \
             UNION ALL \
             SELECT service_name, min(timestamp), max(timestamp), 0, 0, count(*) \
             FROM {prefix}sigiro_metrics_exp_histogram GROUP BY service_name \
         ) AS per_signal \
         GROUP BY service_name \
         ORDER BY total_count DESC, service_name ASC"
    );
    let (results, _truncated) = execute_sql(&state, &auth, &sql)
        .await
        .map_err(|e| bad_request("services query failed", &format!("{e:#}")))?;

    let mut services = Vec::new();
    for row in results {
        let row: ServiceQueryRow = serde_json::from_value(row)
            .map_err(|e| bad_request("services query failed", &e.to_string()))?;
        services.push(ServiceEntry {
            name: row.service_name,
            first_seen: row.first_seen,
            last_seen: row.last_seen,
            signals: ServiceSignals {
                spans: row.span_count,
                logs: row.log_count,
                metrics: row.metric_count,
            },
            total_count: row.total_count,
        });
    }

    Ok(Json(ServicesResponse { services }))
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct AlertsQuery {
    /// Filter by lifecycle status: `firing` or `resolved`.
    status: Option<String>,
}

#[utoipa::path(
    get,
    path = "/v1/alerts",
    operation_id = "list-alerts",
    summary = "List alerts",
    params(AlertsQuery),
    responses(
        (status = 200, description = "Alert instances", body = AlertsResponse),
        (status = 400, description = "Invalid status filter"),
        (status = 401, description = "Missing or invalid bearer token")
    ),
    security(("bearer" = [])),
)]
#[instrument(skip_all)]
async fn alerts_handler(
    _path: AlertsPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<TenantId>,
    axum::extract::Query(query): axum::extract::Query<AlertsQuery>,
) -> ApiResult<Json<AlertsResponse>> {
    if let Some(status) = query.status.as_deref()
        && !matches!(status, "firing" | "resolved")
    {
        return Err(bad_request(
            "invalid alert status",
            "status must be firing or resolved",
        ));
    }
    let alerts = state
        .alerts
        .list(auth.as_ref(), query.status.as_deref())
        .map_err(|error| {
            tracing::error!(project_id = auth.as_ref(), "alerts query failed: {error:#}");
            internal_error("alerts query failed", "unable to read alert state")
        })?;
    Ok(Json(AlertsResponse { alerts }))
}

/// Query parameters for GET /v1/anomalies.
///
/// `parameter_in = Query` is not the default: without it utoipa documents these as
/// PATH parameters, which made the published spec claim `/v1/anomalies` takes three
/// required path segments. They are optional filters on the query string.
#[derive(serde::Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct AnomaliesQuery {
    /// Filter to one service.
    service: Option<String>,
    /// Only shifts at or after this time (microseconds since the Unix epoch).
    #[param(value_type = i64)]
    from_ts: Option<TimestampMicros>,
    /// Only shifts at or before this time (microseconds since the Unix epoch).
    #[param(value_type = i64)]
    to_ts: Option<TimestampMicros>,
}

/// GET /v1/anomalies — the precomputed anomalies table, ALL signals,
/// biggest shift first by the UNIT-FREE magnitude
/// (after − before) / (after + before), computed at read, raw
/// |after − before| breaking zero-baseline ties. Unit-free so shifts of
/// different signals are comparable — raw deltas would let a µs latency
/// shift dwarf every percentage shift. Ranking, not a threshold — every
/// detected shift is returned. Rows are written continuously by the
/// scheduled anomaly pass (BOCPD changepoints over each entity's persisted
/// series — no thresholds), so reading here costs one table scan, never a
/// detection run. The same rows drive `is_anomalous` and the anomaly
/// findings in /v1/diagnose.
#[utoipa::path(
    get,
    path = "/v1/anomalies",
    operation_id = "list-anomalies",
    summary = "List anomalies",
    params(AnomaliesQuery),
    description = "\
Precomputed anomalies: worsening regime shifts (BOCPD changepoints, \
threshold-free) detected continuously over persisted 5-minute series of \
every signal — error_rate and latency_p95 per operation, log_volume, \
error_log_rate and profile_cost (the service's total profiled cost) per \
service (the `operation` field holds the entity) — and persisted to the \
`sigiro_anomalies` table. Ordered by the unit-free shift magnitude \
(after - before) / (after + before) descending (1.0 = appeared from a zero \
baseline, 1/3 = doubled), so the most significant shifts lead regardless \
of the signal's units — nothing is filtered out; judge significance from \
`before`/`after` (raw signal units: %, µs, logs, profile cost). Optionally \
filter by `service` and a `from_ts`/`to_ts` window (microseconds since the \
Unix epoch) over the shift time. Each row carries a ready-to-run \
`drill_down_sql` for POST /v1/query. Rows whose shifts fall in the same or \
adjacent 5-minute detection bucket on the same service share an \
`incident_id` — one incident across signals, not N independent anomalies \
(the correlation window is the detection pass's own bucket resolution, \
never a tuned threshold); `incident_id` is null for uncorrelated rows. \
`truncated` is true when the query row cap omitted shifts: this is an \
incomplete candidate snapshot and automatic submission must wait for a \
complete response.",
    responses(
        (status = 200, description = "Anomaly rows, biggest shift first", body = AnomaliesResponse),
        (status = 401, description = "Missing or invalid bearer token")
    ),
    security(("bearer" = [])),
)]
#[instrument(skip_all)]
async fn anomalies_handler(
    _path: AnomaliesPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<TenantId>,
    axum::extract::Query(q): axum::extract::Query<AnomaliesQuery>,
) -> ApiResult<Json<AnomaliesResponse>> {
    let prefix = ProjectPool::prefix();
    let mut filters = vec!["1 = 1".to_string()];
    if let Some(service) = &q.service {
        filters.push(format!("service_name = '{}'", escape_sql_string(service)));
    }
    if let Some(from_ts) = q.from_ts {
        let from_ts: i64 = from_ts.into();
        filters.push(format!("ts_shift >= make_timestamp({from_ts})"));
    }
    if let Some(to_ts) = q.to_ts {
        let to_ts: i64 = to_ts.into();
        filters.push(format!("ts_shift <= make_timestamp({to_ts})"));
    }
    let sql = format!(
        "SELECT detected_at::VARCHAR AS detected_at, service_name, operation, signal, kind, \
                direction, ts_shift::VARCHAR AS ts_shift, epoch_us(ts_shift) AS ts_shift_us, \
                \"before\", \"after\", drill_down_sql \
         FROM {prefix}sigiro_anomalies \
         WHERE {} \
         ORDER BY abs(\"after\" - \"before\") / nullif(abs(\"before\") + abs(\"after\"), 0) DESC, \
                  abs(\"after\" - \"before\") DESC, ts_shift DESC",
        filters.join(" AND ")
    );
    let (results, truncated) = execute_sql(&state, &auth, &sql)
        .await
        .map_err(|e| bad_request("anomalies query failed", &format!("{e:#}")))?;

    let mut anomalies = Vec::with_capacity(results.len());
    for row in results {
        anomalies.push(serde_json::from_value::<AnomalyEntry>(row).map_err(|e| {
            bad_request(
                "anomalies query failed",
                &format!("unexpected sigiro_anomalies row shape: {e}"),
            )
        })?);
    }

    // Cross-signal correlation over the returned rows: per service, shifts
    // in the same or adjacent detection bucket share an incident_id (see
    // crate::anomaly::correlate_shifts — the window is the detection pass's
    // own bucket resolution, not a threshold). Lone rows keep incident_id
    // null. Raw rows are always ALL returned; the id only says which of
    // them travel together.
    let mut by_service: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (i, a) in anomalies.iter().enumerate() {
        by_service
            .entry(a.service_name.clone())
            .or_default()
            .push(i);
    }
    for (service, idxs) in by_service {
        let ts: Vec<i64> = idxs.iter().map(|&i| anomalies[i].ts_shift_us).collect();
        for group in crate::anomaly::correlate_shifts(&ts) {
            if group.len() < 2 {
                continue;
            }
            let started_at = group.iter().map(|&g| ts[g]).min().expect("non-empty group");
            let started_iso = crate::diagnose::us_to_iso(started_at).map_err(|e| {
                bad_request("anomalies query failed", &format!("bad ts_shift: {e:#}"))
            })?;
            let id = format!("{service}@{started_iso}");
            for &g in &group {
                anomalies[idxs[g]].incident_id = Some(id.clone());
            }
        }
    }
    Ok(Json(AnomaliesResponse {
        anomalies,
        truncated,
    }))
}

/// GET /v1/traces/{trace_id} -- return a single trace with full detail including critical path and service breakdown.
#[utoipa::path(
     get,
     path = "/v1/traces/{trace_id}",
     operation_id = "get-trace",
     summary = "Fetch one whole trace",
     description = "Every span of one trace, parent links included, so a caller \
can rebuild the tree. Trace IDs come from the sampled spans in a diagnosis, \
or from any SQL query over `sigiro_spans`.",
     params(
         ("trace_id" = String, Path, description = "Trace ID")
     ),
     responses(
         (status = 200, description = "Trace detail", body = TraceDetailResponse)
     ),
 )]
#[instrument(skip_all)]
async fn trace_detail_handler(
    _path: TraceDetailPath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<TenantId>,
) -> ApiResult<Json<TraceDetailResponse>> {
    let prefix = ProjectPool::prefix();
    let trace_id = _path.trace_id;
    let safe_trace_id = escape_sql_string(trace_id.as_ref());

    let sql = format!(
        "SELECT
            trace_id, span_id, parent_span_id, span_name, duration,
            status_code, service_name, span_kind,
            span_attributes, resource_attributes,
            timestamp::VARCHAR AS start_timestamp,
            epoch_us(timestamp) AS start_us,
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
    let mut span_map: std::collections::HashMap<crate::types::SpanId, SpanDetail> =
        std::collections::HashMap::new();
    let mut parent_to_children: std::collections::HashMap<
        crate::types::SpanId,
        Vec<crate::types::SpanId>,
    > = std::collections::HashMap::new();
    let mut root_span_id: Option<crate::types::SpanId> = None;
    let mut earliest_start_us: Option<i64> = None;
    let mut latest_end_us: Option<i64> = None;
    let mut inclusive_duration_us: i64 = 0;
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

        let span_id = crate::types::SpanId::try_new(
            obj.get("span_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        )
        .map_err(|e| {
            bad_request(
                "trace detail query failed",
                &format!("invalid span_id: {e}"),
            )
        })?;
        let parent_span_id = obj
            .get("parent_span_id")
            .and_then(|v| v.as_str())
            .filter(|id| !id.is_empty())
            .map(|id| crate::types::SpanId::try_new(id.to_string()))
            .transpose()
            .map_err(|e| {
                bad_request(
                    "trace detail query failed",
                    &format!("invalid parent_span_id: {e}"),
                )
            })?;
        let duration_us = obj
            .get("duration")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let start_us = obj
            .get("start_us")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                bad_request(
                    "trace detail query failed",
                    "span start timestamp is not an integer",
                )
            })?;
        let end_us = obj
            .get("end_timestamp")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| {
                bad_request(
                    "trace detail query failed",
                    "span end timestamp is not an integer",
                )
            })?;
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
            trace_id: trace_id.clone(),
            span_id: span_id.clone(),
            parent_span_id: parent_span_id.clone(),
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
        earliest_start_us = Some(earliest_start_us.map_or(start_us, |start| start.min(start_us)));
        latest_end_us = Some(latest_end_us.map_or(end_us, |end| end.max(end_us)));
        inclusive_duration_us = inclusive_duration_us.saturating_add(duration_us);
        if status_code == 2 {
            error_count += 1;
        }
        services_set.insert(service_name);

        if let Some(parent_span_id) = parent_span_id {
            parent_to_children
                .entry(parent_span_id)
                .or_default()
                .push(span_id.clone());
        } else {
            root_span_id = Some(span_id.clone());
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

    let trace_start_us = earliest_start_us.expect("non-empty traces have a start timestamp");
    let trace_end_us = latest_end_us.expect("non-empty traces have an end timestamp");
    let total_duration_us = trace_end_us.checked_sub(trace_start_us).ok_or_else(|| {
        bad_request(
            "trace detail query failed",
            "trace end timestamp precedes its start timestamp",
        )
    })?;

    // Service durations remain cumulative inclusive work; their percentages
    // therefore use cumulative work rather than elapsed wall-clock time.
    let service_breakdown = compute_service_breakdown(&spans, inclusive_duration_us);

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
    root_id: &crate::types::SpanId,
    parent_to_children: &std::collections::HashMap<crate::types::SpanId, Vec<crate::types::SpanId>>,
    span_map: &std::collections::HashMap<crate::types::SpanId, SpanDetail>,
) -> Vec<crate::types::SpanId> {
    #[allow(clippy::too_many_arguments)]
    fn dfs(
        span_id: &crate::types::SpanId,
        parent_to_children: &std::collections::HashMap<
            crate::types::SpanId,
            Vec<crate::types::SpanId>,
        >,
        span_map: &std::collections::HashMap<crate::types::SpanId, SpanDetail>,
        current_path: &[crate::types::SpanId],
        current_duration: i64,
        best_path: &mut Vec<crate::types::SpanId>,
        best_duration: &mut i64,
    ) {
        let span = match span_map.get(span_id) {
            Some(s) => s,
            None => return,
        };
        let mut path = current_path.to_vec();
        path.push(span_id.clone());
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
    breakdown.sort_by_key(|e| std::cmp::Reverse(e.duration_us));
    breakdown
}

#[utoipa::path(
    post,
    path = "/v1/diagnose",
    operation_id = "diagnose-services",
    summary = "Diagnose services",
    request_body = diagnose::DiagnoseRequest,
    description = "\
Root-cause an incident for one or more services over a time window. Returns a \
triage summary — NOT raw telemetry: a ranked `findings` array (read this \
first), per-operation anomaly flags, and bounded samples of spans/logs. \
Every lead carries a ready-to-run SQL string (`drill_down_sql` on standalone \
findings, each incident member shift, operations, spans, and log patterns; \
`drill_all_sql` on the spans/logs \
sections) that you paste VERBATIM into `POST /v1/query` to fetch the \
underlying rows and go deeper. \
Typical agent loop: `GET /v1/services` → `POST /v1/diagnose` → for any \
standalone finding, incident member shift, or anomalous operation, run its \
`drill_down_sql` via `/v1/query`. \
Timestamps are microseconds since the Unix epoch. The response is a map \
`{results: {service: block}, errors: {service: message}}` — partial results \
survive a single failing service.",
    responses(
        (status = 200, description = "Per-service diagnosis blocks", body = diagnose::DiagnoseResponse),
        (status = 400, description = "Invalid request (e.g. timestamps not in microseconds, or to_ts <= from_ts)"),
        (status = 401, description = "Missing or invalid bearer token")
    ),
    security(("bearer" = [])),
)]
#[instrument(skip_all)]
async fn diagnose_handler(
    _path: DiagnosePath,
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<TenantId>,
    // `Valid<Json<..>>` runs the `validator` derives on DiagnoseRequest at
    // EXTRACTION time (axum's validating-extractor equivalent of middleware) —
    // from_ts/to_ts must be plausible MICROSECOND timestamps (fail loud on
    // seconds/millis/nanos), to_ts > from_ts. Invalid input never reaches here.
    Valid(Json(req)): Valid<Json<diagnose::DiagnoseRequest>>,
) -> ApiResult<Json<diagnose::DiagnoseResponse>> {
    let data_dir = crate::data_dir().context_internal("Diagnose", "SIGIRO_DATA_DIR not set")?;
    let project_data_path = if state.pool.storage_mode().is_distributed() {
        crate::storage::tenant_data_dir(&data_dir, auth.as_ref())
            .context_internal("Diagnose", "invalid tenant id")?
    } else {
        data_dir.join(auth.as_ref())
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
                "No data found for project {}. Please ingest data before calling diagnose.",
                auth
            ),
        ));
    }
    let pool = Arc::clone(&state.pool);
    let project_id = auth.as_ref().to_owned();
    let blocks = tokio::task::spawn_blocking(move || {
        let readers = pool
            .create_project(&project_id)
            .with_context(|| format!("failed to initialize project {project_id}"))?;
        let conn = readers.get();
        diagnose::diagnose(&conn, &req, ProjectPool::prefix())
    })
    .await
    .map_err(|e| anyhow::anyhow!("diagnosis task failed: {e}"))
    .context_internal("Diagnose", "diagnosis task failed")?
    .context_internal("Diagnose", "diagnosis failed")?;
    Ok(Json(blocks))
}

// --- Admin endpoints ---
// All endpoints, including status, are admin-token gated by the
// admin_auth_middleware LAYER on the /admin router (see build_router).

/// GET /admin/status — returns all instance_status rows as JSON object.
#[utoipa::path(
    get,
    path = "/admin/status",
    responses(
        (status = 200, description = "Server status", body = AdminStatusResponse)
    ),
)]
async fn admin_status(
    _path: AdminStatusPath,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<AdminStatusResponse>> {
    let guard = state.admin_conn.lock();
    let mut map =
        admin::read_all_status(&guard).context_internal("Admin", "failed to read status")?;
    map.insert(
        "ingest_dropped_messages_total".to_string(),
        state.router.dropped_messages().to_string(),
    );
    Ok(Json(AdminStatusResponse { status: map }))
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
)]
async fn create_tenant_handler(
    _path: AdminTenantsPath,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTenantRequest>,
) -> ApiResult<Json<CreateTenantResponse>> {
    let control = control_plane_from_state(&state)?;
    control
        .create_tenant(req.id.as_ref())
        .map_err(auth::ControlPlaneError::into_api_error)?;
    Ok(Json(CreateTenantResponse {
        id: req.id.into(),
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
)]
async fn create_key_handler(
    _path: AdminKeysPath,
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateKeyRequest>,
) -> ApiResult<Json<CreateKeyResponse>> {
    let control = control_plane_from_state(&state)?;
    let minted = control
        .mint_key(req.tenant_id.as_ref())
        .map_err(auth::ControlPlaneError::into_api_error)?;
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
)]
async fn revoke_key_handler(
    AdminKeyPath { short_token }: AdminKeyPath,
    State(state): State<Arc<AppState>>,
) -> ApiResult<Json<RevokeKeyResponse>> {
    let control = control_plane_from_state(&state)?;
    let revoked = control
        .revoke_key_hash_prefix(&short_token)
        .map_err(auth::ControlPlaneError::into_api_error)?;
    Ok(Json(RevokeKeyResponse {
        revoked,
        short_token,
    }))
}
