use anyhow::{Context, Result, bail, ensure};
use oauth2::basic::{BasicClient, BasicErrorResponseType};
use oauth2::{
    AuthType, ClientId, DeviceAuthorizationUrl, RefreshToken, RequestTokenError, Scope,
    StandardDeviceAuthorizationResponse, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};
use std::io::IsTerminal;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

pub(crate) const HOSTED_ENDPOINT: &str = "https://sigiro.com";
const HOSTED_AUTH_ORIGIN: &str = "https://sigiro.com";
pub(crate) const HOSTED_ISSUER: &str = "https://sigiro.com/api/auth";
pub(crate) const HOSTED_RESOURCE: &str = "https://sigiro.com";
pub(crate) const CLI_CLIENT_ID: &str = "https://sigiro.com/.well-known/oauth-client.json";
const KEYRING_SERVICE: &str = "sigiro";
const KEYRING_USER: &str = "hosted-oauth";
const REFRESH_SKEW_SECONDS: u64 = 60;
const REQUESTED_SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "telemetry:ingest",
    "services:read",
    "traces:read",
    "query:read",
];

#[derive(Clone, Deserialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    device_authorization_endpoint: String,
    token_endpoint: String,
    revocation_endpoint: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredCredentials {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: u64,
    scopes: Vec<String>,
    issuer: String,
    resource: String,
    endpoint: String,
    client_id: String,
    token_endpoint: String,
    revocation_endpoint: String,
}

#[derive(Serialize)]
pub struct AuthStatus {
    authenticated: bool,
    issuer: Option<String>,
    resource: Option<String>,
    endpoint: Option<String>,
    scopes: Vec<String>,
    access_token_expires_at: Option<u64>,
    refresh_token_present: bool,
}

pub struct OAuthSession {
    credentials: Mutex<StoredCredentials>,
}

type HostedOAuthClient = BasicClient<
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointNotSet,
    oauth2::EndpointSet,
>;

fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs())
}

fn keyring_entry() -> Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).context("OS credential store is unavailable")
}

async fn read_credentials() -> Result<Option<StoredCredentials>> {
    tokio::task::spawn_blocking(|| {
        let entry = keyring_entry()?;
        match entry.get_password() {
            Ok(encoded) => serde_json::from_str(&encoded)
                .context("stored Sigiro credentials are malformed")
                .map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => {
                Err(anyhow::Error::new(error).context("failed to read Sigiro credentials"))
            }
        }
    })
    .await
    .context("credential-store task failed")?
}

async fn write_credentials(credentials: StoredCredentials) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let encoded =
            serde_json::to_string(&credentials).context("failed to encode Sigiro credentials")?;
        keyring_entry()?
            .set_password(&encoded)
            .context("failed to store Sigiro credentials in the OS credential store")
    })
    .await
    .context("credential-store task failed")?
}

async fn delete_credentials() -> Result<bool> {
    tokio::task::spawn_blocking(|| {
        let entry = keyring_entry()?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => {
                Err(anyhow::Error::new(error).context("failed to delete Sigiro credentials"))
            }
        }
    })
    .await
    .context("credential-store task failed")?
}

fn oauth_http_client() -> Result<oauth2::reqwest::Client> {
    oauth2::reqwest::ClientBuilder::new()
        .redirect(oauth2::reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to build OAuth HTTP client")
}

fn validate_https_endpoint(value: &str, field: &str) -> Result<()> {
    let url = url::Url::parse(value).with_context(|| format!("invalid {field} URL"))?;
    ensure!(url.scheme() == "https", "{field} must use HTTPS");
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "{field} must not contain credentials"
    );
    ensure!(
        url.fragment().is_none(),
        "{field} must not contain a fragment"
    );
    Ok(())
}

