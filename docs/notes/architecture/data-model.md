---
title: Data Model
created: 2026-03-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'competitor', 'data-model', 'decision', 'deployment']
---

<!-- Updated 2026-04-13: [[architecture/profiles.md|Profiles]] and log_templates are now in the active table layout. -->

## Data Model

Per-signal tables. One catalog. All signals queryable. Schema matches `otlp2records` output directly.

---

### Table Layout

Data is stored in [[architecture/ducklake-integration.md|DuckLake]] tables managed by a shared catalog. Each table matches
the Arrow schema output by `otlp2records::transform_*()` — no column renames at ingest.

```sql
ducklake.sigiro_spans                    -- one row per span (otlp2records traces_schema)
ducklake.sigiro_logs                     -- one row per log record (otlp2records logs_schema)
ducklake.sigiro_metrics_gauge            -- gauge metric data points (16 columns)
ducklake.sigiro_metrics_sum              -- sum metric data points (18 columns)
ducklake.sigiro_metrics_histogram        -- histogram metric data points (22 columns)
ducklake.sigiro_metrics_exp_histogram    -- exponential histogram data points (27 columns)
ducklake.sigiro_profiles                 -- one row per profile sample (profile_id, trace_id, span_id, service_name, timestamp, profile_type, stack_trace, value, unit)
ducklake.sigiro_log_templates            -- DRAIN log template dedup (template_id, template, count)
```

[[architecture/ducklake-integration.md|DuckLake]] manages the physical file layout in S3. There are no manually constructed Hive partition paths — DuckLake decides how to organize, flush, and compact data on disk.

This layout serves two access patterns from one catalog:
- **AI agents** query per-signal tables and JOIN or UNION ALL as needed — full cross-signal view in one SQL statement
- **Dashboard tools** ([[designs/future/perses-compat.md|Perses]]) query a single table (e.g. `ducklake.sigiro_metrics_gauge`) — no I/O wasted on other signals

---

### Schema

Tables use the `otlp2records` native Arrow schemas. No column renames, no promoted
columns at ingest. Presentation concerns ([[plans/STATUS.md|status]] strings, duration units, service
filtering) are handled at query time.

#### sigiro_spans (traces_schema)

| field | type | notes |
|---|---|---|
| timestamp | timestamp_us | span start time |
| end_timestamp | int64 | span end time (nanos) |
| duration | int64 | span duration in nanoseconds |
| trace_id | string (nullable) | hex-encoded 16 bytes |
| span_id | string (nullable) | hex-encoded 8 bytes |
| parent_span_id | string (nullable) | hex-encoded 8 bytes |
| trace_state | string (nullable) | W3C trace state |
| service_name | string | from resource attributes |
| service_namespace | string (nullable) | |
| service_instance_id | string (nullable) | |
| span_name | string | operation name |
| span_kind | int32 | 0=UNSPECIFIED, 1=INTERNAL, 2=SERVER, 3=CLIENT, 4=PRODUCER, 5=CONSUMER |
| status_code | int32 | 0=UNSET, 1=OK, 2=ERROR |
| status_message | string (nullable) | |
| resource_attributes | string (nullable) | JSON — host.name, k8s.pod.name, service.version, etc. |
| scope_name | string (nullable) | |
| scope_version | string (nullable) | |
| scope_attributes | string (nullable) | JSON |
| span_attributes | string (nullable) | JSON |
| events_json | string (nullable) | JSON — span events (exceptions, annotations) embedded |
| links_json | string (nullable) | JSON — span links |

#### sigiro_logs (logs_schema)

