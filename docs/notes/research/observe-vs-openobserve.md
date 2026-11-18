---
title: Observe vs OpenObserve — Architecture Comparison
created: 2026-03-05
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'comparison', 'competitor', 'data-model', 'deployment']
---

# Observe vs OpenObserve — Architecture Comparison

Both projects store observability data in [[architecture/data-model.md|Parquet]] on object storage. That's where the similarity ends. The design goals diverge fundamentally: OpenObserve is a **dashboard-first platform** replacing Elasticsearch/Datadog for human operators. Observe is an **agent-first query engine** where the primary consumer is an LLM, not a human clicking through UI panels.

## Data Model

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Table topology** | Per-signal tables (spans, logs, 4× metrics, profiles), each sorted by (service, timestamp) | Separate streams per org/signal type (logs, metrics, traces) |
| **Schema** | Per-signal schemas matching otlp2records output exactly — no column renames, no type discrimination needed | Dynamic schema per stream, schema evolution via lock-based field addition |
| **Cross-signal correlation** | UNION ALL across per-signal tables at query time — agent sees one cross-signal view | Requires joining across separate streams at query time |
| **Schema evolution** | New signal types = new DuckLake table with the appropriate schema | Automatic — new fields discovered at ingest time trigger schema update with locking |

**Why it matters:** OpenObserve's separate-stream model means correlating a log line with its originating span requires joining two separate [[architecture/data-model.md|Parquet]] file sets. Sigiro's per-signal tables are all sorted by `(service, timestamp)`, so `sigiro_investigate` UNION ALLs them in one query — the agent sees a cross-signal view without manual stitching.

**Tradeoff:** OpenObserve's dynamic schema handles arbitrary user fields natively. Sigiro uses typed columns for known fields and JSON columns (`resource_attributes`, `span_attributes`, etc.) for the rest — simpler schemas, but no columnar filtering on ad-hoc JSON fields.

## Write Path

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Ingestion** | OTel Collector with custom exporter | Built-in HTTP/gRPC API (Rust) |
| **Buffering** | In-memory buffer, 15s flush to S3 | WAL + Memtable (Arrow RecordBatch), 5s disk dump, 10s upload cycle |
| **Crash recovery** | OTel `file_storage` extension (bounded 15s loss) | WAL replay (near-zero loss) |
| **File sizing** | One file per node per flush (small files, needs future compaction) | Compactor merges small files into 256 MB chunks before upload |
| **Partitioning** | Epoch-hour flat directories (`s3://{bucket}/{epoch_seconds}/*.parquet`) | Hourly buckets per org/stream, hive-style |
| **Intermediate format** | Go structs → Parquet (parquet-go, no CGO) | Arrow RecordBatch → Parquet (Rust arrow2/arrow-rs) |

**Key difference:** OpenObserve has a mature [[architecture/write-path.md|write path]] with WAL, compactor, and file merging built in. Observe's write path is simpler (buffer → flush → S3) but relies on a not-yet-built compactor for production file management. OpenObserve's Rust-based ingestion pipeline also likely has better single-node throughput.

## Indexing & Search

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Full-text search** | None. `WHERE message LIKE '%x%'` = full scan | Built-in full-text search + secondary index functions |
| **Trace lookup** | Bloom filter on `trace_id` (load-bearing, 300-800ms vs 1-3s without) | File list index in MySQL/PostgreSQL for partition pruning |
| **Metadata index** | Parquet min/max stats + sort order for row-group pruning | File list catalog in relational DB + optional memory cache |
| **Inverted index** | Not planned | Supported (full-text search is a first-class feature) |

**Why it matters:** OpenObserve is built for humans typing search queries into a UI. Full-text search on log messages is essential. Observe deliberately skips this — an LLM agent doesn't grep logs by keyword, it asks structured questions (`service=checkout, signal=log, time=14:00-14:25`). [[architecture/data-model.md|Parquet]] sort order + bloom filters are sufficient for structured access patterns.

## Query Engine

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Engine** | Embedded DuckDB (ephemeral, per-query) | Custom distributed query engine (Rust) |
| **Query language** | Raw SQL over Parquet | SQL for logs/traces, PromQL for metrics |
| **Distribution** | Single-node (DuckDB does the work) | Leader/worker fan-out across querier nodes |
| **Statefulness** | Fully stateless — no persistent indexes, zero recovery | Stateless queriers, but metadata DB required |
| **Caching** | None (cold reads every time) | Parquet file cache in memory (default 50% of RAM) |
| **Response format** | Structured investigation block (metrics + spans + logs + profiles in one response) | Standard query results per stream |

