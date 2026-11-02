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
    /// The workspace (Sigiro tenant) every request from this CLI is scoped to.
    workspace: Option<Workspace>,
    issuer: Option<String>,
    resource: Option<String>,
    endpoint: Option<String>,
    scopes: Vec<String>,
    access_token_expires_at: Option<u64>,
    refresh_token_present: bool,
}

#[derive(Serialize)]
struct Workspace {
    id: String,
    name: Option<String>,
}

/// Claims the CLI reads from its own access token for display.
#[derive(Deserialize)]
struct WorkspaceClaims {
    tid: String,
    tenant_name: Option<String>,
}

/// The workspace named in a stored access token. Display only: the server
/// verifies the signature on every request, so the CLI does not need to.
fn workspace_of(access_token: &str) -> Option<Workspace> {
    jsonwebtoken::dangerous::insecure_decode_claims::<WorkspaceClaims>(access_token)
        .ok()
        .map(|claims| Workspace {
            id: claims.tid,
            name: claims.tenant_name,
        })
}

/// Where a session's credentials live between processes.
///
/// Every `sigiro` process (a shell's `sigiro auth token`, an agent's
/// long-running `sigiro mcp`) holds its own in-memory copy of one shared entry.
/// The authorization server rotates the refresh token on every refresh, so this
/// store, not the in-memory copy, is the only place the current refresh token
/// is known. A trait only so a test can share one store between two sessions.
pub(crate) trait CredentialStore: Send + Sync {
    fn read(&self) -> Result<Option<StoredCredentials>>;
    fn write(&self, credentials: &StoredCredentials) -> Result<()>;
    fn delete(&self) -> Result<bool>;
}

/// The operating-system credential store (Keychain, Credential Manager,
/// Secret Service).
struct KeyringStore;

impl CredentialStore for KeyringStore {
    fn read(&self) -> Result<Option<StoredCredentials>> {
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
    }

    fn write(&self, credentials: &StoredCredentials) -> Result<()> {
        let encoded =
            serde_json::to_string(credentials).context("failed to encode Sigiro credentials")?;
        keyring_entry()?
            .set_password(&encoded)
            .context("failed to store Sigiro credentials in the OS credential store")
    }

    fn delete(&self) -> Result<bool> {
        let entry = keyring_entry()?;
        match entry.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => {
                Err(anyhow::Error::new(error).context("failed to delete Sigiro credentials"))
            }
        }
    }
}

async fn blocking<T: Send + 'static>(
    store: &std::sync::Arc<dyn CredentialStore>,
    operation: impl FnOnce(&dyn CredentialStore) -> Result<T> + Send + 'static,
) -> Result<T> {
    let store = std::sync::Arc::clone(store);
    tokio::task::spawn_blocking(move || operation(store.as_ref()))
        .await
        .context("credential-store task failed")?
}

/// How long to wait for a concurrent refresh in another process to finish
/// writing its rotated token before treating a rejected refresh token as dead.
const ROTATION_SETTLE: Duration = Duration::from_millis(750);

