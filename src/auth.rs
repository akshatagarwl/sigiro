use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use axum_anyhow::ApiError;
use headers::{Authorization, HeaderMapExt, authorization::Bearer};
use std::sync::Arc;

/// Auth context extracted from the API key — attached to request extensions.
/// Self-hosted: all requests route to the default project.
#[derive(Clone, Debug)]
pub struct AuthContext {
    pub project_id: String,
}

/// Verify an API key against the configured key.
/// Self-hosted: single API key set via SIGIRO_API_KEY env var.
/// Empty configured_key means open mode (no auth required).
pub fn verify_key(configured_key: &str, raw_key: &str) -> Result<AuthContext, ApiError> {
    if configured_key.is_empty() {
        return Ok(AuthContext {
            project_id: "default".to_string(),
        });
    }
    if raw_key == configured_key {
        Ok(AuthContext {
            project_id: "default".to_string(),
        })
    } else {
        Err(axum_anyhow::unauthorized(
            "Invalid API Key",
            "The provided API key does not match SIGIRO_API_KEY",
        ))
    }
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

/// axum middleware: verify API key against the configured key.
/// Self-hosted: simple constant-time comparison. No DB needed.
/// Open mode (empty configured_key): allow requests without Authorization header.
pub async fn auth_middleware(
    State(state): State<Arc<crate::server::AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    // Open mode: no auth required, but still inject default context
    if state.configured_key.is_empty() {
        let mut req = req;
        req.extensions_mut().insert(AuthContext {
            project_id: "default".to_string(),
        });
        return Ok(next.run(req).await);
    }

    let token = extract_bearer_token(req.headers())?;
    let ctx = verify_key(&state.configured_key, &token)?;
    let mut req = req;
    req.extensions_mut().insert(ctx);
    Ok(next.run(req).await)
}