async fn discover() -> Result<AuthorizationServerMetadata> {
    let client = oauth_http_client()?;
    let response = client
        .get(format!("{HOSTED_ISSUER}/.well-known/openid-configuration"))
        .send()
        .await
        .context("failed to fetch Sigiro authorization-server metadata")?
        .error_for_status()
        .context("Sigiro authorization-server metadata request failed")?;
    let metadata = response
        .json::<AuthorizationServerMetadata>()
        .await
        .context("failed to parse Sigiro authorization-server metadata")?;
    ensure!(
        metadata.issuer == HOSTED_ISSUER,
        "authorization-server issuer mismatch"
    );
    validate_https_endpoint(
        &metadata.device_authorization_endpoint,
        "device authorization endpoint",
    )?;
    validate_https_endpoint(&metadata.token_endpoint, "token endpoint")?;
    validate_https_endpoint(&metadata.revocation_endpoint, "revocation endpoint")?;
    Ok(metadata)
}

fn basic_client(metadata: &AuthorizationServerMetadata) -> Result<HostedOAuthClient> {
    Ok(BasicClient::new(ClientId::new(CLI_CLIENT_ID.to_string()))
        .set_auth_type(AuthType::RequestBody)
        .set_device_authorization_url(DeviceAuthorizationUrl::new(
            metadata.device_authorization_endpoint.clone(),
        )?)
        .set_token_uri(TokenUrl::new(metadata.token_endpoint.clone())?))
}

async fn request_device_authorization(
    client: &HostedOAuthClient,
    http: &oauth2::reqwest::Client,
) -> Result<StandardDeviceAuthorizationResponse> {
    client
        .exchange_device_code()
        .add_scopes(
            REQUESTED_SCOPES
                .iter()
                .map(|scope| Scope::new((*scope).to_string())),
        )
        .add_extra_param("resource", HOSTED_RESOURCE)
        .request_async(http)
        .await
        .context("device authorization request failed")
}

fn credentials_from_token(
    token: &impl TokenResponse,
    metadata: &AuthorizationServerMetadata,
) -> Result<StoredCredentials> {
    let expires_in = token
        .expires_in()
        .context("authorization server omitted access-token expiry")?
        .as_secs();
    let refresh_token = token.refresh_token().map(|token| token.secret().to_owned());
    ensure!(
        refresh_token.is_some(),
        "authorization server omitted the refresh token"
    );
    Ok(StoredCredentials {
        access_token: token.access_token().secret().to_owned(),
        refresh_token,
        expires_at: now()?.saturating_add(expires_in),
        scopes: token
            .scopes()
            .map(|scopes| {
                scopes
                    .iter()
                    .map(|scope| scope.as_ref().to_owned())
                    .collect()
            })
            .unwrap_or_else(|| {
                REQUESTED_SCOPES
                    .iter()
                    .map(|scope| (*scope).to_string())
                    .collect()
            }),
        issuer: metadata.issuer.clone(),
        resource: HOSTED_RESOURCE.to_string(),
        endpoint: HOSTED_ENDPOINT.to_string(),
        client_id: CLI_CLIENT_ID.to_string(),
        token_endpoint: metadata.token_endpoint.clone(),
        revocation_endpoint: metadata.revocation_endpoint.clone(),
    })
}

async fn exchange_device_token(
    client: &HostedOAuthClient,
    http: &oauth2::reqwest::Client,
    details: &StandardDeviceAuthorizationResponse,
    metadata: &AuthorizationServerMetadata,
) -> Result<AuthStatus> {
    let token = client
        .exchange_device_access_token(details)
        .add_extra_param("resource", HOSTED_RESOURCE)
        .request_async(http, tokio::time::sleep, None)
        .await
        .context("device authorization did not complete")?;
    let credentials = credentials_from_token(&token, metadata)?;
    write_credentials(credentials.clone()).await?;
    Ok(status_from(Some(&credentials)))
}

async fn expect_success(response: reqwest::Response, operation: &str) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    bail!("{operation} failed ({status}): {body}")
}

