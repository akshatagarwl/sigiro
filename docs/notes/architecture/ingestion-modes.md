---
title: Ingestion Modes
created: 2026-05-28
updated: 2026-05-28
type: concept
tags: ['architecture', 'otel-collector', 'otlp', 'ingestion']
---

## Ingestion Modes

Sigiro supports two ways to receive telemetry: **native OTLP** (direct) and **OTel Collector bridging** (intermediate). Both deliver the same data to the same tables. The difference is whether an OpenTelemetry Collector sits between your services and Sigiro.

---

### Mode 1: Native OTLP (Recommended)

Send telemetry directly from your OTel SDKs, agents, or eBPF instrumentation to Sigiro's OTLP endpoints. No intermediate collector.

```
┌─────────────┐     ┌─────────────┐
│  Your App   │────▶│   Sigiro    │
│  (OTel SDK) │ OTLP│ :4317/:4318 │
└─────────────┘     └─────────────┘
```

**When to use:**
- Greenfield deployments where you control the OTLP endpoint configuration
- Environments where you want the fewest moving parts
- Dev, test, or small production fleets

**Configuration (hosted TenantKeys only; omit the header for a local Open server):**
```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://sigiro:4318
OTEL_EXPORTER_OTLP_HEADERS="authorization=Bearer sk_<tenant>_<random>"  # hosted TenantKeys only
```

Sigiro listens on:
- `:4317` — OTLP gRPC (protobuf)
- `:4318` — OTLP HTTP (protobuf + JSON)

Both accept traces, logs, metrics, and profiles. Open mode accepts requests without per-request authentication. Hosted TenantKeys mode requires a bearer key in `authorization` metadata (gRPC) or an `Authorization: Bearer` header (HTTP).

---

### Mode 2: OTel Collector Bridge

Route telemetry through the OpenTelemetry Collector first, then forward to Sigiro. The collector can enrich, filter, batch, or route to multiple backends simultaneously.

```
┌─────────────┐     ┌──────────────────┐     ┌─────────────┐
│  Your App   │────▶│ OTel Collector   │────▶│   Sigiro    │
│  (OTel SDK) │ OTLP│ (routing/filter) │ OTLP│ :4317/:4318 │
└─────────────┘     └──────────────────┘     └─────────────┘
```

**When to use:**
- Brownfield deployments that already run an OTel Collector
- You need collector features: tail-based sampling, attribute enrichment, multi-backend routing
- Gradual migration from Jaeger/Tempo/Zipkin — collector fans out to old + new backends
- Kubernetes clusters where the collector is deployed as a DaemonSet or sidecar

**Collector configuration:**

```yaml
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: 0.0.0.0:4317
      http:
        endpoint: 0.0.0.0:4318

exporters:
  otlphttp/sigiro:
    endpoint: http://sigiro:4318
    encoding: json
    compression: none
    headers:
      Authorization: Bearer <sigiro-key>
    tls:
      insecure: true

service:
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [otlphttp/sigiro]
    metrics:
      receivers: [otlp]
      exporters: [otlphttp/sigiro]
    logs:
      receivers: [otlp]
      exporters: [otlphttp/sigiro]
```

**Important:** Sigiro's OTLP HTTP endpoint currently requires:
- `encoding: json` — Sigiro accepts JSON OTLP payloads
- `compression: none` — gzip/deflate is not yet supported on the HTTP path

If your SDK defaults to gRPC, point it at `:4317` instead. The collector can receive gRPC from apps and forward HTTP to Sigiro (or vice versa).

---

### Comparison

| Aspect | Native OTLP | OTel Collector Bridge |
|--------|-------------|----------------------|
| Components | App → Sigiro | App → Collector → Sigiro |
| Latency | Lower (one hop) | Higher (two hops + batching) |
| Reliability | SDK retry + Sigiro backpressure | Collector retry queue + Sigiro backpressure |
| Enrichment | None (SDK attributes only) | Collector processors (resource, k8s, host, etc.) |
| Sampling | SDK-side only | Tail-based sampling in collector |
| Multi-backend | No | Yes (collector fans out) |
| Compression | zstd (gRPC), none (HTTP) | Configurable per exporter |
| JSON encoding | Supported (:4318) | Required for HTTP exporter to Sigiro |

---

### OTel Collectors are an ingestion detail, not a federation mechanism

An OTel Collector bridge (either mode above) only decides how telemetry
*reaches* a given Sigiro node — it has nothing to do with querying across
multiple Sigiro nodes. There is no "central Sigiro instance" that collectors
forward to; each self-hosted Sigiro node ingests into its own local
DuckLake, whether it's fed directly or via a Collector in front of it.

If you're running more than one self-hosted node and want to query across
all of them, that's a separate, opt-in feature — see
[[guides/self-host-edge-federation.md|Self-Host Edge Federation]].

---

### Migration Path

Start with **native OTLP** for simplicity. Add an **OTel Collector** later when you need:
- Sampling, enrichment, or routing logic
- A buffer against Sigiro downtime
- Compatibility with existing collector-based infra

The switch is transparent — both modes write to the same `sigiro_spans`, `sigiro_logs`, `sigiro_metrics_*`, and `sigiro_profiles` tables. No data migration, no schema changes.
