# Sigiro authentication service

Node.js 24 LTS service running Better Auth 1.7.5 against the private PostgreSQL database on the hosted Sigiro network. It is deployed separately from the Rust resource server.

## Responsibilities

- email OTP sessions and passkeys;
- organizations and membership, used as Sigiro tenants;
- OAuth/OIDC discovery, consent, pre-registration, device authorization, token refresh/revocation, and JWKS;
- MCP 2026-07-28 Client ID Metadata Document registration using Better Auth's DNS-pinned Node transport;
- audience-bound EdDSA access tokens whose server-owned organization reference is emitted as `tid`.

Dynamic Client Registration is disabled and is not advertised. Public authorization-code clients use CIMD or explicit pre-registration, PKCE S256, and an RFC 8707 `resource` parameter. Device Authorization remains a separate optional OAuth extension.

## Production configuration

Required environment variables:

- `ENVIRONMENT=production`
- `PORT=3100`
- `BETTER_AUTH_URL=https://sigiro.com`
- `BETTER_AUTH_SECRET`
- `DATABASE_URL`
- `SIGIRO_RESOURCE=https://sigiro.com`
- `EMAIL_FROM=noreply@sigiro.com`
- `CLOUDFLARE_ACCOUNT_ID`
- `CLOUDFLARE_EMAIL_API_TOKEN`

PostgreSQL remains private. The service connects directly to it over the Alchemy-managed `sigiro` network; there is no Hyperdrive or VPC bridge in the steady-state application path. Cloudflare Email Sending's REST API delivers verification email for the existing `sigiro.com` sender domain; the Worker-only email binding is not required.

## Local verification

```sh
cp .env.example .env
aube install --frozen-lockfile
aube run test
docker buildx bake sigiro-auth --load
```

The E2E runner starts PostgreSQL, applies migrations, and exercises the real Node HTTP adapter. It verifies OTP, organization activation, OAuth/OIDC discovery, JWKS, confidential client credentials, native S256 PKCE, refresh rotation and replay rejection, device authorization, MCP metadata, real CIMD discovery and persistence, DNS-pinned transport rejection of loopback targets, DCR absence, audience/scopes, and `tid` propagation.

## Build and migration

```sh
aube run check
aube run build
aube run migrate
docker buildx bake sigiro-auth --load
```

Run migrations from a trusted network path before deployment. The migrator uses Better Auth's programmatic migration API and rejects unsafe schema changes.
