---
title: OpenObserve Query Layer Internals — Extraction Analysis
created: 2026-03-05
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'duckdb', 'ducklake']
---

# OpenObserve Query Layer Internals — Extraction Analysis

Deep-dive into [[research/observe-vs-openobserve.md|OpenObserve]]'s query layer to evaluate whether any components can be extracted for Sigiro's query engine. Conclusion: **steal ideas, not code.**

## Architecture (5 layers)

```
HTTP  →  /api/{org_id}/_search
SQL   →  Sql::new_from_req() parses, rewrites, extracts columns
Cache →  QUERY_RESULT_CACHE check, calculate_deltas() for partial hits
Files →  file_list::get_file_list() prunes by time + partition keys + bloom
Exec  →  DataFusion SessionContext + custom TableProviders over Parquet
Agg   →  Arrow Flight fan-out to querier nodes, concat_batches(), VRL transforms
```

## Key Source Files

| File | Role |
|---|---|
| `src/service/search/sql/mod.rs` | SQL parsing + rewrite (meta_sql) |
| `src/service/search/grpc/storage.rs` | Parquet scan + Tantivy inverted index filter |
| `src/service/search/datafusion/exec.rs` | DataFusion session setup, custom TableProviders |
| `src/infra/src/file_list/mod.rs` | File metadata cache (in-memory mirror of metadata DB) |
| `src/service/search/cache/cacher.rs` | Query result cache + delta calculation |
| `src/service/search/cluster/flight.rs` | Distributed query coordination (leader/worker) |
| `src/service/search/grpc/wal.rs` | WAL search (search_parquet, search_memtable) |

## Storage Model vs Ours

| Aspect | OpenObserve | Sigiro |
|---|---|---|
| File layout | `files/<org>/<stream_type>/<stream_name>/<Y>/<M>/<D>/<H>/<id>.parquet` | `s3://<bucket>/<epoch_seconds>/*.parquet` |
| File count | Many small files → compactor merges to 256MB | Few files per hour (needs future compactor) |
| Metadata | MySQL/PostgreSQL `meta` table: path, min_ts, max_ts, records, sizes, flattened flag | None — glob-based discovery |
| File pruning | Query metadata DB by org + stream + time range | Enumerate one glob per epoch-hour |

**Learning:** Their file_list metadata table gives tighter pruning than glob enumeration. Each file record stores `min_ts`, `max_ts`, `records`, `original_size`, `compressed_size`. The query planner uses this to skip files entirely before touching S3. This avoids the S3 LIST latency that glob-based discovery pays.

## Query Result Caching (worth stealing the idea)

**How it works:**
1. Hash the query (SQL + time range + org)
2. Check `QUERY_RESULT_CACHE` for a hit
3. Full hit → return cached result immediately
4. Partial hit → `calculate_deltas()` computes the uncovered time range, fetches only that, merges

**Why this matters for us:** Agent queries are often "same service, slightly wider time window" or "same query, 5 minutes later." Delta computation means the second round of an [[architecture/investigation.md|investigation]] is nearly free — only scan the new epoch-hour partitions.

**Our implementation path:** LRU cache in query-server keyed on `(service, signal, time_start)`, storing serialized [[architecture/investigation.md|InvestigationBlock]]. On repeat query, check if `cached_max_ts >= query_start`, fetch only hours after `cached_max_ts`, merge.

## Tantivy Inverted Index (not worth copying, but the sidecar pattern is)

**How it works:**
1. At compaction time, build Tantivy `.ttv` index files alongside each [[architecture/data-model.md|Parquet]] file
2. At query time, `tantivy_search()` extracts index-eligible terms from the WHERE clause
3. Files without matching terms are eliminated before DataFusion touches them

**Why we don't need this:** Our queries are structured (`service=X, signal=Y, time=Z`), not full-text search. [[architecture/data-model.md|Parquet]] row-group statistics + sort order already handle this.

**What we should steal:** The sidecar file pattern. Instead of Tantivy, write **bloom filter sidecars** per [[architecture/data-model.md|Parquet]] file for `trace_id` and `service`. This directly accelerates trace lookup (our most latency-sensitive query) without an inverted index.

## DataFusion Integration (not extractable)

[[research/observe-vs-openobserve.md|OpenObserve]] wraps DataFusion with custom components that are deeply coupled:

- **`NewUnionTable`** — merges WAL + object storage into one logical table. We don't have WAL.
- **`ListingTableAdapter`** — adapts their file_list metadata to DataFusion's `ListingTable`. We use [[architecture/data-model.md|DuckDB]]'s native `parquet_scan()`.
- **Schema management** — `infra::schema` handles dynamic schema evolution with lock-based field addition. Our schema is fixed.
- **`ParquetAccessPlan`** — generates row-group-level access plans using sampling. [[architecture/data-model.md|DuckDB]] does this internally.

**Verdict:** Every custom TableProvider assumes [[research/observe-vs-openobserve.md|OpenObserve]]'s file layout, metadata DB, and stream-per-signal model. Extracting DataFusion pieces and adapting them to our unified table would be more work than our existing [[architecture/data-model.md|DuckDB]] integration.

