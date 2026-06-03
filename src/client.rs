use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::investigate::InvestigationBlock;

/// Explicit client auth mode — mirrors server-side AuthMode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientAuth {
    /// No API key configured. Server must be in open mode.
    Open,
    /// Bearer token auth.
    Bearer { token: String },
}

impl ClientAuth {
    /// Build from a raw API key string.
    /// Empty string → Open (logged once at build time).
    pub fn from_key(api_key: String) -> Self {
        if api_key.is_empty() {
            tracing::debug!("client_auth=open — no API key configured");
            ClientAuth::Open
        } else {
            ClientAuth::Bearer { token: api_key }
        }
    }

    /// True if this client will send an Authorization header.
    pub fn sends_auth(&self) -> bool {
        matches!(self, ClientAuth::Bearer { .. })
    }

    /// Apply auth to a reqwest request builder if needed.
    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            ClientAuth::Open => req,
            ClientAuth::Bearer { token } => req.bearer_auth(token),
        }
    }
}

/// HTTP client for hosted mode — CLI and MCP go through sigiro serve instead of direct DuckDB.
#[derive(Clone)]
pub struct SigiroClient {
    http: reqwest::Client,
    base_url: String,
    auth: ClientAuth,
}

impl SigiroClient {
    pub fn new(base_url: &str, auth: ClientAuth) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
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
        let req = self
            .http
            .post(format!("{}/v1/query", self.base_url))
            .body(sql.to_string());
        let resp = self
            .auth
            .apply(req)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        resp.json().await.context("failed to parse query response")
    }

    pub async fn status(&self) -> Result<crate::responses::AdminStatusResponse> {
        let req = self.http.get(format!("{}/health", self.base_url));
        let resp = self
            .auth
            .apply(req)
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
        let req = self
            .http
            .post(format!("{}/v1/investigate", self.base_url))
            .json(&body);
        let resp = self
            .auth
            .apply(req)
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
        let req = self.http.get(format!("{}/v1/services", self.base_url));
        let resp = self
            .auth
            .apply(req)
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
        let req = self
            .http
            .post(format!("{}/v1/federation/workers", self.base_url))
            .json(worker);
        let resp = self
            .auth
            .apply(req)
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
        let req = self
            .http
            .delete(format!("{}/v1/federation/workers/{}", self.base_url, id));
        let resp = self
            .auth
            .apply(req)
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

        let req = self
            .http
            .get(format!("{}/v1/federation/workers", self.base_url));
        let resp = self
            .auth
            .apply(req)
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

/// Build a client from config.
/// If no API key is configured, builds an open-mode client (logs once).
pub fn try_build_client() -> anyhow::Result<SigiroClient> {
    let api_key = crate::config::get_api_key().unwrap_or_default();
    let auth = ClientAuth::from_key(api_key);
    if matches!(auth, ClientAuth::Open) {
        tracing::warn!(
            "No API key configured. Building open-mode client. \
             If the server requires auth, requests will fail with 401."
        );
    }
    let base_url = crate::config::ENDPOINT
        .value()
        .map_err(anyhow::Error::from)?;
    Ok(SigiroClient::new(&base_url, auth))
}

// ---------------------------------------------------------------------------
// GET /v1/traces/{trace_id}
// ---------------------------------------------------------------------------

impl SigiroClient {
    pub async fn trace(&self, trace_id: &str) -> Result<crate::responses::TraceDetailResponse> {
        let req = self
            .http
            .get(format!("{}/v1/traces/{}", self.base_url, trace_id));
        let resp = self
            .auth
            .apply(req)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        resp.json().await.context("failed to parse trace response")
    }
}
