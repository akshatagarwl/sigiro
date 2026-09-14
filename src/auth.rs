use crate::types::TenantId;
use anyhow::Context;
use axum::{
    extract::{Request, State},
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use axum_anyhow::ApiError;
use headers::{Authorization, HeaderMapExt, authorization::Bearer};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex as AsyncMutex, RwLock};

pub(crate) const TELEMETRY_INGEST_SCOPE: &str = "telemetry:ingest";
pub(crate) const QUERY_READ_SCOPE: &str = "query:read";
pub(crate) const DIAGNOSE_READ_SCOPE: &str = "diagnose:read";
pub(crate) const SERVICES_READ_SCOPE: &str = "services:read";
pub(crate) const TRACES_READ_SCOPE: &str = "traces:read";
pub(crate) const ANOMALIES_READ_SCOPE: &str = "anomalies:read";
pub(crate) const ALERTS_READ_SCOPE: &str = "alerts:read";

const JWKS_CACHE_TTL: Duration = Duration::from_secs(300);
const JWKS_FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const UNKNOWN_KID_REFRESH_COOLDOWN: Duration = Duration::from_secs(5);
const MAX_JWKS_BYTES: usize = 64 * 1024;
const MAX_JWKS_KEYS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthContext {
    pub(crate) tenant_id: TenantId,
    pub(crate) subject: String,
    pub(crate) scopes: HashSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AuthError {
    #[error("invalid access token")]
    InvalidToken,
    #[error("access token lacks required scope {0}")]
    InsufficientScope(&'static str),
    #[error("authorization service is unavailable")]
    JwksUnavailable,
}

impl AuthError {
    pub(crate) fn into_api_error(self) -> ApiError {
        match self {
            AuthError::InsufficientScope(scope) => axum_anyhow::forbidden(
                "Insufficient scope",
                &format!("The access token requires scope {scope}"),
            ),
            AuthError::InvalidToken | AuthError::JwksUnavailable => axum_anyhow::unauthorized(
                "Invalid access token",
                "The bearer access token could not be verified",
            ),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct AccessTokenClaims {
    sub: String,
    tid: String,
    scope: String,
}

#[derive(Debug)]
struct CachedJwks {
    keys: JwkSet,
    expires_at: Instant,
}

pub struct JwtVerifier {
    issuer: String,
    audience: String,
    jwks_url: String,
    client: reqwest::Client,
    cache: RwLock<Option<CachedJwks>>,
    refresh: AsyncMutex<()>,
    last_unknown_kid_refresh: RwLock<Option<(String, Instant)>>,
}

impl std::fmt::Debug for JwtVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtVerifier")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("jwks_url", &self.jwks_url)
            .finish_non_exhaustive()
    }
}

impl JwtVerifier {
    pub(crate) async fn new(
        issuer: String,
        audience: String,
        jwks_url: String,
    ) -> anyhow::Result<Self> {
        for (name, value) in [
            ("SIGIRO_AUTH_ISSUER", issuer.as_str()),
            ("SIGIRO_AUTH_AUDIENCE", audience.as_str()),
            ("SIGIRO_AUTH_JWKS_URL", jwks_url.as_str()),
        ] {
            anyhow::ensure!(!value.trim().is_empty(), "{name} must not be empty");
            validate_auth_url(name, value)?;
        }
        let verifier = Self {
            issuer,
            audience,
            jwks_url,
            client: reqwest::Client::builder()
                .timeout(JWKS_FETCH_TIMEOUT)
                .build()
                .context("failed to build JWKS HTTP client")?,
            cache: RwLock::new(None),
            refresh: AsyncMutex::new(()),
            last_unknown_kid_refresh: RwLock::new(None),
        };
        verifier
            .refresh_jwks(None)
            .await
            .map_err(|_| anyhow::anyhow!("failed to fetch Better Auth JWKS during startup"))?;
        Ok(verifier)
    }

    pub(crate) async fn verify(
        &self,
        token: &str,
        required_scope: &'static str,
    ) -> Result<AuthContext, AuthError> {
        let header = decode_header(token).map_err(|_| AuthError::InvalidToken)?;
        if header.alg != Algorithm::EdDSA {
            return Err(AuthError::InvalidToken);
        }
        let kid = header.kid.ok_or(AuthError::InvalidToken)?;
        let key = self.key_for(&kid).await?;
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&[&self.audience]);
        validation.validate_nbf = true;
        let claims = decode::<AccessTokenClaims>(token, &key, &validation)
            .map_err(|_| AuthError::InvalidToken)?
            .claims;
        let tenant_id = TenantId::try_new(claims.tid).map_err(|_| AuthError::InvalidToken)?;
        if claims.sub.trim().is_empty() {
            return Err(AuthError::InvalidToken);
        }
        let scopes = claims
            .scope
            .split_ascii_whitespace()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        if !scopes.contains(required_scope) {
            return Err(AuthError::InsufficientScope(required_scope));
        }
        Ok(AuthContext {
            tenant_id,
            subject: claims.sub,
            scopes,
        })
    }

    async fn key_for(&self, kid: &str) -> Result<DecodingKey, AuthError> {
        if let Some(key) = self.cached_key(kid).await? {
            return Ok(key);
        }
        self.refresh_jwks(Some(kid)).await?;
        self.cached_key(kid).await?.ok_or(AuthError::InvalidToken)
    }

    async fn cached_key(&self, kid: &str) -> Result<Option<DecodingKey>, AuthError> {
        let cache = self.cache.read().await;
        let Some(cache) = cache
            .as_ref()
            .filter(|cache| cache.expires_at > Instant::now())
        else {
            return Ok(None);
        };
        cache
            .keys
            .find(kid)
            .map(DecodingKey::from_jwk)
            .transpose()
            .map_err(|_| AuthError::InvalidToken)
    }

    async fn refresh_jwks(&self, unknown_kid: Option<&str>) -> Result<(), AuthError> {
        let _guard = self.refresh.lock().await;
        if let Some(unknown_kid) = unknown_kid
            && self
                .last_unknown_kid_refresh
                .read()
                .await
                .as_ref()
                .is_some_and(|(kid, last)| {
                    kid == unknown_kid && last.elapsed() < UNKNOWN_KID_REFRESH_COOLDOWN
                })
        {
            return Ok(());
        }
        if unknown_kid.is_none()
            && self
                .cache
                .read()
                .await
                .as_ref()
                .is_some_and(|cache| cache.expires_at > Instant::now())
        {
            return Ok(());
        }
        let mut response = self
            .client
            .get(&self.jwks_url)
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|_| AuthError::JwksUnavailable)?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_JWKS_BYTES as u64)
        {
            return Err(AuthError::JwksUnavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AuthError::JwksUnavailable)?
        {
            if body.len().saturating_add(chunk.len()) > MAX_JWKS_BYTES {
                return Err(AuthError::JwksUnavailable);
            }
            body.extend_from_slice(&chunk);
        }
        let keys: JwkSet = serde_json::from_slice(&body).map_err(|_| AuthError::JwksUnavailable)?;
        if keys.keys.is_empty() || keys.keys.len() > MAX_JWKS_KEYS {
            return Err(AuthError::JwksUnavailable);
        }
        *self.cache.write().await = Some(CachedJwks {
            keys,
            expires_at: Instant::now() + JWKS_CACHE_TTL,
        });
        if let Some(unknown_kid) = unknown_kid {
            *self.last_unknown_kid_refresh.write().await =
                Some((unknown_kid.to_owned(), Instant::now()));
        }
        Ok(())
    }
}

fn validate_auth_url(name: &str, value: &str) -> anyhow::Result<url::Url> {
    let parsed =
        url::Url::parse(value).map_err(|error| anyhow::anyhow!("invalid {name}: {error}"))?;
    let loopback = match parsed.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    anyhow::ensure!(
        parsed.scheme() == "https" || loopback,
        "{name} must use HTTPS unless it targets loopback"
    );
    Ok(parsed)
}

/// Explicit auth mode — Open for local self-host/edge, JWT for hosted storage.
#[derive(Clone, Debug)]
pub enum AuthMode {
    /// No auth required. Any request is accepted.
    Open,
    /// Hosted mode: Better Auth OAuth access tokens verified against its JWKS.
    Jwt { verifier: Arc<JwtVerifier> },
}

impl AuthMode {
    /// Build auth for the current storage mode.
    ///
    /// Local no-catalog and quack edge deployments remain Open. Distributed
    /// storage is hosted-only and must have a complete Better Auth resource
    /// server configuration; there is no legacy key fallback.
    pub async fn from_runtime_config(
        query_surface: crate::config::QuerySurface,
        storage_mode: &crate::storage::StorageMode,
    ) -> anyhow::Result<Self> {
        if query_surface == crate::config::QuerySurface::Quack {
            tracing::info!("auth_mode=open — quack edge, protected by the network perimeter");
            return Ok(AuthMode::Open);
        }
        if let Some((issuer, audience, jwks_url)) = crate::config::jwt_resource_server_config()? {
            let verifier = JwtVerifier::new(issuer, audience, jwks_url).await?;
            tracing::info!("auth_mode=jwt — Better Auth access tokens required");
            return Ok(AuthMode::Jwt {
                verifier: Arc::new(verifier),
            });
        }
        ensure_open_mode_allowed(storage_mode)?;
        tracing::warn!(
            "sigiro running in OPEN auth mode — no per-request authentication; protect the network perimeter"
        );
        Ok(AuthMode::Open)
    }

    /// True if this mode requires an Authorization header.
    pub fn requires_auth(&self) -> bool {
        !matches!(self, AuthMode::Open)
    }

    pub(crate) async fn authenticate_ingest(
        &self,
        token: Option<&str>,
    ) -> Result<TenantId, AuthError> {
        Ok(self
            .authenticate(token.unwrap_or_default(), TELEMETRY_INGEST_SCOPE)
            .await?
            .tenant_id)
    }

    pub(crate) async fn authenticate(
        &self,
        token: &str,
        required_scope: &'static str,
    ) -> Result<AuthContext, AuthError> {
        match self {
            AuthMode::Open => Ok(AuthContext {
                tenant_id: TenantId::try_new("default".to_owned())
                    .expect("default is a valid tenant id"),
                subject: "local".to_owned(),
                scopes: HashSet::new(),
            }),
            AuthMode::Jwt { verifier } => verifier.verify(token, required_scope).await,
        }
    }
}

fn ensure_open_mode_allowed(storage_mode: &crate::storage::StorageMode) -> anyhow::Result<()> {
    anyhow::ensure!(
        !storage_mode.is_distributed(),
        "distributed/PostgreSQL storage requires SIGIRO_AUTH_ISSUER, SIGIRO_AUTH_AUDIENCE, and SIGIRO_AUTH_JWKS_URL"
    );
    Ok(())
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

/// axum middleware: verify the configured authentication mode.
/// Open mode: injects default context without checking headers.
/// Admin-token gate for `/admin/*`, as a LAYER — the counterpart to
/// [`auth_middleware`] for `/v1/*`.
///
/// Applying the gate as a layer keeps authorization at the router boundary, so
/// adding an admin handler without an explicit check cannot expose it.
///
/// Admin auth remains a separate credential and middleware from hosted JWT auth.
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
            req.extensions_mut().insert(
                TenantId::try_new("default".to_owned()).expect("default is a valid tenant id"),
            );
            Ok(next.run(req).await)
        }
        AuthMode::Jwt { .. } => {
            let token = extract_bearer_token(req.headers())?;
            let required_scope = required_http_scope(req.method(), req.uri().path())?;
            let context = state
                .auth_mode
                .authenticate(&token, required_scope)
                .await
                .map_err(AuthError::into_api_error)?;
            let mut req = req;
            req.extensions_mut().insert(context.tenant_id.clone());
            req.extensions_mut().insert(context);
            Ok(next.run(req).await)
        }
    }
}

