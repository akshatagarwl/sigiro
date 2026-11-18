---
title: Observe vs ClickStack — Architecture Comparison
created: 2026-03-06
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'comparison', 'competitor', 'data-model', 'deployment']
---

# Observe vs ClickStack — Architecture Comparison

ClickStack is ClickHouse Inc.'s entry into managed observability, announced February 2026. It's an observability platform built on ClickHouse Cloud that ingests logs, metrics, traces, and session replays. Available as both open-source self-managed and Managed ClickStack (beta) on ClickHouse Cloud. The UI layer is HyperDX.

This is the most architecturally credible competitor we've seen. ClickHouse is the engine behind SigNoz, HyperDX, Gigapipe/qryn — ClickStack is the database vendor cutting out the middleware and going direct. Same dynamic that happened when AWS launched OpenSearch to compete with Elastic's customers.

## Data Model

| Dimension | Observe | ClickStack |
|-----------|---------|------------|
| **Table topology** | One unified Parquet table, all signals interleaved | Separate tables per signal type (OTel schema per signal) |
| **Schema** | Fixed 18-column schema, `signal` column discriminates type | OpenTelemetry-native schemas — separate optimized table per signal |
| **Cross-signal correlation** | Native — same table, same sort order, one query | Requires joining across separate tables |
| **Storage format** | Parquet on S3 (DuckDB reads directly) | ClickHouse MergeTree on object storage with distributed caching |
| **Schema evolution** | No evolution. Schema is frozen | ClickHouse ALTER TABLE — mature schema evolution |

**Why it matters:** ClickStack's separate-table-per-signal model is standard ClickHouse observability architecture (same as SigNoz, Jaeger-ClickHouse, etc.). Cross-signal correlation requires joins. Observe's unified table stores all signals contiguously — one query, no joins, one sort order.

**Tradeoff:** ClickStack gets ClickHouse's mature columnar engine with specialized table engines per signal type. Separate tables mean each signal's schema is optimized for its access patterns. Our unified table pays nullable-column overhead for absent fields, but gets free cross-signal correlation.

## Write Path

| Dimension | Observe | ClickStack |
|-----------|---------|------------|
| **Ingestion** | OTel Collector with custom exporter | ClickHouse Cloud native ingest (HTTP, gRPC, OTel, Vector) |
| **Throughput claim** | Not benchmarked yet | "Up to 20 MB/s per ingest core" |
| **Buffering** | In-memory buffer, 15s flush to S3 | ClickHouse buffer tables + MergeTree async merge |
| **Crash recovery** | OTel `file_storage` extension (bounded 15s loss) | ClickHouse replication (near-zero loss) |
| **Compaction** | Not yet built | ClickHouse background merges (mature, automatic) |
| **Partitioning** | Epoch-hour flat directories on S3 | ClickHouse automatic partitioning with MergeTree |

**Key difference:** ClickStack inherits ClickHouse's production-grade [[architecture/write-path.md|write path]] — buffer tables, async merges, automatic compaction, replication. This is years ahead of our simple buffer→flush→S3 pipeline. Their "20 MB/s per core" claim suggests they've benchmarked heavily against observability workloads.

## Query Engine

| Dimension | Observe | ClickStack |
|-----------|---------|------------|
| **Engine** | Embedded DuckDB (ephemeral, per-query) | ClickHouse Cloud (distributed, managed) |
| **Query language** | Raw SQL over Parquet | SQL (ClickHouse dialect) |
| **Distribution** | Single-node | Distributed — separation of storage and compute |
| **Scaling** | Vertical only (single DuckDB instance) | Independent scaling of ingest and query compute |
| **Caching** | None (cold reads every time) | "Intelligent local and distributed caching" driven by access patterns |
| **Compression** | zstd level ~7, 6-10x | "10-15x compression on observability data" |
| **Response format** | Structured investigation block for LLM agents | Standard query results for HyperDX dashboard rendering |

**Key difference:** ClickStack gets ClickHouse Cloud's separation of storage and compute — independently scalable ingest and query pools, distributed caching, and a query engine battle-tested at petabyte scale. Our embedded [[architecture/data-model.md|DuckDB]] is simpler to operate (zero infrastructure) but can't horizontally scale query throughput.

**Their compression claim of 10-15x is notable.** ClickHouse's native columnar format with codecs (Delta, DoubleDelta, Gorilla for metrics, LZ4/ZSTD for strings) genuinely achieves higher compression than [[architecture/data-model.md|Parquet]]+zstd on structured observability data. This narrows our [[company/cost.md|cost]] advantage — their effective storage cost per GB may be comparable to ours despite higher list prices.

## Storage Architecture

