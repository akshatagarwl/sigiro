# Sigiro

Observability platform with OTLP ingestion, ranked diagnosis, and SQL queries for agents.

## Hosted or self-hosted

For a new hosted account, sign up directly in the terminal. The command emails a
six-digit OTP, creates one personal workspace, completes OAuth device authorization,
and stores credentials in the OS credential store:

```bash
curl -fsSL https://sigiro.com/install | sh
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

For an existing account, use browser-based `sigiro auth login` instead. Use
`sigiro auth logout` to revoke the refresh token and remove credentials.
Self-hosting requires no account. Open mode requires no token. Set
`SIGIRO_STATIC_BEARER_TOKEN` to require one shared token on the local query API
and OTLP listeners, or configure issuer, audience, and JWKS URL for JWT mode. See the
[self-hosted configuration reference](www/docs/reference/self-hosted-configuration.md)
for retention, disk limits, authentication, and OTLP TLS.

## Quick Start

Download the latest binary for your platform from the releases page, or use the
installer above, then run:

```bash
sigiro serve
```

The server starts on:
- HTTP API: `http://localhost:9999`
- OTLP gRPC: `localhost:4317`
- OTLP HTTP: `localhost:4318`

No configuration required. Data is stored in the platform data directory (`~/.local/share/sigiro` on Unix, `%LOCALAPPDATA%\sigiro` on Windows). Override with `SIGIRO_DATA_DIR`. The [local data layout](docs/notes/architecture/ducklake-integration.md#local-mode) identifies the DuckLake catalog, Parquet files, and separate administrative database.

## Authentication

Sigiro has three server authentication modes:

* **Open** (default): self-hosted servers and edge/query-surface nodes run with
  no per-request authentication. Secure the network perimeter with a tailnet,
  private network, or reverse proxy.
* **Static bearer** (local self-hosting only): set `SIGIRO_STATIC_BEARER_TOKEN`
  to require one secret for the query API and all OTLP signals. Protect the
  connection with TLS; the main HTTP listener requires a TLS-terminating proxy.
* **Better Auth OAuth/JWT** (hosted): configure the issuer, audience, and JWKS
  settings. Requests carry OAuth access-token JWTs; organizations are tenants.

For automation, `SIGIRO_ACCESS_TOKEN` is **client-only** configuration for calling an authenticated endpoint:

```bash
SIGIRO_ACCESS_TOKEN=<oauth-access-token> sigiro status
# or: sigiro --token <oauth-access-token> status
```

### MCP

`sigiro mcp` speaks MCP over stdio and always exposes the four operational
tools. When Blume's documentation MCP is reachable, it also discovers and
proxies its four documentation tools without copying their schemas or corpus.
The upstream endpoint defaults to `https://sigiro.com/mcp/docs`; override it
with `SIGIRO_DOCS_MCP_URL` for a self-hosted docs deployment. Documentation
connection failures are isolated to documentation tools, and no Sigiro bearer
token is forwarded to the public documentation endpoint. The hosted Blume
implementation is currently published at `/mcp/docs`; this does not claim a
hosted unified `/mcp` endpoint.

### Hosted agent access

Users stay on Sigiro surfaces. Do not install or invoke a separate Agent Auth
executable:

- create a hosted account with `sigiro signup --name "Your Name" --email you@example.com`;
- connect operational MCP clients through `sigiro mcp`;
- register and manage agent identities through the Sigiro Agent Auth API at
  `https://sigiro.com/api/auth`.

The canonical provider discovery document is
`https://sigiro.com/.well-known/agent-configuration`; the protected API
resource is `https://sigiro.com`. Integrations discover the provider, enroll
their host, register agents, request the smallest required capability set, rotate
keys, and revoke agents through that API. The relevant lifecycle routes include
`/host/enroll`, `/agent/register`, `/agent/status`, `/agent/rotate-key`, and
`/agent/revoke`, relative to the API base above.

Sigiro uses the official Agent Auth implementation internally for protocol
behavior, signing, encrypted agent-side state, approval, replay protection,
rotation, and revocation. It is not a second user-facing CLI. A telemetry agent
should receive only `telemetry:ingest`; do not grant unrelated capabilities or
add application-specific key handling.

## End-to-End Test

Verify the full stack (OpenTelemetry Demo + Sigiro) with one command:

```bash
cd tests/e2e/k8s
./run.sh
```

This starts the complete demo environment and verifies telemetry flows from collectors through to queries.

## Output

Every command prints pretty JSON. One format, so a human and a pipeline read the
same bytes:

```bash
sigiro status
sigiro anomalies | jq '.anomalies[] | select(.signal == "latency_p95")'
sigiro query "SELECT count(*) FROM sigiro_spans" > report.json
```

## Documentation

Full documentation is available at [docs.sigiro.com](https://docs.sigiro.com) (or in `docs-site/docs/`).

## Development

```bash
# Build
cargo build --release

# Run with custom data directory
SIGIRO_DATA_DIR=./data cargo run -- serve

# Run tests
cargo test --release

# Run E2E test
cd tests/e2e/k8s && ./run.sh
```