**Key difference:** OpenObserve built a distributed query engine from scratch in Rust with leader/worker coordination, query caching, and PromQL support. Observe embeds [[architecture/data-model.md|DuckDB]] and lets it handle [[architecture/data-model.md|Parquet]]-over-S3 reads directly — zero query infrastructure to operate, but limited to single-node query performance.

**Response shape is the real differentiator:** Observe returns a single structured [[architecture/investigation.md|investigation]] block (4 parallel signal queries + linked traces) designed for LLM consumption. OpenObserve returns standard per-stream results designed for dashboard rendering.

## Query Pattern

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Primary consumer** | LLM agent | Human via UI/API |
| **Typical query** | 1 API call → 4 parallel sub-queries → structured block | Many small queries per dashboard panel |
| **Concurrency model** | 1-2 queries per incident (agent) | Dozens of concurrent queries (dashboard refresh) |
| **Latency target** | `<1.5s` total (fits in LLM reasoning gap) | Sub-second per panel |
| **Pre-aggregation** | None — compute on read | Likely needed for dashboard-scale refresh rates |

**Why it matters:** OpenObserve must handle 50 panels × 10-second refresh = 300 queries/minute for a single dashboard user. This demands caching, pre-aggregation, and distributed query execution. Observe handles 1-2 queries per incident — raw [[architecture/data-model.md|Parquet]] scans are fast enough.

## Infrastructure Requirements

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Minimum deployment** | 1 binary (collector + query server) + S3 | 1 binary (single-node mode) + local disk |
| **HA deployment** | Multiple collectors writing to same S3 prefix | Router + Ingester + Querier + Compactor + AlertManager + NATS + MySQL/PostgreSQL + S3 |
| **Metadata store** | None | MySQL or PostgreSQL (required) |
| **Coordination** | None — collectors are independent | NATS for cluster coordination |
| **External dependencies** | S3 only | S3 + relational DB + NATS |

**Key difference:** OpenObserve HA requires 5+ component types plus NATS plus a relational database. Observe's HA is just "run more collectors pointing at the same S3 bucket" — no coordination, no metadata store, no cluster management.

## Compression & Storage Economics

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Compression** | zstd level ~7 + dictionary encoding on low-cardinality columns | Parquet default (likely zstd, level unspecified) |
| **Target ratio** | 6-10x (aggressive, trades CPU for storage) | "140x lower storage costs" vs Elasticsearch (different baseline) |
| **Pricing model** | $0.005/GB ingested, no other fees | $0.30/GB ingested (cloud) or self-hosted |
| **Storage layout** | Unified table — nullable columns cost nothing in Parquet for absent signals | Separate streams — no wasted columns, but more files to manage |

## Signals Coverage

| Signal | Observe | OpenObserve |
|--------|---------|-------------|
| Metrics | Yes (unified table) | Yes (separate stream, PromQL) |
| Logs | Yes (unified table) | Yes (separate stream, full-text search) |
| Traces | Yes (unified table, span trees assembled in Go) | Yes (separate stream) |
| Profiles | Yes (unified table, stacktrace column) | No |
| RUM | Planned — `signal='rum'`, same unified table | Yes (frontend monitoring) |
| Alerts | Planned — agent-native (LLM reasoning replaces static thresholds) | Yes (built-in alert manager) |
| Dashboards | Not planned (agent is the UI) | Yes (built-in) |

## Technology

| Dimension | Observe | OpenObserve |
|-----------|---------|-------------|
| **Language** | Go | Rust |
| **Parquet library** | parquet-go (pure Go, no CGO) | arrow2/arrow-rs (Rust native) |
| **Query engine** | DuckDB (embedded) | Custom (Rust, distributed) |
| **Collector** | OpenTelemetry Collector (custom exporter) | Built-in ingestion |
| **API** | gRPC (`POST /v1/query`) | HTTP REST + gRPC |

## Why OpenObserve Costs 15x More

Both use [[architecture/data-model.md|Parquet]] on S3 — per-GB storage [[company/cost.md|cost]] is roughly identical. The price gap comes from infrastructure OpenObserve must run to serve dashboard interaction patterns:

| Infrastructure | OpenObserve | Observe | Why they need it |
|---|---|---|---|
| **Querier fleet** (50% RAM cache each) | Yes — large instances | No — embedded DuckDB | Dashboards: 50 panels × 10s refresh = 300 queries/min per user |
| **Metadata DB** (MySQL/PostgreSQL) | Yes — managed database | No | File list index, schemas, orgs, alert rules |
| **NATS cluster** | Yes | No | HA coordination between 5+ component types |
| **Compactor nodes** | Yes — continuous | No (future: periodic job) | Merge small Parquet files into 256MB chunks |
| **Alert manager** | Yes — evaluates at ingest time | No | Static threshold alerting |
| **Full-text indexing** | Yes — CPU at ingest + index storage | No | Humans grep logs by keyword |

