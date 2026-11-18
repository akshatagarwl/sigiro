---
title: Database Tradeoffs
created: 2026-03-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'duckdb', 'ducklake']
---

<!-- Updated 2026-03-21: Durability and S3 write strategy moved to [[architecture/write-path.md|write-path]].md. -->

# Database Tradeoffs

> See [[[architecture/write-path.md|write-path]].md](write-path.md) for the [[architecture/write-path.md|write path]], durability, and S3 strategy details.

Every database makes performance tradeoffs. These aren't accidental — they're
intentional design choices made to meet application needs.

Source: [Benjamin Dicken on X](https://x.com/BenjDicken/[[plans/STATUS.md|status]]/2028100200213184905)

The axes covered here: time/space, read/write, startup time, consistency, text search.
Durability and S3 write strategy live in [[[architecture/write-path.md|write-path]].md](write-path.md).

---

## 1. Time / Space

**We trade space for query speed.**

- **Zstd compression** configured via [[architecture/ducklake-integration.md|DuckLake]] on every Parquet file it writes.
  Measured 10.5x on synthetic observability data. At 100 TB/day ingest,
  the difference between 3x and 10x is 3 PB vs 300 TB monthly — this is the
  primary cost lever for $0.005/GB [[designs/pricing.md|pricing]].
- **Delta binary packed encoding** on the `timestamp` column. Sorted timestamps
  have small deltas between consecutive values — delta encoding represents these
  as differences, which then compress extremely well under zstd.
- **Dictionary encoding** on low-cardinality columns (`service`, `signal`,
  `host`, `pod`, `status`, `kind`, `unit`, `name`). Near-zero overhead for
  absent nullable columns (metric rows pay nothing for `stacktrace`, `events`).
- **Sort order `(service, timestamp)`** declared per [[architecture/ducklake-integration.md|DuckLake]] table. [[architecture/data-model.md|DuckDB]] uses row group min/max stats to skip irrelevant data. A query for one service reads only its row groups. Signal is implicit in the table name — removed from the sort key.
- **Bloom filters on `trace_id`** (10 bits/value) are load-bearing for linked trace queries. Without bloom: 1-3s full column scan. With bloom: 300-800ms. Configured as [[architecture/ducklake-integration.md|DuckLake]] table options.
- **No pre-aggregation.** We don't store rollups, materialized views, or pre-computed summaries. All aggregation (bucketing, percentiles, top-N) is computed by [[architecture/data-model.md|DuckDB]] at read time. This trades CPU at query time for simpler writes and zero storage duplication.

Net: we spend CPU on compression and compute-on-read, but keep storage minimal.
We don't duplicate data into multiple formats — four [[architecture/ducklake-integration.md|DuckLake]] tables, consistent sort order, one encoding scheme per column type.

---

## 2. Read / Write

**We optimize for write throughput, accept slower reads.**

- **Append-only writes.** [[architecture/ducklake-integration.md|DuckLake]] INSERT is append-only. No updates, no deletes, no B-tree maintenance. [[architecture/write-path.md|Write path]] is: mapper → INSERT INTO ducklake table → DuckLake handles flush. That's it.
- **No indexes beyond sort order.** We rely on [[architecture/data-model.md|Parquet]] row group statistics and bloom filters, not explicit secondary indexes. A query that doesn't align with the sort order (`service, timestamp`) does more work.
- **Many small files mitigated by [[architecture/ducklake-integration.md|DuckLake]] compaction.** DuckLake's background compaction merges small data files automatically. No application-side compactor needed. File-open overhead is managed by the catalog layer, not our code.
- **Compute-on-read aggregation.** Every query re-scans raw data. No
  pre-computed time series, no rollup tables. A 1-hour query over a busy
  service reads and aggregates from scratch.

Net: writes are cheap and embarrassingly simple (append-only, no coordination).
Reads pay for this by scanning raw [[architecture/data-model.md|Parquet]]. This works because our read pattern
is narrow: one service, one time window, agent-driven (1-2 queries per
incident, not dashboards refreshing every 10s).

---

## 3. Startup Time

**We trade startup time for operational simplicity.**

- **No persistent indexes.** [[architecture/data-model.md|DuckDB]] is embedded and ephemeral — no index files
  to maintain, no WAL to replay, no catalog to recover. Start the query server,
  point it at S3, query immediately.
- **No connection pool warmup.** Per-project [[architecture/data-model.md|DuckDB]] pool (ProjectPool with DashMap). Connections created lazily, evicted after 5min idle.
  First query pays the S3 credential setup [[company/cost.md|cost]] (SET s3_* statements), then
  subsequent queries reuse the connection.
- **No state to rebuild.** The query server is stateless. Kill it, restart it,
  it works. No index rebuilds, no cache warming, no leader election.
- **Cold query penalty.** First query against a new time window fetches [[architecture/data-model.md|Parquet]]
  files from S3 with no local cache. Subsequent queries within the same window
  may benefit from OS page cache, but we don't manage this explicitly.

Net: near-instant startup, zero recovery time, but first queries are cold.
This is the right tradeoff for a stateless query server that can be killed and
restarted without ceremony.

---

## 4. Consistency

**We accept eventual consistency for write independence.**

- **No coordination between collectors.** Each collector node buffers and
  flushes independently. Two collectors writing data for the same service in
  the same second produce two separate files. No distributed locking, no
  ordering guarantees across nodes.
- **S3 eventual consistency (mostly resolved).** S3 now provides strong
  read-after-write consistency for PUTs. But a query issued at time T may miss
  a file flushed at T-1s if that flush hasn't completed yet. Accepted — the
  60s flush interval means queries should look at `[from, to - 60s]` for
  completeness.
- **No cross-signal transactions.** Each INSERT goes to a separate [[architecture/ducklake-integration.md|DuckLake]] table. There is no guarantee that a span and its corresponding log line land in the same DuckLake flush — they may arrive in different batches and be written to different files.
- **Query-time joins only.** Linked trace queries (find all spans/logs for a
  trace ID across services) are resolved at query time by scanning with bloom
  filters. No pre-built trace index, no dependency graph.

Net: collectors are fully independent writers with no coordination overhead.
Consistency is resolved at query time, not write time. This is viable because
observability data is append-only and queries tolerate small gaps.

---

## 5. Text Search

**We have no inverted index. Free-text log search is a full scan.**

- **No tokenization at write time.** Elasticsearch tokenizes every log message
  into terms and builds an inverted index. A keyword search is an O(1) index
  lookup. We store log messages as raw strings in a [[architecture/data-model.md|Parquet]] column — a keyword
  search (`message LIKE '%connection refused%'`) scans every string in every
  matching row group.
- **[[architecture/data-model.md|Parquet]] min/max stats don't help.** Row group statistics store the
  lexicographic min/max of the `message` column. Knowing the range is
  `"aaa"..."zzz"` tells you nothing about whether `"connection refused"` exists.
  Every row group is read.
- **Bloom filters don't help either.** Even with bloom filters on `message`,
  bloom tests exact token membership — not substring containment. A bloom filter
  can answer "does this row group contain the exact string X?" but not "does any
  message contain the substring X?".
- **Network-bound at scale.** Each search downloads and decompresses the full
  `message` column from S3 for every [[architecture/data-model.md|Parquet]] file in the time range. At 1 TB/day
  of logs, a 1-hour search reads ~42 GB of compressed string data over the
  network.
- **No faceted browsing.** Elasticsearch supports instant field-value facets
  (top 10 [[plans/STATUS.md|status]] codes, services, hosts). We can compute these with `GROUP BY`
  but it's a full scan each time, not an index lookup.

**What does work:** structured field queries are fast. `WHERE service='checkout'
AND [[plans/STATUS.md|status]]='ERROR' AND timestamp BETWEEN ...` uses dict encoding, sort-order
pruning, and row group statistics effectively. The architecture is built for
agent-driven [[architecture/investigation.md|investigation]] (structured queries, bounded windows), not
human-driven log exploration (type a keyword, get results in 200ms).

**Mitigation paths (not planned):**
- [[architecture/data-model.md|DuckDB]] FTS extension on local temp tables (works post-scan, doesn't help
  with I/O)
- Tantivy/Bluge sidecar index on the [[architecture/write-path.md|write path]] (adds complexity, a second
  storage format)
- Restrict search to structured fields only and accept that free-text search
  is not a product capability

---

## Summary

| axis | our choice | what we gain | what we pay |
|---|---|---|---|
| time/space | compute-on-read, compress aggressively | minimal storage, no duplication | CPU at query time |
| read/write | optimize writes, accept slower reads | append-only, no coordination | raw scans on every query |
| startup | no persistent indexes, stateless server | instant startup, zero recovery | cold first queries |
| consistency | eventual, no cross-node coordination | independent writers, no locking | query-time resolution |
| text search | no inverted index, structured fields only | simple write path, no tokenization overhead | free-text log search is a full scan |

Durability and S3 write strategy tradeoffs are documented in [[[architecture/write-path.md|write-path]].md](write-path.md).

These tradeoffs are correct for our workload: **agent-driven [[architecture/investigation.md|investigation]]
queries (1-2 per incident) against narrow time windows (5-60 min) for a single
service.** A dashboard product refreshing 50 panels every 10 seconds would make
different choices.
