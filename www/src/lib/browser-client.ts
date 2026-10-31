/**
 * The one browser OAuth client on sigiro.com: authorization code + PKCE against
 * `/api/auth`, with the same-origin CIMD document `src/worker.ts` serves at
 * `/.well-known/oauth-client`. WebMCP (`/`) and the dashboard (`/dashboard/`)
 * are its two redirect URIs; each surface runs its own flow with its own
 * transient key, so a callback is only ever consumed by the page it returns to.
 *
 * DUPLICATION: the auth Worker cannot fetch this document (Cloudflare 1014 on a
 * same-zone Worker-to-Worker fetch), so `auth/src/client-metadata.ts` carries
 * the production copy. Edit both together.
 */
export const BROWSER_CLIENT_METADATA_PATH = "/.well-known/oauth-client";
export const BROWSER_CLIENT_REDIRECT_PATHS = ["/", "/dashboard/"] as const;
export const BROWSER_CLIENT_SCOPE =
  "openid services:read query:read traces:read";

export const browserClientMetadata = (origin: string) => ({
  client_id: `${origin}${BROWSER_CLIENT_METADATA_PATH}`,
  client_name: "Sigiro browser",
  redirect_uris: BROWSER_CLIENT_REDIRECT_PATHS.map(
    (path) => `${origin}${path}`
  ),
  grant_types: ["authorization_code", "refresh_token"],
  response_types: ["code"],
  token_endpoint_auth_method: "none",
  application_type: "native",
  scope: BROWSER_CLIENT_SCOPE,
});
