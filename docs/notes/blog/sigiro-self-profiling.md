---
title: "Sigiro's Self-Profiling Loop: Watching the Watcher Profile Itself"
description: A senior SRE who never sleeps, profiling itself while you sleep.
---

# Sigiro's Self-Profiling Loop: Watching the Watcher Profile Itself

*A senior SRE who never sleeps, profiling itself while you sleep.*

---

## The Blind Spot

Most profiling pipelines profile your services. But who profiles the profiler?

Sigiro is an observability pipeline that receives OpenTelemetry data, stores it, and serves investigative queries. It can also profile itself. When `SIGIRO_PYROSCOPE_ENDPOINT` is set, Sigiro runs a continuous CPU and memory profiler powered by the pyroscope-rs SDK, sending profile data to the same ingestion endpoint it exposes. Sigiro investigating Sigiro, with the same tool, the same schema, the same query API.

This is not a demo feature. It is how we catch hot spots in the ingest path, find memory growth in long-running connections, and verify that the pipeline is not profiling itself into overhead.

---

## The Architecture: Sigiro's Self-Profiling Loop

```
Pyroscope Agent (sigiro) --[HTTP, gzip PushRequest]--> Sigiro /ingest/pyroscope
                                                               |
                                                               v
                                                      transform_pyroscope_profiles()
                                                               |
                                                               v
                                                   lake.sigiro_profiles (Arrow/DuckDB)
                                                               |
                                                               v
                                          POST /v1/diagnose --> profile data in response
```

The loop is self-contained. The Pyroscope agent ships profile samples over HTTP to Sigiro's own `/ingest/pyroscope` endpoint. Sigiro decodes the protobuf, transforms it into Arrow RecordBatches via `transform_pyroscope_profiles()`, and writes rows to `lake.sigiro_profiles`. The investigate endpoint reads from that same table and returns profile summaries alongside spans, logs, and metrics.

Two instances are in play: the **worker** runs the Pyroscope agent and ships profiles. The **test instance** receives them, stores them, and serves investigate queries. This separation keeps the profiling path off the hot path of the pipeline under test.

---

## How Self-Profiling Works: Four Technical Steps

### Step 1: Enabling with `SIGIRO_PYROSCOPE_ENDPOINT`

The feature is opt-in. Set one environment variable and profiling starts at launch:

```bash
export OTEL_SERVICE_NAME="sigiro"
export SIGIRO_PYROSCOPE_ENDPOINT="http://localhost:4040"
./sigiro
```

`SIGIRO_PYROSCOPE_ENDPOINT` is defined in `src/config.rs` as an optional variable with an empty default. If it is unset, `start_profiling()` in `src/profiling.rs` returns `Ok(None)` and the admin DB records `pyroscope_status: "disabled"`. No profiling runs, no overhead.

The service name tag comes from `OTEL_SERVICE_NAME` (default: `"sigiro"`). All profile samples are tagged with this name, so a multi-service Pyroscope deployment can separate profiles from different instances.

### Step 2: Pyroscope Agent Startup

The agent starts in `src/main.rs` before the HTTP server begins accepting traffic:

```rust
let _profiling = match start_profiling() {
    Ok(Some(handle)) => {
        sigiro::admin::write_instance_status(&admin_conn, "pyroscope_status", "active")?;
        Some(handle)
    }
    Ok(None) => {
        sigiro::admin::write_instance_status(&admin_conn, "pyroscope_status", "disabled")?;
        None
    }
    Err(e) => {
        tracing::warn!("Pyroscope profiling failed to start: {e:#}");
        sigiro::admin::write_instance_status(&admin_conn, "pyroscope_status", "error")?;
        None
    }
};
```

`start_profiling()` in `src/profiling.rs` builds a pprof backend at 100 Hz and connects to the endpoint:

```rust
let backend = pyroscope::backend::pprof_backend(
    pyroscope::backend::pprof::PprofConfig::default(),
    pyroscope::backend::BackendConfig::default(),
);

let agent = pyroscope::pyroscope::PyroscopeAgentBuilder::new(
    &endpoint,
    &service_name,
    100, // sample rate Hz
    "pyroscope-rs",
    env!("CARGO_PKG_VERSION"),
    backend,
)
.build()
.context("failed to build Pyroscope agent")?;

let agent = agent.start().context("failed to start Pyroscope agent")?;
```

