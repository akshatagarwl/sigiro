use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use axum_anyhow::ApiError;
use camino::Utf8Path;
use duckdb::Connection;
use headers::{Authorization, HeaderMapExt, authorization::Bearer};
use parking_lot::Mutex;
use std::sync::Arc;

/// Auth context extracted from the API key — attached to request extensions.
/// Self-hosted: all requests route to the default project.
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub tenant_id: String,
    pub project_id: String,
}

impl AuthContext {
    fn default_project() -> Self {
        Self::for_tenant("default".to_string())
    }

    fn for_tenant(tenant_id: String) -> Self {
        Self {
            project_id: tenant_id.clone(),
            tenant_id,
        }
    }
}

/// Explicit auth mode — never implicit.
#[derive(Clone, Debug)]
pub enum AuthMode {
    /// No auth required. Any request is accepted.
    Open,
    /// Local/static mode: single shared API key routes to the default project.
    Key { key: String },
    /// Distributed mode: bearer keys are hashed and resolved through the control plane.
    TenantKeys { control: Arc<ControlPlane> },
}

pub const CONTROL_SCHEMA: &str = "sigiro_control";

impl AuthMode {
    /// Build the local/static auth mode from the configured key string.
    /// Empty string → Open mode (logged).
    /// Non-empty → Key mode (logged).
    pub fn from_config(configured_key: String) -> Self {
        if configured_key.is_empty() {
            tracing::warn!("auth_mode=open — SIGIRO_API_KEY not set, no auth required");
            AuthMode::Open
        } else {
            tracing::info!("auth_mode=key — requests require Bearer token");
            AuthMode::Key {
                key: configured_key,
            }
        }
    }

    /// Build auth for the current storage mode.
    ///
    /// `query_surface == Quack` is a single-tenant self-host edge: one
    /// operator's own dataset, reachable only behind a mandatory network
    /// perimeter (see `SIGIRO_QUACK_BIND` docs). It always uses the same
    /// static open/key auth as local mode, even when `SIGIRO_CATALOG_URL`
    /// (and S3) are configured for its DuckLake storage — `TenantKeys` is
    /// the control-plane, per-tenant auth model for the HOSTED multi-tenant
    /// product (`query_surface == None`) and does not apply here. Without
    /// this branch, a quack edge configured against a distributed catalog
    /// would demand a control-plane Bearer token on ingest that its own
    /// OTLP senders never carry, so no telemetry would ever land.
    ///
    /// Otherwise (hosted/default), distributed storage always uses
    /// control-plane tenant keys; local storage keeps the existing static
    /// behavior.
    pub fn from_runtime_config(
        configured_key: String,
        data_dir: &Utf8Path,
        query_surface: crate::config::QuerySurface,
    ) -> anyhow::Result<Self> {
        if query_surface == crate::config::QuerySurface::Quack {
            return Ok(Self::from_config(configured_key));
        }
        if let Some(storage_config) = crate::storage::load_config()? {
            if !configured_key.is_empty() {
                tracing::warn!(
                    "SIGIRO_CATALOG_URL is set; SIGIRO_API_KEY no longer authenticates requests directly. Use /admin/tenants + /admin/keys to mint tenant keys"
                );
            }
            let ext_dir = crate::storage::resolve_extension_dir(data_dir);
            let control = ControlPlane::open(&storage_config.catalog_url, &ext_dir)?;
            tracing::info!("auth_mode=tenant_keys — bearer tokens resolve through sigiro_control");
            Ok(AuthMode::TenantKeys {
                control: Arc::new(control),
            })
        } else {
            Ok(Self::from_config(configured_key))
        }
    }

    /// True if this mode requires an Authorization header.
    pub fn requires_auth(&self) -> bool {
        matches!(self, AuthMode::Key { .. } | AuthMode::TenantKeys { .. })
    }

    pub fn control_plane(&self) -> Option<Arc<ControlPlane>> {
        match self {
            AuthMode::TenantKeys { control } => Some(Arc::clone(control)),
            AuthMode::Open | AuthMode::Key { .. } => None,
        }
    }