| Dimension | Observe | ClickStack |
|-----------|---------|------------|
| **Storage tier** | S3 only — no hot/cold distinction | Object storage primary, with intelligent caching layer |
| **Retention model** | Flat $0.005/GB, no tier distinction | "No hot/cold tiers — data treated consistently regardless of age" |
| **Historical query perf** | Same as recent (S3 reads, 500ms-1.3s) | Same as recent (cached based on access patterns, not recency) |
| **File format** | Parquet (open, portable) | ClickHouse native format (proprietary, optimized) |

**Important convergence:** Both platforms reject the hot/cold tier model that plagues traditional observability. ClickStack's "no forced expiration, no sampling, no rollups" messaging is almost identical to ours. This validates the architectural direction — the industry is moving toward full-fidelity retention on cheap storage.

**Divergence:** Our [[architecture/data-model.md|Parquet]] files are portable — any tool that reads Parquet ([[architecture/data-model.md|DuckDB]], Spark, Pandas, Polars) can query our data. ClickStack data lives in ClickHouse's native format. Vendor lock-in is real: migrating out of ClickHouse Cloud means exporting data, not just pointing a new engine at the same files.

## Pricing

| Dimension | Observe | ClickStack |
|-----------|---------|------------|
| **Storage** | $0.005/GB ingested (includes compute) | < $0.03/GB/month retention |
| **Ingest compute** | $0 (runs on existing K8s nodes) | < $0.01/GB/month |
| **Query compute** | $0 (embedded DuckDB) | Pay-per-use (independently scalable) |
| **Per-host / per-user** | $0 | $0 |
| **Pricing model** | Single rate, all-inclusive | Three-axis: storage + ingest compute + query compute |

### Cost Comparison at Scale

ClickStack's three-axis [[designs/pricing.md|pricing]] makes exact comparison harder, but we can estimate:

| Scale | Observe | ClickStack (estimated) | Ratio |
|-------|---------|----------------------|-------|
| **10 GB/day** | $6/mo | ~$12-18/mo (storage $9 + ingest $3 + query variable) | 2-3x cheaper |
| **100 GB/day** | $60/mo | ~$120-180/mo | 2-3x cheaper |
| **1 TB/day** | $600/mo | ~$1,200-1,800/mo | 2-3x cheaper |

**Estimates assume:** 30-day retention, 10-15x compression on ClickStack side. Their "$0.03/GB storage" is on compressed data, so 100 GB/day × 30 days = 3 TB raw → ~200-300 GB compressed × $0.03 = $6-9/mo storage. Ingest at $0.01/GB × 3 TB = $30. Query compute is usage-based and hard to estimate.

**The gap narrows dramatically.** Against SigNoz ($949/mo at 100 GB/day) or Datadog ($4,300/mo), we're 15-70x cheaper. Against ClickStack, we're only 2-3x cheaper. ClickStack's 10-15x compression and infrastructure-based [[designs/pricing.md|pricing]] (not per-GB-ingested markup) mean they've eliminated most of the margin that budget-tier competitors charge.

### Why the Gap is Smaller

| Factor | Budget Tier ($0.30/GB) | ClickStack | Observe |
|--------|----------------------|------------|---------|
| **Markup over infra cost** | 10-15x | 2-4x (estimated) | 2-4x |
| **Compression** | 2-5x typical | 10-15x (ClickHouse codecs) | 6-10x (Parquet+zstd) |
| **Query infrastructure** | Included in per-GB fee | Pay-per-use (transparent) | Zero (embedded DuckDB) |
| **Dashboard infrastructure** | Included | Included (HyperDX) | None (agent is the UI) |

ClickStack competes on infrastructure [[company/cost.md|cost]], not on markup. They're [[designs/pricing.md|pricing]] at near-cost because they want ClickHouse Cloud adoption — observability is the wedge.

## Signals Coverage

| Signal | Observe | ClickStack |
|--------|---------|------------|
| Metrics | Yes (unified table) | Yes (OTel schema) |
| Logs | Yes (unified table) | Yes (OTel schema) |
| Traces | Yes (unified table, span trees assembled in Go) | Yes (OTel schema) |
| Profiles | Yes (unified table, stacktrace column) | No |
| Session Replays | Not planned | Yes |
| RUM | Planned — `signal='rum'` | Via session replays |
| Alerts | Planned — agent-native (LLM reasoning) | Not mentioned (beta) |
| Dashboards | Not planned (agent is the UI) | Yes (HyperDX) |

**Profile support is our edge here.** ClickStack doesn't mention [[architecture/profiles.md|profiles]]. Our OTLP and Pyroscope ingest paths write profiles to per-signal [[architecture/ducklake-integration.md|DuckLake]] tables. Profiling data correlated with traces, logs, and metrics in the same query is something no ClickHouse-based platform offers today.

## Infrastructure Requirements

