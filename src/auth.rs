use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use axum_anyhow::ApiError;
use dashmap::DashMap;
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
/// Returns AuthContext on success, ApiError on failure.
/// Failed verifications are NOT cached (attacker can't poison cache).
pub fn verify_key_cached(
    conn: &Mutex<rusqlite::Connection>,
    raw_key: &str,
) -> Result<AuthContext, ApiError> {
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
    let (project_id, plan) = crate::admin::verify_key_db(conn, raw_key).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Malformed API key") || msg.contains("Invalid API key") {
            axum_anyhow::unauthorized("Invalid API Key", &msg)
        } else {
            axum_anyhow::internal_error("Internal Error", &msg)
        }
    })?;
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
pub fn extract_bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    let value = headers
        .get("authorization")
        .ok_or_else(|| {
            axum_anyhow::unauthorized("Missing Authentication", "Missing Authorization header")
        })?
        .to_str()
        .map_err(|_| {
            axum_anyhow::unauthorized(
                "Missing Authentication",
                "Invalid Authorization header encoding",
            )
        })?;
    value.strip_prefix("Bearer ").ok_or_else(|| {
        axum_anyhow::unauthorized("Missing Authentication", "Expected: Bearer <key>")
    })
}

/// axum middleware: verify API key against local DuckDB.
/// No network calls. No cache needed. Hash comparison is <1us.
pub async fn auth_middleware(
    State(state): State<Arc<crate::server::AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
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
        crate::admin::verify_key_db(&state.admin_conn, token).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Malformed API key") || msg.contains("Invalid API key") {
                axum_anyhow::unauthorized("Invalid API Key", &msg)
            } else {
                axum_anyhow::internal_error("Internal Error", &msg)
            }
        })?;
    let ctx = AuthContext { project_id, plan };
    let mut req = req;
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}
