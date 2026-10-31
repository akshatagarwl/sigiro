/**
 * The Sigiro CLI's OAuth client metadata document.
 *
 * This is ALSO served as a static asset at `www/public/.well-known/oauth-client.json`,
 * which is the public, canonical copy that external clients fetch. It is
 * duplicated here because the auth Worker cannot fetch that URL at runtime: a
 * Worker requesting a hostname served by another Worker on the same zone gets
 * `403 error code: 1014` ("CNAME Cross-User Banned") from Cloudflare, which
 * broke the OAuth device flow in production.
 *
 * DUPLICATION: nothing checks that the two copies agree, so edit both together.
 */
export const FIRST_PARTY_CLIENT_ID =
  "https://sigiro.com/.well-known/oauth-client.json";

export const FIRST_PARTY_CLIENT_METADATA = {
  client_id: FIRST_PARTY_CLIENT_ID,
  client_name: "Sigiro CLI",
  client_uri: "https://sigiro.com/",
  application_type: "native",
  redirect_uris: ["com.sigiro.cli:/oauth/callback"],
  grant_types: [
    "urn:ietf:params:oauth:grant-type:device_code",
    "refresh_token",
  ],
  token_endpoint_auth_method: "none",
  scope:
    "openid profile email offline_access telemetry:ingest services:read traces:read query:read",
} as const;

export const FIRST_PARTY_CLIENT_METADATA_JSON = JSON.stringify(
  FIRST_PARTY_CLIENT_METADATA
);

/**
 * The sigiro.com browser client: WebMCP on `/` and the telemetry dashboard on
 * `/dashboard/`, authorization code + PKCE.
 *
 * The website Worker serves this document at the client_id URL, but this Worker
 * cannot fetch it there (the same `1014` as the CLI document above), so
 * `cimd-fetch.ts` answers from this copy. DUPLICATION: it must equal
 * `browserClientMetadata("https://sigiro.com")` in
 * `www/src/lib/browser-client.ts`; edit both together.
 */
export const BROWSER_CLIENT_ID = "https://sigiro.com/.well-known/oauth-client";

export const BROWSER_CLIENT_METADATA_JSON = JSON.stringify({
  client_id: BROWSER_CLIENT_ID,
  client_name: "Sigiro browser",
  redirect_uris: ["https://sigiro.com/", "https://sigiro.com/dashboard/"],
  grant_types: ["authorization_code", "refresh_token"],
  response_types: ["code"],
  token_endpoint_auth_method: "none",
  application_type: "native",
  scope: "openid services:read query:read traces:read",
});