    /// Verify a raw bearer token against this auth mode.
    /// Open mode: always succeeds.
    /// Key mode: token must match exactly.
    pub fn verify(&self, raw_key: &str) -> Result<AuthContext, ApiError> {
        match self {
            AuthMode::Open => Ok(AuthContext::default_project()),
            AuthMode::Key { key } => {
                if raw_key == key {
                    Ok(AuthContext::default_project())
                } else {
                    Err(axum_anyhow::unauthorized(
                        "Invalid API Key",
                        "The provided API key does not match SIGIRO_API_KEY",
                    ))
                }
            }
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
    pub tenant_id: String,
}

impl ControlPlane {
    const POSTGRES_ALIAS: &'static str = "sigiro_control_pg";

    pub fn open(catalog_url: &str, ext_dir: &Utf8Path) -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| anyhow::anyhow!("failed to open in-memory DuckDB for auth: {e}"))?;
        conn.execute_batch(&format!(
            "SET extension_directory = '{}';",
            crate::storage::escape_sql_string(ext_dir.as_str())
        ))
        .map_err(|e| anyhow::anyhow!("failed to set auth DuckDB extension_directory: {e}"))?;
        crate::storage::install_extensions_with_progress(&conn, &["postgres"])?;

        let catalog_path = crate::storage::postgres_uri_to_libpq(catalog_url)?;
        let attach_sql = format!(
            "ATTACH '{}' AS {} (TYPE postgres)",
            crate::storage::escape_sql_string(&catalog_path),
            Self::POSTGRES_ALIAS
        );
        conn.execute_batch(&attach_sql)
            .map_err(|e| anyhow::anyhow!("failed to attach control-plane Postgres catalog: {e}"))?;

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

    pub fn create_schema(&self) -> anyhow::Result<()> {
        for sql in control_plane_ddl() {
            self.execute_control_sql(sql)?;
        }
        Ok(())
    }

    pub fn create_tenant(&self, tenant_id: &str) -> anyhow::Result<()> {
        let tenant_id = crate::storage::sanitize_tenant_id(tenant_id)?;
        let tenant = quote_sql_literal(&tenant_id);
        self.execute_control_sql(&format!(
            "INSERT INTO {CONTROL_SCHEMA}.tenants (id, status) VALUES ({tenant}, 'active') \
             ON CONFLICT (id) DO UPDATE SET status = EXCLUDED.status"
        ))
    }

    pub fn mint_key(&self, tenant_id: &str) -> anyhow::Result<MintedApiKey> {
        let tenant_id = crate::storage::sanitize_tenant_id(tenant_id)?;
        anyhow::ensure!(
            self.tenant_is_active(&tenant_id)?,
            "tenant {tenant_id} does not exist or is not active"
        );

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

    pub fn resolve_tenant(&self, raw_key: &str) -> anyhow::Result<String> {
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
                conn.query_row(&sql, duckdb::params![key_hash], |row| row.get(0))
                    .map_err(|e| anyhow::anyhow!("API key hash not found or revoked: {e}"))
            }
        }
    }

    pub fn revoke_key_hash_prefix(&self, key_hash_prefix: &str) -> anyhow::Result<bool> {
        anyhow::ensure!(
            (8..=64).contains(&key_hash_prefix.len())
                && key_hash_prefix.bytes().all(|b| b.is_ascii_hexdigit()),
            "key hash prefix must be 8-64 hexadecimal characters"
        );
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
            _ => anyhow::bail!(
                "key hash prefix matched {} active keys; provide a longer prefix",
                matches.len()
            ),
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
                crate::storage::escape_sql_string(postgres_sql)
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
            crate::storage::escape_sql_string(postgres_sql)
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
            crate::storage::escape_sql_string(postgres_sql)
        );
        conn.query_row(&sql, [], |row| row.get(0))
            .map_err(|e| anyhow::anyhow!("failed to query control-plane Postgres count: {e}"))
    }
}

pub fn control_plane_ddl() -> [&'static str; 3] {
    [
        "CREATE SCHEMA IF NOT EXISTS sigiro_control",
        "CREATE TABLE IF NOT EXISTS sigiro_control.tenants (
            id TEXT PRIMARY KEY,
            created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
            status TEXT NOT NULL
        )",
        "CREATE TABLE IF NOT EXISTS sigiro_control.api_keys (
            key_hash TEXT PRIMARY KEY,
            tenant_id TEXT NOT NULL,
            created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT now(),
            revoked_at TIMESTAMP WITH TIME ZONE NULL,
            FOREIGN KEY (tenant_id) REFERENCES sigiro_control.tenants(id)
        )",
    ]
}

pub fn hash_api_key(raw_key: &str) -> String {
    use prefixed_api_key::sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn generate_api_key(tenant_id: &str) -> anyhow::Result<String> {
    let tenant_id = crate::storage::sanitize_tenant_id(tenant_id)?;
    let mut bytes = [0_u8; 32];
    let mut rng = prefixed_api_key::rand::rngs::OsRng;
    prefixed_api_key::rand::RngCore::try_fill_bytes(&mut rng, &mut bytes)
        .map_err(|e| anyhow::anyhow!("failed to generate API key randomness: {e}"))?;
    Ok(format!("sk_{tenant_id}_{}", hex::encode(bytes)))
}

fn quote_sql_literal(value: &str) -> String {
    format!("'{}'", crate::storage::escape_sql_string(value))
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
/// Key mode: requires valid Bearer token.
pub async fn auth_middleware(
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
        AuthMode::Key { .. } | AuthMode::TenantKeys { .. } => {
            let token = extract_bearer_token(req.headers())?;
            let ctx = state.auth_mode.verify(&token)?;
            let mut req = req;
            req.extensions_mut().insert(ctx);
            Ok(next.run(req).await)
        }
    }
}
