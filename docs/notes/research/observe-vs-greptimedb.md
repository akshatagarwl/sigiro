---
title: Observe vs GreptimeDB
created: 2026-03-10
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'comparison', 'competitor', 'data-model', 'deployment']
---

# Observe vs GreptimeDB

**Date:** 2026-03-10
**[[plans/STATUS.md|Status]]:** Research complete

## What is GreptimeDB

Rust-based, distributed, cloud-native time-series database positioning as an "Observability 2.0" engine — one database replacing Prometheus + Loki + Elasticsearch. Apache 2.0 licensed, ~6K GitHub stars, ~19 employees, v1.0 GA shipped January 2026. Backed by Glory Ventures + Picus Capital.

Four components: **Frontend** (stateless query router, DataFusion planner), **Datanode** (stateful, holds data regions), **Metasrv** (coordinator, 3-node HA, leader election via etcd/Postgres/MySQL), **Flownode** (continuous aggregation / streaming ETL). Also runs as a single standalone binary.

## Head-to-Head Comparison

| Dimension | Observe | GreptimeDB | Analysis |
|-----------|---------|------------|----------|
| **Table model** | Per-signal Parquet tables (spans, logs, gauge/sum/histogram/exp_histogram metrics, profiles), each sorted by (service, timestamp) | Separate tables per signal type, per metric name even. "Unified" = one engine, not one table | We both split by signal type — but our metrics tables match otlp2records schemas exactly for zero-copy ingest |
| **Schema** | Per-signal schemas matching otlp2records output — no metadata blob, no frozen 18-column schema | Dynamic schema, auto-created columns on ingest. Tag/Field/Timestamp semantic types | Their flexibility is a strength for ad-hoc exploration. Our per-type schema is a strength for agent queries (predictable, no schema drift) |
| **Storage format** | Raw Parquet files on S3, written directly by exporter | LSM-tree engine (WAL -> memtable -> Parquet SSTs -> compaction -> object storage) | They get mutable data (MVCC updates/deletes), compaction, indexes. We get radical simplicity — zero moving parts |
| **Query engine** | Embedded DuckDB, single process, reads Parquet from S3 | DataFusion (Rust), distributed execution across Datanodes, Arrow Flight streaming | They scale horizontally. We scale by keeping queries narrow (5-60 min windows, 1-2 per incident) |
| **Compression** | zstd-7, delta-packed timestamps, dictionary encoding, bloom on trace_id. 6-10x ratio | zstd, delta-packed timestamps, dictionary encoding, inverted/fulltext/bloom/skip indexes. Claims 18x vs TimescaleDB, logs at 13% original size | Their LSM + compaction gives better compression (more opportunities to reorganize). We accept slightly worse ratio for zero compaction overhead |
| **Indexes** | Sort order (service, signal, timestamp) + Parquet min/max stats + bloom on trace_id | Inverted index (FST-based), fulltext index (Bloom or Tantivy), bloom filters, skip indexes, vector/ANN index | Massive indexing advantage. They can do `WHERE message LIKE '%error%'` efficiently. Our free-text log search is a full scan |
| **Protocol support** | OTel-native only (OTLP gRPC + HTTP, Pyroscope /ingest) | OTLP, Prometheus remote write, PromQL, InfluxDB line protocol, MySQL wire, PostgreSQL wire, Loki, Jaeger, OpenTSDB, Arrow Flight | They win massively on protocol breadth. We're OTel-only by design (simpler, but limits adoption) |
| **Pre-aggregation** | None, compute on read | Flownode continuous aggregation (tumbling windows, HyperLogLog, UDDsketch, roll-up) | Their Flownode is genuinely useful for dashboards. We don't need it because agents query raw data infrequently |
| **Profiles** | Full support (eBPF + Pyroscope SDK, stacktrace column, pprof/JFR/folded) | No profile support | We have profiles. They don't. Profiles are critical for "why is this slow" |
| **Consumer** | LLM agents (structured InvestigationBlock, 1-2 queries/incident) | Humans (dashboards, Grafana, SQL clients, PromQL) | Fundamentally different products for different consumers |
| **Infrastructure** | Zero — Parquet files on S3, DuckDB embedded | Full distributed system: Metasrv (3-node), Datanodes, Frontend, WAL (Kafka or RaftEngine), compactor | We have nothing to operate. They have a database cluster |
| **Cloud pricing** | $0.005/GB ingested, no base fee | Starting $290/month, WCU/RCU capacity units, contact sales for per-unit pricing | We're dramatically cheaper for small-to-medium workloads. Their minimum is 58x our 10GB/day cost |
| **Maturity** | Pre-launch, 25 tests passing, POC scale | v1.0 GA, production users (Poizon case study), JSONBench #1 | They're production-ready. We're building |

