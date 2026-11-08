# Sigiro

Sigiro is an observability backend built for coding agents. It accepts
OpenTelemetry traces, logs, metrics, and profiles over OTLP, stores them as
Parquet, and answers SQL, ranked-diagnosis, and MCP queries so an agent can
investigate a production problem and propose a fix as a draft pull request.

It runs two ways:

* **Self-hosted**: one `sigiro` binary (Rust + DuckDB) that serves OTLP ingest,
  the HTTP query API, the CLI, and MCP on your machine. No account needed.
* **Hosted**: a multi-tenant Cloudflare Workers application at
  [sigiro.com](https://sigiro.com).

See [Architecture](#architecture) and [Build from source](#build-from-source)
below. Contributions are welcome; read [CONTRIBUTING.md](CONTRIBUTING.md) and
report vulnerabilities as described in [SECURITY.md](SECURITY.md).

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
Self-hosting requires no account;
Open mode requires no token, while production deployments can require one static
bearer token:

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

No configuration required. Data is stored in the platform data directory (`~/.local/share/sigiro` on Unix, `%LOCALAPPDATA%\sigiro` on Windows). Override with `SIGIRO_DATA_DIR`.

## Authentication

Sigiro has two server authentication modes:

* **Open** (default): self-hosted servers and edge/query-surface nodes run with
  no per-request authentication. Secure the network perimeter with a tailnet,
  private network, or reverse proxy.
* **Better Auth OAuth/JWT** (hosted): configure the issuer, audience, and JWKS
  settings. Requests carry OAuth access-token JWTs; organizations are tenants.

For automation, `SIGIRO_ACCESS_TOKEN` is **client-only** configuration for calling a hosted endpoint:

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

## Output

Every command prints pretty JSON. One format, so a human and a pipeline read the
same bytes:

```bash
sigiro status
sigiro anomalies | jq '.anomalies[] | select(.signal == "latency_p95")'
sigiro query "SELECT count(*) FROM sigiro_spans" > report.json
```

## Documentation

Full documentation is available at [docs.sigiro.com](https://docs.sigiro.com). Its source lives in `www/`.

## Build from source

Toolchains are pinned in `mise.toml`. With [mise](https://mise.jdx.dev) installed:

```bash
git clone https://github.com/akshatagarwl/sigiro.git
cd sigiro
mise trust && mise install
mise run build-dev          # debug binary at target/debug/sigiro
mise run build              # release binary, with the web dashboard built in
./target/debug/sigiro serve # OTLP on :4317/:4318, API on :9999
```

DuckDB is linked statically from a pinned, checksum-verified prebuilt archive
(`scripts/prepare-duckdb-static.sh`), so no C++ build of DuckDB is needed.
`mise run build` also builds the dashboard (Node.js 24 and Aube) and embeds it,
so the binary serves it at `/dashboard/` with no other files. A plain
`cargo build` embeds whatever `dashboard/dist` holds, possibly nothing.
No secrets or cloud accounts are needed to build, test, or run the self-hosted binary.

Checks, from the repository root:

```bash
mise run full               # rustfmt, cargo check, clippy -D warnings, deployment contract tests
scripts/run-with-duckdb-static.sh cargo test --workspace --locked
aube install --frozen-lockfile && aube run fmt:check && aube run lint
```

Install package dependencies from each package directory (`auth/`, `cloud/`,
`dashboard/`, `www/`) with its frozen Aube lockfile, for example
`aube -C dashboard install --frozen-lockfile`. `cloud/` imports a generated
WASM module, so run `aube -C cloud run build:wasm` once before
`aube -C cloud run check`.

## Architecture

```text
 OTLP (gRPC/HTTP)                       SQL / diagnose / MCP
        │                                        ▲
        ▼                                        │
 ┌──────────────┐   Parquet segments   ┌──────────────────┐
 │ ingest       │ ───────────────────▶ │ query (DuckDB)   │
 │ (per tenant, │   + manifest          │ ranked diagnosis │
 │  per signal) │                       │ anomalies, SQL   │
 └──────────────┘                       └──────────────────┘
                                                 │
                                                 ▼
                                   agent sessions → draft pull requests
```

* Each signal (spans, logs, metrics, profiles) has a fixed table layout.
  Ingest groups rows and commits them as immutable Parquet **segments**; the
  **manifest** is the authoritative list of live segments. Compaction and
  retention are manifest changes.
* The self-hosted binary keeps segments and the manifest on local disk. The
  hosted service runs the same model in Cloudflare Durable Object "cells", with
  segments in R2.
* Agents run as durable sessions. They publish repository changes only as draft
  pull requests that a human reviews.

Terms are defined in [CONTEXT.md](CONTEXT.md).

## Repository layout

| Path | What it is |
|---|---|
| `src/`, `crates/` | Rust: the `sigiro` binary (ingest, query, CLI, MCP), `sigiro-core`, segment format (+ WASM build) |
| `cloud/` | Hosted service: Cloudflare Worker and Durable Object cells |
| `auth/` | Hosted auth: Better Auth on Workers + D1 (OAuth, device flow, organizations) |
| `dashboard/` | Web dashboard (Next.js static export) |
| `www/` | Website and documentation (docs.sigiro.com) |
| `tests/` | Rust integration tests, end-to-end and deployment contract tests |

## Development

From the repository root, install the shared JavaScript tooling and run the same
format and lint configuration across auth, the website, and the agent:

```bash
aube install --frozen-lockfile
aube run fmt
aube run fmt:check
aube run lint
```

Format Rust with `mbx fmt` and run the full Rust gate with `mise run full`.

## License

Sigiro is licensed under the [Functional Source License, Version 1.1, ALv2 Future License](LICENSE.md) (FSL-1.1-ALv2), © 2026 Akshat Agarwal.

You may use, copy, modify and redistribute it for any purpose except a competing use: offering it, or a product built on it, as a commercial product or service that substitutes for Sigiro. Internal use, non-commercial education and research, and professional services for a licensee are all permitted. Each version becomes available under the Apache License, Version 2.0 on the second anniversary of its release. The license text is authoritative.
