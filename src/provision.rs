use anyhow::{Context, Result, bail};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

type HmacSha256 = Hmac<Sha256>;

/// Verify Logto webhook HMAC-SHA256 signature.
pub fn verify_logto_hmac(body: &[u8], signature: &str, secret: &str) -> Result<()> {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).context("invalid HMAC secret length")?;
    mac.update(body);
    let expected = hex::decode(signature).context("invalid hex in signature")?;
    mac.verify_slice(&expected)
        .map_err(|_| anyhow::anyhow!("HMAC signature mismatch"))
}

/// In-memory idempotency set for processed webhook event IDs.
/// Bounded: evicts oldest when capacity is reached.
pub struct IdempotencySet {
    inner: StdMutex<HashSet<String>>,
    capacity: usize,
}

impl IdempotencySet {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: StdMutex::new(HashSet::with_capacity(capacity)),
            capacity,
        }
    }

    /// Returns true if the event_id was already seen (duplicate).
    /// Inserts it if new.
    pub fn check_and_insert(&self, event_id: &str) -> bool {
        let mut set = self.inner.lock().expect("idempotency set lock poisoned");
        if set.contains(event_id) {
            return true;
        }
        if set.len() >= self.capacity {
            // Simple eviction: clear all when full (acceptable for webhook dedup)
            set.clear();
        }
        set.insert(event_id.to_string());
        false
    }
}

/// Logto webhook payload (subset of fields we care about).
#[derive(serde::Deserialize, Debug)]
pub struct LogtoWebhookPayload {
    pub event: String,
    #[serde(rename = "hookId")]
    pub hook_id: String,
    pub data: Option<LogtoUserData>,
}

#[derive(serde::Deserialize, Debug)]
pub struct LogtoUserData {
    pub id: String,
    pub username: Option<String>,
    #[serde(rename = "primaryEmail")]
    pub primary_email: Option<String>,
}

/// Provisioning config for external services.
pub struct ProvisionConfig {
    pub logto_webhook_secret: String,
    pub lago_url: String,
    pub lago_api_key: String,
    pub unkey_root_key: String,
    pub unkey_api_id: String,
    pub idempotency: IdempotencySet,
}

/// Create a Lago customer via their API.
pub async fn create_lago_customer(
    client: &reqwest::Client,
    lago_url: &str,
    lago_api_key: &str,
    user_id: &str,
    email: Option<&str>,
    name: Option<&str>,
) -> Result<()> {
    let body = serde_json::json!({
        "customer": {
            "external_id": user_id,
            "email": email.unwrap_or(""),
            "name": name.unwrap_or(""),
        }
    });
    let resp = client
        .post(format!("{lago_url}/api/v1/customers"))
        .bearer_auth(lago_api_key)
        .json(&body)
        .send()
        .await
        .context("failed to reach Lago")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("Lago customer creation failed: {status} — {text}");
    }
    tracing::info!("created Lago customer: {user_id}");
    Ok(())
}

/// Create an Unkey API key for a user.
pub async fn create_unkey_key(
    client: &reqwest::Client,
    root_key: &str,
    api_id: &str,
    owner_id: &str,
) -> Result<String> {
    let body = serde_json::json!({
        "apiId": api_id,
        "ownerId": owner_id,
        "meta": {
            "project_id": owner_id,
        },
        "prefix": "sk_live",
    });
    let resp = client
        .post("https://api.unkey.dev/v1/keys.createKey")
        .header("Authorization", format!("Bearer {root_key}"))
        .json(&body)
        .send()
        .await
        .context("failed to reach Unkey")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        bail!("Unkey key creation failed: {status} — {text}");
    }
    let result: serde_json::Value = resp.json().await.context("invalid Unkey response")?;
    result["key"]
        .as_str()
        .map(String::from)
        .context("no key in Unkey response")
}

/// Create a DuckDB file for a new user project.
pub fn create_project_db(data_dir: &std::path::Path, user_id: &str) -> Result<()> {
    let project_dir = data_dir.join(user_id);
    std::fs::create_dir_all(&project_dir)
        .with_context(|| format!("failed to create project dir: {}", project_dir.display()))?;
    let db_path = project_dir.join("sigiro.duckdb");
    let conn = duckdb::Connection::open(&db_path)
        .with_context(|| format!("failed to open DuckDB at {}", db_path.display()))?;
    crate::schema::create_tables(&conn, "")?;
    tracing::info!("created project DB: {}", db_path.display());
    Ok(())
}