The agent must be kept alive. `ProfilingHandle` holds `PyroscopeAgentRunning` and drops only when Sigiro exits.

### Step 3: Ingest and Transform

Sigiro's `/ingest/pyroscope` endpoint accepts gzipped `PushRequest` protobuf (application/x-protobuf) or multipart/form-data from the Pyroscope Go/Python SDKs. `parse_pyroscope_multipart()` in `src/ingest.rs` handles SDK-format payloads by extracting the `profile` form field, then passes raw bytes to `transform_pyroscope_profiles()`:

```rust
pub async fn transform_pyroscope_profiles(body_bytes: Vec<u8>) -> Result<RecordBatch> {
    // Stage 1: gunzip
    let decompressed = decode_gzip(&body_bytes)?;
    // Stage 2: protobuf decode
    let push_request = pyroscope::encode::r#gen::push::PushRequest::decode(&decompressed[..])?;
    // Stage 3: pprof -> Arrow RecordBatch
    pprof_to_arrow(push_request)
}
```

The transform maps pprof Profile data into the `sigiro_profiles` schema:

```sql
CREATE TABLE IF NOT EXISTS lake.sigiro_profiles (
    profile_id            VARCHAR,
    trace_id              VARCHAR,
    span_id               VARCHAR,
    service_name         VARCHAR NOT NULL,
    timestamp            TIMESTAMP NOT NULL,
    profile_type         VARCHAR,
    stack_trace          VARCHAR[],
    value                BIGINT,
    unit                 VARCHAR,
    resource_attributes  VARCHAR
)
```

Stack traces are stored as `VARCHAR[]` arrays, one array per sample. The function name is the last element of the array.

### Step 4: Querying Profiles via `/v1/diagnose`

Profile data appears in the investigate response under `profiles`:

```json
{
  "sigiro": {
    "coverage": {
      "profile_sample_count": [X],
      "has_profiles": true
    },
    "profiles": [
      {
        "function": "handle_ingest",
        "profile_type": "cpu",
        "total_value": [X],
        "unit": "samples",
        "sample_count": [X]
      },
      {
        "function": "db_batch_insert",
        "profile_type": "cpu",
        "total_value": [X],
        "unit": "samples",
        "sample_count": [X]
      }
    ]
  }
}
```

`query_profiles()` in `src/investigate/mod.rs` aggregates by function name and profile type, ordering by total value descending. This gives you the top hot spots in the ingest path, queryable in the same response as span latency percentiles and error rates.

---

## What Gets Profiled

The Pyroscope agent captures CPU time and memory allocations at 100 Hz. Every sample includes:

- **Stack trace** — a list of function names from leaf to root
- **Value** — sample count for CPU; byte count for memory
- **Profile type** — `cpu` or `memory`
- **Unit** — `samples` for CPU; `bytes` for memory allocations

The pprof backend uses the standard pprof format, which means the data is portable. You can point `go tool pprof` or any pprof-compatible viewer at the same endpoint if you add `GET /debug/pprof/profile`.

---

## Investigate Output: A Real Template

Here is the structure of a full investigate response for `service=sigiro` when profiling is enabled:

```json
{
  "service": "sigiro",
  "sigiro": {
    "coverage": {
      "profile_sample_count": [X],
      "has_profiles": true,
      "has_spans": true,
      "has_logs": false
    },
    "profiles": [
      {
        "function": "handle_ingest",
        "profile_type": "cpu",
        "total_value": [X],
        "unit": "samples",
        "sample_count": [X]
      },
      {
        "function": "pool::get_or_create",
        "profile_type": "cpu",
        "total_value": [X],
        "unit": "samples",
        "sample_count": [X]
      },
      {
        "function": "flusher_loop",
        "profile_type": "cpu",
        "total_value": [X],
        "unit": "samples",
        "sample_count": [X]
      }
    ]
  }
}
```

Placeholder `[X]` values must be replaced with real data from an actual profiling run before publishing.

---

## What This Means for You

When Sigiro profiles itself:

- **No separate profiling infrastructure** — one binary, one endpoint, one database
- **Hot spots in the ingest path are visible** — `handle_ingest`, `db_batch_insert`, `pool::get_or_create` show up in the same query as error spans
- **Memory growth is traceable** — memory allocation profiles identify where heap usage grows under load

---

*Sigiro is open source. The profiling code is in `src/profiling.rs`.*