fn read_verification_code() -> Result<String> {
    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password("Verification code: ")
            .context("failed to read the email verification code");
    }
    let mut otp = String::new();
    std::io::stdin()
        .read_line(&mut otp)
        .context("failed to read the email verification code from stdin")?;
    Ok(otp)
}

pub async fn login(no_browser: bool) -> Result<AuthStatus> {
    let metadata = discover().await?;
    let client = basic_client(&metadata)?;
    let http = oauth_http_client()?;
    let details = request_device_authorization(&client, &http).await?;

    let verification_url = details
        .verification_uri_complete()
        .map(|url| url.secret().to_owned())
        .unwrap_or_else(|| details.verification_uri().to_string());
    validate_https_endpoint(&verification_url, "device verification URL")?;
    eprintln!("Open this URL to sign in or create an account:\n{verification_url}");
    eprintln!(
        "Confirm that the page shows this code: {}",
        details.user_code().secret()
    );
    if !no_browser && webbrowser::open(&verification_url).is_err() {
        eprintln!("Could not open a browser. Open the URL above manually.");
    }

    exchange_device_token(&client, &http, &details, &metadata).await
}

pub async fn signup(name: String, email: String) -> Result<AuthStatus> {
    let name = name.trim().to_string();
    let email = email.trim().to_lowercase();
    ensure!(!name.is_empty(), "name must not be empty");
    ensure!(!email.is_empty(), "email must not be empty");

    let metadata = discover().await?;
    let oauth_client = basic_client(&metadata)?;
    let oauth_http = oauth_http_client()?;
    let device = request_device_authorization(&oauth_client, &oauth_http).await?;
    let mut browser_headers = reqwest::header::HeaderMap::new();
    browser_headers.insert(
        reqwest::header::ORIGIN,
        reqwest::header::HeaderValue::from_static(HOSTED_AUTH_ORIGIN),
    );
    let browser = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .default_headers(browser_headers)
        .timeout(Duration::from_secs(15))
        .build()
        .context("failed to build signup HTTP client")?;

    expect_success(
        browser
            .post(format!("{HOSTED_ISSUER}/email-otp/send-verification-otp"))
            .json(&serde_json::json!({ "email": email, "type": "sign-in" }))
            .send()
            .await
            .context("failed to request the email verification code")?,
        "email verification request",
    )
    .await?;
    eprintln!("A six-digit verification code was sent to {email}.");
    let otp = read_verification_code()?;

    expect_success(
        browser
            .post(format!("{HOSTED_ISSUER}/sign-in/email-otp"))
            .json(&serde_json::json!({ "email": email, "otp": otp.trim(), "name": name }))
            .send()
            .await
            .context("failed to verify the email code")?,
        "email verification",
    )
    .await?;
    expect_success(
        browser
            .post(format!(
                "{HOSTED_ISSUER}/onboarding/ensure-personal-organization"
            ))
            .json(&serde_json::json!({}))
            .send()
            .await
            .context("failed to provision the personal workspace")?,
        "personal workspace provisioning",
    )
    .await?;
    let user_code = device.user_code().secret();
    expect_success(
        browser
            .get(format!("{HOSTED_ISSUER}/device"))
            .query(&[("user_code", user_code)])
            .send()
            .await
            .context("failed to claim the CLI device request")?,
        "device request claim",
    )
    .await?;
    expect_success(
        browser
            .post(format!("{HOSTED_ISSUER}/device/approve"))
            .json(&serde_json::json!({ "userCode": user_code }))
            .send()
            .await
            .context("failed to approve the CLI device request")?,
        "device request approval",
    )
    .await?;

    exchange_device_token(&oauth_client, &oauth_http, &device, &metadata).await
}