fn required_http_scope(method: &axum::http::Method, path: &str) -> Result<&'static str, ApiError> {
    use axum::http::Method;
    match (method, path) {
        (&Method::POST, "/v1/query" | "/query") => Ok(QUERY_READ_SCOPE),
        (&Method::POST, "/v1/diagnose" | "/diagnose") => Ok(DIAGNOSE_READ_SCOPE),
        (&Method::GET, "/v1/services" | "/services") => Ok(SERVICES_READ_SCOPE),
        (&Method::GET, "/v1/alerts" | "/alerts") => Ok(ALERTS_READ_SCOPE),
        (&Method::GET, "/v1/anomalies" | "/anomalies") => Ok(ANOMALIES_READ_SCOPE),
        (&Method::GET, path) if path.starts_with("/v1/traces/") || path.starts_with("/traces/") => {
            Ok(TRACES_READ_SCOPE)
        }
        _ => Err(axum_anyhow::forbidden(
            "No authorization policy",
            "The requested API route has no configured OAuth scope",
        )),
    }
}

#[cfg(test)]
mod jwt_tests {
    use super::*;
    use crate::storage::{DataLocation, StorageConfig, StorageMode};
    use axum::{Json, Router, extract::State, routing::get};
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde::Serialize;
    use std::time::{SystemTime, UNIX_EPOCH};