| field | type | notes |
|---|---|---|
| timestamp | timestamp_us | log timestamp |
| observed_timestamp | int64 | when the log was observed |
| trace_id | string (nullable) | correlation |
| span_id | string (nullable) | correlation |
| service_name | string | from resource attributes |
| service_namespace | string (nullable) | |
| service_instance_id | string (nullable) | |
| severity_number | int32 | OTel severity number |
| severity_text | string | ERROR, WARN, INFO, etc. |
| body | string (nullable) | log message |
| resource_attributes | string (nullable) | JSON |
| scope_name | string (nullable) | |
| scope_version | string (nullable) | |
| scope_attributes | string (nullable) | JSON |
| log_attributes | string (nullable) | JSON |

#### sigiro_metrics_gauge (16 columns)

| field | type | notes |
|---|---|---|
| timestamp | timestamp_us | data point timestamp |
| start_timestamp | int64 (nullable) | |
| metric_name | string | |
| metric_description | string (nullable) | |
| metric_unit | string (nullable) | |
| value | float64 | gauge value |
| service_name | string | |
| service_namespace | string (nullable) | |
| service_instance_id | string (nullable) | |
| resource_attributes | string (nullable) | JSON |
| scope_name | string (nullable) | |
| scope_version | string (nullable) | |
| scope_attributes | string (nullable) | JSON |
| metric_attributes | string (nullable) | JSON |
| flags | int32 (nullable) | |
| exemplars_json | string (nullable) | JSON |

#### sigiro_metrics_sum (18 columns)

All gauge columns plus:

| field | type | notes |
|---|---|---|
| aggregation_temporality | int32 | 0=UNSPECIFIED, 1=CUMULATIVE, 2=DELTA |
| is_monotonic | bool | |

#### sigiro_metrics_histogram (22 columns)

| field | type | notes |
|---|---|---|
| timestamp | timestamp_us | |
| start_timestamp | int64 (nullable) | |
| metric_name | string | |
| metric_description | string (nullable) | |
| metric_unit | string (nullable) | |
| count | int64 | |
| sum | float64 (nullable) | |
| min | float64 (nullable) | |
| max | float64 (nullable) | |
| bucket_counts | string | JSON array |
| explicit_bounds | string | JSON array |
| service_name | string | |
| service_namespace | string (nullable) | |
| service_instance_id | string (nullable) | |
| resource_attributes | string (nullable) | JSON |
| scope_name | string (nullable) | |
| scope_version | string (nullable) | |
| scope_attributes | string (nullable) | JSON |
| metric_attributes | string (nullable) | JSON |
| flags | int32 (nullable) | |
| exemplars_json | string (nullable) | JSON |
| aggregation_temporality | int32 | |

#### sigiro_metrics_exp_histogram (27 columns)

Histogram columns plus:

| field | type | notes |
|---|---|---|
| scale | int32 | |
| zero_count | int64 | |
| zero_threshold | float64 (nullable) | |
| positive_offset | int32 (nullable) | |
| positive_bucket_counts | string (nullable) | JSON |
| negative_offset | int32 (nullable) | |
| negative_bucket_counts | string (nullable) | JSON |

otlp2records outputs separate RecordBatches per metric type (each with a different Arrow schema).
`sigiro_ingest` inserts each into its matching per-type table — zero-copy, no schema projection, no NULL padding.

---

### Query-time Mappings

`sigiro_investigate` maps from native schema to the [[architecture/investigation.md|InvestigationBlock]] JSON contract:

```sql
-- Status: int → string
CASE status_code WHEN 2 THEN 'ERROR' WHEN 1 THEN 'OK' ELSE 'UNSET' END AS status

-- Duration: nanos → ms
duration / 1000000.0 AS duration_ms

-- Span kind: int → string
CASE span_kind WHEN 1 THEN 'INTERNAL' WHEN 2 THEN 'SERVER' WHEN 3 THEN 'CLIENT'
  WHEN 4 THEN 'PRODUCER' WHEN 5 THEN 'CONSUMER' ELSE 'UNSPECIFIED' END AS kind

-- Host/pod: extracted from resource_attributes JSON
json_extract_string(resource_attributes, '$.host.name') AS host
json_extract_string(resource_attributes, '$.k8s.pod.name') AS pod
json_extract_string(resource_attributes, '$.service.version') AS deploy_version
```

