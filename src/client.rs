use anyhow::{Context, Result, bail};

use crate::investigate::InvestigationBlock;

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
        let resp = self
            .http
            .post(format!("{}/investigate", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&serde_json::json!({
                "service_name": service,
                "from_ts": from,
                "to_ts": to,
            }))
            .send()
            .await
            .context("failed to connect to sigiro server")?;

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
        let resp = self
            .http
            .post(format!("{}/query", self.base_url))
            .bearer_auth(&self.api_key)
            .body(sql.to_string())
            .send()
            .await
            .context("failed to connect to sigiro server")?;

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
        let resp = self
            .http
            .delete(format!("{}/admin/keys/{}", self.base_url, short_token))
            .send()
            .await
            .context("failed to connect to sigiro server")?;

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
        let resp = self
            .http
            .get(format!("{}/health", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

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
        let body = serde_json::json!({
            "service_name": service,
            "services": services,
            "from_ts": from,
            "to_ts": to,
        });
        let resp = self
            .http
            .post(format!("{}/investigate", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

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
        let resp = self
            .http
            .get(format!("{}/services", self.base_url))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

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

/// Build a client from config if API key is available.
pub fn try_build_client() -> Option<SigiroClient> {
    let api_key = crate::config::get_api_key().ok()?;
    let base_url =
        std::env::var("SIGIRO_ENDPOINT").unwrap_or_else(|_| "https://app.sigiro.com".to_string());
    Some(SigiroClient::new(&base_url, &api_key))
}
