---
title: Platform Data Correlation — Design
created: 2026-03-28
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'decision', 'deployment']
---

# Platform Data Correlation — Design

## Principle

Sigiro doesn't rebuild what platforms give free ([[designs/future/web-vitals.md|web vitals]], deploy events,
machine metrics). Instead, the AI agent correlates platform data with sigiro data.

## Phase 1 (Now): Agent Correlates

```
AI agent investigating a problem:
  1. sigiro diagnose checkout → traces, logs, anomalies, DB visibility
  2. Vercel API → recent deploys, web vitals scores
  3. Fly API → machine events, CPU/memory metrics
  4. Agent correlates: "deploy v1.2.3 at 14:32 → error spike at 14:33"
```

Sigiro stays pure. Agent is the glue.

Requires: agent prompts/tools that know how to query each platform API
and correlate timestamps with sigiro's [[architecture/investigation.md|InvestigationBlock]].

## Phase 2 (Later): Sigiro Ingests or Queries Platform Data

Two options to evaluate when needed:

**Option A: Query at [[architecture/investigation.md|investigate]] time**
- sigiro stores platform API credentials (Vercel token, Fly token)
- `sigiro diagnose` fetches deploys/events/vitals from APIs on the fly
- [[architecture/investigation.md|InvestigationBlock]] includes platform data OOTB
- Pro: always fresh, no storage
- Con: API latency, rate limits, credential management

**Option C: Continuous ingest**
- sigiro polls platform APIs on a schedule (or receives webhooks)
- Stores deploy events, machine states, vitals in sigiro tables
- [[architecture/investigation.md|investigate]] queries local data only — fast, no external deps
- Pro: fast queries, works offline, richer [[designs/future/anomaly-view.md|anomaly]] correlation
- Con: polling overhead, stale data between polls

Decision on A vs C deferred until we have real users and know which platforms matter.

## What This Means for Roadmap

- [[designs/future/web-vitals.md|Web Vitals]]: NOT building a SDK. Platforms provide this. Agent reads platform APIs.
- Deploy events: NOT building webhook endpoint yet. Agent reads CI/CD or platform APIs.
- Fly machine events: NOT building a poller yet. Agent reads Fly API.
- DB visibility: YES building — this is data only sigiro has (from OTel spans).
- Profiling: YES building — platforms don't provide this.
- [[designs/future/anomaly-view.md|Anomaly]] detection: YES building — platforms don't correlate across signals.
- [[designs/log-template-dedup.md|DRAIN]] log dedup: YES building — platforms don't do this.

## Summary: Build What Platforms Don't Give

  BUILD (sigiro's unique value):        DON'T BUILD (platforms give free):
  ├─ Profiling (OTLP profiles)          ├─ [[designs/future/web-vitals.md|Web Vitals]] (Vercel/Cloudflare)
  ├─ DB visibility (from span attrs)    ├─ Deploy events (CI/CD, platform API)
  ├─ DRAIN [[designs/log-template-dedup.md|log template]] dedup           ├─ Machine metrics (Fly/Vercel/AWS)
  ├─ Relative [[designs/future/anomaly-view.md|anomaly]] detection (3σ)    └─ Basic uptime/health checks
  ├─ N+1 query detection
  ├─ Cross-signal correlation
  └─ WAL (data durability)
