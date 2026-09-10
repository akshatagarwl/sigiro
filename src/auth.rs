use crate::types::TenantId;
use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use axum_anyhow::{ApiError, IntoApiError};
use camino::Utf8Path;
use duckdb::Connection;
use headers::{Authorization, HeaderMapExt, authorization::Bearer};
use parking_lot::Mutex;
use std::sync::Arc;

/// Auth context extracted from the API key — attached to request extensions.
/// Self-hosted: all requests route to the default project.
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub tenant_id: TenantId,
    pub project_id: String,
}

impl AuthContext {
    fn default_project() -> Self {
        Self::for_tenant(
            TenantId::try_new("default".to_owned()).expect("default is a valid tenant id"),
        )
    }

    fn for_tenant(tenant_id: TenantId) -> Self {
        Self {
            project_id: tenant_id.as_ref().to_owned(),
            tenant_id,
        }
    }
}

/// Explicit auth mode — never implicit. The dual-mode auth axis:
/// - `Open`: no per-request auth (self-host / edge / quack, protected by the
///   network perimeter). Selected when `SIGIRO_CATALOG_URL` is unset, or on any
///   quack edge regardless of catalog.
/// - `TenantKeys`: per-tenant bearer keys hashed and resolved through the
///   HOSTED control plane. Selected when `SIGIRO_CATALOG_URL` is set (and the
///   node is not a quack edge).
#[derive(Clone, Debug)]
pub enum AuthMode {
    /// No auth required. Any request is accepted.
    Open,
    /// Distributed mode: bearer keys are hashed and resolved through the control plane.
    TenantKeys { control: Arc<ControlPlane> },
}

pub(crate) const CONTROL_SCHEMA: &str = "sigiro_control";

impl AuthMode {
    /// Build auth for the current storage mode.
    ///
    /// `query_surface == Quack` is a single-tenant self-host edge: one
    /// operator's own dataset, reachable only behind a mandatory network
    /// perimeter (see `SIGIRO_QUACK_BIND` docs). It always uses `Open` auth,
    /// even when `SIGIRO_CATALOG_URL` (and S3) are configured for its DuckLake
    /// storage — `TenantKeys` is the control-plane, per-tenant auth model for
    /// the HOSTED multi-tenant product (`query_surface == None`) and does not
    /// apply here. Without this branch, a quack edge configured against a
    /// distributed catalog would demand a control-plane Bearer token on ingest
    /// that its own OTLP senders never carry, so no telemetry would ever land.
    ///
    /// Otherwise: distributed storage (`SIGIRO_CATALOG_URL`) uses control-plane
    /// tenant keys; local storage is `Open`.
    pub fn from_runtime_config(
        data_dir: &Utf8Path,
        query_surface: crate::config::QuerySurface,
        storage_mode: &crate::storage::StorageMode,
    ) -> anyhow::Result<Self> {
        if query_surface == crate::config::QuerySurface::Quack {
            tracing::info!("auth_mode=open — quack edge, protected by the network perimeter");
            return Ok(AuthMode::Open);
        }
        match storage_mode.distributed_config() {
            Some(storage_config) => {
                let ext_dir = crate::storage::resolve_extension_dir(data_dir);
                let control = ControlPlane::open(storage_config.catalog_url(), &ext_dir)?;
                tracing::info!(
                    "auth_mode=tenant_keys — bearer tokens resolve through sigiro_control"
                );
                Ok(AuthMode::TenantKeys {
                    control: Arc::new(control),
                })
            }
            None => {
                tracing::warn!(
                    "sigiro running in OPEN auth mode — no per-request authentication; \
                     protect the network perimeter (tailnet/private network/reverse-proxy)"
                );
                Ok(AuthMode::Open)
            }
        }
    }

    /// True if this mode requires an Authorization header.
    pub fn requires_auth(&self) -> bool {
        matches!(self, AuthMode::TenantKeys { .. })
    }

    pub(crate) fn control_plane(&self) -> Option<Arc<ControlPlane>> {
        match self {
            AuthMode::TenantKeys { control } => Some(Arc::clone(control)),
            AuthMode::Open => None,
        }
    }

