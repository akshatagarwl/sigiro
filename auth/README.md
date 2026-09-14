# Sigiro authentication service

Cloudflare Worker running Better Auth 1.7.4 with Effect v4, a private Hyperdrive connection to PostgreSQL on Hetzner, and the native Cloudflare Email Sending binding. It is deployed separately from the Rust resource server.

## Responsibilities

- email OTP sessions and passkeys;
- organizations and membership, used as Sigiro tenants;
- OAuth/OIDC discovery, consent, client registration, device authorization, token refresh/revocation, and JWKS;
- audience-bound EdDSA access tokens whose server-owned organization reference is emitted as `tid`;
- MCP protected-resource discovery and CIMD metadata validation.

Sigiro's Rust server verifies the JWT signature, issuer, audience, expiry, `tid`, and route scope. The auth service never selects a data tenant from client-provided metadata.

## Scopes

- `telemetry:ingest`
- `services:read`
- `diagnose:read`
- `traces:read`
- `query:read`
- `anomalies:read`
- `alerts:read`, `alerts:write`
- `tenant:members`, `tenant:agents`, `tenant:admin`
- OIDC scopes `openid`, `profile`, `email`, and `offline_access`

`offline_access` must remain in the protected resource's allowed scopes. Better Auth persists the granted scope set on refresh tokens and only rotates tokens carrying that scope.

## Production configuration

`wrangler.jsonc` defines the non-secret production configuration:

- `BETTER_AUTH_URL=https://auth.sigiro.com`
- `SIGIRO_RESOURCE=https://api.sigiro.com`
- `EMAIL_FROM=noreply@sigiro.com`
- `HYPERDRIVE` — cache-disabled connection to the dedicated `sigiro_auth` database
- `EMAIL` — Email Sending binding restricted to `noreply@sigiro.com`

`BETTER_AUTH_SECRET` is a Worker secret and must be at least 32 random bytes. The Worker has no database password or Cloudflare API token binding: Hyperdrive and Email Sending own those credentials. `workers.dev` and preview URLs are disabled; production is served only from `auth.sigiro.com`.

PostgreSQL remains private. Hyperdrive reaches it through a Workers VPC service and Cloudflare Tunnel on the Kamal network. The origin requires TLS and the Hyperdrive cache is disabled because authentication reads must be strongly consistent.

The request path uses Better Auth's Web `Request`/`Response` API. `@effectify/node-better-auth`, `@effect/platform-node`, and `@better-auth/cimd/node` are deliberately absent because they depend on Node HTTP and socket APIs that are unavailable in Workers. Effect v4 owns request execution and database cleanup around the Worker handler.

## Local verification

```sh
cp .env.example .env
bun install --frozen-lockfile
bun run test
```

The test runner starts PostgreSQL, applies migrations, runs the Worker locally through Wrangler's Hyperdrive emulation, and tears everything down. Tests exercise email OTP, organization activation, passkey registration options, OAuth/OIDC discovery, JWKS, confidential client credentials, native S256 PKCE, refresh rotation and replay rejection, device authorization, MCP metadata, CIMD, audience/scopes, and `tid` propagation.

## Build and deployment

```sh
bun run types
bun run check
bun run build
bunx wrangler secret put BETTER_AUTH_SECRET
bun run deploy -- --env-file /dev/null
```

Run `bun src/migrate.ts` from a trusted network path with `DATABASE_URL`, `BETTER_AUTH_URL`, `BETTER_AUTH_SECRET`, and `SIGIRO_RESOURCE` set before deploying a schema change. Migrations use Better Auth's programmatic migration API; `@better-auth/cli` is not used.
