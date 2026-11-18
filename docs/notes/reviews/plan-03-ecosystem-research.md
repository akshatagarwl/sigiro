---
title: Plan 03 Ecosystem Research — Autoresearch Results
created: 2026-03-21
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'decision']
---

# Plan 03 Ecosystem Research — Autoresearch Results

**Date:** 2026-03-21
**Iterations:** 8
**Goal:** Minimize custom code by finding ecosystem replacements

## Results Log

| # | Finding | Result | LOC Saved | Deps Changed |
|---|---------|--------|-----------|-------------|
| 1 | DuckDB COPY TO S3 replaces OpenDAL | KEEP | -170 | -opendal, -parquet |
| 2 | DuckDB ACID transaction replaces watermark table | KEEP | -50 | -sigiro_compactor_state table |
| 3 | `envy` crate for env var parsing | KEEP | -40 | +envy |
| 4 | Apache `datasketches` replaces `tdigest` | KEEP | 0 (better crate) | datasketches replaces tdigest |
| 5 | `parquet` dep eliminated entirely | KEEP | 0 (dep removed) | -parquet |
| 6 | COPY TO needs spawn_blocking | KEEP | +10 (new code) | 0 |
| 7 | Build TDigest in Rust from raw durations | KEEP | 0 | 0 |
| 8 | Final tally | — | **-210 net** | net -1 dep |

## Crates Evaluated and Rejected

| Crate | Why Rejected |
|-------|-------------|
| DataFusion | Overkill — no partitioned Parquet write, adds second query engine |
| deltalake-core | DuckDB can't read Delta Lake format (needs _delta_log) |
| iceberg-rust | DuckDB can't read Iceberg format, less mature than Delta |
| lance | Vector DB format, no Hive partitioning |
| object_store | No built-in retry/logging, OpenDAL is better — but both eliminated by COPY TO |
| tower | Service middleware, not fan-out orchestration |
| reqwest-middleware | Only saves ~30 LOC retry boilerplate, not worth dep |
| axum-extra | Bearer auth saves ~1 LOC, not worth dep |
| ballista | 60K+ LOC distributed engine, 30x larger than all of sigiro |
| tarpc | Saves ~40 LOC vs reqwest, less mature |
| volo-thrift | ByteDance's 1M RPS framework, overkill |
| tdigest | Unmaintained (2021), replaced by Apache datasketches |
| config-rs/figment | Overkill for env-only config |

## Key Discovery: DuckDB COPY TO S3

```sql
BEGIN;
COPY (SELECT *, year(timestamp) AS year, month(timestamp) AS month, day(timestamp) AS day
      FROM sigiro_spans WHERE timestamp < now() - INTERVAL '7 DAY')
TO 's3://bucket/prefix/sigiro_spans/'
(FORMAT PARQUET, PARTITION_BY (year, month, day), COMPRESSION ZSTD, OVERWRITE_OR_IGNORE);
DELETE FROM sigiro_spans WHERE timestamp < now() - INTERVAL '7 DAY';
COMMIT;
CHECKPOINT;
```

One SQL statement replaces ~200 LOC of: [[architecture/data-model.md|Arrow]] RecordBatch extraction, [[architecture/data-model.md|Parquet]] ArrowWriter,
OpenDAL S3 upload, Hive path construction, watermark idempotency, retry logic.

**Tradeoff:** Holds [[architecture/data-model.md|DuckDB]] mutex during S3 upload (~2-5s for hourly 60MB).
Acceptable for stated scale (44GB/mo). Revisit at 500GB/mo.

## Key Discovery: Apache datasketches v0.2.0

- Edition 2024, rust-version 1.85 (exact match for sigiro)
- TDigest: `TDigestMut::update()`, `merge()`, `serialize()`/`deserialize()`, `quantile()`
- HLL included (for future pod_count scaling)
- Binary format compatible with Java/C++/Python Apache DataSketches
- Actively maintained (Jan 2026, Apache)
- Replaces dead `tdigest` crate (last update 2021)