## Distributed Execution (not relevant yet)

- Leader node partitions file list across querier nodes via strategies: `file_num`, `file_size`, `file_hash`
- Workers execute via [[architecture/data-model.md|Arrow]] Flight, stream RecordBatches back
- Leader merges with `concat_batches()`

**When we'd need this:** Only if single-node [[architecture/data-model.md|DuckDB]] can't handle query volume. At 1-2 queries per incident, this is unlikely soon. If needed, DuckDB's own remote execution or a simple sharding layer (partition by service → dedicated DuckDB instance per service group) would be simpler than reimplementing [[architecture/data-model.md|Arrow]] Flight coordination.

## Multi-tenancy Threading (pervasive, would need to be gutted)

`org_id` appears in:
- Every HTTP endpoint path
- SQL parsing and rewrite
- File list queries
- Storage paths
- Schema lookups
- Cache keys
- Distributed query coordination

**If extracting any code:** Every function signature has `org_id: &str` threaded through. Removing it is not just deleting a parameter — it changes file paths, cache keys, query planning, and access control. This alone makes extraction [[company/cost.md|cost]]-prohibitive.

## Ideas Worth Implementing in Sigiro

### 1. File metadata table (replaces glob discovery)

```sql
-- Populated by writer.go on each Parquet flush
CREATE TABLE file_meta (
    path       TEXT PRIMARY KEY,
    min_ts     BIGINT NOT NULL,
    max_ts     BIGINT NOT NULL,
    services   TEXT,          -- comma-separated, for fast pre-filter
    signals    TEXT,          -- comma-separated
    row_count  INTEGER,
    size_bytes BIGINT,
    created_at BIGINT
);
```

**Benefit:** `ParquetGlobs()` currently does S3 LIST per hour-partition (with signal Hive sub-partitions). A metadata table turns this into a local SQL query: `SELECT path FROM file_meta WHERE max_ts >= ? AND min_ts <= ? AND services LIKE ? AND signal = ?`. Eliminates S3 LIST latency entirely.

**Storage:** Embedded [[architecture/data-model.md|DuckDB]] or SQLite file alongside the query-server. Updated by the exporter on flush. ~1 row per [[architecture/data-model.md|Parquet]] file, negligible size.

### 2. Bloom filter sidecars

Write a bloom filter for `trace_id` (and optionally `service`) alongside each [[architecture/data-model.md|Parquet]] file during flush. At query time, check bloom before reading the file.

**Benefit:** Trace lookup (`WHERE trace_id = ?`) currently scans all files in the time range. Bloom filter skips files that definitely don't contain the trace. Expected: 90%+ file skip rate for point trace lookups.

**Implementation:** `parquet-go` supports bloom filter metadata natively. Enable on `trace_id` column in writer.go.

### 3. Query result cache with delta computation

```go
type CacheEntry struct {
    Key       string    // hash(service, signal, query_template)
    MaxTS     int64     // latest timestamp in cached result
    Result    []byte    // serialized InvestigationBlock
}
```

On repeat query: if `cached_max_ts >= query_start`, only scan hours after `cached_max_ts`, merge new results into cached block.

**Benefit:** Agent's second query round (drill-down, wider window) pays only for the incremental data.

### 4. Per-file min/max stats for service + signal

Even without a full metadata table, writing `service` and `signal` min/max values into [[architecture/data-model.md|Parquet]] file metadata (key-value pairs in the file footer) lets the query engine skip files without reading row-group stats.

**Benefit:** Faster than bloom filters for equality predicates on low-cardinality columns. [[architecture/data-model.md|DuckDB]] reads [[architecture/data-model.md|Parquet]] metadata automatically.

## Why Not Fork OpenObserve's Query Layer

1. **Language mismatch** — Rust → Go rewrite eliminates any extraction benefit
2. **Architecture mismatch** — per-stream files + metadata DB vs unified table + glob discovery
3. **Feature mismatch** — we'd gut 80% (multi-tenancy, distributed execution, WAL search, Tantivy, PromQL, dashboards, alerting) and keep 20% ([[architecture/data-model.md|Parquet]] scan + predicate pushdown), which [[architecture/data-model.md|DuckDB]] already provides
4. **Coupling** — `org_id` threaded through every function, stream type routing everywhere, schema evolution logic interleaved with query planning
5. **[[architecture/data-model.md|DuckDB]] already does it** — `SELECT * FROM parquet_scan(?) WHERE service=? AND signal=? AND timestamp BETWEEN ? AND ?` with predicate pushdown, row-group pruning, and parallel execution — zero custom code needed

## Bottom Line

[[research/observe-vs-openobserve.md|OpenObserve]] built a distributed query engine because dashboards demand it (300 queries/min per user). We don't have dashboards. Our query budget is 1-2 calls per incident. The right move is to keep [[architecture/data-model.md|DuckDB]] and add surgical optimizations (metadata table, bloom filters, result cache) that each take 50-200 lines of Go — not extract thousands of lines of tightly-coupled Rust.
