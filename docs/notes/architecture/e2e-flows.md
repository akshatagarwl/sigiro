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
export SIGIRO_API_KEY=sk_<tenant>_<random>
```

The operator CLI reads these environment variables when it calls a hosted server. `--key <token>` can replace `SIGIRO_API_KEY` for one command. For a local Open server, the default endpoint is `http://localhost:9999` and no key is needed.

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
| TenantKeys | `SIGIRO_CATALOG_URL` configures the hosted control plane | `Authorization: Bearer sk_<tenant>_<random>` |

---

## The Three Actors

| Actor | What they do | How often |
|-------|-------------|-----------|
| **Human** | Configures an OTel SDK; obtains a tenant key from the hosted control plane only when using hosted Sigiro | Once |
| **User's app** | Sends OTLP telemetry to sigiro | Continuous |
| **Agent** | Queries sigiro to investigate incidents | On demand |

---

## Flow 1: OTLP Ingest (Continuous)

```
User's App (OTel SDK)
    │
     │  OTLP gRPC/HTTP to sigiro
     │  Hosted only: Authorization: Bearer sk_<tenant>_<random>
    │
    ▼
sigiro binary (axum)
    │
    ├── 1. Open mode: accept request without per-request auth
    │       TenantKeys mode: verify the hosted tenant bearer key
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
    │  (uses SIGIRO_ENDPOINT and, for hosted mode, SIGIRO_API_KEY or --key)
    │
    ▼
sigiro CLI
    │
     │  POST http://localhost:9999/v1/diagnose
     │  Hosted only: Authorization: Bearer sk_<tenant>_<random>
    │  Body: { "service_name": "checkout-service", "window": "15m" }
    │
    ▼
sigiro binary (axum :9999)
    │
    ├── 1. Open mode accepts; TenantKeys verifies the hosted tenant key
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
    ├── Hosted control plane creates tenant credentials
    ├── Create DuckDB file
    │
    ▼
Vercel calls POST /v1/installations/{id}/resources
    │  sigiro returns:
     │  { "secrets": [
     │      { "name": "SIGIRO_ENDPOINT", "value": "https://ingest.sigiro.com" },
     │      { "name": "SIGIRO_API_KEY", "value": "sk_<tenant>_<random>" }
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
   │  │ Tenant key │  │  Tenant/local │  │ Shared Postgres  │ │
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
| OTLP + query auth | Open mode has no per-request auth; hosted TenantKeys requires `sk_<tenant>_<random>`. |
| Data isolation | Hosted tenant key metadata routes to one tenant's storage. |
| Rate limiting | Hosted control-plane concern. |
| SQL injection | Keyword blocklist (21 keywords, word-boundary) + parameterized queries |
| Vercel SSO | OIDC JWT against Vercel JWKS (~30 lines) |
| Secrets | Hosted tenant keys belong in client environment variables, never in code. |

---

## Dev/Prod Parity

Same code path always runs. No `Option<>` gates, no feature flags.

| Config | Open (default) | Hosted TenantKeys |
|--------|-------------------|---------------------|
| Auth | No per-request authentication | `sk_<tenant>_<random>` verified via the shared Postgres control plane |
| Storage | Local DuckDB files | DuckLake (Postgres + S3) |
| Vercel | Partner ID = `dev` | Real Vercel JWKS |
