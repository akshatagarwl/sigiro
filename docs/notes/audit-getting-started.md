---
title: "Docs Audit: getting-started.md vs Source Code"
created: 2026-04-12
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'deployment', 'duckdb']
---

# Docs Audit: getting-started.md vs Source Code
Date: 2026-04-11

## Status Legend
- REAL: Code exists and works
- ASPIRATIONAL: Described in docs but no code exists
- WRONG: Code exists but docs describe it incorrectly

---

## 1. CLI Subcommands

| Docs say | Code reality | Status |
|---|---|---|
| `sigiro serve` | `Command::Serve` exists, no flags | REAL but docs show flags that don't exist |
| `sigiro investigate X --from --to` | `Command::Investigate { service, --from, --to }` | REAL |
| `sigiro query <sql>` | `Command::Query { sql }` | REAL |
| `sigiro status` | `Command::Status` | REAL (exists, fine) |

## 2. CLI Flags That Don't Exist

| Docs reference | Code reality | Status |
|---|---|---|
| `--central-url` | No code at all | ASPIRATIONAL |
| `--central` | No code at all | ASPIRATIONAL |

## 3. HTTP Routes

| Docs say | Code reality | Status |
|---|---|---|
| `POST /v1/query` | `post(query_handler)` | REAL |
| `POST /v1/investigate` | `post(investigate_handler)` | REAL |
| `GET /v1/services` | `get(services_handler)` | REAL |
| `GET /health` | `get(health)` | REAL |
| `POST /v1/events` (webhook adapter) | Does NOT exist | ASPIRATIONAL |
| Port 4040 (Pyroscope ingest) | No code at all | ASPIRATIONAL |

## 4. Fleet Observability Extensions

| Docs say | Code reality | Status |
|---|---|---|
|| `fsquery_scan('/var/log')` | Verified: `fsquery_scan` exists in fsquery extension | REAL |

## 5. Quack Federation

| Docs say | Code reality | Status |
|---|---|---|
| "Central node uses quack for query federation" | quack federation implemented via DuckDB ATTACH over HTTP | REAL |
| Collectors self-register via POST | `/v1/federation/workers` endpoint implemented | REAL |
| Worker registry in PostgreSQL catalog | WorkerInfo stored in federation table | REAL | |

## 6. InvestigationBlock JSON Example

| Docs show | Code reality | Status |
|---|---|---|
| `"metrics": [{..., "trend": "↑ 3.2x"}]` | MetricBucket has: bucket, metric_name, p95, avg, count. No "trend" | WRONG |
| `"deployments": [...]` | Field is `events: Vec<DerivedEvent>`, not "deployments" | WRONG |
| `"downstream_clues": [...]` | Not a field in InvestigationBlock | WRONG |
| Actual struct fields | service, window, truncation, coverage, context, events, operation_summaries, metrics, spans, span_trees, logs, linked_traces, profiles | WRONG (docs oversimplified) |

## 7. Infrastructure

| Docs say | Code reality | Status |
|---|---|---|
| `curl -fsSL sigiro.com/install \| sh` | No install script exists | ASPIRATIONAL |
| `systemctl enable --now sigiro` | No systemd unit file | ASPIRATIONAL |
| `ghcr.io/sigiroai/sigiro:latest` | Dockerfile exists but no published image evidence | ASPIRATIONAL |
| Kubernetes Deployment + PVC manifests | No k8s manifests in repo | ASPIRATIONAL |
| docker-compose.yml example | No docker-compose.yml in repo | ASPIRATIONAL |

## 8. Storage

| Docs say | Code reality | Status |
|---|---|---|
| DuckLake (DuckDB-native file catalog + Parquet) | REAL | REAL |
| `catalog.ducklake` + `catalog.ducklake.files/` | Local DuckLake layout | REAL |
| `SIGIRO_DATA_DIR` env var | Used in code | REAL |

## 9. Deployment Events Section

| Docs say | Code reality | Status |
|---|---|---|
| Jenkins/GitHub Actions OTel integration | Just standard OTLP ingest, no special handling | REAL (by convention) |
| `POST /v1/events` webhook adapter for ArgoCD/Flux | Does NOT exist | ASPIRATIONAL |
| `cicd.*` span attribute queries | Would work IF sent, no special indexing | ASPIRATIONAL |

---

## Priority: What to Fix

### Quick wins (docs-only fixes)
- ~~Fix fleet extension function names~~ ✅ Verified correct (2026-04-11)
- Fix [[architecture/investigation.md|InvestigationBlock]] JSON example to match actual struct
- Remove or mark `POST /v1/events` as planned

### Large effort (new code needed)
- `--central-url` / `--central` flags + federation
- `POST /v1/events` webhook adapter
- Port 4040 [[architecture/profiles.md|Pyroscope]] ingest
- Install script + [[roadmap.md|Docker]] image publish + K8s manifests
