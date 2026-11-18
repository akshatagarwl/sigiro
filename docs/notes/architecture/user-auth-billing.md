---
title: Authentication & Project Isolation
created: 2026-03-21
updated: 2026-05-17
type: concept
tags: ['architecture', 'auth', 'multi-tenant']
---

# Authentication & Project Isolation

Sigiro has two server modes. Open mode is the default for self-hosted, edge, and local deployments and does no per-request authentication; secure it with the network perimeter. Hosted deployments use Better Auth OAuth/JWT when `SIGIRO_CATALOG_URL` configures the shared Postgres Better Auth/OAuth API.

---

## Hosted Tenant Key Format

The Better Auth/OAuth API mints per-tenant bearer keys:

```
<oauth-access-token>
```

Example: `<oauth-access-token>...`. OAuth access tokens are managed by the Better Auth/OAuth API, not by a self-hosted server.

---

## Auth Flow

```
Request
  │
  ├─→ Open mode (default): accept without per-request authentication
  │
  └─→ Better Auth OAuth/JWT mode (`SIGIRO_CATALOG_URL` set): verify
       Authorization: Bearer <oauth-access-token>
       against the shared Postgres Better Auth/OAuth API and route to that tenant
```

---

## Key Management

### Configure the operator CLI for hosted mode

```bash
export SIGIRO_ENDPOINT=https://<hosted-endpoint>
export SIGIRO_ACCESS_TOKEN=<oauth-access-token>
```

`SIGIRO_ACCESS_TOKEN` is client-only configuration; `--token <token>` is the per-command alternative. There is no Sigiro CLI auth file. The Better Auth/OAuth API mints and manages OAuth access tokens.

---

## Rate Limiting

Hosted rate limits are a control-plane concern. Open mode relies on the deployment's network perimeter.

---

## Cache

OAuth access token verification is backed by the shared Postgres Better Auth/OAuth API in hosted mode. Open mode does not verify or cache request credentials.

---

## Per-Project Isolation

Hosted tenant routing selects the tenant's storage. Open mode uses the local deployment's storage:

```
$SIGIRO_DATA_DIR/
  default/
    catalog.ducklake        # DuckDB-native DuckLake catalog for default project
    catalog.ducklake.files/ # Local Parquet data
  <project_id>/
    catalog.ducklake        # DuckDB-native DuckLake catalog
    catalog.ducklake.files/ # Local Parquet data
```

In Better Auth OAuth/JWT mode, the verified OAuth access token determines the storage route. There is no key-derived project context in Open mode.

---

## Status

Open mode is the self-hosted default. Better Auth OAuth/JWT is enabled only for hosted deployments with `SIGIRO_CATALOG_URL`.
