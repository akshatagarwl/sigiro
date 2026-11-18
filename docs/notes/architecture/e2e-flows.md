---
title: End-to-End Flows
created: 2026-03-21
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'data-model', 'deployment', 'sqlite']
---

# End-to-End Flows

> **Date:** 2026-03-22
> **[[plans/STATUS.md|Status]]:** Implemented (109 tests, clippy clean).

## CLI Remote Configuration

```bash
export SIGIRO_ENDPOINT=https://<hosted-endpoint>
export SIGIRO_ACCESS_TOKEN=<oauth-access-token>
```

The operator CLI reads these environment variables when it calls a hosted server. `--token <token>` can replace `SIGIRO_ACCESS_TOKEN` for one command. For a local Open server, the default endpoint is `http://localhost:9999` and no key is needed.

```bash
sigiro diagnose checkout-service     # just works
sigiro diagnose checkout --window 1h # just works
sigiro query "SELECT count(*) FROM sigiro_spans" # just works
sigiro status                           # just works
```

### Server modes

| Mode | Enabled when | Request authentication |
|------|--------------|------------------------|
| Open | Default self-hosted / edge / local | None; use a tailnet, private network, or reverse proxy |
| Better Auth OAuth/JWT | `SIGIRO_CATALOG_URL` configures the Better Auth/OAuth API | `Authorization: Bearer <oauth-access-token>` |

---

## The Three Actors

| Actor | What they do | How often |
|-------|-------------|-----------|
| **Human** | Configures an OTel SDK; obtains a OAuth access token from the Better Auth/OAuth API only when using hosted Sigiro | Once |
| **User's app** | Sends OTLP telemetry to sigiro | Continuous |
| **Agent** | Queries sigiro to investigate incidents | On demand |

---

## Flow 1: OTLP Ingest (Continuous)

```
User's App (OTel SDK)
    │
     │  OTLP gRPC/HTTP to sigiro
     │  Hosted only: Authorization: Bearer <oauth-access-token>
    │
    ▼
sigiro binary (axum)
    │
    ├── 1. Open mode: accept request without per-request auth
    │       Better Auth OAuth/JWT mode: verify the hosted tenant bearer key
    ├── 2. tokio::spawn_blocking → otlp2records → Arrow RecordBatch
    ├── 3. Send to the storage channel
    ├── 4. Flusher thread: buffered append (10k rows / 1s timer)
    ├── 5. DuckDB insert (local file or DuckLake)
    └── 6. Return 200 OK
```

**Background tasks:**
```
├── Evict idle DuckDB connections (no data for 5 min)
└── Retention cleanup (DELETE + CHECKPOINT per retention days)
```

---

## Flow 2: Agent Query (On Demand)

```
Agent (Claude Code / Cursor / Codex)
    │
    │  sigiro diagnose checkout-service
    │  (uses SIGIRO_ENDPOINT and, for hosted mode, SIGIRO_ACCESS_TOKEN or --token)
    │
    ▼
sigiro CLI
    │
     │  POST http://localhost:9999/v1/diagnose
     │  Hosted only: Authorization: Bearer <oauth-access-token>
    │  Body: { "services": ["checkout-service"] }
    │
    ▼
sigiro binary (axum :9999)
    │
    ├── 1. Open mode accepts; Better Auth OAuth/JWT verifies the hosted OAuth access token
    ├── 2. Open/get the relevant DuckDB connection
    ├── 3. Run 10+ investigation SQL queries
    ├── 4. Assemble InvestigationBlock
    └── 5. Return JSON
    │
    ▼
Agent receives InvestigationBlock JSON → reasons about data → suggests fix
```

## Flow 3: Vercel Marketplace (Future — Roadmap Phase 2)

```
Vercel user clicks "Add Sigiro" in Marketplace
    │
    ▼
Vercel calls PUT /v1/installations/{id}
    ├── Verify Vercel OIDC JWT
    ├── Hosted Better Auth/OAuth API creates tenant credentials
    ├── Create DuckDB file
    │
    ▼
Vercel calls POST /v1/installations/{id}/resources
    │  sigiro returns:
     │  { "secrets": [
     │      { "name": "SIGIRO_ENDPOINT", "value": "https://ingest.sigiro.com" },
     │      { "name": "SIGIRO_ACCESS_TOKEN", "value": "<oauth-access-token>" }
     │  ]}
    │
    ▼
Vercel injects env vars → user redeploys → OTLP flows → done
```

Code exists in src/vercel.rs (7 tests) but routes not yet mounted in server.rs.

---

## Architecture

```
  ┌──────────────┐     ┌────────────────┐     ┌────────────┐
  │  User's App  │     │ Agent (Claude  │     │   Human    │
  │  (OTel SDK)  │     │  Code/Cursor)  │     │ (one-time  │
  └──────┬───────┘     └───────┬────────┘     │  setup)    │
         │                     │              └──────┬─────┘
         │ OTLP                │ HTTP/CLI            │ CLI
         │                     │                     │
  ┌──────▼─────────────────────▼─────────────────────▼──────┐
  │                    sigiro binary                         │
  │                                                         │
   │  ┌──────────┐  ┌───────────────┐  ┌──────────────────┐ │
   │  │ OAuth access token │  │  Tenant/local │  │ Shared Postgres  │ │
   │  │ verification│─│  DuckDB pool  │  │ (hosted catalog) │ │
   │  │ (hosted)   │  │  data/*.duckdb│ └──────────────────┘ │
   │  └───────────┘  └───────────────┘                       │
  └─────────────────────────────────────────────────────────┘
```

Local mode: [[architecture/data-model.md|DuckDB]] files on disk, zero config.
[[architecture/ducklake-integration.md|DuckLake]] mode: SIGIRO_CATALOG_URL + SIGIRO_S3_BUCKET → Postgres catalog + S3 [[architecture/data-model.md|Parquet]].

---

## Pricing

**Free locally.** No limits, no expiration. Hosted pricing TBD — see [[designs/pricing.md|docs/designs/pricing.md]].

---

## Security

| Concern | Approach |
|---------|----------|
| OTLP + query auth | Open mode has no per-request auth; hosted Better Auth OAuth/JWT requires `<oauth-access-token>`. |
| Data isolation | Hosted OAuth access token metadata routes to one tenant's storage. |
| Rate limiting | Hosted control-plane concern. |
| SQL injection | Keyword blocklist (21 keywords, word-boundary) + parameterized queries |
| Vercel SSO | OIDC JWT against Vercel JWKS (~30 lines) |
| Secrets | Hosted OAuth access tokens belong in client environment variables, never in code. |

---

## Dev/Prod Parity

Same code path always runs. No `Option<>` gates, no feature flags.

| Config | Open (default) | Hosted Better Auth OAuth/JWT |
|--------|-------------------|---------------------|
| Auth | No per-request authentication | `<oauth-access-token>` verified via the shared Postgres Better Auth/OAuth API |
| Storage | Local DuckDB files | DuckLake (Postgres + S3) |
| Vercel | Partner ID = `dev` | Real Vercel JWKS |