pub struct OAuthSession {
    credentials: Mutex<StoredCredentials>,
    store: std::sync::Arc<dyn CredentialStore>,
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

fn keyring_store() -> std::sync::Arc<dyn CredentialStore> {
    std::sync::Arc::new(KeyringStore)
}

async fn read_credentials() -> Result<Option<StoredCredentials>> {
    blocking(&keyring_store(), |store| store.read()).await
}

async fn write_credentials(credentials: StoredCredentials) -> Result<()> {
    blocking(&keyring_store(), move |store| store.write(&credentials)).await
}

async fn delete_credentials() -> Result<bool> {
    blocking(&keyring_store(), |store| store.delete()).await
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
    let body = response
        .bytes()
        .await
        .context("failed to read Sigiro authorization-server metadata")?;
    let metadata = serde_json::from_slice::<AuthorizationServerMetadata>(&body)
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

#[derive(Deserialize)]
struct ProvisionedWorkspace {
    organization: ProvisionedOrganization,
    #[serde(default)]
    organizations: usize,
}

#[derive(Deserialize)]
struct ProvisionedOrganization {
    id: String,
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
    let provisioned: ProvisionedWorkspace = expect_success(
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
    .await?
    .json()
    .await
    .context("failed to read the provisioned workspace")?;
    // Signup never picks between workspaces on the user's behalf.
    ensure!(
        provisioned.organizations <= 1,
        "{email} already belongs to several workspaces; run `sigiro auth login` and choose one"
    );
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
            .json(&serde_json::json!({
                "userCode": user_code,
                "organizationId": provisioned.organization.id,
            }))
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
            workspace: workspace_of(&credentials.access_token),
            issuer: Some(credentials.issuer.clone()),
            resource: Some(credentials.resource.clone()),
            endpoint: Some(credentials.endpoint.clone()),
            scopes: credentials.scopes.clone(),
            access_token_expires_at: Some(credentials.expires_at),
            refresh_token_present: credentials.refresh_token.is_some(),
        },
        None => AuthStatus {
            authenticated: false,
            workspace: None,
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
        Self::load_from(keyring_store()).await
    }

    pub(crate) async fn load_from(
        store: std::sync::Arc<dyn CredentialStore>,
    ) -> Result<Option<Self>> {
        Ok(blocking(&store, |store| store.read())
            .await?
            .map(|credentials| Self {
                credentials: Mutex::new(credentials),
                store,
            }))
    }

    pub(crate) async fn endpoint(&self) -> String {
        self.credentials.lock().await.endpoint.clone()
    }

    /// The stored entry, when another process has rotated the refresh token
    /// away from `held` since this session last read or wrote it.
    async fn rotated_elsewhere(&self, held: &Option<String>) -> Result<Option<StoredCredentials>> {
        Ok(blocking(&self.store, |store| store.read())
            .await?
            .filter(|stored| stored.refresh_token.is_some() && &stored.refresh_token != held))
    }

    /// Refresh the access token.
    ///
    /// Rotation makes a refresh token single-use, and several processes share
    /// one stored entry. So this re-reads the store first and adopts a token
    /// another process already rotated to, and it deletes the entry only when
    /// the token the server rejected is still the stored one. Before, the
    /// second process to refresh sent the already-rotated token, got
    /// `invalid_grant`, and wiped the first process's fresh login.
    async fn refresh(&self, credentials: &mut StoredCredentials) -> Result<()> {
        if let Some(stored) = self.rotated_elsewhere(&credentials.refresh_token).await? {
            *credentials = stored;
            if credentials.expires_at > now()?.saturating_add(REFRESH_SKEW_SECONDS) {
                return Ok(());
            }
        }
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
                // A process that refreshed at the same moment may still be
                // writing the token it rotated to. Give it a moment, and keep
                // its login rather than deleting it.
                for wait in [Duration::ZERO, ROTATION_SETTLE] {
                    tokio::time::sleep(wait).await;
                    if let Some(stored) = self.rotated_elsewhere(&credentials.refresh_token).await?
                    {
                        *credentials = stored;
                        return Ok(());
                    }
                }
                blocking(&self.store, |store| store.delete()).await?;
                let reason = response
                    .error_description()
                    .map_or("it expired or was revoked", String::as_str);
                bail!("your Sigiro login ended ({reason}); run `sigiro auth login`");
            }
            Err(error) => return Err(error).context("failed to refresh Sigiro access token"),
        };
        let rotated = credentials_from_token(&token, &metadata)?;
        let written = rotated.clone();
        blocking(&self.store, move |store| store.write(&written)).await?;
        *credentials = rotated;
        Ok(())
    }

    pub(crate) async fn access_token(&self) -> Result<String> {
        let mut credentials = self.credentials.lock().await;
        if credentials.expires_at <= now()?.saturating_add(REFRESH_SKEW_SECONDS) {
            self.refresh(&mut credentials).await?;
        }
        Ok(credentials.access_token.clone())
    }