    /// Verify a raw bearer token against this auth mode.
    /// Open mode: always succeeds.
    /// TenantKeys mode: token must resolve to an active tenant via the control plane.
    pub fn verify(&self, raw_key: &str) -> Result<AuthContext, ApiError> {
        match self {
            AuthMode::Open => Ok(AuthContext::default_project()),
            AuthMode::TenantKeys { control } => {
                let tenant_id = control.resolve_tenant(raw_key).map_err(|e| {
                    tracing::warn!("tenant API key rejected: {e:#}");
                    axum_anyhow::unauthorized(
                        "Invalid API Key",
                        "The provided API key is unknown or revoked",
                    )
                })?;
                Ok(AuthContext::for_tenant(tenant_id))
            }
        }
    }
}

/// Errors from the hosted control-plane provisioning surface. The non-`Internal`
/// variants are DETERMINISTIC CLIENT-INPUT failures — they depend only on what
/// the admin caller sent, not on server/DB state — so the admin HTTP handlers
/// map them to 4xx (via [`ControlPlaneError::into_api_error`]) instead of the
/// blanket 500 a raw `anyhow` error would produce. `Internal` stays a genuine
/// server error (5xx): it wraps a real control-DB/connection failure.
///
/// The mapping is an inherent method, NOT `From<_> for ApiError`, because
/// `axum_anyhow` already blankets `From<E: Into<anyhow::Error>> for ApiError`
/// (→ 500); a specific `From` would conflict, and a bare `?` on this error in a
/// handler would silently take that blanket 500 path. Handlers therefore call
/// `.map_err(ControlPlaneError::into_api_error)?`.
#[derive(Debug, thiserror::Error)]
pub enum ControlPlaneError {
    /// Malformed admin input — bad tenant id (empty, illegal chars, too long,
    /// or a reserved system name) or a bad key-hash prefix. Maps to 400.
    #[error("{0}")]
    InvalidInput(String),
    /// Request conflicts with a reserved name or an ambiguous match: the edge
    /// name `default` on the hosted provisioning path, or a key-hash prefix
    /// that matches more than one live key. Maps to 409.
    #[error("{0}")]
    Conflict(String),
    /// The target tenant does not exist or is suspended. Maps to 404.
    #[error("{0}")]
    TenantNotActive(String),
    /// A genuine control-plane/database failure. Maps to 500.
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ControlPlaneError {
    /// Map to the HTTP error: deterministic client-input failures → 4xx, a real
    /// control-plane/DB failure → 500 (with the cause attached, unchanged from
    /// the previous `context_internal` behavior).
    pub(crate) fn into_api_error(self) -> ApiError {
        match self {
            ControlPlaneError::InvalidInput(detail) => {
                axum_anyhow::bad_request("Invalid request", &detail)
            }
            ControlPlaneError::Conflict(detail) => axum_anyhow::conflict("Conflict", &detail),
            ControlPlaneError::TenantNotActive(detail) => {
                axum_anyhow::not_found("Tenant not found", &detail)
            }
            ControlPlaneError::Internal(e) => {
                e.context_internal("Admin", "control-plane operation failed")
            }
        }
    }
}

#[derive(Clone)]
pub struct ControlPlane {
    conn: Arc<Mutex<Connection>>,
    qualified_schema: String,
    postgres_alias: Option<&'static str>,
}

impl std::fmt::Debug for ControlPlane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlPlane")
            .field("qualified_schema", &self.qualified_schema)
            .field("postgres_alias", &self.postgres_alias)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedApiKey {
    pub key: String,
    pub key_hash: String,
    pub(crate) tenant_id: String,
}

impl ControlPlane {
    const POSTGRES_ALIAS: &'static str = "sigiro_control_pg";

    pub(crate) fn open(catalog_url: &str, ext_dir: &Utf8Path) -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| anyhow::anyhow!("failed to open in-memory DuckDB for auth: {e}"))?;
        conn.execute_batch(&format!(
            "SET extension_directory = '{}';",
            crate::sql::escape_sql_string(ext_dir.as_str())
        ))
        .map_err(|e| anyhow::anyhow!("failed to set auth DuckDB extension_directory: {e}"))?;
        crate::storage::install_extensions_with_progress(&conn, &["postgres"])?;

