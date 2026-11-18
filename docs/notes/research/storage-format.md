---
title: Storage Format Research
created: 2026-03-21
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'decision', 'duckdb']
---

# Storage Format Research

Researched March 2026. Query speed is not a concern — decompression hides behind LLM inference (2-10s per reasoning step). Storage [[company/cost.md|cost]] is the only priority.

---

## Current Stack

[[architecture/data-model.md|DuckDB]] + [[architecture/data-model.md|Parquet]] on S3. Unified table sorted by `(service, signal, timestamp)`. Bloom filter on `trace_id` for cross-service lookups. zstd ~level 7, delta on timestamps, dict on low-cardinality strings.

**Current baseline:** 10.5x compression on synthetic data.

### What DuckDB Already Does

- Reads [[architecture/data-model.md|Parquet]] natively with predicate pushdown
- Uses row group min/max stats to skip irrelevant data (free skip-indexing from our sort order)
- Column pruning — only reads columns referenced in the query
- Parallel execution across files, handles S3 range reads directly
- `time_bucket`, `approx_quantile`, bloom-aware scans — all built in

A "custom data layer" would reimplement most of this. **Rejected** — revisit when profiling shows [[architecture/data-model.md|DuckDB]] internals as the bottleneck.

---

## Compression Opportunities

### 1. Binary trace_id / span_id — 50% column savings, zero risk

**The single highest-ROI change.** Currently stored as hex strings. OTel trace_id is 16 bytes, span_id is 8 bytes. Hex encoding doubles them.

| Field | Hex string (current) | Binary FIXED_LEN_BYTE_ARRAY | Savings |
|---|---|---|---|
| trace_id | 36 bytes/value | 16 bytes/value | **55%** |
| span_id | 20 bytes/value | 8 bytes/value | **60%** |
| parent_span_id | 20 bytes/value | 8 bytes/value | **60%** |

Convert hex→binary in the mapper, hex-encode at query time (`encode(trace_id, 'hex')` in [[architecture/data-model.md|DuckDB]]). DuckDB reads FIXED_LEN_BYTE_ARRAY natively. Bloom filters work on binary values.

### 2. Background recompression — zstd level 19+ on settled files

klauspost/compress pure Go only exposes 4 presets (max ~level 11). True high-level zstd requires cgo or a background job.

| Level | Ratio vs level 7 | Compression speed | Decompression |
|---|---|---|---|
| 7 (current) | baseline | ~150 MB/s | ~1 GB/s |
| 11 (SpeedBestCompression) | +3-5% | ~60 MB/s | ~1 GB/s |
| 19 | +5-10% | ~5-10 MB/s | ~1 GB/s |
| 22 | +6-12% | ~2-5 MB/s | ~1 GB/s |

**Verdict: 5-12% improvement, not transformative.** Not worth cgo in the hot [[architecture/write-path.md|write path]]. Free as a background job on settled data. Also merges small files (solves 12K files/hour problem) and re-sorts data.

```sql
COPY (SELECT * FROM read_parquet('s3://sigiro/1709683200/*.parquet'))
TO 's3://sigiro-compressed/1709683200/merged.parquet'
(FORMAT parquet, COMPRESSION zstd, COMPRESSION_LEVEL 19, ROW_GROUP_SIZE 500000);
```

### 3. BYTE_STREAM_SPLIT on float Value column — ~30% savings on numerics

Separates IEEE 754 float bytes into K streams, zstd compresses the resulting streams dramatically better. Our `Value` column (float64, nullable) is present on every metric row and every span row (duration_ms).

**[[plans/STATUS.md|Status]]:** parquet-go supports BYTE_STREAM_SPLIT via `split` tag, but only on non-pointer types. Our Value column is `*float64` (nullable). **Best path:** Apply via pyarrow in the background recompression job.

### 4. Stacktrace deduplication — biggest win for profile data

Pre-deduplicate in the mapper: fold stacktrace to single semicolon-joined string. [[architecture/data-model.md|Parquet]]'s dictionary encoding on strings deduplicates identical folded stacks automatically.

**Expected savings:** 60-80% on profile stacktrace data. Real [[architecture/profiles.md|profiles]] have ~50-200 unique stacks out of thousands of samples.

### 5. Log message template extraction — 3-5x on message column