    pub(crate) async fn force_refresh(&self) -> Result<String> {
        let mut credentials = self.credentials.lock().await;
        self.refresh(&mut credentials).await?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// One shared entry, like the OS credential store two processes see.
    #[derive(Default)]
    struct MemoryStore(parking_lot::Mutex<Option<StoredCredentials>>);

    impl CredentialStore for MemoryStore {
        fn read(&self) -> Result<Option<StoredCredentials>> {
            Ok(self.0.lock().clone())
        }
        fn write(&self, credentials: &StoredCredentials) -> Result<()> {
            *self.0.lock() = Some(credentials.clone());
            Ok(())
        }
        fn delete(&self) -> Result<bool> {
            Ok(self.0.lock().take().is_some())
        }
    }

    /// A token endpoint that rotates: each refresh token works once, and the
    /// one it replaced answers `invalid_grant`, as Better Auth does.
    async fn rotating_token_endpoint(first: &str) -> (String, Arc<parking_lot::Mutex<u32>>) {
        let current = Arc::new(parking_lot::Mutex::new((first.to_string(), 0_u32)));
        let exchanges = Arc::new(parking_lot::Mutex::new(0_u32));
        let counter = Arc::clone(&exchanges);
        let app = axum::Router::new().route(
            "/token",
            axum::routing::post(
                move |axum::Form(form): axum::Form<std::collections::HashMap<String, String>>| {
                    let current = Arc::clone(&current);
                    let counter = Arc::clone(&counter);
                    async move {
                        let mut state = current.lock();
                        if form.get("refresh_token") != Some(&state.0) {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                axum::Json(serde_json::json!({ "error": "invalid_grant" })),
                            );
                        }
                        state.1 += 1;
                        *counter.lock() += 1;
                        state.0 = format!("refresh-{}", state.1);
                        (
                            axum::http::StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "access_token": format!("access-{}", state.1),
                                "token_type": "Bearer",
                                "expires_in": 900,
                                "refresh_token": state.0,
                            })),
                        )
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}/token"), exchanges)
    }

    fn expired(token_endpoint: &str, refresh_token: &str) -> StoredCredentials {
        StoredCredentials {
            access_token: "access-0".to_string(),
            refresh_token: Some(refresh_token.to_string()),
            expires_at: 0,
            scopes: Vec::new(),
            issuer: HOSTED_ISSUER.to_string(),
            resource: HOSTED_RESOURCE.to_string(),
            endpoint: HOSTED_ENDPOINT.to_string(),
            client_id: CLI_CLIENT_ID.to_string(),
            token_endpoint: token_endpoint.to_string(),
            revocation_endpoint: format!("{HOSTED_ISSUER}/oauth2/revoke"),
        }
    }

    #[tokio::test]
    async fn a_second_holder_adopts_a_rotated_token_instead_of_wiping_the_login() {
        let (token_endpoint, exchanges) = rotating_token_endpoint("refresh-0").await;
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
        store.write(&expired(&token_endpoint, "refresh-0")).unwrap();

        // Two processes, say an agent's `sigiro mcp` and a shell's
        // `sigiro auth token`, load the same refresh token.
        let mcp = OAuthSession::load_from(Arc::clone(&store))
            .await
            .unwrap()
            .unwrap();
        let shell = OAuthSession::load_from(Arc::clone(&store))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(shell.access_token().await.unwrap(), "access-1");
        // `mcp` still holds the rotated-away refresh-0 in memory.
        assert_eq!(mcp.access_token().await.unwrap(), "access-1");
        assert_eq!(*exchanges.lock(), 1, "the fresh stored token is reused");
        let stored = store.read().unwrap().expect("the login must survive");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));

        // After a 401 the session forces a refresh; the adopted token works.
        assert_eq!(mcp.force_refresh().await.unwrap(), "access-2");
        assert_eq!(shell.force_refresh().await.unwrap(), "access-2");
        assert!(store.read().unwrap().is_some());
    }

    #[tokio::test]
    async fn a_token_the_server_rejects_and_nobody_replaced_is_deleted() {
        let (token_endpoint, _) = rotating_token_endpoint("refresh-live").await;
        let store: Arc<dyn CredentialStore> = Arc::new(MemoryStore::default());
        store
            .write(&expired(&token_endpoint, "refresh-revoked"))
            .unwrap();
        let session = OAuthSession::load_from(Arc::clone(&store))
            .await
            .unwrap()
            .unwrap();

        let error = session.access_token().await.unwrap_err();
        assert!(format!("{error:#}").contains("run `sigiro auth login`"));
        assert!(store.read().unwrap().is_none());
    }
}