        let catalog_path = crate::storage::postgres_uri_to_libpq(catalog_url)?;
        let attach_sql = format!(
            "ATTACH '{}' AS {} (TYPE postgres)",
            crate::sql::escape_sql_string(&catalog_path),
            Self::POSTGRES_ALIAS
        );
        conn.execute_batch(&attach_sql).map_err(|_| {
            // Do NOT include the DuckDB error — a Postgres ATTACH failure can
            // echo the connection string (with password). Secret-free message.
            anyhow::anyhow!(
                "failed to attach control-plane PostgreSQL catalog — verify \
                 SIGIRO_CATALOG_URL host/port/credentials and PostgreSQL reachability"
            )
        })?;

        let control = Self {
            conn: Arc::new(Mutex::new(conn)),
            qualified_schema: CONTROL_SCHEMA.to_string(),
            postgres_alias: Some(Self::POSTGRES_ALIAS),
        };
        control.create_schema()?;
        Ok(control)
    }

    #[doc(hidden)]
    pub fn open_in_memory_for_tests() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| anyhow::anyhow!("failed to open in-memory DuckDB for auth test: {e}"))?;
        let control = Self {
            conn: Arc::new(Mutex::new(conn)),
            qualified_schema: CONTROL_SCHEMA.to_string(),
            postgres_alias: None,
        };
        control.create_schema()?;
        Ok(control)
    }

    pub(crate) fn create_schema(&self) -> anyhow::Result<()> {
        for sql in control_plane_ddl() {
            self.execute_control_sql(sql)?;
        }
        // Partial indexes are a PostgreSQL feature the DuckDB in-memory test
        // control plane cannot parse, so they only run against a real hosted
        // Postgres catalog (`postgres_alias.is_some()`).
        if self.postgres_alias.is_some() {
            for sql in HOSTED_CONTROL_PLANE_DDL {
                self.execute_control_sql(sql)?;
            }
        }
        Ok(())
    }

    /// Create or reactivate a HOSTED tenant. Idempotent + concurrency-safe: the
    /// `ON CONFLICT (id)` upsert means a retried or concurrent admin call
    /// converges on exactly one `active` row (never a duplicate/ambiguous one).
    pub fn create_tenant(&self, tenant_id: &str) -> Result<(), ControlPlaneError> {
        let tenant_id = ensure_hosted_tenant_id(tenant_id)?;
        let tenant = quote_sql_literal(&tenant_id);
        self.execute_control_sql(&format!(
            "INSERT INTO {CONTROL_SCHEMA}.tenants (id, status) VALUES ({tenant}, 'active') \
             ON CONFLICT (id) DO UPDATE SET status = EXCLUDED.status"
        ))?;
        Ok(())
    }

    pub fn mint_key(&self, tenant_id: &str) -> Result<MintedApiKey, ControlPlaneError> {
        let tenant_id = ensure_hosted_tenant_id(tenant_id)?;
        if !self.tenant_is_active(&tenant_id)? {
            return Err(ControlPlaneError::TenantNotActive(format!(
                "tenant {tenant_id} does not exist or is not active"
            )));
        }

        let key = generate_api_key(&tenant_id)?;
        let key_hash = hash_api_key(&key);
        self.execute_control_sql(&format!(
            "INSERT INTO {CONTROL_SCHEMA}.api_keys (key_hash, tenant_id) VALUES ({}, {})",
            quote_sql_literal(&key_hash),
            quote_sql_literal(&tenant_id)
        ))?;

        Ok(MintedApiKey {
            key,
            key_hash,
            tenant_id,
        })
    }

    pub(crate) fn resolve_tenant(&self, raw_key: &str) -> anyhow::Result<TenantId> {
        anyhow::ensure!(
            raw_key.starts_with("sk_"),
            "tenant API key must use the sk_<tenant>_<random> format"
        );
        let key_hash = hash_api_key(raw_key);
        match self.postgres_alias {
            Some(alias) => {
                let sql = format!(
                    "SELECT k.tenant_id \
                     FROM {CONTROL_SCHEMA}.api_keys k \
                     JOIN {CONTROL_SCHEMA}.tenants t ON t.id = k.tenant_id \
                     WHERE k.key_hash = {} AND k.revoked_at IS NULL AND t.status = 'active' \
                     LIMIT 1",
                    quote_sql_literal(&key_hash)
                );
                self.query_postgres_strings(alias, &sql)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("API key hash not found or revoked"))
                    .and_then(|id| {
                        TenantId::try_new(id)
                            .map_err(|e| anyhow::anyhow!("invalid tenant id in control plane: {e}"))
                    })
            }
            None => {
                let conn = self.conn.lock();
                let sql = format!(
                    "SELECT k.tenant_id \
                     FROM {}.api_keys k \
                     JOIN {}.tenants t ON t.id = k.tenant_id \
                     WHERE k.key_hash = ? AND k.revoked_at IS NULL AND t.status = 'active' \
                     LIMIT 1",
                    self.qualified_schema, self.qualified_schema
                );
                conn.query_row(&sql, duckdb::params![key_hash], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| anyhow::anyhow!("API key hash not found or revoked: {e}"))
                .and_then(|id| {
                    TenantId::try_new(id)
                        .map_err(|e| anyhow::anyhow!("invalid tenant id in control plane: {e}"))
                })
            }
        }
    }

    pub fn revoke_key_hash_prefix(&self, key_hash_prefix: &str) -> Result<bool, ControlPlaneError> {
        if !((8..=64).contains(&key_hash_prefix.len())
            && key_hash_prefix.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(ControlPlaneError::InvalidInput(
                "key hash prefix must be 8-64 hexadecimal characters".to_string(),
            ));
        }
        let matches = self.matching_unrevoked_hashes(key_hash_prefix)?;
        match matches.as_slice() {
            [] => Ok(false),
            [key_hash] => {
                self.execute_control_sql(&format!(
                    "UPDATE {CONTROL_SCHEMA}.api_keys SET revoked_at = now() WHERE key_hash = {}",
                    quote_sql_literal(key_hash)
                ))?;
                Ok(true)
            }
            _ => Err(ControlPlaneError::Conflict(format!(
                "key hash prefix matched {} active keys; provide a longer prefix",
                matches.len()
            ))),
        }
    }

    fn tenant_is_active(&self, tenant_id: &str) -> anyhow::Result<bool> {
        match self.postgres_alias {
            Some(alias) => {
                let sql = format!(
                    "SELECT count(*)::BIGINT \
                     FROM {CONTROL_SCHEMA}.tenants \
                     WHERE id = {} AND status = 'active'",
                    quote_sql_literal(tenant_id)
                );
                Ok(self.query_postgres_i64(alias, &sql)? > 0)
            }
            None => {
                let conn = self.conn.lock();
                let sql = format!(
                    "SELECT count(*) FROM {}.tenants WHERE id = ? AND status = 'active'",
                    self.qualified_schema
                );
                let count: i64 = conn
                    .query_row(&sql, duckdb::params![tenant_id], |row| row.get(0))
                    .map_err(|e| anyhow::anyhow!("failed to check tenant status: {e}"))?;
                Ok(count > 0)
            }
        }
    }

    fn matching_unrevoked_hashes(&self, key_hash_prefix: &str) -> anyhow::Result<Vec<String>> {
        match self.postgres_alias {
            Some(alias) => {
                let sql = format!(
                    "SELECT key_hash FROM {CONTROL_SCHEMA}.api_keys \
                     WHERE key_hash LIKE {} AND revoked_at IS NULL \
                     ORDER BY key_hash",
                    quote_sql_literal(&format!("{key_hash_prefix}%"))
                );
                self.query_postgres_strings(alias, &sql)
            }
            None => {
                let conn = self.conn.lock();
                let sql = format!(
                    "SELECT key_hash FROM {}.api_keys \
                     WHERE key_hash LIKE ? AND revoked_at IS NULL \
                     ORDER BY key_hash",
                    self.qualified_schema
                );
                let mut stmt = conn
                    .prepare(&sql)
                    .map_err(|e| anyhow::anyhow!("failed to prepare key revoke lookup: {e}"))?;
                let rows = stmt
                    .query_map(duckdb::params![format!("{key_hash_prefix}%")], |row| {
                        row.get::<_, String>(0)
                    })
                    .map_err(|e| anyhow::anyhow!("failed to query key revoke lookup: {e}"))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|e| anyhow::anyhow!("failed to read key revoke lookup row: {e}"))
            }
        }
    }

    fn execute_control_sql(&self, postgres_sql: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock();
        if let Some(alias) = self.postgres_alias {
            let sql = format!(
                "CALL postgres_execute('{}', '{}')",
                alias,
                crate::sql::escape_sql_string(postgres_sql)
            );
            conn.execute_batch(&sql)
                .map_err(|e| anyhow::anyhow!("control-plane Postgres statement failed: {e}"))
        } else {
            conn.execute_batch(postgres_sql)
                .map_err(|e| anyhow::anyhow!("control-plane statement failed: {e}"))
        }
    }

    fn query_postgres_strings(
        &self,
        alias: &str,
        postgres_sql: &str,
    ) -> anyhow::Result<Vec<String>> {
        let conn = self.conn.lock();
        let sql = format!(
            "SELECT * FROM postgres_query('{}', '{}')",
            alias,
            crate::sql::escape_sql_string(postgres_sql)
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| anyhow::anyhow!("failed to prepare control-plane Postgres query: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| anyhow::anyhow!("failed to query control-plane Postgres: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("failed to read control-plane Postgres row: {e}"))
    }

    fn query_postgres_i64(&self, alias: &str, postgres_sql: &str) -> anyhow::Result<i64> {
        let conn = self.conn.lock();
        let sql = format!(
            "SELECT * FROM postgres_query('{}', '{}')",
            alias,
            crate::sql::escape_sql_string(postgres_sql)
        );
        conn.query_row(&sql, [], |row| row.get(0))
            .map_err(|e| anyhow::anyhow!("failed to query control-plane Postgres count: {e}"))
    }
}

