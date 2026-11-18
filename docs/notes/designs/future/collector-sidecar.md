---
title: Managed OTel Collector Architecture
created: 2026-04-08
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'data-model', 'decision', 'deployment']
---

# Managed OTel Collector Architecture

## Status: UNVALIDATED DESIGN

**Concern:** running a separate otel-collector-contrib process per project introduces:
- Extra process overhead (memory per collector instance)
- Serialization [[company/cost.md|cost]] (collector receives [[architecture/write-path.md|OTLP]], processes, re-serializes OTLP to send to Sigiro)
- Network hop (even if localhost:4317)
- Operational complexity (process lifecycle, crash recovery, resource limits)
- Potential perf penalty under high throughput

**This architecture needs critique before committing.** See "Alternatives" section below.

## Proposed Architecture

```
sigiro serve
  ├── HTTP server (:9999)
  ├── OTLP ingest (:4317, :4318)  ← receives from collectors
  ├── DuckDB pool (DuckLake + DuckDB-native catalog)
  └── Collector Manager
        ├── otelcol-contrib --config project_abc.yaml  → exports to localhost:4317
        └── otelcol-contrib --config project_def.yaml  → exports to localhost:4317
```

### How it works

1. User uploads `config.yaml` via `POST /collectors/{project_id}/deploy`
2. Config is validated first (see `config-validation.md`)
3. Sigiro's CollectorManager spawns `otelcol-contrib --config /path/to/config.yaml`
4. Collector's exporter is injected to point at `localhost:4317` (Sigiro ingest)
5. User's app sends to collector's receiver port
6. Collector processes, exports [[architecture/write-path.md|OTLP]] to Sigiro
7. Sigiro writes to [[architecture/data-model.md|DuckDB]] as usual

### User controls
- Receivers (otlp, syslog, filelog, prometheus, etc.)
- Processors (transform, filter, attributes, batch, etc.)
- Pipeline wiring

### Sigiro controls
- Exporter (always [[architecture/write-path.md|OTLP]] to localhost:4317, injected)
- Collector process lifecycle
- Resource limits per project

### Data flow with transforms
```
User's app → otelcol subprocess (user's config) → localhost:4317 → [[architecture/write-path.md|src/otlp.rs]] Arrow RecordBatch builders → DuckDB
```

### Data flow without transforms (today's behavior)
```
User's app → Sigiro :4317 → [[architecture/write-path.md|src/otlp.rs]] Arrow RecordBatch builders → DuckDB
```

## Implementation

### Docker image
```dockerfile
FROM otel/opentelemetry-collector-contrib:latest AS collector
FROM rust:1.xx AS builder
# ... existing build ...

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/sigiro /usr/local/bin/
COPY --from=collector /otelcol-contrib /usr/local/bin/
```

### CollectorManager (`collector_manager.rs`)
```rust
struct CollectorManager {
    collectors: DashMap<String, CollectorProcess>,
    sigiro_ingest_addr: String,  // "localhost:4317"
}

impl CollectorManager {
    fn deploy(&self, project_id: &str, config_yaml: &str) -> Result<()> {
        // 1. Inject exporter config pointing to sigiro ingest
        let config = inject_exporter(config_yaml, &self.sigiro_ingest_addr);
        // 2. Write config to /var/lib/sigiro/{project_id}/otelcol-config.yaml
        // 3. Spawn: otelcol-contrib --config /path/to/config.yaml
        // 4. Health check the collector's receiver port
        // 5. Store in DashMap
    }

    fn reload(&self, project_id: &str, config_yaml: &str) -> Result<()> {
        // SIGHUP for hot-reload, or respawn
    }
}
```

### Config injection
Sigiro's collector_manager injects the exporter and appends sigiro_defaults processor:

```yaml
# User sees this effective config
exporters:
  otlp:
    endpoint: localhost:4317
    headers:
      Authorization: "Bearer <oauth-access-token>"  # hosted Better Auth OAuth/JWT only

processors:
  sigiro_defaults:  # injected by Sigiro — ensures required fields survive
    # fallback: set service.name to "unknown" if missing

service:
  pipelines:
    logs:
      receivers: [otlp]
      processors: [transform, sigiro_defaults]  # Sigiro appends sigiro_defaults
      exporters: [otlp]
```

## Concerns to Validate

### Performance
- [ ] Memory: each otelcol-contrib process uses ~30-50MB base. With 100 projects, 3-5GB extra.
- [ ] CPU: protobuf serialize → deserialize → serialize cycle on localhost
- [ ] Latency: extra network hop even on localhost (~0.1ms?)
- [ ] Throughput: can the collector keep up with high-volume ingest? Benchmark needed.

### Operational
- [ ] Process lifecycle: what if collector crashes? Restart policy?
- [ ] Resource limits: how to cap per-project collector CPU/memory?
- [ ] Logging: collector logs mixed with Sigiro logs?
- [ ] Version management: which otelcol-contrib version? How to upgrade?

### Complexity
- [ ] Is this over-engineered for the current scale?
- [ ] Does the benefit (full OTel ecosystem) justify the overhead?
- [ ] Are there simpler alternatives?

## Alternatives to Critique

### A. OTTL-in-Rust
Reimplement OTTL in Rust. ~2-3k lines for core contexts. No subprocess, no serialization overhead. But: maintenance burden, always behind upstream OTTL.

### B. WASM transforms
Compile OTTL or user transforms to WASM. Load in-process. Sandboxed, hot-reloadable. But: WASM + [[architecture/write-path.md|OTLP]] data marshaling overhead, debugging is harder.

### C. Do nothing — transform in user's collector
Users run their own collector fleet, point at Sigiro's [[architecture/write-path.md|OTLP]] endpoint. Sigiro stays simple. Downside: no "deploy from UI" story, users manage their own infra.

### D. Query-time virtual columns
No ingestion-side transforms. Let users define [[architecture/data-model.md|DuckDB]] computed columns at query time. Example: `ALTER TABLE sigiro_logs ADD COLUMN http_method AS (json_extract(log_attributes, '$.http.method'))`. No process overhead but doesn't help with filtering/reduction at ingest.

### E. Inline transform engine (hybrid)
- Embed a lightweight OTTL evaluator in Sigiro's ingest path. Not full otelcol — just the transform processor's logic. Runs in the same process, on the tokio blocking pool (like [[architecture/write-path.md|src/otlp.rs]] does today). No subprocess, no localhost hop, no serialization overhead. Most promising if the perf concern is real.

## Decision

TODO — needs benchmarking and architecture review before committing to any approach.
