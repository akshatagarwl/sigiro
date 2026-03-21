use anyhow::{Context, Result};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use jsonwebtoken::{DecodingKey, Validation, decode};
use std::sync::Arc;

/// Vercel OIDC JWT claims.
#[derive(serde::Deserialize, Debug)]
pub struct VercelClaims {
    pub sub: String,
    pub iss: String,
    pub aud: Option<String>,
    pub exp: u64,
    pub iat: u64,
}

/// JWKS key entry from Vercel's .well-known endpoint.
#[derive(serde::Deserialize, Debug, Clone)]
pub struct JwksKey {
    pub kty: String,
    pub n: String,
    pub e: String,
    pub kid: String,
}

/// JWKS response.
#[derive(serde::Deserialize, Debug)]
pub struct JwksResponse {
    pub keys: Vec<JwksKey>,
}

/// Vercel Partner API configuration.
pub struct VercelConfig {
    pub jwks_cache: moka::future::Cache<String, Vec<JwksKey>>,
    pub partner_id: String,
}

impl VercelConfig {
    pub fn new(partner_id: &str) -> Self {
        Self {
            jwks_cache: moka::future::Cache::builder()
                .max_capacity(10)
                .time_to_live(std::time::Duration::from_secs(3600))
                .build(),
            partner_id: partner_id.to_string(),
        }
    }
}

/// Fetch JWKS from Vercel with caching.
pub async fn fetch_jwks(
    client: &reqwest::Client,
    cache: &moka::future::Cache<String, Vec<JwksKey>>,
) -> Result<Vec<JwksKey>> {
    let cache_key = "vercel_jwks".to_string();
    if let Some(keys) = cache.get(&cache_key).await {
        return Ok(keys);
    }
    let resp = client
        .get("https://marketplace.vercel.com/.well-known/jwks")
        .send()
        .await
        .context("failed to fetch Vercel JWKS")?;
    let jwks: JwksResponse = resp.json().await.context("invalid JWKS response")?;
    cache.insert(cache_key, jwks.keys.clone()).await;
    Ok(jwks.keys)
}

/// Verify a Vercel OIDC JWT.
pub fn verify_jwt(token: &str, key: &JwksKey, expected_issuer: &str) -> Result<VercelClaims> {
    let decoding_key =
        DecodingKey::from_rsa_components(&key.n, &key.e).context("invalid RSA key components")?;
    let mut validation = Validation::new(jsonwebtoken::Algorithm::RS256);
    validation.set_issuer(&[expected_issuer]);
    validation.validate_aud = false;
    let token_data = decode::<VercelClaims>(token, &decoding_key, &validation)
        .context("JWT verification failed")?;
    Ok(token_data.claims)
}

/// Extract JWT from Authorization header and verify against JWKS.
pub async fn verify_vercel_request(
    client: &reqwest::Client,
    config: &VercelConfig,
    headers: &HeaderMap,
) -> Result<VercelClaims, (StatusCode, String)> {
    let token = crate::auth::extract_bearer_token(headers)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "missing JWT".into()))?;

    let keys = fetch_jwks(client, &config.jwks_cache)
        .await
        .map_err(|e| (StatusCode::SERVICE_UNAVAILABLE, e.to_string()))?;

    // Try each key until one works (key rotation)
    for key in &keys {
        if let Ok(claims) = verify_jwt(token, key, "https://marketplace.vercel.com") {
            return Ok(claims);
        }
    }
    Err((StatusCode::UNAUTHORIZED, "invalid JWT".into()))
}

/// Vercel installation upsert request.
#[derive(serde::Deserialize, Debug)]
pub struct InstallationRequest {
    pub installation_id: String,
    pub team_id: Option<String>,
    pub user_id: Option<String>,
}

/// Vercel resource provisioning request.
#[derive(serde::Deserialize, Debug)]
pub struct ResourceRequest {
    pub installation_id: String,
    pub product_id: Option<String>,
}