    const ISSUER: &str = "https://auth.sigiro.test/api/auth";
    const AUDIENCE: &str = "https://api.sigiro.test";
    const PUBLIC_KEY_X: &str = "Tn3DoRl__bp9FXiHIBEXzVMxEOXRVx1l-cD2mzDZMN4";

    #[test]
    fn distributed_storage_cannot_use_open_auth() {
        let storage = StorageMode::Distributed(
            StorageConfig::new(
                "postgres://localhost/sigiro".to_owned(),
                DataLocation::LocalFilesystem,
            )
            .unwrap(),
        );

        let error = ensure_open_mode_allowed(&storage).expect_err("distributed auth must fail");
        assert!(error.to_string().contains("requires SIGIRO_AUTH_ISSUER"));
    }

    #[derive(Serialize)]
    struct TestClaims<'a> {
        iss: &'a str,
        aud: &'a str,
        exp: usize,
        nbf: usize,
        sub: &'a str,
        tid: &'a str,
        scope: &'a str,
    }

    fn jwks(kid: &str) -> JwkSet {
        serde_json::from_value(serde_json::json!({
            "keys": [{
                "kty": "OKP",
                "crv": "Ed25519",
                "x": PUBLIC_KEY_X,
                "alg": "EdDSA",
                "use": "sig",
                "kid": kid
            }]
        }))
        .unwrap()
    }

    fn token(kid: &str, audience: &str, scope: &str) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as usize;
        let claims = TestClaims {
            iss: ISSUER,
            aud: audience,
            exp: now + 300,
            nbf: now.saturating_sub(1),
            sub: "user-1",
            tid: "tenant_a",
            scope,
        };
        let mut header = Header::new(Algorithm::EdDSA);
        header.kid = Some(kid.to_owned());
        encode(
            &header,
            &claims,
            &EncodingKey::from_ed_der(include_bytes!("../tests/fixtures/auth_ed25519_private.der")),
        )
        .unwrap()
    }

    async fn serve_jwks(State(keys): State<Arc<RwLock<JwkSet>>>) -> Json<JwkSet> {
        Json(keys.read().await.clone())
    }

    async fn verifier_result(keys: Arc<RwLock<JwkSet>>) -> anyhow::Result<JwtVerifier> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new()
                    .route("/jwks", get(serve_jwks))
                    .with_state(keys),
            )
            .await
            .unwrap();
        });
        JwtVerifier::new(
            ISSUER.to_owned(),
            AUDIENCE.to_owned(),
            format!("http://{address}/jwks"),
        )
        .await
    }

    async fn verifier(keys: Arc<RwLock<JwkSet>>) -> JwtVerifier {
        verifier_result(keys).await.unwrap()
    }

    #[tokio::test]
    async fn verifies_better_auth_claims_and_required_scope() {
        let verifier = verifier(Arc::new(RwLock::new(jwks("key-1")))).await;
        let context = verifier
            .verify(
                &token("key-1", AUDIENCE, QUERY_READ_SCOPE),
                QUERY_READ_SCOPE,
            )
            .await
            .unwrap();
        assert_eq!(context.tenant_id.as_ref(), "tenant_a");
        assert_eq!(context.subject, "user-1");

        let error = verifier
            .verify(
                &token("key-1", AUDIENCE, QUERY_READ_SCOPE),
                DIAGNOSE_READ_SCOPE,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AuthError::InsufficientScope(DIAGNOSE_READ_SCOPE)
        ));
    }

    #[tokio::test]
    async fn rejects_wrong_audience() {
        let verifier = verifier(Arc::new(RwLock::new(jwks("key-1")))).await;
        let error = verifier
            .verify(
                &token("key-1", "https://other.example", QUERY_READ_SCOPE),
                QUERY_READ_SCOPE,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, AuthError::InvalidToken));
    }

    #[tokio::test]
    async fn refreshes_jwks_once_for_an_unknown_kid() {
        let keys = Arc::new(RwLock::new(jwks("key-1")));
        let verifier = verifier(Arc::clone(&keys)).await;
        *keys.write().await = jwks("key-2");

        let context = verifier
            .verify(
                &token("key-2", AUDIENCE, QUERY_READ_SCOPE),
                QUERY_READ_SCOPE,
            )
            .await
            .unwrap();
        assert_eq!(context.tenant_id.as_ref(), "tenant_a");
    }

    #[tokio::test]
    async fn bogus_unknown_kid_does_not_block_signing_key_rotation() {
        let keys = Arc::new(RwLock::new(jwks("key-1")));
        let verifier = verifier(Arc::clone(&keys)).await;

        let error = verifier
            .verify(
                &token("bogus", AUDIENCE, QUERY_READ_SCOPE),
                QUERY_READ_SCOPE,
            )
            .await
            .unwrap_err();
        assert!(matches!(error, AuthError::InvalidToken));

        *keys.write().await = jwks("key-2");
        let context = verifier
            .verify(
                &token("key-2", AUDIENCE, QUERY_READ_SCOPE),
                QUERY_READ_SCOPE,
            )
            .await
            .unwrap();
        assert_eq!(context.tenant_id.as_ref(), "tenant_a");
    }

    #[test]
    fn rejects_plaintext_auth_urls_except_loopback() {
        assert!(validate_auth_url("test", "http://auth.example.com/jwks").is_err());
        assert!(validate_auth_url("test", "http://127.0.0.1:3100/jwks").is_ok());
        assert!(validate_auth_url("test", "http://[::1]:3100/jwks").is_ok());
        assert!(validate_auth_url("test", "https://auth.example.com/jwks").is_ok());
    }

    #[tokio::test]
    async fn rejects_jwks_with_too_many_keys() {
        let key = jwks("key-1").keys.into_iter().next().unwrap();
        let keys = JwkSet {
            keys: std::iter::repeat_n(key, MAX_JWKS_KEYS + 1).collect(),
        };
        assert!(verifier_result(Arc::new(RwLock::new(keys))).await.is_err());
    }
}
