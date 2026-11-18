---
title: Auth, RBAC, and Billing — Design
created: 2026-03-30
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'data-model', 'deployment', 'design']
---

# Auth, RBAC, and Billing — Design

**Status:** Superseded — Zitadel (RBAC recommendation) is off-roadmap. Self-hosted Sigiro is Open by default; hosted deployments use TenantKeys. RBAC and hosted billing TBD.

## Current State
- Hosted multi-tenant auth uses per-tenant keys through the shared Postgres control plane
- Hosted Sigiro on [[company/gtm.md|Fly.io]] with scale-to-zero
- No user accounts, no RBAC, no billing
- Auth: Open mode locally; TenantKeys for hosted deployments. No external auth SaaS.

## Billing Strategy

### Marketplace users (Vercel, Netlify, Railway, Render)
Platform handles auth + billing (rev share). Sigiro just receives provisioning
webhooks → create tenant + tenant key → return [[architecture/write-path.md|OTLP]] endpoint.

No [[designs/pricing.md|Stripe]] needed for marketplace users.

### Direct / K8s users
[[designs/pricing.md|Stripe]] alone. No billing platform (Lago, etc.).
- [[designs/pricing.md|Stripe]] Meters API: report usage (GB ingested) → Stripe calculates bill
- [[designs/pricing.md|Stripe]] Checkout: self-serve signup, no custom UI
- [[designs/pricing.md|Stripe]] Customer Portal: users manage subscriptions
- ~50 lines to integrate

### Self-hosted
Free. No billing needed.

## Auth/RBAC — Evaluated Options (Mar 2026)

| Solution    | Free MAU   | Self-Host | Rust/axum Support     | RBAC | Orgs | License     |
|-------------|------------|-----------|----------------------|------|------|-------------|
| Zitadel     | 25,000     | Yes       | Official crate+middleware | Yes | Yes  | Apache 2.0  |
| WorkOS      | 1,000,000  | No        | JWT validation (manual) | Yes (FGA) | Yes | Proprietary |
| PropelAuth  | 1,000      | No        | Official axum middleware | Yes | Yes  | Proprietary |
| Ory         | 10,000     | Yes       | Low-quality auto-gen crate | Yes (Keto) | Manual | Apache 2.0 |
| Stack Auth  | 10,000     | Yes (MIT) | No Rust SDK           | Yes  | Yes  | MIT         |
| Clerk       | 10,000     | No        | Community crate       | Yes  | Yes  | Proprietary |
| Hanko       | 10,000     | Yes (AGPL)| No Rust SDK           | No   | No   | AGPL        |
| Unkey       | N/A (keys) | Yes (AGPL)| REST API              | Key-lvl | No | AGPL       |

## Recommendation

### Self-host path: Zitadel (OFF-ROADMAP)
- ~~Free, Apache 2.0, single binary deployment~~
- ~~Official Rust/[[architecture/write-path.md|axum]] crate with middleware~~
- ~~Built-in RBAC, orgs, multi-tenancy, SAML~~
- **Zitadel is no longer on the roadmap.** Hosted TenantKeys serve hosted auth; self-hosted servers are Open.

### Future (TBD)
- RBAC and hosted user accounts: undecided
- If needed later: revisit Zitadel, WorkOS, or Unkey

### API key management: Unkey (complementary, for future)
- Could pair with future user auth for hosted tenant-key lifecycle management
- Key creation, verification, rate limiting

### Skip
- Hanko: no RBAC, no orgs. Auth-only (passkey focused).
- Clerk: expensive at scale, no self-host, no official Rust SDK. Frontend-focused.
- Ory: most powerful but 3 services to manage, worst Rust integration.

## Status
UNDECIDED — revisit when ready to add user accounts. Current Open/TenantKeys model
is sufficient. Zitadel is the leading candidate for least integration effort.