/// `"default"` is reserved in the HOSTED control plane: it names the local/edge
/// project (pre-created by `run_serve`, reachable via Open auth), so
/// a hosted operator must never provision a control-plane tenant literally
/// named `default`. `sanitize_tenant_id` deliberately still ACCEPTS `"default"`
/// (the edge depends on it) — this extra rejection applies only on the hosted
/// provisioning path.
fn ensure_hosted_tenant_id(tenant_id: &str) -> Result<String, ControlPlaneError> {
    let tenant_id = crate::storage::sanitize_tenant_id(tenant_id)
        .map_err(|e| ControlPlaneError::InvalidInput(format!("{e:#}")))?;
    if tenant_id.as_ref() == "default" {
        return Err(ControlPlaneError::Conflict(
            "tenant id \"default\" is reserved for the local/edge project and cannot be provisioned in the hosted control plane".to_string(),
        ));
    }
    Ok(tenant_id.into())
}

/// Portable control-plane DDL — runs against BOTH the real hosted PostgreSQL
/// catalog and the DuckDB in-memory test control plane, so every statement must
/// parse under both engines (no PostgreSQL-only features here; those live in
/// [`HOSTED_CONTROL_PLANE_DDL`]).
///
/// Hardening over the original bare PK/FK schema:
/// - `tenants.status` gets a `CHECK` allowing only `active`/`suspended`, so an
///   unknown status can never enter the table (the key-resolution join treats
///   anything other than `active` as unusable).
/// - `api_keys.key_hash` stays the PRIMARY KEY (uniqueness), with `revoked_at`
///   as the explicit revocation timestamp — `NULL` = live, non-`NULL` = revoked.
/// - `api_keys(tenant_id)` is indexed to support the FK join / per-tenant
///   lookups without a table scan.
pub(crate) fn control_plane_ddl() -> &'static [&'static str] {
    &[
        "CREATE SCHEMA IF NOT EXISTS sigiro_control",
        "CREATE TABLE IF NOT EXISTS sigiro_control.tenants (
            id TEXT PRIMARY KEY,
            created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
            status TEXT NOT NULL DEFAULT 'active'
                CHECK (status IN ('active', 'suspended'))
        )",
        "CREATE TABLE IF NOT EXISTS sigiro_control.api_keys (
            key_hash TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
            revoked_at TIMESTAMP WITH TIME ZONE NULL,
            FOREIGN KEY (tenant_id) REFERENCES sigiro_control.tenants(id)
        )",
        "CREATE INDEX IF NOT EXISTS api_keys_tenant_id_idx \
         ON sigiro_control.api_keys (tenant_id)",
    ]
}

