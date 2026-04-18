use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::investigate::InvestigationBlock;

// --- Shared HTTP response handler ---

/// Handle a sigiro HTTP response, checking for auth errors and rate limits.
/// Returns the parsed JSON body on 200; otherwise bails with a descriptive message.
///
/// # Arguments
/// * `resp` - The raw HTTP response
/// * `success_msg` - Context message for successful parse
/// * `not_found_msg` - Message to return (as false) on 404 (optional)
///
/// # Type Parameters
/// * `T` - The target type to deserialize from JSON
fn handle_http_response<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
    success_msg: &str,
    not_found_msg: Option<&str>,
) -> Result<std::result::Result<T, bool>> {
    match resp.status().as_u16() {
        200 => resp
            .json()
            .await
            .context(success_msg)
            .map(Ok),
        401 => bail!("Invalid API key. Run: sigiro auth <key>"),
        429 => bail!("Rate limited. Try again later."),
        404 if not_found_msg.is_some() => Ok(Err(false)),
        s => {
            let body = resp.text().await.context("failed to read error body")?;
            bail!("Server error ({s}): {body}")
        }
    }
}

/// Map a Result<T, bool> to Result<T>, converting false to "not found".
fn not_found_to_err<T>(result: std::result::Result<T, bool>, msg: &str) -> Result<T> {
    match result {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(false)) if !msg.is_empty() => bail!("{msg}"),
        Ok(Err(false)) => Ok(T::default()),
        Err(e) => Err(e),
    }
}

/// HTTP client for hosted mode — CLI and MCP go through sigiro serve instead of direct DuckDB.
#[derive(Clone)]
pub struct SigiroClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
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

    pub async fn investigate(
        &self,
        service: &str,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<InvestigationBlock> {
        let blocks = self
            .investigate_multi(Some(service), None, from, to)
            .await?;
        blocks
            .into_values()
            .next()
            .context("no investigation block returned for service")
    }

    pub async fn query(&self, sql: &str) -> Result<Vec<serde_json::Value>> {
        let resp = self
            .http
            .post(format!("{}/v1/query", self.base_url))
            .bearer_auth(&self.api_key)
            .body(sql.to_string())
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response(resp, "failed to parse query response", None)?;
        not_found_to_err(result, "")
    }

    /// Create an API key via the running server's admin endpoint.
    pub async fn admin_create_key(
        &self,
        project: &str,
        plan: &str,
        rate_limit: u32,
    ) -> Result<String> {
        let resp = self
            .http
            .post(format!("{}/admin/keys", self.base_url))
            .json(&serde_json::json!({
                "project": project,
                "plan": plan,
                "rate_limit": rate_limit,
            }))
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        #[derive(Deserialize)]
        struct CreateKeyResponse {
            key: String,
        }
        let result = handle_http_response::<CreateKeyResponse>(resp, "failed to parse response", None)?;
        match result {
            Ok(Ok(body)) => Ok(body.key),
            Ok(Err(false)) => bail!("unexpected false"),
            Err(e) => Err(e),
        }
    }

    /// Revoke an API key via the running server's admin endpoint.
    pub async fn admin_revoke_key(&self, short_token: &str) -> Result<bool> {
        let resp = self
            .http
            .delete(format!("{}/admin/keys/{}", self.base_url, short_token))
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response::<serde::Value>(resp, "failed to parse response", Some(""))?;
        not_found_to_err(result, "")
    }

    pub async fn status(&self) -> Result<serde_json::Value> {
        let resp = self
            .http
            .get(format!("{}/health", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response::<serde::Value>(resp, "failed to parse status response", None)?;
        match result {
            Ok(Ok(body)) => {
                let body_str = body.as_str().unwrap_or("").to_string();
                Ok(serde_json::json!({ "status": body_str }))
            }
            Ok(Err(false)) => Ok(serde_json::Value::Null),
            Err(e) => Err(e),
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
        let body = serde_json::json!({
            "service": service,
            "services": services,
            "from_ts": from,
            "to_ts": to,
        });
        let resp = self
            .http
            .post(format!("{}/v1/investigate", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response(resp, "failed to parse investigation response", None)?;
        not_found_to_err(result, "")
    }

    /// List services with first/last seen and span counts.
    pub async fn services(&self) -> Result<serde_json::Value> {
        let resp = self
            .http
            .get(format!("{}/v1/services", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response(resp, "failed to parse services response", None)?;
        not_found_to_err(result, "")
    }

    /// Register a federation worker with the server.
    pub async fn register_worker(&self, worker: &crate::federation::WorkerInfo) -> Result<()> {
        let resp = self
            .http
            .post(format!("{}/v1/federation/workers", self.base_url))
            .bearer_auth(&self.api_key)
            .json(worker)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response::<serde::Value>(resp, "failed to parse response", None)?;
        match result {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(false)) => bail!("unexpected false"),
            Err(e) => Err(e),
        }
    }

    /// Deregister a federation worker from the server.
    /// Returns `true` if the worker was found and removed, `false` if 404.
    pub async fn deregister_worker(&self, id: &str) -> Result<bool> {
        let resp = self
            .http
            .delete(format!("{}/v1/federation/workers/{}", self.base_url, id))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response::<serde::Value>(resp, "failed to parse response", Some(""))?;
        not_found_to_err(result, "")
    }

    /// List all registered federation workers.
    pub async fn list_workers(&self) -> Result<Vec<crate::federation::WorkerInfo>> {
        #[derive(Deserialize)]
        struct ListWorkersResponse {
            workers: Vec<crate::federation::WorkerInfo>,
        }

        let resp = self
            .http
            .get(format!("{}/v1/federation/workers", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let result = handle_http_response::<ListWorkersResponse>(resp, "failed to parse list_workers response", None)?;
        match result {
            Ok(Ok(body)) => Ok(body.workers),
            Ok(Err(false)) => bail!("unexpected false"),
            Err(e) => Err(e),
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
