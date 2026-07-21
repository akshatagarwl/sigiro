use anyhow::{Context, Result, bail};

use crate::investigate::{InvestigateRequest, InvestigateResponse, InvestigationBlock};

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
    pub(crate) fn from_key(api_key: String) -> Self {
        if api_key.is_empty() {
            tracing::debug!("client_auth=open — no API key configured");
            ClientAuth::Open
        } else {
            ClientAuth::Bearer { token: api_key }
        }
    }


    /// Apply auth to a reqwest request builder if needed.
    pub(crate) fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self {
            ClientAuth::Open => req,
            ClientAuth::Bearer { token } => req.bearer_auth(token),
        }
    }
}

/// HTTP client for hosted mode — CLI clients go through sigiro serve instead of direct DuckDB.
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
        let mut resp = self
            .investigate_multi(Some(service), None, from, to)
            .await?;
        if let Some(block) = resp.results.remove(service) {
            return Ok(block);
        }
        if let Some(err) = resp.errors.get(service) {
            bail!("investigation failed for {service}: {err}");
        }
        bail!("no investigation block returned for service {service}")
    }

    async fn expect_ok(&self, resp: reqwest::Response) -> Result<reqwest::Response> {
        match resp.status().as_u16() {
            200 => Ok(resp),
            401 => bail!(
                "Unauthorized (401): this hosted endpoint requires a bearer token; set SIGIRO_API_KEY"
            ),
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

    /// GET /health — typed response. Returns the parsed `HealthResponse` for
    /// both 200 (Ready) and 503 (Starting/Degraded) so callers can branch on
    /// the variant. Auth is not required (health is public).
    pub async fn health(&self) -> Result<crate::responses::HealthResponse> {
        let req = self.http.get(format!("{}/health", self.base_url));
        let resp = self
            .auth
            .apply(req)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        match resp.status().as_u16() {
            200 | 503 => resp
                .json::<crate::responses::HealthResponse>()
                .await
                .context("failed to parse /health response"),
            401 => bail!(
                "Unauthorized (401): this hosted endpoint requires a bearer token; set SIGIRO_API_KEY"
            ),
            s => {
                let body = resp.text().await.context("failed to read error body")?;
                bail!("Server error ({s}): {body}")
            }
        }
    }

    /// Investigate one or more services. Pass `service` for single, `services` for multi.
    /// Returns partial results: successful blocks in `results`, per-service
    /// failures in `errors`.
    pub(crate) async fn investigate_multi(
        &self,
        service: Option<&str>,
        services: Option<Vec<String>>,
        from: i64,
        to: i64,
    ) -> Result<InvestigateResponse> {
        let body = InvestigateRequest {
            service: service.map(str::to_string),
            services,
            from_ts: from,
            to_ts: to,
        };
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

    /// Read /admin/status — the instance status key/value map.
    /// The server populates this from its admin SQLite (e.g., eBPF status,
    /// pyroscope status). The server now requires `SIGIRO_ADMIN_TOKEN` on this
    /// route (M-4); this client sends no Bearer token, so callers in
    /// distributed/hosted mode should expect a 401 here (e.g. `sigiro diagnose`'s
    /// eBPF check degrades to a warning, it does not have the admin token).
    pub(crate) async fn admin_status(&self) -> Result<crate::responses::AdminStatusResponse> {
        let resp = self
            .http
            .get(format!("{}/admin/status", self.base_url))
            .send()
            .await
            .context("failed to connect to sigiro server")?;
        let resp = self.expect_ok(resp).await?;
        resp.json()
            .await
            .context("failed to parse admin/status response")
    }

}

/// Build a client from config.
///
/// `SIGIRO_API_KEY` is client-only configuration. When present, it is sent as
/// a bearer token to a hosted (`TenantKeys`) endpoint; it never configures
/// server-side authentication.
pub fn try_build_client() -> anyhow::Result<SigiroClient> {
    let base_url = crate::config::ENDPOINT
        .value()
        .map_err(anyhow::Error::from)?;
    let auth = ClientAuth::from_key(std::env::var("SIGIRO_API_KEY").unwrap_or_default());
    Ok(SigiroClient::new(&base_url, auth))
}

