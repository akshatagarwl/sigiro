---
title: Authentication & Project Isolation
created: 2026-03-21
updated: 2026-05-17
type: concept
tags: ['architecture', 'auth', 'multi-tenant']
---

# Authentication & Project Isolation

Sigiro has two server modes. Open mode is the default for self-hosted, edge, and local deployments and does no per-request authentication; secure it with the network perimeter. Hosted deployments use TenantKeys when `SIGIRO_CATALOG_URL` configures the shared Postgres control plane.

---

## Hosted Tenant Key Format

The hosted control plane mints per-tenant bearer keys:

```
sk_<tenant>_<random>
```

Example: `sk_acme_7b3f...`. Tenant keys are managed by the hosted control plane, not by a self-hosted server.

---

## Auth Flow

```
Request
  │
  ├─→ Open mode (default): accept without per-request authentication
  │
  └─→ TenantKeys mode (`SIGIRO_CATALOG_URL` set): verify
       Authorization: Bearer sk_<tenant>_<random>
       against the shared Postgres control plane and route to that tenant
```

---

## Key Management

### Configure the operator CLI for hosted mode

```bash
export SIGIRO_ENDPOINT=https://<hosted-endpoint>
export SIGIRO_API_KEY=sk_<tenant>_<random>
```

`SIGIRO_API_KEY` is client-only configuration; `--key <token>` is the per-command alternative. There is no Sigiro CLI auth file. The hosted control plane mints and manages tenant keys.

---

## Rate Limiting

Hosted rate limits are a control-plane concern. Open mode relies on the deployment's network perimeter.

---

## Cache

Tenant key verification is backed by the shared Postgres control plane in hosted mode. Open mode does not verify or cache request credentials.

---

## Per-Project Isolation

Hosted tenant routing selects the tenant's storage. Open mode uses the local deployment's storage:

```
$SIGIRO_DATA_DIR/
  default/
    catalog.sqlite          # DuckLake catalog for default project
  <project_id>/
    catalog.sqlite          # Per-project DuckLake catalog
```

In TenantKeys mode, the verified tenant key determines the storage route. There is no key-derived project context in Open mode.

---

## Status

Open mode is the self-hosted default. TenantKeys is enabled only for hosted deployments with `SIGIRO_CATALOG_URL`.