**Observe's $0.005/GB** pays for B2 storage + compute-on-read with [[architecture/data-model.md|DuckDB]]. No fleet, no coordination, no metadata DB.

**OpenObserve's $0.30/GB** pays for all of the above infrastructure that exists to serve human dashboard interaction patterns — patterns we've eliminated by making the LLM agent the primary consumer.

## How OpenObserve Achieves Fast Queries

Three techniques, all expensive:

### 1. Aggressive memory caching
Querier nodes cache [[architecture/data-model.md|Parquet]] files in RAM (default 50% of available memory). Hot queries hit memory, not S3. This requires large instances per querier node. Our queries always read from S3 (cold), targeting 500ms-1.3s instead of sub-100ms — acceptable because the LLM's reasoning step takes 2-10s anyway.

### 2. File list index in a relational DB
Every [[architecture/data-model.md|Parquet]] file's location, partition, and min/max stats are cataloged in MySQL/PostgreSQL. The leader querier consults this index to know exactly which files to read — no glob/list operations against S3. We use `ParquetGlobs()` to enumerate one glob per hour (with signal Hive partitioning) against S3, which is slower but requires zero external state.

### 3. Distributed fan-out
The leader querier partitions the file list across worker queriers via gRPC. Each worker reads its subset in parallel, results merge at the leader. More querier nodes = more parallelism = faster queries. We're single-node [[architecture/data-model.md|DuckDB]] — fast enough for 1-2 agent queries per incident, but can't horizontally scale query throughput the same way.

### Why we don't need these

| Their technique | Cost | Why we skip it |
|---|---|---|
| Memory cache (50% RAM per querier) | Large instance fleet | Agent queries 1-2x per incident, not 300x/min per dashboard user |
| File list index (MySQL/PostgreSQL) | Managed DB | S3 glob per hour-partition is fast enough for narrow time windows |
| Distributed fan-out (leader/worker) | N querier nodes + NATS | DuckDB handles 25-minute service windows in `<1.5s` single-node |

## Future: RUM and Alerts

### RUM (Real User Monitoring)

OpenObserve has RUM built in. We'll need it too — browser/mobile performance data is a signal type like any other. The unified table approach extends naturally:

- `signal='rum'` — new enum value, same columns
- `name` = metric name (e.g., `page.load`, `lcp`, `fid`, `cls`)
- `value` = metric value (milliseconds, score)
- `service` = frontend app name
- `trace_id` = links browser spans to backend traces (existing column)
- `metadata` = user agent, page URL, geo, session ID
- No schema change needed — RUM events are rows with `signal='rum'`

The agent already gets cross-signal correlation for free: a slow `checkout` service response correlates with high `lcp` on the checkout page because both share `trace_id` and `service` context.

### Alerts

OpenObserve evaluates alert conditions at ingest time with a dedicated AlertManager component. Our approach will be fundamentally different:

**Agent-native alerting** — the LLM agent replaces static threshold rules:

- **No static thresholds.** Instead of `IF error_rate > 5% THEN page`, the agent periodically queries `/v1/query` for each service and reasons about whether the current state is anomalous given context (deploy version, time of day, historical patterns).
- **No alert fatigue.** The agent decides what's worth escalating vs. what's expected (e.g., higher error rate during a known deploy rollout).
- **Cross-signal reasoning.** A latency spike alone isn't alarming. A latency spike + error rate increase + memory climb + new deploy version = page the on-call.
- **Implementation:** Background agent loop — query each service every N minutes, compare against recent baseline, escalate via webhook/Slack/PagerDuty when warranted.

This is cheaper than OpenObserve's approach (no AlertManager component, no rule engine, no rule storage) and more effective (contextual reasoning vs. static thresholds).

**Fallback:** For compliance/contractual requirements where customers need deterministic alerting rules (not LLM judgment), we may add simple threshold alerts evaluated at query time — but this is a concession, not the primary design.

## Summary

Different tools for different jobs. OpenObserve is a complete observability platform for human operators. Sigiro is a minimal query engine for LLM agents.

> For Sigiro's positioning, see [[[research/competitive-landscape-2026.md|competitive-landscape-2026]].md](competitive-landscape-2026.md).
