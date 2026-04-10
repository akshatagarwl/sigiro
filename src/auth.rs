use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use dashmap::DashMap;
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

/// Verify an API key with caching. Used by OTLP ingest hot path.
/// Checks in-memory cache first; on miss, falls back to SQLite verify
/// and caches result for AUTH_CACHE_TTL.
///
/// Returns AuthContext on success, ProblemDetails on failure.
/// Failed verifications are NOT cached (attacker can't poison cache).
#[allow(clippy::result_large_err)]
pub fn verify_key_cached(
    conn: &Mutex<rusqlite::Connection>,
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

    // Slow path: verify against SQLite
    let (project_id, plan) =
        crate::admin::verify_key_db(conn, raw_key).map_err(classify_auth_error)?;
    let ctx = AuthContext { project_id, plan };

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

    let (project_id, plan) =
        crate::admin::verify_key_db(&state.admin_conn, token).map_err(classify_auth_error)?;
    let ctx = AuthContext { project_id, plan };
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

/// Classify an anyhow::Error from verify_key_db into the right ProblemDetails.
/// Auth failures (malformed/invalid key) → 401.  DB errors → 500.
fn classify_auth_error(e: anyhow::Error) -> ProblemDetails {
    let msg = e.to_string();
    if msg.contains("Malformed API key") || msg.contains("Invalid API key") {
        ProblemDetails::new()
            .with_status(StatusCode::UNAUTHORIZED)
            .with_title("Invalid API Key")
            .with_detail(msg)
    } else {
        ProblemDetails::new()
            .with_status(StatusCode::INTERNAL_SERVER_ERROR)
            .with_title("Internal Error")
            .with_detail(msg)
    }
}