/// PostgreSQL-only control-plane DDL — applied only when a real hosted Postgres
/// catalog is attached (see [`ControlPlane::create_schema`]). The partial index
/// mirrors the key-resolution predicate (`key_hash = ? AND revoked_at IS NULL`),
/// keeping the hot auth lookup on live keys only; DuckDB cannot parse the
/// `WHERE` clause, so it never runs on the in-memory test control plane.
pub(crate) const HOSTED_CONTROL_PLANE_DDL: &[&str] =
    &["CREATE INDEX IF NOT EXISTS api_keys_live_key_hash_idx \
     ON sigiro_control.api_keys (key_hash) WHERE revoked_at IS NULL"];

pub fn hash_api_key(raw_key: &str) -> String {
    use prefixed_api_key::sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) fn generate_api_key(tenant_id: &str) -> anyhow::Result<String> {
    let tenant_id = crate::storage::sanitize_tenant_id(tenant_id)?;
    let mut bytes = [0_u8; 32];
    let mut rng = prefixed_api_key::rand::rngs::OsRng;
    prefixed_api_key::rand::RngCore::try_fill_bytes(&mut rng, &mut bytes)
        .map_err(|e| anyhow::anyhow!("failed to generate API key randomness: {e}"))?;
    Ok(format!("sk_{tenant_id}_{}", hex::encode(bytes)))
}

