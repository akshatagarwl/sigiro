---
title: Sigiro — Current Status
created: 2026-03-22
updated: 2026-07-11
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'decision']
---

# Sigiro — Current Status

> **[[roadmap.md|Roadmap]]:** [docs/roadmap.md](../roadmap.md) — single source of truth for all phases.
> This file tracks what's built vs what's next. If it contradicts [[roadmap.md|roadmap]].md, that doc wins.

## What Works (109 tests, clippy clean)

| Component | Status | Key facts |
|-----------|--------|-----------|
| OTLP ingest (gRPC :4317, HTTP :4318) | ✅ | Per-project flusher threads, buffered flush (10k rows/1s), transform on tokio blocking pool |
| Per-project DuckDB isolation | ✅ | ProjectPool with DashMap, 256MB/1 thread per project, idle eviction 5min |
| Investigation engine | ✅ | 10+ SQL queries → InvestigationBlock (coverage, topology, baselines, errors, logs, metrics) |
| Auth | ✅ | Open by default; hosted TenantKeys enabled with `SIGIRO_CATALOG_URL` |
| HTTP API (:9999) | ✅ | /v1/diagnose, /v1/query, /v1/services |
| DuckLake storage | ✅ | DuckDB-native `catalog.ducklake` + local Parquet by default; distributed mode uses PostgreSQL metadata with local or S3/R2 data |
| CLI | ✅ | serve, query, diagnose, status; remote config via `SIGIRO_ENDPOINT` and hosted `SIGIRO_API_KEY` / `--key` |
| Rate limiting | ✅ | GCRA 100 req/min per API key (in-memory, per-node) |
| Docker | ✅ | Multi-arch (amd64/arm64), cargo-chef cached builds |
| Website | ✅ | Cloudflare Pages, waitlist API |
| Self-host edge federation (quack) | ✅ | `SIGIRO_QUERY_SURFACE=quack` serves a node's DuckLake read-only over the quack wire protocol; `sigiro query` CLI is the coordinator (tailnet discovery + additive-partial fold, no central node/registry). See [self-host-edge-federation.md](../guides/self-host-edge-federation.md), [distributed-query.md](architecture/distributed-query.md). |

## Designed, Implementation Pending

| Component | Design doc | Key facts |
|-----------|-----------|-----------|
| Deployment events | [distributed-query.md](architecture/distributed-query.md) | CI/CD as OTel spans (Jenkins, GitHub Actions, Keptn). Webhook adapter for ArgoCD/Flux (~50 lines). |
| Cold (S3) tier for quack edges | [self-host-edge-federation.md](../guides/self-host-edge-federation.md) | Refused at startup today — quack only serves the local, non-distributed "default" project (`SIGIRO_CATALOG_URL` unset). Needs a reader/table allowlist on the quack gate before an S3-backed catalog can be served. |
| instance_status table | — | Key-value table in admin.sqlite, tracks mode, platform, version, and profiling status |

## Written But Not Wired

| Component | File | What's missing |
|-----------|------|---------------|
| Vercel Marketplace | src/vercel.rs | Routes not mounted in server.rs, AppState missing VercelConfig |
| Billing counters | src/billing.rs | UsageCounters exist but nothing calls increment() |

## Architecture (single binary, same code local and self-hosted-edge)

```
OTLP req → auth cache → tokio::spawn_blocking(transform) → per-project
flusher → DuckDB (appender API) → DuckLake (Parquet on disk / S3)

sigiro serve
    │
    ├─ OTLP ingest → DuckLake (existing)
    └─ quack query surface (if SIGIRO_QUERY_SURFACE=quack) — read-only, in-process

sigiro query "<SQL>"   ← separate coordinator subcommand, not a server mode.
    Discovers edges (tailnet tag:sigiro-edge, or SIGIRO_EDGES/--edge),
    pushes a per-edge partial via quack_query(), folds results.
```

## Key Design Decisions (2026-04-11, updated 2026-07-11)

1. **[[architecture/distributed-query.md|Quack]] for federation** — `quack_query()` push-down (not `ATTACH`, which doesn't push predicates), additive-partial fold at the `sigiro query` coordinator, no external binary
2. **No worker registry** — edges are discovered via tailnet ACL tags (`tag:sigiro-edge`) or an explicit `SIGIRO_EDGES`/`--edge` override; no central control plane, no self-registration endpoint
3. **instance_status in admin.sqlite** — queryable, not a JSON file
4. **Deployment events as OTel spans** — no custom table, use CICD semantic conventions
5. **All [[architecture/data-model.md|DuckDB]] extensions loaded by default** — no opt-in, platform failures are warnings

## CI TODO

- [ ] Multi-platform / multi-arch release artifacts in CI — currently builds amd64 + arm64 in Docker, but aarch64 binary is uploaded as artifact. Consider: release builds for more platforms (e.g. x86_64-unknown-linux-musl, aarch64-apple-darwin) with proper release artifact publishing.
