---
title: Profiles
created: 2026-03-05
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'comparison', 'competitor', 'data-model', 'deployment']
---

<!-- Updated 2026-03-20: Reflects otlp2records native schema, flamegraph-capable design. -->

## Profiles

do|Profiling data in the `sigiro_profiles` [[architecture/ducklake-integration.md|DuckLake]] table.

---

### Schema Requirement: Flamegraph-Capable

The profile schema MUST store full ordered stacktraces to support flamegraph rendering.
This is a hard requirement, not optional.

```sql
CREATE TABLE IF NOT EXISTS lake.sigiro_profiles (
    profile_id            VARCHAR,
    trace_id              VARCHAR,               -- optional OTLP sample link
    span_id               VARCHAR,               -- optional OTLP sample link
    service_name          VARCHAR NOT NULL,
    timestamp             TIMESTAMP NOT NULL,
    profile_type          VARCHAR,               -- cpu, alloc_space, etc.
    stack_trace           VARCHAR[],             -- ordered frames, caller-first
    value                 BIGINT,                -- sample count or bytes
    unit                  VARCHAR,               -- count, bytes, nanoseconds
    resource_attributes   VARCHAR                -- JSON
);
```

**Key design choices:**
- `stack_trace` is `VARCHAR[]` with ordered frames — this is the minimum needed for flamegraphs
- One row per sample (not per profile). Each sample carries its full resolved stack.
- `profile_type` is explicit (not overloaded on `name`) — required to filter CPU vs heap
- `trace_id` and `span_id` come only from OTLP `Sample.link_index -> ProfilesDictionary.link_table`;
  rows without a sample link keep both fields null. Sigiro does not infer trace ownership from
  service/resource/time overlap.

---

### Flamegraph Queries

**Top functions (leaf-only, fast triage):**
```sql
SELECT stack_trace[len(stack_trace)] AS function, sum(value) AS samples
FROM ducklake.sigiro_profiles
WHERE service_name = $service AND profile_type = 'cpu'
  AND timestamp BETWEEN $t1 AND $t2
GROUP BY function ORDER BY samples DESC LIMIT 10
```

**Full flamegraph (folded stacks):**
```sql
SELECT array_to_string(stack_trace, ';') AS folded,
       sum(value)::BIGINT AS count
FROM ducklake.sigiro_profiles
WHERE service_name = $service AND profile_type = 'cpu'
  AND timestamp BETWEEN $t1 AND $t2
GROUP BY folded
```

Output format: `main;handleRequest;queryDB 42` — one line per unique stack. Feeds:

| renderer | format | notes |
|---|---|---|
| flamegraph.pl | `a;b;c 42\n` | Brendan Gregg's SVG generator |
| d3-flame-graph | hierarchical JSON | Tree construction from folded stacks (~30 lines) |
| speedscope | SampledProfile JSON | Indices into shared frames array |

**Profile diff:** Same query over two time windows, compare stacks.

---

### Ingestion Path (future)

```
profiler (eBPF / SDK)
    ↓ OTLP gRPC :4317
sigiro_ingest
    ├─ resolve pprofile Dictionary → function names
    ├─ one row per sample with full stack_trace
    └─ INSERT INTO ducklake.sigiro_profiles
```

**Implementation:** Sigiro handles Pyroscope profiles directly via the ingest endpoint. The Pyroscope SDK sends profiles as `application/x-protobuf` (pprof format) which is parsed and stored.

---

### Multi-Value Profile Samples

Native pprof profiles pair each `Profile.sample_type` entry with the value at the
same index in every `Sample.value`. Heap profiles commonly carry allocation and
in-use object and byte values together. Sigiro writes one row per pair and rejects
the complete request when the type and value counts differ.

OTLP Profiles uses one `sample_type` per `Profile`; repeated sample values are
events of that same type. Producers send separate OTLP `Profile` messages for
different value types.

### Known Gaps (carry forward from Go codebase)

#### Gap 1: Dictionary location assumption

Mapper assumes Dictionary is at top-level Profiles container (pprofile v0.147.0 API).
If future version moves dictionaries per-Profile, index lookups silently return wrong data.

#### Gap 2: Profile metadata not populated

Profilers carry metadata (language runtime, GC state, thread ID, Pyroscope labels).
Need to extract profile-level attributes from pprofile into `profile_attributes` JSON.

---

### Profiler Compatibility

#### Works today (OTLP)

| profiler | protocol | notes |
|---|---|---|
| **OTel eBPF profiler** | OTLP gRPC | Zero instrumentation, kernel-level CPU |
| **Grafana Alloy eBPF** | OTLP gRPC | Same eBPF approach, more mature |

#### Works with configuration

| profiler | how | limitations |
|---|---|---|
| **Pyroscope SDK** (Go, Java, Python, .NET, Ruby) | OTLP export | Experimental `/v1development/profiles` path |
| **Pyroscope eBPF** | OTLP bridge | Same experimental caveats |

#### Does NOT work

| profiler | why | alternative |
|---|---|---|
| **Parca Agent** | Own gRPC protocol, `--otlp-address` is for internal telemetry traces only | Use OTel eBPF profiler |
| **Beyla** | Metrics + traces only, no profiles | Not a profiler |
| **Linux perf** | Text output, not OTLP | Future: collapsed-stack HTTP endpoint |

---

### Collapsed-Stack HTTP Endpoint (future)

Accept Brendan Gregg's folded format (`func1;func2;func3 count\n`). Unlocks `perf script`,
manual upload, any folded-stack tool. ~100-150 lines. Not needed until we have users who can't use [[architecture/write-path.md|OTLP]].