---

### Span Events

otlp2records stores span events as `events_json` on the trace row (not as separate log rows). For MVP, `sigiro_investigate` extracts span events at query time. Post-MVP: consider a post-ingest step to emit separate log rows.

---

### What One API Call Looks Like

Agent asks: "what happened to checkout at 14:10?"

The query layer runs parallel queries — one per [[architecture/ducklake-integration.md|DuckLake]] table — with appropriate structuring (metric aggregation, span tree assembly, log capping). Plus a follow-up for linked traces/logs across services. Returns a structured [[architecture/investigation.md|investigation]] block. See investigation.md.

The per-signal tables make this efficient: each query hits only its own table with sort order `(service_name, timestamp)`. [[architecture/ducklake-integration.md|DuckLake]] manages row group pruning.

---

### Why Per-Signal Tables Work

- One API call returns all signals structured for agent consumption — parallel queries against [[architecture/ducklake-integration.md|DuckLake]] tables, no manual stitching
- Each table has its own sort order `(service_name, timestamp)` — no wasted row group reads for other signal types
- Linked traces follow error trace_ids across services in one follow-up query joining `sigiro_spans` and `sigiro_logs` — the agent sees the full cross-service call chain
- Adding a new signal type = adding a new [[architecture/ducklake-integration.md|DuckLake]] table with the appropriate schema
- Four per-type metrics tables — zero-copy insert (no schema projection, no NULL padding), each table's schema matches its otlp2records RecordBatch exactly

---

### Compression Strategy

Agent hides decompression [[company/cost.md|cost]] behind LLM inference (2-10s per reasoning step). this means:
- zstd compression — [[architecture/ducklake-integration.md|DuckLake]] configures compression on the underlying Parquet files
- **Delta binary packed encoding on `timestamp`** — sorted timestamps compress to near-zero
- **Bloom filters on `trace_id`** — 10 bits per value. Enables row-group-level pruning for linked trace queries (300-800ms vs 1-3s full scan)
- **Sorting columns declared in Parquet metadata** — `(service_name, timestamp)` sort order per table
- Dictionary encoding on low-cardinality columns (`service_name`, `severity_text`, `metric_name`, `span_name`)
- Target 8-12x compression ratios vs typical 2-3x

---

### PROBLEM AREA: Dependency Graph Is Missing

**Decision: derive at query time (option 2) for POC. revisit with dependency signal rows (option 1) only if multi-hop discovery becomes a bottleneck in practice.**

**Why this isn't as bad as it looks:**

- The actual [[company/cost.md|cost]] is one extra query, not N. `WHERE service_name = 'checkout' AND span_kind = 3` (CLIENT) returns spans with downstream service names in `peer.service` / `server.address` from `span_attributes`. That's 2 queries total.
- Trace-based [[architecture/investigation.md|investigation]] (`WHERE trace_id = 'abc123'`) already gives the full cross-service call chain with no graph needed.
- Single-service [[architecture/investigation.md|investigation]] (the most common case overall) doesn't need the graph at all.

**Where it does hurt:**

- Large time windows — scanning spans to extract unique downstream services over 24h is expensive. For a 5-minute [[architecture/investigation.md|investigation]] window, negligible.
- Multi-hop chains — checkout → payments → stripe-gateway → ... requires iterative discovery.
- Cold start — first question about any service requires edge discovery.

---

### Sort Order in Parquet Files

```
sort: service_name, timestamp   (per DuckLake table)
```

This gives:
- All rows for a service are contiguous (best for the common case)
- Within a service, time-ordered (range scans are sequential)
- Signal dimension is implicit in the table — no need to sort on it

[[architecture/ducklake-integration.md|DuckLake]] declares this sort order in Parquet file metadata so DuckDB can leverage row group min/max stats for pruning.
