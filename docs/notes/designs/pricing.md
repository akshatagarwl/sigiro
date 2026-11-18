---
title: Pricing — Design (Future)
created: 2026-03-30
updated: 2026-05-17
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'design']
---

# Pricing — Design (Future)

> 🚧 **Not Yet Implemented**
>
> Sigiro is currently **free for all usage**. There is no billing system, no paid tiers,
> and no usage limits. This doc captures the pricing model we may implement in the future.

## Current Status

- **Self-hosted**: Free, unlimited data, unlimited retention
- **Hosted (if available)**: Free, no usage limits
- **Billing system**: Not implemented

## Future Pricing Model (Proposed)

When billing is implemented, the tentative model is:

| Tier | Price | Includes |
|------|-------|----------|
| Free | $0 | 10 GB/mo, 30-day retention, 1 project |
| Pro | $5/mo | 100 GB/mo, 90-day retention, 5 projects |
| Team | $15/mo | 500 GB/mo, 1-year retention, unlimited projects |

## Competitor Pricing Reference

| Platform | $/GB | Free tier | Model |
|----------|------|-----------|-------|
| Datadog | $2.50-3.75 effective | None meaningful | Host + per-GB + per-signal |
| New Relic | $0.35 | 100 GB/mo | Per-GB + user seats |
| Grafana Cloud | $0.50 | 50 GB per signal | Per-GB per signal |
| SigNoz Cloud | $0.30 | — | Per-GB |
| HyperDX | $0.20 | 2 GB/day | Per-GB |

## Cost Structure

Sigiro's infrastructure cost at scale (self-hosted on B2):

| Scale | Storage | Total |
|-------|---------|-------|
| 10 GB/day | 300 GB compressed | ~$2.70/mo |
| 100 GB/day | 3 TB compressed | ~$29.70/mo |

See [[company/cost.md|cost.md]] for full competitor comparison and cost breakdown.

## Status

**NOT IMPLEMENTED** — revisit when a hosted multi-tenant offering is planned.
