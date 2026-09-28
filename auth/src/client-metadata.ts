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
 * The two copies are kept honest by `test/client-metadata.ts`, which parses the
 * served file and asserts it is deep-equal to this one. Edit either and the
 * test fails, so they cannot silently drift.
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