Templates repeat heavily → dict encoding compresses them to near-zero. Options: FSST (no Go implementation yet), template extraction at write time (2 columns: template + params), or just rely on zstd-19 in background recompression.

**Verdict:** Defer template extraction. Get 80% of the benefit from zstd-19 recompression with zero [[architecture/write-path.md|write-path]] changes.

### 6. Promote top metadata fields to columns

Current `Metadata *string` is a JSON blob with ~60% overhead. Promote top 5-8 attributes (http_method, http_status_code, http_route, error_type, db_system) to dict-encoded columns.

**Expected savings:** 40-60% reduction in metadata column size. Already P1 in wide-event-gaps [[company/plan.md|plan]].

### 7. Row group sizing — larger is better for compression

For maximum compression: larger row groups + larger pages = more data for dictionary encoder, better zstd ratios. Trade-off: random access is slower. We don't care.

**Verdict:** Row group sizing matters only in the background compaction job (ROW_GROUP_SIZE 1000000).

### Quick win: SpeedBestCompression — DONE

Applied in writer.go. klauspost/compress SpeedBestCompression (~zstd level 11) + 8MB page buffers + dict encoding on stacktrace list elements. Measured 2.2% improvement on synthetic data.

---

## ClickHouse-Style Codec Chaining — The Real Gap

ClickHouse achieves 10-20x using codec chains: specialized lightweight encoding first, then zstd.

| Data type | ClickHouse codec chain | Parquet equivalent |
|---|---|---|
| Timestamps | DoubleDelta + ZSTD → 800:1 on uniform | Delta + ZSTD (what we have) |
| Gauge metrics | Gorilla (XOR) + ZSTD | Nothing (plain + ZSTD) |
| Counters | DoubleDelta + LZ4 | Nothing |
| Log messages | ZSTD | ZSTD (same) |

**The gap:** [[architecture/data-model.md|Parquet]] doesn't natively support Gorilla or DoubleDelta. BYTE_STREAM_SPLIT partially closes this for floats. The remaining gap is ~2-3x on timestamp+metric columns, negligible on string-heavy data.

### Path B: Parquet ingest → ClickHouse background conversion

Best of both worlds. Keep pure Go/Rust [[architecture/write-path.md|write path]] for fast ingest, convert settled data to ClickHouse MergeTree for maximum compression.

| Workload mix | Optimized Parquet | ClickHouse MergeTree |
|---|---|---|
| 80% logs, 20% metrics/traces | 14-16x | 16-18x |
| 50% logs, 50% metrics/traces | 14-18x | 20-25x |
| 80% metrics, 20% logs | 12-15x | **25-35x** |

**Recommendation:** Start with Path A ([[architecture/data-model.md|Parquet]] quick wins, 14-18x), migrate to Path B when storage costs matter (20-25x).

### Pcodec — 44-158% better on numerics, not available yet

Rust library only. Active proposal to add to [[architecture/data-model.md|Parquet]] spec. Our data is ~70% strings, ~30% numerics. Watch for when it lands in [[architecture/data-model.md|DuckDB]]/pyarrow.

---

## Vortex Evaluation

Extensible columnar file format (Rust-native), LFAI&Data incubation project. Keeps data in [[architecture/data-model.md|Arrow]]-compatible compressed layout — queries run on compressed data without a decode step. [[architecture/data-model.md|DuckDB]] shipped a core extension (Jan 2026).

### Performance vs Parquet

| Metric | vs Parquet |
|---|---|
| Scan speed | 10-20x faster |
| Random access | 100x faster |
| Write speed | 5x faster |
| Compression | Similar ratio |
| DuckDB TPC-H SF100 | 12-18% faster (geometric mean) |

### Why Not Use It Today