## Where GreptimeDB is Genuinely Better

### 1. Indexing depth

GreptimeDB has **five index types**: inverted (FST), fulltext (Bloom/Tantivy), bloom filters, skip indexes, and vector/ANN. We have sort-order pruning + bloom on trace_id. For log-heavy workloads where users grep for error patterns, their fulltext index is orders of magnitude faster than our full-scan approach.

**Should we care?** Not for agent queries. Agents query by `(service, signal, time_range)` — our sort order handles this. Free-text grep is a human workflow. But if we ever add a human-facing log viewer, this gap matters.

### 2. Compaction and file management

Their Mito2 engine runs TWCS (Time-Window Compaction Strategy) — merges overlapping SSTs, reduces file count, reorganizes data for better compression. We produce 240+ small files per hour at scale and have no compactor.

**Should we care?** Yes, eventually. At 12K files/hour, [[architecture/data-model.md|DuckDB]]'s file-open overhead will dominate query latency. We have this on the [[roadmap.md|roadmap]] ([[company/plan.md|plan]] 07) but haven't built it. Their compaction is battle-tested.

### 3. Continuous aggregation (Flownode)

SQL-native streaming: `CREATE FLOW ... AS SELECT host, avg(cpu) FROM metrics GROUP BY tumble(ts, '5 min')`. Supports HyperLogLog for approximate distinct counts, UDDsketch for quantiles, tumbling windows.

**Should we care?** Not for agent queries (raw data, infrequent). But if we ever need real-time alerting thresholds ([[company/plan.md|plan]] 15), we'd build something similar. Their approach is clean — SQL-defined flows, not a separate stream processor.

### 4. Protocol breadth

MySQL wire protocol, PostgreSQL wire protocol, PromQL native support, Loki ingest, Jaeger query API, InfluxDB line protocol — they accept data from anything.

**Should we care?** Our OTel-only stance is deliberate (one protocol, one schema, one pipeline). But it limits adoption for teams not yet on OTel. Their "drop-in replacement" story (point your existing Prometheus/Loki/Jaeger at us) is compelling for migration.

### 5. Distributed query execution

Frontend routes queries to specific Datanode regions, merges results. Scales horizontally by adding Datanodes.

**Should we care?** Our [[architecture/data-model.md|DuckDB]]-on-S3 model is single-node. For our query pattern (narrow windows, 1-2 queries/incident), it's fine. But if query volume grows or windows widen, we'd hit a ceiling they don't.

## Where We're Genuinely Better

### 1. Unified table model (no JOINs)

GreptimeDB's "unified observability" still means separate tables joined via SQL:

```sql
-- GreptimeDB: correlate logs with traces requires JOIN
SELECT l.body, t.span_name, t.duration_nano
FROM opentelemetry_logs l
JOIN opentelemetry_traces t ON l.trace_id = t.trace_id
WHERE t.service_name = 'checkout' AND t.span_status_code = 'ERROR';
```

```sql
-- Observe: same data, same table, no JOIN
SELECT message, name, value
FROM read_parquet('s3://sigiro/.../*.parquet')
WHERE service = 'checkout' AND status = 'ERROR'
  AND signal IN ('log', 'span');
```

No JOINs means: simpler query plans, no hash-join memory pressure, no schema coordination between tables, and the LLM agent can query with a single WHERE clause instead of reasoning about table relationships.

### 2. Zero infrastructure

