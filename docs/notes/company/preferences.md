---
title: Building Preferences
created: 2026-03-01
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'data-model', 'duckdb', 'ducklake', 'investigation']
---

## Building Preferences

- JS/TS code: always use Effect (https://effect.website/) for error handling, concurrency, data validation, and async workflows — no raw try/catch, no hand-rolled Result types, no bare Promise chains
- CLI-first, no dashboards, no [[designs/future/perses-compat.md|Grafana]]
- No PromQL — SQL only
- Alerts fire at the edge (collector), not from a central backend
- Alert payload carries enough context to begin [[architecture/investigation.md|investigation]] without querying
- [[architecture/investigation.md|Investigation]] via ad-hoc queries, not pre-built panels
- Canned queries / runbooks over dashboard layouts
- [[architecture/data-model.md|DuckDB]] for POC query layer, not Trino
- 1m batch window, background compaction job merges to hourly files
- [[architecture/data-model.md|Parquet]] + S3, no Loki/Tempo/Mimir/Prometheus

- Single unified table, all signals interleaved — no separate tables per signal type
- Single query path: WHERE timestamp BETWEEN t1 AND t2
- No pre-aggregation — compute on read, agent hides the latency behind LLM reasoning
- Aggressive compression (zstd high levels) — decompression [[company/cost.md|cost]] hidden behind LLM inference
- Throughput over latency — 500ms scan is fine, agent doesn't care
- Scale test early: simulate 100 nodes × 7 days, verify query <5s
