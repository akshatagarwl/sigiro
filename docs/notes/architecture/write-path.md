---
title: Write Path
created: 2026-03-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'duckdb', 'ducklake']
---

<!-- Updated 2026-04-13: Fixed sigiro_profiles and sigiro_log_templates column names to match schema.rs. -->

## Write Path

---

### Architecture

Single Rust binary (`sigiro`). No OTel Collector in the path.

```
OTLP client (SDK, Beyla, etc.)
  │
  ├─ :4317 gRPC (tonic) ─── OtlpGrpcService
  ├─ :4318 HTTP (axum)  ─── HTTP handlers (protobuf + JSON)
  │

        └─→ direct Arrow builder (src/otlp.rs) transform (on tokio blocking pool)
        │
        └─→ crossbeam_channel<IngestMsg>
              │
              └─→ ProjectFlusher threads (one per project, spawned on first message)
                    │
                    └─→ DuckDB INSERT (via appender API)
                          │
ae|                          └─ DuckLake: SQLite catalog (`catalog.sqlite`) + local Parquet (`catalog.ducklake.files/`)
                              (Set `SIGIRO_S3_BUCKET` to transparently write data files to S3)
```

### Data Flow

1. **Receive.** gRPC (`:4317`) and HTTP (`:4318`) accept OTLP for all four signals: traces, logs, metrics, [[architecture/profiles.md|profiles]]. HTTP handlers decode protobuf or JSON, gRPC handlers use tonic streaming.


2. **Transform.** Raw OTLP protobuf → [[architecture/data-model.md|Arrow]] RecordBatches via direct Arrow builder (src/otlp.rs). This runs on the tokio blocking thread pool (parallel, not blocking the async runtime). Each signal produces a typed `IngestMsg` variant.

3. **Dispatch.** Transformed `IngestMsg` goes into a `crossbeam_channel`. Bounded to apply backpressure — if the inserter falls behind, gRPC/HTTP handlers return `UNAVAILABLE` after the channel fills.

4. **Buffer + Flush.** Per-project flusher threads (`ProjectFlusher` in `inserter.rs`) receive from the channel. Each maintains a `BTreeMap<SignalTable, Vec<RecordBatch>>` buffer. Flush triggers:
   - **Row threshold:** 10,000 rows per table
   - **Time threshold:** 1 second
   Whichever comes first.

ry|5. **Write.** On flush, each table's buffered batches are appended to [[architecture/data-model.md|DuckDB]] via the appender API (`append_record_batch`). DuckDB writes to `catalog.sqlite` (catalog metadata) and Parquet files (`catalog.ducklake.files/`), managed transparently by [[architecture/ducklake-integration.md|DuckLake]].

6. **Log dedup.** The `DrainTree` ([[designs/log-template-dedup.md|DRAIN]] algorithm) runs on the log buffer before flush, producing `sigiro_log_templates` entries (template_id, template_text, first_seen). Same log message pattern → one template row, reducing log storage by 5-10x.

### Per-Signal Tables

8 tables, not one mega-table. Each has its own schema optimized for that signal type:

| Table | Source Signal | Key Columns |
|---|---|---|
| `sigiro_spans` | traces | trace_id, span_id, parent_span_id, name, kind, status |
| `sigiro_logs` | logs | timestamp, severity, message, template_id |
| `sigiro_log_templates` | logs (DRAIN) | template_id, template_text, first_seen |
| `sigiro_metrics_gauge` | metrics (gauge) | metric_name, value, start_time, time |
| `sigiro_metrics_sum` | metrics (sum) | metric_name, value, is_monotonic, start_time, time |
| `sigiro_metrics_histogram` | metrics (histogram) | metric_name, count, sum, min, max, bucket_counts |
| `sigiro_metrics_exp_histogram` | metrics (exp histogram) | metric_name, count, sum, scale, zero_threshold, positive/negative buckets |
| `sigiro_profiles` | profiles | profile_id, trace_id, span_id, service_name, timestamp, profile_type, stack_trace, value, unit, resource_attributes |

**Why separate tables, not one:** Zero-copy insert per metric type (no schema projection), no nullable columns for irrelevant fields, simpler compression (each table's columns are homogeneous). See [[architecture/tradeoffs.md|tradeoffs]].md for the full analysis.

### Crash Recovery

[[architecture/data-model.md|DuckDB]]'s own WAL provides durability — once `append_record_batch` returns, data is on disk (local [[architecture/data-model.md|Parquet]]) or in S3 (when `SIGIRO_S3_BUCKET` is set). The only data-loss window is the in-memory buffer: up to 1 second or 10k rows between the 200 OK response to the client and the DuckDB append. This is acceptable because the data source (OTel SDK) will re-emit on retry, and the window is smaller than SRE reaction time.

No custom WAL, no persistent queue, no OTel Collector file_storage extension. [[architecture/data-model.md|DuckDB]] handles it.

### Port Layout

| Port | Protocol | Purpose |
|------|----------|---------|
| 4317 | gRPC (tonic) | OTLP traces + logs + metrics + profiles |
| 4318 | HTTP (axum) | OTLP HTTP (protobuf + JSON), all signals |
| 9999 | HTTP (axum) | SQL API (`/v1/query`, `/v1/diagnose`), admin (`/admin/`), health (`/health`) |

Port 4040 ([[architecture/profiles.md|Pyroscope]] pprof/JFR) is planned but not yet wired.

### Compression

zstd level 7 + delta encoding on timestamps + dictionary encoding on low-cardinality columns. Measured 10.5x on synthetic observability data. Aggressive compression works because the primary reader is an LLM agent with 500ms-1.3s acceptable latency — decompression time hides behind inference time.

---

### Research Context

These sections capture design decisions from the Go-era exploration. Kept for historical context.

#### Why Not otel-arrow's Star Schema

The otel-arrow project writes a STAR schema with separate [[architecture/data-model.md|Parquet]] files per signal component (logs, log_attrs, resource_attrs, scope_attrs, etc.). Queries require JOINs across these tables.

We rejected this for our use case:

- **No semantic coherence.** Agent still makes multiple reads and joins across tables. "4 queries across 4 backends" becomes "JOINs across 12 [[architecture/data-model.md|Parquet]] tables."
- **No single query.** Our `WHERE timestamp BETWEEN t1 AND t2` returning all signals is impossible with star schema — you need per-signal JOINs, then UNION ALL.
- **No chronological timeline.** The agent can't read events in causal order from a single scan.

The otel-arrow team chose star schema for valid general-purpose reasons (attribute explosion, type inconsistency, arrow memory). But those are problems for a platform serving arbitrary users. We control our schema.

#### Attribute Queryability Tradeoff

`attributes map<string, string>` can't be efficiently filtered in [[architecture/data-model.md|Parquet]] — `WHERE attributes['http.method'] = 'GET'` requires deserializing the entire map for every row.

Mitigations:
1. **Promote hot attributes to top-level columns.** 10-15 known OTel semantic conventions as real columns per table. The map holds the long tail.
2. **Bloom filters on trace_id and message.** For point lookups.
3. **Accept it for POC.** Time range + service narrows the scan significantly. Map deserialization on a small result set is fast.

Option 1 is the long-term answer.
