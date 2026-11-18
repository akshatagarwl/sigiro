---
title: Perses/Grafana Protocol Compatibility — Design
created: 2026-03-28
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'design', 'investigation', 'logs']
---

# Perses/Grafana Protocol Compatibility — Design

## Principle

Instead of building custom flamegraph/log/trace viewer endpoints, implement
compatibility with the protocols Perses (and Grafana) already understand.
Sigiro becomes a drop-in backend for Prometheus, Tempo, Loki, and [[architecture/profiles.md|Pyroscope]].

## What Perses Supports (as of v0.53)

| Signal    | Datasource | Query Language | Panels                              |
|-----------|------------|----------------|-------------------------------------|
| Metrics   | Prometheus | PromQL         | Time series, gauge, stat, bar, table |
| Traces    | Tempo      | TraceQL        | Scatter, Gantt/waterfall, table     |
| Logs      | Loki       | LogQL          | Log viewer, search, filter          |
| Profiles  | Pyroscope  | label matchers | Flamegraphs (interactive)           |

## Endpoints to Implement

### Prometheus compat (~200 lines)

```
/prom/api/v1/query          — instant query
/prom/api/v1/query_range    — range query
/prom/api/v1/labels         — label names
/prom/api/v1/label/<name>/values  — label values
/prom/api/v1/series         — series matching
```

Perses config: Prometheus datasource → URL: `http://sigiro:9999/prom`

Crate: `promql-parser` (v0.4.3, by [[research/observe-vs-greptimedb.md|GreptimeDB]], production-grade)
- Parse PromQL → AST
- Walk AST → SQL against sigiro_metrics_* tables
- Return Prometheus JSON response format

Don't need full PromQL — subset that Perses dashboards use:
rate, sum, avg, quantile, label matchers, range vectors.

Reference: [[research/observe-vs-greptimedb.md|GreptimeDB]] source for AST → SQL translation.

### Tempo compat (~150 lines)

```
/tempo/api/traces/<traceID>   — get full trace by ID
/tempo/api/search             — search traces (service, duration, status)
/tempo/api/v2/search/tags     — available tag names
/tempo/api/v2/search/tag/<name>/values  — tag values
```

Perses config: Tempo datasource → URL: `http://sigiro:9999/tempo`

Crate: `traceql` (v0.0.2) or `traceql_parser` (v0.0.2)
- Parse TraceQL → AST
- Walk AST → SQL against sigiro_spans
- Return [[architecture/write-path.md|OTLP]] JSON trace format (Tempo's native response)

GET /api/traces/<id> is trivial: WHERE trace_id = ? → [[architecture/write-path.md|OTLP]] JSON.

### Loki compat (~150 lines)

```
/loki/api/v1/query_range   — log query with time range
/loki/api/v1/query         — instant log query
/loki/api/v1/labels        — label names
/loki/api/v1/label/<name>/values  — label values
```

Perses config: Loki datasource → URL: `http://sigiro:9999/loki`

Crate: `logql` (v0.3.0) — parser + AST, or `logql_parser` (v0.0.4)
- Parse LogQL → AST
- Walk AST → SQL against sigiro_logs
- Return Loki streams response format

### Pyroscope compat (~100 lines)

```
/pyroscope/api/v1/query           — profile query
/pyroscope/api/v1/labels          — label names
/pyroscope/api/v1/label-values    — label values
```

Perses config: [[architecture/profiles.md|Pyroscope]] datasource → URL: `http://sigiro:9999/pyroscope`

No parser crate exists. [[architecture/profiles.md|Pyroscope]] query API is simple:
`process_cpu:cpu{service_name="X"}` — just label matchers on profile series.
Parse by hand (~30 lines).

Depends on: OTLP [[architecture/profiles.md|Profiles]] ingestion (Phase 1 design, not yet built).

## Crate Dependencies

```toml
promql-parser = "0.4"     # PromQL AST (GreptimeDB, production-grade)
logql = "0.3"             # LogQL AST
traceql = "0.0.2"         # TraceQL AST (early but typed)
# loki-api = "0.1"        # Optional: Loki gRPC protobuf bindings
```

## What This Gets You

1. Perses dashboards work out of the box (all 4 signals)
2. Grafana dashboards also work (same protocols)
3. Any tool that speaks Prometheus/Tempo/Loki/[[architecture/profiles.md|Pyroscope]] HTTP API works
4. No custom UI code needed
5. Flamegraphs via [[architecture/profiles.md|Pyroscope]] panel in Perses (replaces custom d3-flame-graph)
6. Log viewer via Loki panel in Perses (no custom log UI needed)

## What This Does NOT Do

- Not a full PromQL/LogQL/TraceQL engine — subset that covers common queries
- Not a replacement for `sigiro diagnose` — agent-first is still primary
- Perses is optional — Sigiro works fine without any dashboard

## Effort

~600 lines of Rust. Four [[architecture/write-path.md|axum]] route groups. Three crate deps.
Each endpoint: parse query → SQL → format response.

## Status

PROPOSED — saved for Phase 3. Implementation details TBD.
Parser crates exist but AST → SQL translation is Sigiro-specific.
[[architecture/profiles.md|Pyroscope]] compat blocked on OTLP Profiles ingestion.
