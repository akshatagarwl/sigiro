---
title: OTLP Profiles — Design
created: 2026-03-28
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'design', 'duckdb', 'investigation']
---

# OTLP Profiles — Design

High priority. Profiling is a differentiator — most observability tools skip it.

## What

Implement ProfilesService gRPC on :4317. Receive OTel pprofile data,
resolve dictionary-encoded function names + stack frames, store as rows in [[architecture/data-model.md|DuckDB]].

## pprofile Format (OTel profiling spec, stable 2024)

pprofile uses dictionary encoding to compress repeated strings:

```
ProfilesData {
  resource_profiles: [{
    resource: { attributes: [...] },
    scope_profiles: [{
      profiles: [{
        // Dictionary tables (shared across all samples)
        string_table: ["", "main", "json_serialize", "serde::to_string", ...],
        function: [{ name: 1, filename: 5 }, ...],  // indexes into string_table
        location: [{ function_index: 0, line: 42 }, ...],
        
        // Actual samples
        sample: [{
          location_index: [2, 1, 0],  // stack: to_string → json_serialize → main
          value: [28000000],           // 28ms CPU time
        }]
      }]
    }]
  }]
}
```

## Pipeline Placement

Same as traces/logs: ingest.rs → channel → inserter.rs → [[architecture/data-model.md|DuckDB]]

```
ProfilesService gRPC ──→ resolve dictionary ──→ flatten to rows ──→ channel ──→ DuckDB
```

Dictionary resolution happens at ingest time (before channel send):
- Walk each sample's location_index chain
- Resolve function names from string_table
- Produce flat rows with resolved strings

## Storage Schema

```sql
CREATE TABLE sigiro_profiles (
  profile_id            VARCHAR,           -- unique profile ID
  trace_id              VARCHAR,           -- optional OTLP Sample.link_index trace
  span_id               VARCHAR,           -- optional OTLP Sample.link_index span
  service_name          VARCHAR NOT NULL,
  timestamp             TIMESTAMP NOT NULL,
  profile_type          VARCHAR,           -- 'cpu', 'alloc_space', 'alloc_objects', 'goroutine', 'contentions', 'wall', 'off_cpu'
  stack_trace           VARCHAR[],         -- resolved function names, root first
  value                 BIGINT,            -- sample value (ns for cpu, bytes for heap, count for goroutines)
  unit                  VARCHAR,           -- 'nanoseconds', 'bytes', 'count'
  resource_attributes   VARCHAR
);
```

Schema handles all profile types — CPU, heap, goroutine, lock contention, wall, off-CPU.
For native pprof profiles with parallel sample types (for example `alloc_objects`,
`alloc_space`, `inuse_objects`, and `inuse_space`), write one row per type/value
pair with the same stack trace. OTLP sends each profile type as a separate
`Profile` message.

No raw pprof blob storage needed — flamegraph viewers (speedscope, d3-flame-graph) accept
collapsed format which is one SQL query: `array_to_string(stack_trace, ';') || ' ' || value`.

[[architecture/profiles.md|Pyroscope]] SDK profiles (Phase 2) use the same table — their `/ingest` format is collapsed
stacks that map directly to rows.
```

## Query (investigate.rs)

Flamegraph data:
```sql
SELECT stack_trace, SUM(value) as total
FROM sigiro_profiles
WHERE service_name = ? AND timestamp BETWEEN ? AND ?
  AND profile_type = 'cpu'
GROUP BY stack_trace
ORDER BY total DESC
```

Top functions:
```sql
SELECT stack_trace[1] as function, SUM(value) as self_time
FROM sigiro_profiles
WHERE service_name = ? AND profile_type = 'cpu'
GROUP BY function
ORDER BY self_time DESC
LIMIT 20
```

## Correlation with Traces

OTLP Profiles supports direct request correlation: each `Sample` can point at
`ProfilesDictionary.link_table` through `sample.link_index`; the linked `Link`
contains the trace and span IDs. Sigiro stores those as nullable `trace_id` and
`span_id` columns on every profile sample row.

Direct trace-to-profile query:
```sql
SELECT array_to_string(stack_trace, ';') AS folded,
       SUM(value) AS total
FROM sigiro_profiles
WHERE trace_id = ? AND profile_type = 'cpu'
GROUP BY folded
ORDER BY total DESC
```

There is no service/resource/time fallback for trace-to-profile correlation.
Profile rows without an OTLP sample link keep `trace_id` and `span_id` null, so
queries must not infer trace ownership for pprof/Pyroscope payloads.

## Files to Change

- `src/server.rs` — register ProfilesService on :4317 tonic server
- `src/ingest.rs` — handle ExportProfilesServiceRequest, resolve dictionary, send to channel
- `src/inserter.rs` — flush profile rows to DuckDB
- `src/schema.rs` — CREATE TABLE sigiro_profiles
- `src/investigate.rs` — add profile queries to InvestigationBlock
- New: proto dependencies for opentelemetry-proto profiles (check otlp2records support)

## Open Questions

- Does otlp2records handle profiles? If not, dictionary resolution is manual (~50 lines).
- Stack depth limit? Cap at 128 frames to prevent pathological cases.
- Flamegraph rendering: Phase 3 has "GET :9999/flamegraph → d3-flame-graph HTML". 
  This phase just stores the data.

## Estimate: ~100 lines Rust (+ schema SQL)
