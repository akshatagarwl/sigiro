use anyhow::{Context, Result, bail};

use crate::TimestampMicros;
use crate::diagnose::{DiagnoseRequest, DiagnoseResponse, DiagnoseServices, DiagnosisBlock};

/// Explicit client auth mode — mirrors server-side AuthMode.
#[derive(Clone)]
pub enum ClientAuth {
    /// No access token configured. Server must be in open mode.
    Open,
    /// Bearer token auth.
    Bearer { token: String },
    /// OAuth tokens stored in the operating-system credential store.
    OAuth {
        session: std::sync::Arc<crate::oauth::OAuthSession>,
    },
}

impl ClientAuth {
    /// Build from a raw OAuth access token.
    /// Empty string → Open (logged once at build time).
    pub(crate) fn from_access_token(access_token: String) -> Self {
        if access_token.is_empty() {
            tracing::debug!("client_auth=open — no access token configured");
            ClientAuth::Open
        } else {
            ClientAuth::Bearer {
                token: access_token,
            }
        }
    }

    /// Apply authentication to a request.
    pub(crate) async fn apply(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::RequestBuilder> {
        match self {
            ClientAuth::Open => Ok(req),
            ClientAuth::Bearer { token } => Ok(req.bearer_auth(token)),
            ClientAuth::OAuth { session } => Ok(req.bearer_auth(session.access_token().await?)),
        }
    }

    async fn refresh(&self) -> Result<Option<String>> {
        match self {
            ClientAuth::OAuth { session } => session.force_refresh().await.map(Some),
            ClientAuth::Open | ClientAuth::Bearer { .. } => Ok(None),
        }
    }

    fn unauthorized_message(&self) -> &'static str {
        match self {
            ClientAuth::OAuth { .. } => {
                "Unauthorized (401): the stored OAuth session was rejected; run `sigiro auth login`"
            }
            ClientAuth::Bearer { .. } => {
                "Unauthorized (401): the configured bearer token was rejected"
            }
            ClientAuth::Open => "Unauthorized (401): this endpoint requires authentication",
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

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let retry = request.try_clone();
        let response = self
            .auth
            .apply(request)
            .await?
            .send()
            .await
            .context("failed to connect to sigiro server")?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(token) = self.auth.refresh().await? else {
            return Ok(response);
        };
        retry
            .context("authenticated request cannot be retried after token refresh")?
            .bearer_auth(token)
            .send()
            .await
            .context("failed to retry request after refreshing the access token")
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
            401 => bail!(self.auth.unauthorized_message()),
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
        let resp = self.send(req).await?;

        let resp = self.expect_ok(resp).await?;
        resp.json().await.context("failed to parse query response")
    }

    /// GET /v1/services — every service that has reported telemetry, with
    /// per-signal counts and the first and last timestamps seen.
    pub async fn services(&self) -> Result<crate::responses::ServicesResponse> {
        let req = self.http.get(format!("{}/v1/services", self.base_url));
        let resp = self.send(req).await?;

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
        let resp = self.send(req).await?;

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
        let resp = self.send(req).await?;

        match resp.status().as_u16() {
            200 | 503 => resp
                .json::<crate::responses::HealthResponse>()
                .await
                .context("failed to parse /health response"),
            401 => bail!(self.auth.unauthorized_message()),
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
        let resp = self.send(req).await?;

        let resp = self.expect_ok(resp).await?;
        resp.json()
            .await
            .context("failed to parse diagnosis response")
    }
}

/// Build a client from config.
///
/// An explicit `SIGIRO_ACCESS_TOKEN` takes precedence over the operating-system
/// credential store. A stored OAuth login selects the hosted endpoint unless
/// `SIGIRO_ENDPOINT` is explicitly configured.
pub async fn try_build_client() -> anyhow::Result<SigiroClient> {
    let configured_endpoint = crate::config::ENDPOINT
        .value()
        .map_err(anyhow::Error::from)?;
    let access_token = crate::config::ACCESS_TOKEN.value().unwrap_or_default();
    if !access_token.is_empty() {
        let endpoint = if configured_endpoint.is_empty() {
            "http://localhost:9999".to_string()
        } else {
            configured_endpoint
        };
        return Ok(SigiroClient::new(
            &endpoint,
            ClientAuth::from_access_token(access_token),
        ));
    }
    if let Some(session) = crate::oauth::OAuthSession::load().await? {
        let endpoint = if configured_endpoint.is_empty() {
            session.endpoint().await
        } else {
            configured_endpoint
        };
        return Ok(SigiroClient::new(
            &endpoint,
            ClientAuth::OAuth {
                session: std::sync::Arc::new(session),
            },
        ));
    }
    let endpoint = if configured_endpoint.is_empty() {
        "http://localhost:9999".to_string()
    } else {
        configured_endpoint
    };
    Ok(SigiroClient::new(&endpoint, ClientAuth::Open))
}