1. **No Go writer library** — requires CGO wrapping C FFI
2. **Pre-1.0, no forward compatibility** — 1.0 targeted H2 2025, still hasn't shipped
3. **Still finding crashes via fuzzing** — panic in encoding path (issue #6526, Feb 2026)
4. **Writer flexibility = inconsistency risk** — unlike [[architecture/data-model.md|Parquet]]'s rigid spec
5. **Narrow ecosystem** — [[architecture/data-model.md|DuckDB]], Polars, DataFusion only. No Spark/Trino/Snowflake/Athena
6. **We're not bottlenecked on decode** — POC latency dominated by S3 round trips
7. **Bloom filter gap** — no equivalent row-level bloom for trace_id lookups

### No OTel Ecosystem Support

Zero Vortex support in OTel collector. The [[architecture/data-model.md|Parquet]] exporter was removed Oct 2023. Two subsequent proposals both closed inactive.

### Recommended Migration Path (if ever needed)

Don't replace the [[architecture/write-path.md|write path]]. Use background compaction: parquet-go writes [[architecture/data-model.md|Parquet]] to S3, background Rust job rewrites settled files as Vortex, [[architecture/data-model.md|DuckDB]] reads Vortex for cold data.

### Revisit Triggers

- Profiling shows [[architecture/data-model.md|Parquet]] decode as significant % of query latency
- Vortex ships 1.0 with forward compatibility
- Go bindings appear
- We build a compaction job anyway (natural place to add Vortex conversion)

---

## Real Bottlenecks

| Bottleneck | Solution | Effort |
|---|---|---|
| S3 file listing / metadata overhead | Metadata catalog (Postgres table: service + time_range → S3 keys) | Weeks |
| Cold scan latency (first query, no cache) | Read cache (local SSD or memory-mapped) | Weeks |
| Cross-signal correlation | Already handled — 4 parallel DuckDB queries | Done |
| Small file proliferation | Compaction job (merge small Parquet files) | Days |
| Query planning control | Custom DuckDB table function or replacement scan | Days |

None of these require replacing [[architecture/data-model.md|DuckDB]].

---

## Implementation Priority

Ranked by impact/effort ratio against 10.5x baseline.

| # | Change | Expected improvement | Effort | Where |
|---|---|---|---|---|
| 1 | Binary trace_id/span_id | +1-2x on ID columns | Small | Write path |
| 2 | Stacktrace dict on list elements | +60-80% on profile data | Small | **DONE** |
| 3 | Background recompression (zstd-19 + merge) | +5-12% overall | Medium | New cron job |
| 4 | Promote top metadata fields to columns | +40-60% on metadata column | Medium | Schema + mapper |
| 5 | BYTE_STREAM_SPLIT on float columns | +30% on Value column | Medium | Background recompression |
| 6 | SpeedBestCompression in write path | +3-5% overall | Tiny | **DONE** |
| 7 | Log template extraction | +2-4x on message column | Large | Mapper + new columns |

### Combined realistic target

- **Current:** 10.5x
- **Binary IDs + stacktrace folding:** ~12x
- **+ Background zstd-19 + merge + BYTE_STREAM_SPLIT:** ~14x
- **+ Promoted metadata columns:** ~16-18x

**Realistic target: 14-18x.** To reach 20x+, need Pcodec (when available), template extraction, or ClickHouse codec chains.

---

## Sources

- [The [[architecture/data-model.md|Parquet]] We Could Have](https://graphallthethings.com/posts/the-parquet-we-could-have/) — pcodec integration PoC
- [Pcodec paper](https://arxiv.org/html/2502.06112v2) — 29-94% higher compression than zstd on numerics
- [FSST paper](https://www.vldb.org/pvldb/vol13/p2649-boncz.pdf) — up to 3.3x on text
- [[[architecture/data-model.md|Parquet]] BYTE_STREAM_SPLIT encoding](https://parquet.apache.org/docs/file-format/data-pages/encodings/)
- [ClickHouse compression codecs](https://clickhouse.com/blog/optimize-clickhouse-codecs-compression-schema)
- [[[architecture/data-model.md|DuckDB]] COPY TO compression](https://duckdb.org/docs/stable/data/parquet/overview)
- [[[architecture/data-model.md|DuckDB]] Vortex extension](https://duckdb.org/2026/01/23/duckdb-vortex-extension)
- [Polar Signals Vortex migration](https://www.polarsignals.com/blog/posts/2025/11/25/interface-parquet-vortex)
- [Vortex GitHub](https://github.com/vortex-data/vortex)
- [clickhouse-local docs](https://clickhouse.com/docs/operations/utilities/clickhouse-local)
- [chdb-go](https://github.com/chdb-io/chdb-go) — embedded ClickHouse in Go
- [Character.AI compression](https://clickhouse.com/resources/engineering/database-compression) — 15-20x with ClickHouse codecs