fn status_from(credentials: Option<&StoredCredentials>) -> AuthStatus {
    match credentials {
        Some(credentials) => AuthStatus {
            authenticated: true,
            issuer: Some(credentials.issuer.clone()),
            resource: Some(credentials.resource.clone()),
            endpoint: Some(credentials.endpoint.clone()),
            scopes: credentials.scopes.clone(),
            access_token_expires_at: Some(credentials.expires_at),
            refresh_token_present: credentials.refresh_token.is_some(),
        },
        None => AuthStatus {
            authenticated: false,
            issuer: None,
            resource: None,
            endpoint: None,
            scopes: Vec::new(),
            access_token_expires_at: None,
            refresh_token_present: false,
        },
    }
}

pub async fn status() -> Result<AuthStatus> {
    Ok(status_from(read_credentials().await?.as_ref()))
}

pub async fn access_token() -> Result<String> {
    let session = OAuthSession::load()
        .await?
        .context("not signed in; run `sigiro auth login`")?;
    session.force_refresh().await
}

impl OAuthSession {
    pub(crate) async fn load() -> Result<Option<Self>> {
        Ok(read_credentials().await?.map(|credentials| Self {
            credentials: Mutex::new(credentials),
        }))
    }

    pub(crate) async fn endpoint(&self) -> String {
        self.credentials.lock().await.endpoint.clone()
    }

    async fn refresh(credentials: &mut StoredCredentials) -> Result<()> {
        let refresh_token = credentials
            .refresh_token
            .clone()
            .context("stored OAuth session has no refresh token; run `sigiro auth login`")?;
        let metadata = AuthorizationServerMetadata {
            issuer: credentials.issuer.clone(),
            device_authorization_endpoint: format!("{}/device/code", credentials.issuer),
            token_endpoint: credentials.token_endpoint.clone(),
            revocation_endpoint: credentials.revocation_endpoint.clone(),
        };
        let client = basic_client(&metadata)?;
        let http = oauth_http_client()?;
        let result = client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .add_extra_param("resource", credentials.resource.clone())
            .request_async(&http)
            .await;
        let token = match result {
            Ok(token) => token,
            Err(RequestTokenError::ServerResponse(response))
                if response.error() == &BasicErrorResponseType::InvalidGrant =>
            {
                delete_credentials().await?;
                bail!("stored OAuth session was rejected; run `sigiro auth login`");
            }
            Err(error) => return Err(error).context("failed to refresh Sigiro access token"),
        };
        let rotated = credentials_from_token(&token, &metadata)?;
        write_credentials(rotated.clone()).await?;
        *credentials = rotated;
        Ok(())
    }

    pub(crate) async fn access_token(&self) -> Result<String> {
        let mut credentials = self.credentials.lock().await;
        if credentials.expires_at <= now()?.saturating_add(REFRESH_SKEW_SECONDS) {
            Self::refresh(&mut credentials).await?;
        }
        Ok(credentials.access_token.clone())
    }

    pub(crate) async fn force_refresh(&self) -> Result<String> {
        let mut credentials = self.credentials.lock().await;
        Self::refresh(&mut credentials).await?;
        Ok(credentials.access_token.clone())
    }
}

pub async fn logout() -> Result<bool> {
    let credentials = read_credentials().await?;
    let mut remote_error = None;
    if let Some(credentials) = &credentials
        && let Some(refresh_token) = &credentials.refresh_token
    {
        let result = oauth_http_client()?
            .post(&credentials.revocation_endpoint)
            .form(&[
                ("token", refresh_token.as_str()),
                ("token_type_hint", "refresh_token"),
                ("client_id", credentials.client_id.as_str()),
            ])
            .send()
            .await;
        match result {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                remote_error = Some(format!("remote revocation returned {}", response.status()))
            }
            Err(error) => remote_error = Some(format!("remote revocation failed: {error}")),
        }
    }
    let deleted = delete_credentials().await?;
    if let Some(error) = remote_error {
        bail!("{error}; local credentials were deleted");
    }
    Ok(deleted)
}
