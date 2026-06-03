use anyhow::{Context, Result, bail};
use serde::Deserialize;

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
        from: i64,
        to: i64,
    ) -> Result<InvestigationBlock> {
        let blocks = self
            .investigate_multi(Some(service), None, from, to)
            .await?;
        blocks
            .into_values()
            .next()
            .context("no investigation block returned for service")
    }

    async fn expect_ok(&self, resp: reqwest::Response) -> Result<reqwest::Response> {
        match resp.status().as_u16() {
            200 => Ok(resp),
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            429 => bail!("Rate limited. Try again later."),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    pub async fn query(&self, sql: &str) -> Result<Vec<serde_json::Value>> {
        let mut req = self
            .http
            .post(format!("{}/v1/query", self.base_url))
            .body(sql.to_string());
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        resp.json().await.context("failed to parse query response")
    }

    pub async fn status(&self) -> Result<crate::responses::AdminStatusResponse> {
        let mut req = self.http.get(format!("{}/health", self.base_url));
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        match resp.status().as_u16() {
            200 => {
                let body = resp.text().await.context("failed to read error body")?;
                Ok(crate::responses::AdminStatusResponse {
                    status: std::collections::HashMap::from([("status".to_string(), body)]),
                })
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
        from: i64,
        to: i64,
    ) -> Result<std::collections::BTreeMap<String, InvestigationBlock>> {
        let body = serde_json::json!({
            "service": service,
            "services": services,
            "from_ts": from,
            "to_ts": to,
        });
        let mut req = self
            .http
            .post(format!("{}/v1/investigate", self.base_url))
            .json(&body);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        resp.json()
            .await
            .context("failed to parse investigation response")
    }

    /// List services with first/last seen and span counts.
    pub async fn services(&self) -> Result<crate::responses::ServicesResponse> {
        let mut req = self.http.get(format!("{}/v1/services", self.base_url));
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        resp.json()
            .await
            .context("failed to parse services response")
    }

    /// Register a federation worker with the server.
    pub async fn register_worker(&self, worker: &crate::federation::WorkerInfo) -> Result<()> {
        let mut req = self
            .http
            .post(format!("{}/v1/federation/workers", self.base_url))
            .json(worker);
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        // Discard body — registration success is implicit from 200 OK.
        let _ = resp.bytes().await.ok();
        Ok(())
    }

    /// Deregister a federation worker from the server.
    /// Returns `true` if the worker was found and removed, `false` if 404.
    pub async fn deregister_worker(&self, id: &str) -> Result<bool> {
        let mut req = self
            .http
            .delete(format!("{}/v1/federation/workers/{}", self.base_url, id));
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        match resp.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    /// List all registered federation workers.
    pub async fn list_workers(&self) -> Result<Vec<crate::federation::WorkerInfo>> {
        #[derive(Deserialize)]
        struct ListWorkersResponse {
            workers: Vec<crate::federation::WorkerInfo>,
        }

        let mut req = self
            .http
            .get(format!("{}/v1/federation/workers", self.base_url));
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        match resp.status().as_u16() {
            200 => {
                let body: ListWorkersResponse = resp
                    .json()
                    .await
                    .context("failed to parse list_workers response")?;
                Ok(body.workers)
            }
            401 => bail!("Invalid API key. Run: sigiro auth <key>"),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }
}

/// Build a client from config. Returns empty API key if none is configured
/// (works against servers running in open mode).
pub fn try_build_client() -> anyhow::Result<SigiroClient> {
    let api_key = crate::config::get_api_key().unwrap_or_default();
    let base_url = crate::config::ENDPOINT
        .value()
        .map_err(anyhow::Error::from)?;
    Ok(SigiroClient::new(&base_url, &api_key))
}

// ---------------------------------------------------------------------------
// GET /v1/traces/{trace_id}
// ---------------------------------------------------------------------------

impl SigiroClient {
    pub async fn trace(&self, trace_id: &str) -> Result<crate::responses::TraceDetailResponse> {
        let mut req = self
            .http
            .get(format!("{}/v1/traces/{}", self.base_url, trace_id));
        if !self.api_key.is_empty() {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        resp.json().await.context("failed to parse trace response")
    }
}