fn quote_sql_literal(value: &str) -> String {
    format!("'{}'", crate::sql::escape_sql_string(value))
}

/// Extract bearer token from Authorization header.
pub fn extract_bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let auth: Authorization<Bearer> = headers.typed_get().ok_or_else(|| {
        axum_anyhow::unauthorized(
            "Missing Authentication",
            "Missing or malformed Authorization header",
        )
    })?;
    Ok(auth.token().to_string())
}

/// axum middleware: verify API key against the configured mode.
/// Open mode: injects default context without checking headers.
/// TenantKeys mode: requires a valid Bearer token.
/// Admin-token gate for `/admin/*`, as a LAYER — the counterpart to
/// [`auth_middleware`] for `/v1/*`.
///
/// Every admin handler used to call a `require_admin_token(&headers)` helper as
/// its first line, which meant the gate was only as good as the author of the
/// next handler remembering to write it. A route added without that line is an
/// unauthenticated admin endpoint — tenant provisioning and key minting live
/// here, so the failure mode is silent and total. Making it a layer means the
/// router, not the handler, enforces it: forgetting is no longer expressible.
///
/// Admin auth is deliberately a DIFFERENT credential from tenant auth (an admin
/// token mints tenant keys; it is not one), so this stays a separate middleware
/// rather than a mode inside `auth_middleware`. What is now shared is the shape:
/// both are layers applied to a nested router, both use
/// [`extract_bearer_token`], and neither is a per-handler call.
pub(crate) async fn admin_auth_middleware(req: Request, next: Next) -> Result<Response, ApiError> {
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
    let token = extract_bearer_token(req.headers())?;
    // Constant-time: a short-circuiting `==` on a fixed secret leaks its prefix
    // through response timing. Lengths are compared first and non-secret.
    let matches = token.len() == expected.len()
        && token
            .as_bytes()
            .iter()
            .zip(expected.as_bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0;
    if !matches {
        return Err(axum_anyhow::unauthorized(
            "Invalid Admin Token",
            "The provided admin bearer token is invalid",
        ));
    }
    Ok(next.run(req).await)
}

pub(crate) async fn auth_middleware(
    State(state): State<Arc<crate::server::AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    match &state.auth_mode {
        AuthMode::Open => {
            let mut req = req;
            req.extensions_mut().insert(AuthContext::default_project());
            Ok(next.run(req).await)
        }
        AuthMode::TenantKeys { .. } => {
            let token = extract_bearer_token(req.headers())?;
            let ctx = state.auth_mode.verify(&token)?;
            let mut req = req;
            req.extensions_mut().insert(ctx);
            Ok(next.run(req).await)
        }
    }
}