GreptimeDB minimum production deployment: 3 Metasrv nodes + N Datanodes + Frontend + WAL backend (Kafka cluster or local RaftEngine) + object storage. That's 7+ processes minimum.

Observe: collector writes [[architecture/data-model.md|Parquet]] to S3. Agent reads with [[architecture/data-model.md|DuckDB]]. Two components.

### 3. Profiles

GreptimeDB has no profile support. No stacktrace storage, no pprof/JFR parsing, no flamegraph capability. [[architecture/profiles.md|Profiles]] are essential for "why is this service slow" — the most common incident [[architecture/investigation.md|investigation]] question after "what changed."

### 4. Agent-native query model

Our [[architecture/investigation.md|InvestigationBlock]] returns pre-structured data: metrics as time series, spans as trees, logs capped, [[architecture/profiles.md|profiles]] as top functions, linked traces resolved. One API call, one structured response.

GreptimeDB returns SQL result sets. An LLM agent would need to: figure out which tables exist, write separate queries for each signal, JOIN them, interpret raw rows, build span trees itself. That's 5-10 queries and significant reasoning overhead per [[architecture/investigation.md|investigation]].

### 5. Cost

| Workload | Observe | GreptimeCloud | Ratio |
|----------|---------|---------------|-------|
| 10 GB/day | $1.50/month | $290+/month (minimum) | 193x |
| 100 GB/day | $15/month | $290+/month + capacity overage | 19x+ |
| 1 TB/day | $150/month | Contact sales | Unknown, likely 5-10x+ |

Their WCU/RCU [[designs/pricing.md|pricing]] makes exact comparison hard without sales quotes, but the $290 minimum alone prices out indie developers and small teams.

### 6. Data portability

Our [[architecture/data-model.md|Parquet]] files on S3 are queryable by [[architecture/data-model.md|DuckDB]], Spark, Polars, Pandas, Snowflake, BigQuery — any engine that reads Parquet. Switch query engines without touching storage.

GreptimeDB stores [[architecture/data-model.md|Parquet]] SSTs with internal columns (`__primary_key`, `__sequence`, `__op_type`) and custom layouts. Not directly queryable by external tools without their engine.

## What We Can Learn From GreptimeDB

### 1. Metric Engine pattern (physical/logical tables)

Their approach to high-cardinality metrics is clever: many logical tables multiplexed onto one wide physical table with a `__table_id` discriminator. This is analogous to our unified table with `signal` + `name` as discriminators. Validates our approach — column multiplexing works.

**Action:** None. We already do this, just with a simpler model (one table, `signal` column).

### 2. BulkMemtable (lazy encoding)

Their BulkMemtable accumulates raw batches without immediate [[architecture/data-model.md|Parquet]] encoding, only encoding when enough data accumulates. Reduces write amplification by batching the encoding step.

**Action:** Consider for our [[architecture/write-path.md|write path]]. Currently we encode to [[architecture/data-model.md|Parquet]] on every 15s flush. If flush intervals decrease (streaming flush at 5-10s per memory-pressure [[company/plan.md|plan]]), lazy encoding could reduce CPU. Low priority — our write path isn't the bottleneck.

### 3. Fulltext index direction (Bloom vs Tantivy)

Their dual-backend fulltext index is pragmatic: Bloom for lightweight cases, Tantivy (Lucene-equivalent in Rust) for heavy text search. Bloom is simpler and smaller; Tantivy is more capable.

**Action:** If we ever need log text search beyond full scans, Bloom filters per row group on the `message` column would be our first step. We already use bloom on `trace_id` — extending to `message` is straightforward. No need for Tantivy-class complexity given our agent query model.

### 4. Skip indexes (min/max per row group)

[[architecture/data-model.md|Parquet]] already stores min/max statistics per row group, and [[architecture/data-model.md|DuckDB]] uses them automatically. GreptimeDB makes this explicit as a "skip index" concept with configurable granularity.

**Action:** None. [[architecture/data-model.md|DuckDB]] already does this for us transparently. Our sort order `(service, signal, timestamp)` ensures min/max stats are tight, enabling effective row group pruning.

