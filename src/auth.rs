use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
use duckdb::Connection;
use prefixed_api_key::{PrefixedApiKey, PrefixedApiKeyController};
use problem_details::ProblemDetails;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Auth context extracted from the API key — attached to request extensions.
/// Handlers use this to route to the correct per-project DuckDB.
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub project_id: String,
    pub plan: String,
}

/// Auth cache TTL — verified keys cached for 60 seconds.
/// API keys don't change often; this eliminates the admin DuckDB mutex
/// contention on the ingest hot path (every OTLP request was hitting it).
const AUTH_CACHE_TTL: Duration = Duration::from_secs(60);

struct CachedAuth {
    ctx: AuthContext,
    expires: Instant,
}

/// Global auth cache: raw_key → (AuthContext, expiry).
/// DashMap for concurrent reads from tokio + inserter threads.
fn auth_cache() -> &'static DashMap<String, CachedAuth> {
    static CACHE: OnceLock<DashMap<String, CachedAuth>> = OnceLock::new();
    CACHE.get_or_init(|| DashMap::with_capacity(1024))
}

/// Verify an API key with caching. Used by the OTLP ingest hot path.
/// Checks the in-memory cache first; on miss, falls back to DuckDB verify
/// and caches the result for AUTH_CACHE_TTL.
///
/// Returns AuthContext on success, ProblemDetails on failure.
/// Failed verifications are NOT cached (attacker can't poison the cache).
#[allow(clippy::result_large_err)]
pub fn verify_key_cached(
    conn: &Mutex<Connection>,
    raw_key: &str,
) -> Result<AuthContext, ProblemDetails> {
    let cache = auth_cache();

    // Fast path: cache hit with valid TTL
    if let Some(entry) = cache.get(raw_key) {
        if entry.expires > Instant::now() {
            return Ok(entry.ctx.clone());
        }
        // Expired — drop ref before removing to avoid deadlock
        drop(entry);
        cache.remove(raw_key);
    }

    // Slow path: verify against DuckDB
    let ctx = verify_key(conn, raw_key)?;

    // Cache successful verification
    cache.insert(
        raw_key.to_string(),
        CachedAuth {
            ctx: ctx.clone(),
            expires: Instant::now() + AUTH_CACHE_TTL,
        },
    );

    Ok(ctx)
}

/// Build the prefixed-api-key controller with sigiro's key format.
/// Keys look like: sigiro_BRTRKFsL_51FwqftsmMDHHbJAMEXXHCgG
pub fn key_controller() -> prefixed_api_key::PakControllerOsSha256 {
    PrefixedApiKeyController::configure()
        .prefix("sigiro".to_string())
        .seam_defaults() // OsRng + SHA-256 + default lengths (8 short, 24 long)
        .finalize()
        .expect("valid controller config")
}

/// Create the api_keys table in the admin DuckDB.
pub fn create_keys_table(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sigiro_api_keys (
            short_token TEXT PRIMARY KEY,
            long_token_hash TEXT NOT NULL,
            project_id TEXT NOT NULL,
            plan TEXT NOT NULL DEFAULT 'free',
            rate_limit INTEGER NOT NULL DEFAULT 100,
            created_at TIMESTAMP DEFAULT current_timestamp
        )",
    )?;
    Ok(())
}

/// Generate a new API key, store the hash in admin DB, return the raw key string.
/// The raw key is shown once to the user and never stored.
pub fn create_key(
    conn: &Mutex<Connection>,
    project_id: &str,
    plan: &str,
    rate_limit: u32,
) -> anyhow::Result<String> {
    let controller = key_controller();
    let (pak, hash) = controller
        .try_generate_key_and_hash()
        .map_err(|e| anyhow::anyhow!("key generation failed: {e}"))?;

    let short_token = pak.short_token().to_string();
    let hash_str = hash.to_string();
    let key_string = pak.to_string();

    let guard = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    guard.execute(
        "INSERT INTO sigiro_api_keys (short_token, long_token_hash, project_id, plan, rate_limit) \
         VALUES (?, ?, ?, ?, ?)",
        duckdb::params![short_token, hash_str, project_id, plan, rate_limit as i32],
    )?;

    Ok(key_string)
}