/// Webhook handler for Logto signup events.
pub async fn logto_webhook_handler(
    State(state): State<Arc<crate::server::AppState>>,
    headers: HeaderMap,
    body: String,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let provision = &state.provision;

    // Verify HMAC signature
    let signature = headers
        .get("logto-signature-sha-256")
        .and_then(|v| v.to_str().ok())
        .ok_or((StatusCode::UNAUTHORIZED, "missing signature".into()))?;

    verify_logto_hmac(body.as_bytes(), signature, &provision.logto_webhook_secret)
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    // Parse payload
    let payload: LogtoWebhookPayload = serde_json::from_str(&body)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid payload: {e}")))?;

    // Idempotency check
    if provision.idempotency.check_and_insert(&payload.hook_id) {
        return Ok(Json(serde_json::json!({ "status": "duplicate" })));
    }

    // Only handle user creation events
    if payload.event != "PostRegister" && payload.event != "User.Created" {
        return Ok(Json(serde_json::json!({ "status": "ignored" })));
    }

    let user = payload
        .data
        .ok_or((StatusCode::BAD_REQUEST, "missing user data".into()))?;

    let client = &state.http_client;

    // 1. Create Lago customer
    create_lago_customer(
        client,
        &provision.lago_url,
        &provision.lago_api_key,
        &user.id,
        user.primary_email.as_deref(),
        user.username.as_deref(),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 2. Create Unkey key
    let key = create_unkey_key(
        client,
        &provision.unkey_root_key,
        &provision.unkey_api_id,
        &user.id,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // 3. Create DuckDB project
    let data_dir =
        crate::data_dir().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    create_project_db(&data_dir, &user.id)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!("provisioned user: {} (key: sk_live_...)", user.id);

    Ok(Json(serde_json::json!({
        "status": "provisioned",
        "user_id": user.id,
        "key_prefix": &key[..std::cmp::min(12, key.len())],
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_logto_hmac_valid() {
        let secret = "test-secret-key";
        let body = b"test body content";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let signature = hex::encode(mac.finalize().into_bytes());
        assert!(verify_logto_hmac(body, &signature, secret).is_ok());
    }

    #[test]
    fn verify_logto_hmac_invalid() {
        assert!(verify_logto_hmac(b"body", "deadbeef", "secret").is_err());
    }

    #[test]
    fn idempotency_duplicate_event() {
        let set = IdempotencySet::new(100);
        assert!(!set.check_and_insert("evt_1")); // first time: not duplicate
        assert!(set.check_and_insert("evt_1")); // second time: duplicate
    }

    #[test]
    fn idempotency_capacity_eviction() {
        let set = IdempotencySet::new(2);
        set.check_and_insert("a");
        set.check_and_insert("b");
        // At capacity, next insert clears all
        assert!(!set.check_and_insert("c"));
        // "a" was evicted
        assert!(!set.check_and_insert("a"));
    }

    #[test]
    fn parse_logto_webhook_payload() {
        let json = r#"{
            "event": "PostRegister",
            "hookId": "hook_123",
            "data": {
                "id": "usr_abc",
                "username": "alice",
                "primaryEmail": "alice@example.com"
            }
        }"#;
        let payload: LogtoWebhookPayload = serde_json::from_str(json).unwrap();
        assert_eq!(payload.event, "PostRegister");
        assert_eq!(payload.hook_id, "hook_123");
        let data = payload.data.unwrap();
        assert_eq!(data.id, "usr_abc");
        assert_eq!(data.primary_email.unwrap(), "alice@example.com");
    }

    #[test]
    fn create_project_db_creates_schema() {
        let dir = tempfile::tempdir().unwrap();
        create_project_db(dir.path(), "test_user").unwrap();
        let db_path = dir.path().join("test_user").join("sigiro.duckdb");
        assert!(db_path.exists());
        // Verify schema was created
        let conn = duckdb::Connection::open(&db_path).unwrap();
        let count: i64 = conn
            .query_row("SELECT count(*) FROM sigiro_spans", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn hmac_with_empty_body() {
        let secret = "secret";
        let body = b"";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());
        assert!(verify_logto_hmac(body, &sig, secret).is_ok());
    }
}