### 5. Write cache (local hot tier)

Their write cache keeps recent data on local disk for low-latency reads. Recent data is the most queried — cache avoids S3 round-trips.

**Action:** We have this in our plans ([[company/plan.md|plan]] 06, local hot tier) but haven't built it. When query latency on recent data matters, a local [[architecture/data-model.md|Parquet]] cache of the last N hours would help. [[architecture/data-model.md|DuckDB]] can read local files + S3 globs in the same query. Medium priority.

### 6. Continuous aggregation SQL syntax

`CREATE FLOW ... AS SELECT ... GROUP BY tumble(ts, '5 min')` is clean. If we ever build alerting ([[company/plan.md|plan]] 15), this SQL-native approach to defining aggregation windows is worth borrowing conceptually.

**Action:** File for [[company/plan.md|plan]] 15. Not needed now.

## What NOT to Learn From GreptimeDB

### 1. Distributed coordination (Metasrv)

Their Metasrv with leader election, heartbeats, region migration, and distributed DDL is impressive engineering — and exactly the complexity we're avoiding. We don't need coordination because our writers are independent and our query model is narrow.

### 2. LSM-tree engine

WAL -> memtable -> flush -> compaction is the standard time-series database architecture. It enables mutable data and incremental compaction. But we don't need mutability (observability data is append-only) and our "compaction" will be a simple offline merge (read N small files, write 1 big file).

### 3. Multiple query protocols

PromQL, MySQL wire, PostgreSQL wire — all serve human operators. Our consumer is an LLM agent that runs [[architecture/data-model.md|DuckDB]] SQL directly. Adding protocol layers adds complexity for a consumer that doesn't need them.

### 4. Dynamic schema

Auto-created columns on ingest sounds flexible, but creates schema drift, unpredictable query plans, and storage bloat. Our frozen 18-column schema is a feature — agents know exactly what columns exist, every query is predictable.

## Summary

GreptimeDB is a better database. Sigiro is a better agent backend. Different architectural philosophies for different consumers.

> For Sigiro's positioning, see [[[research/competitive-landscape-2026.md|competitive-landscape-2026]].md](competitive-landscape-2026.md).

## Sources

- [GreptimeDB Architecture Docs](https://docs.greptime.com/user-guide/concepts/architecture/)
- [GreptimeDB GitHub](https://github.com/GreptimeTeam/greptimedb)
- [Observability 2.0 and the Database for It](https://greptime.com/blogs/2025-04-25-greptimedb-observability2-new-database)
- [Practical Guide to Observability Data Modeling](https://www.greptime.com/blogs/2025-04-15-greptimedb-observability)
- [Full-Text Search Meets Time-Series](https://greptime.com/tech-content/2025-07-03-greptimedb-unified-observability-approach)
- [Storage Architecture Deep Dive](https://www.greptime.com/blogs/2025-03-26-greptimedb-storage-architecture)
- [Log Benchmark v0.12](https://www.greptime.com/blogs/2025-03-10-log-benchmark-greptimedb)
- [JSONBench Performance](https://greptime.com/blogs/2025-03-18-jsonbench-greptimedb-performance)
- [GreptimeDB vs ClickHouse vs Elasticsearch](https://greptime.com/tech-content/2025-05-23-greptimedb-traditional-solutions-comparison)
- [Massive Trace Data at Low [[company/cost.md|Cost]]](https://www.greptime.com/blogs/2025-06-06-greptimedb-massive-trace-data)
- [v1.0 Highlights](https://www.greptime.com/blogs/2025-11-05-greptimedb-v1-highlights)
- [2026 [[roadmap.md|Roadmap]]](https://www.greptime.com/blogs/2026-02-11-greptimedb-roadmap-2026)
- [Continuous Aggregation Docs](https://docs.greptime.com/user-guide/flow-computation/continuous-aggregation/)
- [GreptimeCloud [[designs/pricing.md|Pricing]]](https://greptime.com/pricing)
- [OTel Traces Docs](https://docs.greptime.com/nightly/user-guide/ingest-data/for-observability/opentelemetry-traces/)
