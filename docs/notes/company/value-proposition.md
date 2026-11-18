---
title: Value Proposition
created: 2026-03-21
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'duckdb']
---

# Value Proposition

## Customer Promises

Promises we make to every customer. Non-negotiable.

### 1. Zero management overhead

Observability should not become another thing you manage.

- **Setup**: one DaemonSet, one prompt. Under 5 minutes from nothing to full telemetry.
- **Infrastructure**: no databases to provision, no backends to scale, no plugins to update. S3 is the only dependency you add.
- **Impact on your services**: [[designs/future/ebpf-gpu-position.md|eBPF]] collection means zero SDK integration, zero code changes, zero performance tax on your application. Your services don't know we exist.
- **Ongoing maintenance**: no dashboard JSON to keep in sync, no datasource configs, no retention policies per backend. S3 lifecycle rules handle retention. That's it.

If using us creates work, we failed.

### 2. Cheapest to use

We refuse to be the line item that makes you think twice.

- **Storage**: $0.005/GB (B2) vs $15-25/GB effective at incumbents. That's 3,000-5,000x cheaper on storage alone. (B2 storage [[company/cost.md|cost]] vs SaaS all-in cost including per-host fees, ingest, indexing — but storage is still the dominant cost at scale.)
- **No pre-aggregation tax**: the most expensive operation in observability pipelines doesn't exist here. Compute on read, pay only for what you query.
- **No per-seat [[designs/pricing.md|pricing]]**: your whole team uses it, not just the person who can justify the license.
- **Compression**: 5-10x more aggressive than traditional backends because decompression hides behind LLM inference time. You store less, you pay less.
- **Scaling [[company/cost.md|cost]]**: scales with data volume in S3, not with cluster size or query concurrency. The cost curve stays flat where it matters.

If there's a cheaper way to get full observability, we haven't found it.

## Why This Wins

- No [[designs/future/perses-compat.md|Grafana]], no datasource configs, no plugin maintenance, no dashboard JSON drift
- No Prometheus/Loki/Tempo/Mimir — 4 backends become 1 [[architecture/ducklake-integration.md|DuckLake]] catalog
- S3 lifecycle rules replace per-backend TTL configs
- [[architecture/data-model.md|Parquet]] is open, portable, readable by anything — no vendor lock-in, observability's Parquet moment
- Alerts evaluated locally — no central bottleneck, no remote write lag
- SQL is universal — agents, scripts, notebooks, CLI all speak it
- [[architecture/investigation.md|Investigation]] starts with 1 query returning all signals interleaved, not 4 queries across 4 backends
- No dashboard rot — no stale panels for renamed services
- Onboarding is one DaemonSet, not a monitoring stack
- Agent-native — LLM asks SQL, gets answer, acts
- No correlation tax — current agents make 4-5 queries across different stores then stitch manually. We return it pre-correlated by time

## What We Don't Do

Be honest about these. They're intentional [[architecture/tradeoffs.md|tradeoffs]], not bugs.

- **No Elasticsearch-style log search.** Free-text keyword search (`message LIKE '%connection refused%'`) is a full [[architecture/data-model.md|Parquet]] column scan — no inverted index, no tokenization. At scale this is slow and expensive. Our architecture is built for structured queries (service + time window + [[plans/STATUS.md|status]]), not for typing a keyword and getting results in 200ms. Agents query by structure, not by grep.
- **No real-time tail.** Data lands in [[architecture/data-model.md|DuckDB]] on a 1s flush interval (or 10k rows or 8 MiB, whichever comes first). You can't `tail -f` your logs. If you need sub-second log streaming, use `kubectl logs` or a sidecar.
- **Dashboards are SQL-based.** [[designs/future/perses-compat.md|Perses]] integration via SQL endpoint — dashboard authors write [[architecture/data-model.md|DuckDB]] SQL per panel, not PromQL. No drag-and-drop chart builder. Works but requires SQL fluency. See [[company/plan.md|plan]] 23.
- **No alerting (yet).** We don't evaluate alert rules. Planned but not built. For now, use existing alerting (Prometheus alertmanager, PagerDuty, etc.) alongside us.
