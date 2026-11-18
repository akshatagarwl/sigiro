# Sigiro — hosted multi-tenant beta (design of record)

Invited beta: Sigiro runs as a hosted service so customers ship OpenTelemetry to
it and query it back with SQL, without hosting anything themselves. The operator
hand-provisions every tenant. Target: one Hetzner box, ≤10 tenants.

## The one contract

Everything reduces to a single invariant:

> **one tenant key → one isolated per-tenant DuckLake file catalog → one authenticated SQL surface.**

```
INGEST  (every path = OTLP + tenant bearer key)     STORE                     QUERY  (consumer = SQL + tenant bearer key)
───────────────────────────────────────────        ─────────────────         ─────────────────────────────────────────
App OTel SDK ──────┐                                                          ┌─→ AI coding agent (SQL, later MCP)
OTel Collector ────┼─→ :4317 gRPC / :9999 HTTP → per-tenant DuckLake → POST /v1/query ─┼─→ customer app agent (SQL)
OBI eBPF agent ────┘   (tenant derived from key)   (file catalog + Parquet)            ├─→ dashboard (later)
                                                                                       └─→ curl / raw SQL
```

Every ingest path is identical to the server (OTLP + key). Every consumer is
identical (SQL + key). Nothing in the middle is path- or consumer-specific.

## Customer runs on many nodes → converges to one tenant catalog

Multi-node is entirely a **client-side fan-in**; OTLP + resource attributes solve
it, so Sigiro stays single-node per tenant.

- Every node (pod / VM / region) ships to the **same** endpoint with the **same**
  tenant key. All of it lands in that tenant's one catalog.
- Node identity is preserved and queryable: `service_name` is a column;
  `host.name` / `k8s.pod.name` / `cloud.region` live in `resource_attributes`
  (`resource_attributes ->> 'k8s.pod.name'`).
- Distributed traces stitch automatically — context propagates in the customer's
  app; all spans for a `trace_id` arrive at the one catalog and join by `trace_id`.
- **Recommended topology for a fleet:** an **OTel Collector gateway** (k8s
  DaemonSet or one per cluster/region). Local nodes export to it; it batches,
  scrubs, holds the tenant key as one secret, buffers/retries, and forwards OTLP
  to Sigiro. Direct SDK→Sigiro is fine for a few nodes.

## Deployment topology (Hetzner box)

- Host: `api.sigiro.com` → `91.98.118.235`.
- **HTTP surfaces** (SQL API `POST /v1/query`, admin, OTLP/HTTP `/v1/traces|logs|metrics|profiles`)
  served plaintext on `:9999`; the box proxy (kamal-proxy/Caddy) terminates
  Let's Encrypt TLS on `:443` and forwards to `:9999`.
- **gRPC OTLP** on `:4317` exposed directly; **Sigiro terminates its own TLS**
  (rustls) with the same cert files (HTTP proxies don't cleanly pass h2).
- Container CPU/mem caps; box pruned to ≥10 GB free.

## Auth & provisioning

- Auth modes: exactly **two** — `Open` (no key, single default tenant, local dev)
  and `TenantKeys` (each request's bearer key resolves a tenant). No shared
  server key.
- ControlPlane: `rusqlite` over `admin.sqlite` in the data dir — `tenants` +
  `api_keys` (SHA-256 of `sk_<tenant>_<rand>`). Fail-loud, no fallbacks.
- **Provisioning = admin-token-gated HTTP endpoints** (`SIGIRO_ADMIN_TOKEN`):
  `POST /admin/tenants`, `POST /admin/keys` (key shown once), `DELETE` to revoke.
- **Key lifecycle:** long-lived, **multiple keys per tenant** (per-node/per-env),
  rotate = mint new + revoke old. No expiry.

## Storage & isolation

- One **per-tenant DuckLake file catalog** under `<data>/tenants/<tenant_id>/`.
  No Postgres/S3 catalog.
- `sanitize_tenant_id`: ASCII alphanumeric + underscore, ≤56 chars; reserved ids
  rejected (`default`, `extensions`, `tenants`).
- Query engine filesystem-locked unconditionally: `allowed_directories` = data +
  extension dirs, `enable_external_access=false`. SQL guarded by an AST allowlist
  (SELECT-only against `sigiro_*` tables/CTEs); no file/URL/S3 readers.

## Signals at launch

All four: **traces, logs, metrics (gauge/sum/histogram/exp-histogram), profiles.**

## Limits & retention

| Limit | Value | On breach |
|-------|-------|-----------|
| Retention | 14 days | background age-out |
| Per-tenant disk | ~5 GB | **reject new writes (429/413) + WARN + ops signal** |
| Query timeout | 30 s | 400 |
| Result rows | 100,000 | truncated (`x-sigiro-truncated: true`) |
| Query memory | ~400 MB | 400 |
| Ingest body | 8 MB decompressed | 413 (HTTP) / RESOURCE_EXHAUSTED (gRPC) |
| Request rate | ~20 req/s, burst 100 | 429 |

Fail loud on the disk cap — never silently drop a tenant's data.

## Self-observability (dogfood)

Sigiro emits its **own** OTLP to an internal ops tenant, so operator monitoring is
just SQL over Sigiro's telemetry — including per-tenant disk-usage / cap pressure.
A WARN log line on stdout backs it up (tail via docker/kamal).

## Durability

Live query surface, **no backup**. Customers are told bluntly: not a system of
record, export anything you must keep. Data is ephemeral within the 14-day window;
if the box dies, tenant data is gone. (The linguabench corpus backup is unrelated
and still required.)

## Deleted (one way to do each thing; greenfield, no compat)

- **Federation** (`federation.rs`, `quack_federation.rs`, worker endpoints,
  `execute_sql`/investigate fan-out) — redundant with per-tenant file catalogs
  (one tenant = one box); the multi-box story is tenant→box *routing*, deferred.
  Deletion also **closes a cross-tenant query-hijack vuln**.
- **Server-side eBPF** download/run (`ebpf.rs`) — fixes a boot crash
  (`SIGIRO_EBPF=true` bailed "not implemented" after `main` called it). OBI is a
  **client-side agent** the customer runs → OTLP to their tenant; capability probe
  kept in `diagnose.rs`.
- **`AuthMode::Key` + `SIGIRO_API_KEY`** — replaced by TenantKeys.
- **Dedicated `:4318` listener** — OTLP/HTTP is merged into `:9999`.
- **SQL word-denylist** — AST allowlist + engine lockdown are the real guards.
- **Postgres/S3 catalog** path (`StorageConfig`, `init_distributed`, …).

## Deferred (YAGNI — build when the need is real)

MCP server; web dashboard; tenant→box routing / `box_id` / multi-box gateway;
self-serve signup + billing; backups/DR for tenant data.

## Build checklist

1. **[in progress]** One-way cleanup (delete list above) — `cargo build`+tests green.
2. Per-tenant disk cap (~5 GB) + reject-on-breach at ingest.
3. Self-observability: Sigiro → ops tenant OTLP; per-tenant disk-usage signal.
4. gRPC rustls wiring on `:4317`; HTTP plaintext behind proxy on `:9999`.
5. Docs: fix onboarding OTLP/HTTP port (:4318 → :443/:9999), Collector-gateway
   path, node-grouped SQL examples, Scaling page, blunt no-backup statement.
6. Deploy on the box (`api.sigiro.com`, DNS A record, `SIGIRO_ADMIN_TOKEN` in
   SOPS), provision first tenant, E2E verify (two-tenant isolation, all 4 signals,
   register-worker 404).
