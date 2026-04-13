use anyhow::{Context, Result, bail};
use backon::{ExponentialBuilder, Retryable};

use crate::investigate::InvestigationBlock;

/// HTTP client for hosted mode — CLI and MCP go through sigiro serve instead of direct DuckDB.
#[derive(Clone)]
pub struct SigiroClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

/// Retry policy: exponential backoff with jitter.
/// 3 attempts: ~200ms, ~400ms, ~800ms (with ±25% jitter).
fn retry_policy() -> ExponentialBuilder {
    ExponentialBuilder::default()
        .with_min_delay(std::time::Duration::from_millis(200))
        .with_max_times(3)
        .with_jitter()
}

/// Check if an error is transient and worth retrying.
fn is_transient(err: &anyhow::Error) -> bool {
    if let Some(reqwest_err) = err.downcast_ref::<reqwest::Error>() {
        return reqwest_err.is_connect() || reqwest_err.is_timeout();
    }
    false
}

impl SigiroClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Send an HTTP request with retry on transient connection errors.
    /// Each retry rebuilds the request from method + url + body.
    /// Status code handling (401/429/5xx) is done by callers.
    async fn send_retryable(
        http: &reqwest::Client,
        method: reqwest::Method,
        url: &str,
        bearer: Option<&str>,
        body: Option<String>,
    ) -> Result<reqwest::Response> {
        let http = http.clone();
        let url = url.to_string();
        let method = method.to_string(); // Convert to String so closure can FnMut
        let bearer = bearer.map(str::to_string);

        let attempt = || {
            let http = http.clone();
            let url = url.clone();
            let method = method.clone();
            let bearer = bearer.clone();
            let body = body.clone();
            async move {
                let mut req = http.request(method.parse().expect("valid HTTP method"), &url);
                if let Some(ref token) = bearer {
                    req = req.bearer_auth(token);
                }
                if let Some(ref b) = body {
                    req = req.body(b.clone());
                }
                req.send()
                    .await
                    .context("failed to connect to sigiro server")
            }
        };

        attempt.retry(retry_policy()).when(is_transient).await
    }

    pub async fn investigate(
        &self,
        service: &str,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<InvestigationBlock> {
        let url = format!("{}/investigate", self.base_url);
        let body = serde_json::json!({
            "service_name": service,
            "from_ts": from,
            "to_ts": to,
        });

        let resp = Self::send_retryable(
            &self.http,
            reqwest::Method::POST,
            &url,
            Some(&self.api_key),
            Some(body.to_string()),
        )
        .await?;

        match resp.status().as_u16() {
            200 => resp
                .json()
                .await
                .context("failed to parse investigation response"),
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            429 => bail!("Rate limited. Try again later."),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    pub async fn query(&self, sql: &str) -> Result<Vec<serde_json::Value>> {
        let url = format!("{}/query", self.base_url);

        let resp = Self::send_retryable(
            &self.http,
            reqwest::Method::POST,
            &url,
            Some(&self.api_key),
            Some(sql.to_string()),
        )
        .await?;

        match resp.status().as_u16() {
            200 => resp.json().await.context("failed to parse query response"),
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            429 => bail!("Rate limited. Try again later."),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    /// Create an API key via the running server's admin endpoint.
    pub async fn admin_create_key(
        &self,
        project: &str,
        plan: &str,
        rate_limit: u32,
    ) -> Result<String> {
        let url = format!("{}/admin/keys", self.base_url);
        let body = serde_json::json!({
            "project": project,
            "plan": plan,
            "rate_limit": rate_limit,
        });

        let resp = Self::send_retryable(
            &self.http,
            reqwest::Method::POST,
            &url,
            None,
            Some(body.to_string()),
        )
        .await?;

        match resp.status().as_u16() {
            200 => {
                let body: serde_json::Value =
                    resp.json().await.context("failed to parse response")?;
                body["key"]
                    .as_str()
                    .map(String::from)
                    .context("response missing 'key' field")
            }
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    /// Revoke an API key via the running server's admin endpoint.
    pub async fn admin_revoke_key(&self, short_token: &str) -> Result<bool> {
        let url = format!("{}/admin/keys/{}", self.base_url, short_token);

        let resp =
            Self::send_retryable(&self.http, reqwest::Method::DELETE, &url, None, None).await?;

        match resp.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    pub async fn status(&self) -> Result<serde_json::Value> {
        let url = format!("{}/health", self.base_url);

        let resp = Self::send_retryable(
            &self.http,
            reqwest::Method::GET,
            &url,
            Some(&self.api_key),
            None,
        )
        .await?;

        match resp.status().as_u16() {
            200 => {
                let body = resp.text().await.context("failed to read error body")?;
                Ok(serde_json::json!({ "status": body }))
            }
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    /// Investigate one or more services. Pass `service` for single, `services` for multi.
    pub async fn investigate_multi(
        &self,
        service: Option<&str>,
        services: Option<Vec<String>>,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<std::collections::BTreeMap<String, InvestigationBlock>> {
        let url = format!("{}/investigate", self.base_url);
        let body = serde_json::json!({
            "service_name": service,
            "services": services,
            "from_ts": from,
            "to_ts": to,
        });

        let resp = Self::send_retryable(
            &self.http,
            reqwest::Method::POST,
            &url,
            Some(&self.api_key),
            Some(body.to_string()),
        )
        .await?;

        match resp.status().as_u16() {
            200 => resp
                .json()
                .await
                .context("failed to parse investigation response"),
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            429 => bail!("Rate limited. Try again later."),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    /// List services with first/last seen and span counts.
    pub async fn services(&self) -> Result<serde_json::Value> {
        let url = format!("{}/services", self.base_url);

        let resp = Self::send_retryable(
            &self.http,
            reqwest::Method::GET,
            &url,
            Some(&self.api_key),
            None,
        )
        .await?;

        match resp.status().as_u16() {
            200 => resp
                .json()
                .await
                .context("failed to parse services response"),
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }
}

/// Build a client from config. Fails if API key or endpoint is not configured.
pub fn try_build_client() -> anyhow::Result<SigiroClient> {
    let api_key = crate::config::get_api_key()
        .map_err(|_| anyhow::anyhow!("No API key configured. Run: sigiro auth <key>"))?;
    let base_url = std::env::var("SIGIRO_ENDPOINT")
        .map_err(|_| anyhow::anyhow!("SIGIRO_ENDPOINT not set. Export SIGIRO_ENDPOINT=http://localhost:9999 (or your server URL)"))?;
    Ok(SigiroClient::new(&base_url, &api_key))
}