/// Verify an API key against the admin DB.
/// Returns AuthContext on success, ProblemDetails on failure.
#[allow(clippy::result_large_err)] // ProblemDetails is the axum error response type
pub fn verify_key(conn: &Mutex<Connection>, raw_key: &str) -> Result<AuthContext, ProblemDetails> {
    let pak =
        PrefixedApiKey::from_string(raw_key).map_err(|_| invalid_api_key("Malformed API key"))?;

    let short_token = pak.short_token().to_string();
    let controller = key_controller();

    let guard = conn.lock().map_err(|e| {
        tracing::error!("admin DB mutex poisoned: {e}");
        internal("Auth database unavailable")
    })?;

    let mut stmt = guard
        .prepare(
            "SELECT long_token_hash, project_id, plan FROM sigiro_api_keys WHERE short_token = ?",
        )
        .map_err(|e| internal(e.to_string()))?;

    let result = stmt.query_row([&short_token], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    });

    match result {
        Ok((stored_hash, project_id, plan)) => {
            if controller.check_hash(&pak, &stored_hash) {
                Ok(AuthContext { project_id, plan })
            } else {
                Err(invalid_api_key("Invalid API key"))
            }
        }
        Err(duckdb::Error::QueryReturnedNoRows) => Err(invalid_api_key("Invalid API key")),
        Err(e) => Err(internal(e.to_string())),
    }
}

/// Revoke an API key by short token.
pub fn revoke_key(conn: &Mutex<Connection>, short_token: &str) -> anyhow::Result<bool> {
    let guard = conn.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
    let affected = guard.execute(
        "DELETE FROM sigiro_api_keys WHERE short_token = ?",
        [short_token],
    )?;
    Ok(affected > 0)
}

/// Extract bearer token from Authorization header.
#[allow(clippy::result_large_err)] // ProblemDetails is the axum error response type
pub fn extract_bearer_token(headers: &HeaderMap) -> Result<&str, ProblemDetails> {
    let value = headers
        .get("authorization")
        .ok_or_else(|| missing_auth("Missing Authorization header"))?
        .to_str()
        .map_err(|_| missing_auth("Invalid Authorization header encoding"))?;
    value
        .strip_prefix("Bearer ")
        .ok_or_else(|| missing_auth("Expected: Bearer <key>"))
}

/// axum middleware: verify API key against local DuckDB.
/// No network calls. No cache needed. Hash comparison is <1us.
pub async fn auth_middleware(
    State(state): State<Arc<crate::server::AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ProblemDetails> {
    let token = extract_bearer_token(req.headers())?;

    // Dev key bypass — same middleware runs, dev keys skip DB verification
    if token.starts_with("sigiro_dev_") {
        let ctx = AuthContext {
            project_id: "dev_project".to_string(),
            plan: "free".to_string(),
        };
        let mut req = req;
        req.extensions_mut().insert(ctx);
        return Ok(next.run(req).await);
    }

    let ctx = verify_key(&state.admin_conn, token)?;
    let mut req = req;
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}

fn missing_auth(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new()
        .with_status(StatusCode::UNAUTHORIZED)
        .with_title("Missing Authentication")
        .with_detail(detail)
}

fn invalid_api_key(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new()
        .with_status(StatusCode::UNAUTHORIZED)
        .with_title("Invalid API Key")
        .with_detail(detail)
}

fn internal(detail: impl Into<String>) -> ProblemDetails {
    ProblemDetails::new()
        .with_status(StatusCode::INTERNAL_SERVER_ERROR)
        .with_title("Internal Error")
        .with_detail(detail)
}
