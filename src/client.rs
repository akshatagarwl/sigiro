use anyhow::{Context, Result, bail};

use crate::TimestampMicros;
use crate::diagnose::{DiagnoseRequest, DiagnoseResponse, DiagnoseServices, DiagnosisBlock};

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

    pub async fn diagnose(
        &self,
        service: &str,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<DiagnosisBlock> {
        let mut resp = self
            .diagnose_multi(vec![service.to_string()], from, to)
            .await?;
        if let Some(block) = resp.results.remove(service) {
            return Ok(block);
        }
        if let Some(err) = resp.errors.get(service) {
            bail!("diagnosis failed for {service}: {err}");
        }
        bail!("no diagnosis block returned for service {service}")
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

    /// GET /v1/services — every service that has reported telemetry, with
    /// per-signal counts and the first and last timestamps seen.
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

    /// GET /v1/anomalies — the precomputed anomaly table (all signals, ranked
    /// biggest shift first, with cross-signal `incident_id` grouping). The
    /// entry point for the detect→diagnose loop: poll this, act on new
    /// incidents. Optional service + µs-window filters.
    pub async fn anomalies(
        &self,
        service: Option<&str>,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<crate::responses::AnomaliesResponse> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(s) = service {
            params.push(("service", s.to_string()));
        }
        if let Some(f) = from {
            params.push(("from_ts", f.to_string()));
        }
        if let Some(t) = to {
            params.push(("to_ts", t.to_string()));
        }
        let req = self
            .http
            .get(format!("{}/v1/anomalies", self.base_url))
            .query(&params);
        let resp = self
            .auth
            .apply(req)
            .send()
            .await
            .context("failed to connect to sigiro server")?;

        let resp = self.expect_ok(resp).await?;
        resp.json()
            .await
            .context("failed to parse anomalies response")
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

    /// Diagnose one or more services.
    /// Returns partial results: successful blocks in `results`, per-service
    /// failures in `errors`.
    pub(crate) async fn diagnose_multi(
        &self,
        services: Vec<String>,
        from: Option<i64>,
        to: Option<i64>,
    ) -> Result<DiagnoseResponse> {
        let body = DiagnoseRequest {
            services: DiagnoseServices::try_new(services)?,
            // Passed through, NOT defaulted. `None` means the caller gave no
            // bound, and the server applies the single default in
            // `DiagnoseRequest::window()`. The CLI must not own a second copy
            // of that rule — see roadmap #72.
            from_ts: from.map(TimestampMicros::try_new).transpose()?,
            to_ts: to.map(TimestampMicros::try_new).transpose()?,
        };
        let req = self
            .http
            .post(format!("{}/v1/diagnose", self.base_url))
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
            .context("failed to parse diagnosis response")
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
    let auth = ClientAuth::from_key(crate::config::API_KEY.value().unwrap_or_default());
    Ok(SigiroClient::new(&base_url, auth))
}