/// Handle PUT /v1/installations/:id — create or update an installation.
pub async fn upsert_installation(
    State(state): State<Arc<crate::server::AppState>>,
    headers: HeaderMap,
    Json(req): Json<InstallationRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (client, vercel, saas) = get_vercel_deps(&state)?;
    let claims = verify_vercel_request(client, vercel, &headers).await?;

    let owner_id = req
        .team_id
        .as_deref()
        .or(req.user_id.as_deref())
        .unwrap_or(&claims.sub);

    // Lago customer (idempotent upsert)
    crate::provision::create_lago_customer(
        &saas.http_client,
        &saas.provision.lago_url,
        &saas.provision.lago_api_key,
        owner_id,
        None,
        None,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(
        "upserted Vercel installation: {} (owner: {})",
        req.installation_id,
        owner_id
    );

    Ok(Json(serde_json::json!({
        "status": "ok",
        "installation_id": req.installation_id,
        "owner_id": owner_id,
    })))
}

/// Handle POST /v1/resources — provision a resource and return secrets.
pub async fn create_resource(
    State(state): State<Arc<crate::server::AppState>>,
    headers: HeaderMap,
    Json(req): Json<ResourceRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (client, vercel, saas) = get_vercel_deps(&state)?;
    let claims = verify_vercel_request(client, vercel, &headers).await?;

    let owner_id = &claims.sub;

    // Create Unkey key for this resource
    let api_key = crate::provision::create_unkey_key(
        client,
        &saas.provision.unkey_root_key,
        &saas.provision.unkey_api_id,
        owner_id,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Create project DuckDB
    let data_dir =
        crate::data_dir().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    crate::provision::create_project_db(&data_dir, owner_id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(
        "provisioned Vercel resource for installation: {}",
        req.installation_id
    );

    Ok(Json(serde_json::json!({
        "status": "ok",
        "secrets": [
            { "name": "SIGIRO_API_KEY", "value": api_key },
            { "name": "SIGIRO_ENDPOINT", "value": "https://app.sigiro.com" },
        ],
    })))
}

/// Handle DELETE /v1/installations/:id.
pub async fn delete_installation(
    State(state): State<Arc<crate::server::AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (client, vercel, _saas) = get_vercel_deps(&state)?;
    let _claims = verify_vercel_request(client, vercel, &headers).await?;

    // In a full implementation, this would:
    // 1. Revoke Unkey keys for this installation
    // 2. Cancel Lago subscription
    // For now, just acknowledge the deletion
    tracing::info!("deleted Vercel installation");

    Ok(Json(serde_json::json!({ "status": "deleted" })))
}

fn get_vercel_deps(
    state: &crate::server::AppState,
) -> Result<(&reqwest::Client, &VercelConfig, &crate::server::SaasState), (StatusCode, String)> {
    let saas = state
        .saas
        .as_ref()
        .ok_or((StatusCode::NOT_FOUND, "SaaS mode not configured".into()))?;
    Ok((&saas.http_client, &saas.vercel, saas))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vercel_claims() {
        let json = r#"{
            "sub": "team_abc",
            "iss": "https://marketplace.vercel.com",
            "aud": "partner_123",
            "exp": 9999999999,
            "iat": 1000000000
        }"#;
        let claims: VercelClaims = serde_json::from_str(json).unwrap();
        assert_eq!(claims.sub, "team_abc");
        assert_eq!(claims.iss, "https://marketplace.vercel.com");
    }

    #[test]
    fn parse_jwks_response() {
        let json = r#"{
            "keys": [{
                "kty": "RSA",
                "n": "test_n",
                "e": "AQAB",
                "kid": "key-1"
            }]
        }"#;
        let jwks: JwksResponse = serde_json::from_str(json).unwrap();
        assert_eq!(jwks.keys.len(), 1);
        assert_eq!(jwks.keys[0].kid, "key-1");
    }

    #[test]
    fn parse_installation_request() {
        let json = r#"{
            "installation_id": "inst_123",
            "team_id": "team_abc"
        }"#;
        let req: InstallationRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.installation_id, "inst_123");
        assert_eq!(req.team_id.unwrap(), "team_abc");
    }

    #[test]
    fn parse_resource_request() {
        let json = r#"{
            "installation_id": "inst_123",
            "product_id": "sigiro-observability"
        }"#;
        let req: ResourceRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.installation_id, "inst_123");
        assert_eq!(req.product_id.unwrap(), "sigiro-observability");
    }

    #[test]
    fn vercel_config_creates_cache() {
        let config = VercelConfig::new("partner_test");
        assert_eq!(config.partner_id, "partner_test");
    }

    #[tokio::test]
    async fn jwks_cache_hit() {
        let cache: moka::future::Cache<String, Vec<JwksKey>> = moka::future::Cache::builder()
            .max_capacity(10)
            .time_to_live(std::time::Duration::from_secs(3600))
            .build();
        let key = JwksKey {
            kty: "RSA".into(),
            n: "test".into(),
            e: "AQAB".into(),
            kid: "cached-key".into(),
        };
        cache.insert("vercel_jwks".into(), vec![key]).await;
        let result = cache.get("vercel_jwks").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap()[0].kid, "cached-key");
    }

    #[test]
    fn verify_jwt_with_bad_key_fails() {
        let key = JwksKey {
            kty: "RSA".into(),
            n: "bad".into(),
            e: "AQAB".into(),
            kid: "bad-key".into(),
        };
        let result = verify_jwt("not.a.jwt", &key, "https://marketplace.vercel.com");
        assert!(result.is_err());
    }
}
