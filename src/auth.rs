use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use duckdb::Connection;
use prefixed_api_key::{PrefixedApiKey, PrefixedApiKeyController};
use problem_details::ProblemDetails;
use std::sync::{Arc, Mutex};

/// Auth context extracted from the API key — attached to request extensions.
/// Handlers use this to route to the correct per-project DuckDB.
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub project_id: String,
    pub plan: String,
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    fn test_admin_conn() -> Mutex<Connection> {
        let conn = Connection::open_in_memory().unwrap();
        create_keys_table(&conn).unwrap();
        Mutex::new(conn)
    }

    #[test]
    fn extract_bearer_token_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer sigiro_abc_defghi".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers).unwrap(), "sigiro_abc_defghi");
    }

    #[test]
    fn extract_bearer_token_missing() {
        let headers = HeaderMap::new();
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn extract_bearer_token_malformed() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Basic xyz".parse().unwrap());
        assert!(extract_bearer_token(&headers).is_err());
    }

    #[test]
    fn create_and_verify_key() {
        let conn = test_admin_conn();
        let raw_key = create_key(&conn, "proj_test", "free", 100).unwrap();

        assert!(raw_key.starts_with("sigiro_"));

        let ctx = verify_key(&conn, &raw_key).unwrap();
        assert_eq!(ctx.project_id, "proj_test");
        assert_eq!(ctx.plan, "free");
    }

    #[test]
    fn verify_invalid_key_fails() {
        let conn = test_admin_conn();
        create_key(&conn, "proj_test", "free", 100).unwrap();

        // Completely wrong key
        assert!(verify_key(&conn, "sigiro_AAAAAAAA_BBBBBBBBBBBBBBBBBBBBBBBB").is_err());
    }

    #[test]
    fn verify_nonexistent_short_token() {
        let conn = test_admin_conn();
        assert!(verify_key(&conn, "sigiro_ZZZZZZZZ_BBBBBBBBBBBBBBBBBBBBBBBB").is_err());
    }

    #[test]
    fn verify_malformed_key() {
        let conn = test_admin_conn();
        assert!(verify_key(&conn, "not-a-valid-key").is_err());
    }

    #[test]
    fn revoke_key_works() {
        let conn = test_admin_conn();
        let raw_key = create_key(&conn, "proj_test", "free", 100).unwrap();

        // Extract short token from key
        let pak = PrefixedApiKey::from_string(&raw_key).unwrap();
        let short = pak.short_token().to_string();

        assert!(revoke_key(&conn, &short).unwrap());
        assert!(verify_key(&conn, &raw_key).is_err());
    }

    #[test]
    fn revoke_nonexistent_returns_false() {
        let conn = test_admin_conn();
        assert!(!revoke_key(&conn, "nonexistent").unwrap());
    }

    #[test]
    fn multiple_keys_per_project() {
        let conn = test_admin_conn();
        let key1 = create_key(&conn, "proj_a", "free", 100).unwrap();
        let key2 = create_key(&conn, "proj_a", "builder", 500).unwrap();

        let ctx1 = verify_key(&conn, &key1).unwrap();
        let ctx2 = verify_key(&conn, &key2).unwrap();
        assert_eq!(ctx1.project_id, "proj_a");
        assert_eq!(ctx2.project_id, "proj_a");
        assert_eq!(ctx1.plan, "free");
        assert_eq!(ctx2.plan, "builder");
    }

    #[test]
    fn dev_key_prefix_recognized() {
        assert!("sigiro_dev_test".starts_with("sigiro_dev_"));
        assert!(!"sigiro_BRTRKFsL_xxx".starts_with("sigiro_dev_"));
    }
}
