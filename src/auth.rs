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

/// Explicit auth mode — never implicit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// No auth required. Any request is accepted.
    Open,
    /// Single shared API key. Requests must provide matching Bearer token.
    Key { key: String },
}

impl AuthMode {
    /// Build from the configured key string.
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

    /// True if this mode requires an Authorization header.
    pub fn requires_auth(&self) -> bool {
        matches!(self, AuthMode::Key { .. })
    }

    /// Verify a raw bearer token against this auth mode.
    /// Open mode: always succeeds.
    /// Key mode: token must match exactly.
    pub fn verify(&self, raw_key: &str) -> Result<AuthContext, ApiError> {
        match self {
            AuthMode::Open => Ok(AuthContext {
                project_id: "default".to_string(),
            }),
            AuthMode::Key { key } => {
                if raw_key == key {
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
        }
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
            req.extensions_mut().insert(AuthContext {
                project_id: "default".to_string(),
            });
            Ok(next.run(req).await)
        }
        AuthMode::Key { .. } => {
            let token = extract_bearer_token(req.headers())?;
            let ctx = state.auth_mode.verify(&token)?;
            let mut req = req;
            req.extensions_mut().insert(ctx);
            Ok(next.run(req).await)
        }
    }
}