| Dimension | Observe | ClickStack Managed |
|-----------|---------|-------------------|
| **Operator burden** | Deploy OTel Collector DaemonSet, point at S3 | Zero — fully managed |
| **Scaling** | Manual (add collector nodes) | Automatic (ClickHouse Cloud scales compute) |
| **HA** | Multiple collectors → same S3 bucket | Managed by ClickHouse Cloud |
| **Maintenance** | S3 lifecycle policies for retention | Managed |
| **External dependencies** | S3 only | ClickHouse Cloud (vendor dependency) |

**ClickStack Managed wins on operational simplicity.** Zero infrastructure to manage — it's a fully managed service. Our advantage is zero vendor lock-in: data is in open [[architecture/data-model.md|Parquet]] format on your S3 bucket, queryable by any Parquet-compatible engine.

## The Real Threat Assessment

### Why ClickStack is Dangerous

1. **Database vendor going direct.** ClickHouse powers SigNoz, HyperDX, Gigapipe, and most budget-tier observability tools. Now the database vendor is offering the full stack — same engine, managed, with a UI. This undercuts their own ecosystem.

2. **Compression advantage is real.** 10-15x vs our 6-10x. ClickHouse's specialized codecs (DoubleDelta for timestamps, Gorilla for floats) genuinely outperform generic [[architecture/data-model.md|Parquet]]+zstd on observability data. This narrows the [[company/cost.md|cost]] gap.

3. **Infrastructure-based [[designs/pricing.md|pricing]].** Unlike SigNoz/[[research/observe-vs-openobserve.md|OpenObserve]] charging $0.30/GB on $0.001/GB infrastructure, ClickStack prices close to [[company/cost.md|cost]]. Our 60-100x cost advantage over budget tier shrinks to 8-12x against ClickStack.

4. **Separation of storage and compute.** Same architectural insight we have (cheap object storage, compute on read), but implemented with a mature distributed query engine instead of embedded [[architecture/data-model.md|DuckDB]].

5. **OTel-native.** They speak OTel natively, no custom exporters needed. Lower adoption friction.

### Why ClickStack is Not a Threat to Us

1. **Dashboard-first vs agent-first.** ClickStack ships with HyperDX — it's a dashboard product. Humans browse panels, search logs, click through traces. We have no dashboards. Our consumer is an LLM agent that gets structured [[architecture/investigation.md|investigation]] blocks. These are different products for different workflows.

2. **Vendor lock-in.** Data in ClickHouse native format on ClickHouse Cloud. Can't point [[architecture/data-model.md|DuckDB]]/Spark/Polars at it. Our [[architecture/data-model.md|Parquet]]-on-S3 is portable — any engine reads it, zero migration [[company/cost.md|cost]].

3. **Query [[company/cost.md|cost]] at scale.** ClickStack's pay-per-use query compute means costs scale with query volume. Dashboard users generate 300+ queries/minute. Our agent generates 1-2 queries per incident. Even if per-query cost is higher (S3 cold reads), total query cost is dramatically lower because we make 100-1000x fewer queries.

4. **No [[architecture/profiles.md|profiles]].** ClickStack has logs, metrics, traces, session replays — but no continuous profiling. Profiles correlated with other signals in Sigiro are a differentiation they can't match without building or acquiring a profiling stack.

5. **Three-axis [[designs/pricing.md|pricing]] complexity.** Storage + ingest compute + query compute. Hard to predict bills. Our single $0.005/GB rate is simpler. ClickStack's query compute costs can spike unpredictably with heavy dashboard usage.

6. **The autonomous SRE thesis.** ClickStack is a better mousetrap for the same paradigm — humans monitoring dashboards. We're building for the paradigm shift: AI agents that [[architecture/investigation.md|investigate]] incidents autonomously. If that thesis is right, ClickStack is optimizing for the past.

## What ClickStack Means for the Budget Tier

ClickStack is an existential threat to SigNoz, HyperDX, Gigapipe, and [[research/observe-vs-openobserve.md|OpenObserve]] — not to us. Here's why:

| Budget Tier Tool | ClickHouse Dependency | ClickStack Impact |
|-----------------|----------------------|-------------------|
| **SigNoz** | Self-managed ClickHouse | ClickStack offers same engine, managed, cheaper |
| **HyperDX** | Self-managed ClickHouse | ClickStack literally acquired HyperDX for the UI |
| **Gigapipe/qryn** | Self-managed ClickHouse | ClickStack offers same compatibility, managed |
| **Observe** | No ClickHouse dependency | Different architecture, different consumer (agent vs human) |

The budget-tier observability market is about to get squeezed from above (ClickStack managed) and below (observe's $0.005/GB). The tools in between — charging $0.30/GB for a ClickHouse wrapper with a UI — have less room to exist when the database vendor offers the same thing managed.

## Summary

ClickStack is the most credible competitor — validates our architectural bets while remaining dashboard-first. The [[company/cost.md|cost]] gap narrows to 2-3x (vs 60x against budget tier).

> For Sigiro's positioning, see [[[research/competitive-landscape-2026.md|competitive-landscape-2026]].md](competitive-landscape-2026.md).
