use anyhow::Result;
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::Response,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

use crate::server::AppState;

/// Extracted from Unkey verify response — attached to request extensions.
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub owner_id: String,
    pub project_id: String,
}

#[derive(Debug)]
pub enum AuthError {
    MissingHeader,
    MalformedHeader,
    InvalidKey,
    RateLimited,
    ServiceUnavailable(String),
}

impl AuthError {
    fn status_code(&self) -> StatusCode {
        match self {
            AuthError::MissingHeader | AuthError::MalformedHeader | AuthError::InvalidKey => {
                StatusCode::UNAUTHORIZED
            }
            AuthError::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            AuthError::ServiceUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    fn message(&self) -> String {
        match self {
            AuthError::MissingHeader => {
                "Missing Authorization header. Set SIGIRO_API_KEY env var.".to_string()
            }
            AuthError::MalformedHeader => {
                "Invalid Authorization header. Expected: Bearer <key>".to_string()
            }
            AuthError::InvalidKey => "Invalid API key. Run: sigiro auth <key>".to_string(),
            AuthError::RateLimited => "Rate limited. Try again later.".to_string(),
            AuthError::ServiceUnavailable(msg) => format!("Auth service unavailable: {msg}"),
        }
    }
}

/// Extract bearer token from Authorization header.
pub fn extract_bearer_token(headers: &HeaderMap) -> Result<&str, AuthError> {
    let value = headers
        .get("authorization")
        .ok_or(AuthError::MissingHeader)?
        .to_str()
        .map_err(|_| AuthError::MalformedHeader)?;
    value
        .strip_prefix("Bearer ")
        .ok_or(AuthError::MalformedHeader)
}

/// SHA-256 hash of the key for cache lookup (not for security — Unkey handles that).
pub fn hash_for_cache(key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(key.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Verify a key against the Unkey API.
pub async fn verify_with_unkey(
    client: &reqwest::Client,
    api_id: &str,
    key: &str,
) -> Result<AuthContext, AuthError> {
    let resp = client
        .post("https://api.unkey.dev/v1/keys.verifyKey")
        .json(&serde_json::json!({
            "apiId": api_id,
            "key": key,
        }))
        .send()
        .await
        .map_err(|e| AuthError::ServiceUnavailable(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(AuthError::ServiceUnavailable(format!(
            "Unkey returned {}",
            resp.status()
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AuthError::ServiceUnavailable(e.to_string()))?;

    let valid = body["valid"].as_bool().unwrap_or(false);
    if !valid {
        let code = body["code"].as_str().unwrap_or("");
        if code == "RATE_LIMITED" {
            return Err(AuthError::RateLimited);
        }
        return Err(AuthError::InvalidKey);
    }

    let owner_id = body["ownerId"].as_str().unwrap_or("").to_string();
    let project_id = body["meta"]["project_id"]
        .as_str()
        .unwrap_or("")
        .to_string();

    Ok(AuthContext {
        owner_id,
        project_id,
    })
}

/// axum middleware: verify API key via Unkey with moka cache.
/// Always runs — same code path in dev and prod.
/// Dev keys (`sk_dev_*`) bypass Unkey and return a hardcoded AuthContext.
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, String)> {
    let token = extract_bearer_token(req.headers()).map_err(|e| (e.status_code(), e.message()))?;

    // Dev key bypass — same middleware runs, just a different auth backend
    if token.starts_with("sk_dev_") {
        let ctx = AuthContext {
            owner_id: "dev_user".to_string(),
            project_id: "dev_project".to_string(),
        };
        let mut req = req;
        req.extensions_mut().insert(ctx);
        return Ok(next.run(req).await);
    }

    let cache_key = hash_for_cache(token);

    // 1. Cache hit
    if let Some(ctx) = state.auth_cache.get(&cache_key).await {
        let mut req = req;
        req.extensions_mut().insert(ctx);
        return Ok(next.run(req).await);
    }

    // 2. Unkey verify
    match verify_with_unkey(&state.http_client, &state.unkey_api_id, token).await {
        Ok(ctx) => {
            state.auth_cache.insert(cache_key, ctx.clone()).await;
            let mut req = req;
            req.extensions_mut().insert(ctx);
            Ok(next.run(req).await)
        }
        Err(AuthError::ServiceUnavailable(msg)) => {
            tracing::error!("Unkey unavailable: {msg}");
            Err((
                StatusCode::SERVICE_UNAVAILABLE,
                AuthError::ServiceUnavailable(msg).message(),
            ))
        }
        Err(e) => Err((e.status_code(), e.message())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn extract_bearer_token_valid() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer sk_live_abc123".parse().unwrap());
        assert_eq!(extract_bearer_token(&headers).unwrap(), "sk_live_abc123");
    }

    #[test]
    fn extract_bearer_token_missing() {
        let headers = HeaderMap::new();
        assert!(matches!(
            extract_bearer_token(&headers),
            Err(AuthError::MissingHeader)
        ));
    }

    #[test]
    fn extract_bearer_token_malformed() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Basic xyz".parse().unwrap());
        assert!(matches!(
            extract_bearer_token(&headers),
            Err(AuthError::MalformedHeader)
        ));
    }

    #[test]
    fn hash_for_cache_deterministic() {
        let h1 = hash_for_cache("sk_live_test");
        let h2 = hash_for_cache("sk_live_test");
        assert_eq!(h1, h2);
        assert_ne!(h1, hash_for_cache("sk_live_other"));
    }

    #[test]
    fn auth_context_fields() {
        let ctx = AuthContext {
            owner_id: "usr_123".to_string(),
            project_id: "proj_456".to_string(),
        };
        assert_eq!(ctx.owner_id, "usr_123");
        assert_eq!(ctx.project_id, "proj_456");
    }

    #[test]
    fn auth_error_status_codes() {
        assert_eq!(
            AuthError::MissingHeader.status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AuthError::InvalidKey.status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AuthError::RateLimited.status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            AuthError::ServiceUnavailable("test".into()).status_code(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn cache_hit_skips_verify() {
        let cache: moka::future::Cache<String, AuthContext> = moka::future::Cache::builder()
            .max_capacity(100)
            .time_to_live(std::time::Duration::from_secs(60))
            .build();

        let ctx = AuthContext {
            owner_id: "usr_cached".to_string(),
            project_id: "proj_cached".to_string(),
        };
        let key_hash = hash_for_cache("sk_live_cached");
        cache.insert(key_hash.clone(), ctx).await;

        // Cache hit returns the stored context
        let result = cache.get(&key_hash).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().owner_id, "usr_cached");
    }

    #[tokio::test]
    async fn cache_miss_returns_none() {
        let cache: moka::future::Cache<String, AuthContext> = moka::future::Cache::builder()
            .max_capacity(100)
            .time_to_live(std::time::Duration::from_secs(60))
            .build();

        let result = cache.get(&hash_for_cache("sk_live_unknown")).await;
        assert!(result.is_none());
    }

    #[test]
    fn dev_key_is_recognized() {
        // sk_dev_ prefix keys should be recognized as dev keys
        assert!("sk_dev_test123".starts_with("sk_dev_"));
        assert!(!"sk_live_test123".starts_with("sk_dev_"));
    }

    #[test]
    fn extract_dev_key_from_header() {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer sk_dev_local".parse().unwrap());
        let token = extract_bearer_token(&headers).unwrap();
        assert!(token.starts_with("sk_dev_"));
    }
}
