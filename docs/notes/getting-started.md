---
title: Getting Started
created: 2026-03-01
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'data-model', 'deployment', 'duckdb']
---

<!-- Updated 2026-04-10: [[architecture/ducklake-integration.md|DuckLake]] default storage, SQLite admin DB, corrected deploy examples. -->

# Getting Started

One binary. Three ports. Open-by-default self-hosting, with tenant keys for hosted deployments.

---

## Step 1: Run Sigiro

Sigiro is a single Rust binary embedding [[architecture/data-model.md|DuckDB]] with [[architecture/ducklake-integration.md|DuckLake]] storage (SQLite catalog + local [[architecture/data-model.md|Parquet]]). No OTel Collector, no external dependencies.

### Bare Metal / Dev (simplest)

```bash
cargo run -- serve
# Listening: gRPC :4317, OTLP HTTP :4318, API :9999
# Open mode: no per-request authentication; secure this server with your network perimeter.
```

Local and self-hosted servers run in Open mode: do not set a server API key. Use a tailnet, private network, or reverse proxy as the security boundary. Hosted servers enable TenantKeys when `SIGIRO_CATALOG_URL` is configured; their control plane mints per-tenant keys shaped `sk_<tenant>_<random>`.

### Docker Compose (recommended)

```yaml
services:
  sigiro:
    image: ghcr.io/sigiroai/sigiro:latest
    ports:
      - "4317:4317"  # OTLP gRPC (traces/logs/metrics)
      - "4318:4318"  # OTLP HTTP (traces/logs/metrics)
gm|      - "4040:4040"  # Pyroscope profiles ingest
      - "9999:9999"  # SQL API + admin endpoints
    volumes:
      - sigiro-data:/var/sigiro/data

volumes:
  sigiro-data:
```

[[architecture/ducklake-integration.md|DuckLake]] stores data as `catalog.sqlite` (schema) + `catalog.ducklake.files/` ([[architecture/data-model.md|Parquet]]) inside the data dir. The volume preserves both.

### Kubernetes

```yaml
# Simple pod with PVC
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: sigiro-data
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: 10Gi
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: sigiro
spec:
  replicas: 1
  selector:
    matchLabels:
      app: sigiro
  template:
    metadata:
      labels:
        app: sigiro
    spec:
      containers:
        - name: sigiro
          image: ghcr.io/sigiroai/sigiro:latest
          ports:
            - containerPort: 4317  # OTLP gRPC
            - containerPort: 4318  # OTLP HTTP
            - containerPort: 9999  # API
          env:
            - name: SIGIRO_DATA_DIR
              value: /var/lib/sigiro/data
          volumeMounts:
            - name: data
              mountPath: /var/lib/sigiro
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: sigiro-data
```

### Bare Metal / VM

```bash
# Default: local disk at ~/.sigiro/data/
curl -fsSL sigiro.com/install | sh
systemctl enable --now sigiro

# Or set a custom data directory:
SIGIRO_DATA_DIR=/opt/sigiro/data systemctl enable --now sigiro
```

Storage is [[architecture/ducklake-integration.md|DuckLake]] by default — no extra flags needed. The server creates `catalog.sqlite` + `catalog.ducklake.files/` on first start.

---

## Step 2: Instrument Your Services

Setting env vars alone won't emit telemetry. Your service needs an OTel SDK or agent loaded first — the env vars just tell it where to send data.

### Option A: eBPF auto-instrumentation (no SDK, no code changes, Linux only)

Traces HTTP, gRPC, SQL, DNS at the kernel level. Nothing in your app container.

```yaml
# docker-compose alongside Sigiro
ebpf-instrumentation:
  image: ghcr.io/open-telemetry/opentelemetry-ebpf-instrumentation:latest
  privileged: true
  pid: host
  network_mode: host
  environment:
    OTEL_EXPORTER_OTLP_ENDPOINT: http://sigiro:4317
```

Catches: HTTP client/server calls, gRPC, MySQL/PostgreSQL/MongoDB/Redis queries, DNS lookups, TCP connect events. Works on any language (Go, Java, Python, Rust, etc.) since it hooks syscalls, not your runtime.

Limitations: Linux only, needs `privileged: true` + `hostPID`, no custom span attributes.

### Option B: OTel SDK auto-instrumentation (any platform, richer data)

| Language | Install | Run |
|----------|---------|-----|
| Java | Download `opentelemetry-javaagent.jar` | `java -javaagent:opentelemetry-javaagent.jar -jar app.jar` |
| Python | `pip install opentelemetry-distro` | `opentelemetry-instrument python app.py` |
| Node.js | `npm install @opentelemetry/auto-instrumentations-node` | `node --require @opentelemetry/auto-instrumentations-node/register app.js` |
| Go | No auto-instrumentation — add OTel SDK manually (see below) | |
| .NET | `dotnet add package OpenTelemetry.AutoInstrumentation` | `OTEL_DOTNET_AUTO_HOME` + startup hook |
| Ruby | `gem install opentelemetry-instrumentation-all` | `require 'opentelemetry/instrumentation/all'` in your app |

Add custom spans, metrics, attributes. Works on macOS, Linux, Windows.

### Set the env vars

Once the agent/SDK is loaded, tell it where to send:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=https://<hosted-endpoint>:4318
OTEL_EXPORTER_OTLP_HEADERS="authorization=Bearer sk_<tenant>_<random>"
```

For a local Open server, no Authorization header is needed:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318
```

Your SDK auto-negotiates the best format:
- gRPC (:4317) → protobuf with `authorization` metadata
- HTTP (:4318) → protobuf or JSON with `Authorization: Bearer` header

### What you get automatically

With auto-instrumentation loaded, you get spans, metrics, and logs with zero code changes:

| Language | What you get OOTB |
|----------|-------------------|
| Java | HTTP spans, DB queries, JMS, gRPC, JVM metrics |
| Python | HTTP spans, DB queries, Redis, runtime metrics |
| Node.js | HTTP spans, Express routes, DB queries |
| Go | HTTP spans, gRPC, DB queries (manual SDK) |
| .NET | HTTP, DB, runtime metrics |
| Ruby | HTTP, Rails, Sidekiq |

### Add CPU profiling (separate from traces — flame graphs, not spans)

[[designs/future/ebpf-gpu-position.md|eBPF]] continuous profiler — samples CPU at the kernel level, no SDK, Linux only. Produces flame graphs, not trace spans.

```yaml
# Add alongside Sigiro in your docker-compose
ebpf-profiler:
  image: ghcr.io/open-telemetry/opentelemetry-ebpf-profiler:latest
  privileged: true
  pid: host
  environment:
    OTEL_EXPORTER_OTLP_ENDPOINT: http://sigiro:4317
```

Or SDK/agent-based profiling (any platform, including macOS):

**[[architecture/profiles.md|Pyroscope]] SDK** (HTTP :4318/ingest/pyroscope — pprof, JFR, folded stacks):

```bash
# Python
pip install pyroscope-io
pyroscope.configure(server_address="http://sigiro:4040", application_name="your-app")

# Go
pyroscope.Start(pyroscope.Config{ServerAddress: "http://sigiro:4040", ApplicationName: "your-app"})

# Java — JFR-based, zero code changes
java -javaagent:pyroscope.jar -Dpyroscope.serverAddress=http://sigiro:4040 -jar your-app.jar
```

Profiles via OTel eBPF profiler or [[architecture/profiles.md|Pyroscope]] SDK land in
the `sigiro_profiles` table. [[architecture/profiles.md|Parca]] agent uses its own gRPC protocol and is not supported — use the OTel eBPF profiler instead.

### Add custom instrumentation (Level 2: developer code)

```python
# Custom spans
with tracer.start_as_current_span("checkout.process_payment") as span:
    span.set_attribute("payment.method", "stripe")
    result = process_payment(order)

# Custom metrics
counter = meter.create_counter("orders.processed")
counter.add(1, {"status": "success"})
```

### What if you want to change what's collected?

All standard OTel SDK env vars — Sigiro doesn't control collection, the SDK does:

```bash
# Sampling — reduce trace volume
OTEL_TRACES_SAMPLER=parentbased_traceidratio
OTEL_TRACES_SAMPLER_ARG=0.1  # 10%

# Disable specific auto-instrumentations
OTEL_PYTHON_DISABLED_INSTRUMENTATIONS=urllib3,requests

# Resource attributes
OTEL_RESOURCE_ATTRIBUTES=service.name=checkout,service.version=v2.3.1
```

---

## Step 3: Investigate

### CLI

For a hosted endpoint, set the CLI's remote endpoint and tenant key (or pass `--key <token>`). A local Open server needs neither key.

```bash
export SIGIRO_ENDPOINT=https://<hosted-endpoint>
export SIGIRO_API_KEY=sk_<tenant>_<random>

# Investigate a service
sigiro diagnose checkout-service

# With time range
sigiro diagnose checkout --from 1711100000 --to 1711100900
```

### HTTP (curl, agents, any HTTP client)

```bash
# Check what services exist (local Open server)
curl -s http://localhost:9999/v1/services | jq .

# Investigate a service
curl -s \
  -X POST http://localhost:9999/v1/diagnose \
  -H "Content-Type: application/json" \
  -d '{"service_name": "checkout"}' | jq .

# Raw SQL query
curl -s \
  -X POST http://localhost:9999/v1/query \
  -d "SELECT service_name, count(*) FROM sigiro_spans GROUP BY 1"
```

### Fleet observability (system state via SQL)

Sigiro loads [[architecture/data-model.md|DuckDB]] extensions for system visibility. Agents query them like any other table — no SSH, no agents on hosts:

```bash
# CPU, memory, disk, network per host
curl -s -H "Authorization: Bearer ***" \
  -X POST http://localhost:9999/v1/query \
  -d "SELECT * FROM sys_cpu_info()"

# Find large files
curl -s -H "Authorization: Bearer ***" \
  -X POST http://localhost:9999/v1/query \
  -d "SELECT path, size FROM fsquery_scan('/var/log') WHERE is_regular_file AND size > 1000000000"



# S3 I/O latency — use `httpfs` to query S3 metadata:
curl -s -H "Authorization: Bearer ***" \
  -X POST http://localhost:9999/v1/query \
  -d "SELECT * FROM read_parquet('s3://my-bucket/*.parquet') LIMIT 5"

These `sys_*`/`fsquery_*` functions are local to whichever node you query — running multiple self-hosted nodes does NOT fan these out automatically. See [[guides/self-host-edge-federation.md|Self-Host Edge Federation]] for querying across nodes.

### Deployment events

Sigiro ingests deployment data as OTel spans using standard CICD semantic conventions. No custom table, no custom signal.

**Already works with:** Jenkins OTel Plugin, GitHub Actions OTel, Keptn. Point their [[architecture/write-path.md|OTLP]] export at Sigiro.

```bash
# Recent deployments
curl -s -H "Authorization: Bearer ***" \
  -X POST http://localhost:9999/v1/query \
  -d "SELECT resource_attributes['service.name'], attributes['cicd.pipeline.result'], timestamp \
      FROM sigiro_spans WHERE span_name LIKE 'cicd.pipeline%' \
      AND timestamp > now() - interval '24 hours' ORDER BY timestamp DESC"
```

**For non-OTel tools** (ArgoCD, Flux): Sigiro exposes `POST /v1/events` — a thin webhook adapter translates payload to [[architecture/write-path.md|OTLP]] spans with `cicd.*` attributes.

### Multiple self-hosted nodes (edges)

There's no central node and no self-registration — each host runs the exact
same `sigiro serve`, just with its query surface turned on:

```
# On each host, over a private network only (Tailscale/WireGuard — quack has no TLS of its own):
export SIGIRO_QUERY_SURFACE=quack
export SIGIRO_QUACK_BIND="$(tailscale ip -4)"
sigiro serve
```

Then query across all of them from anywhere on the tailnet:

```
sigiro query "SELECT service_name, count(*) AS n FROM sigiro_spans \
  WHERE timestamp > now() - interval 1 hour GROUP BY service_name ORDER BY n DESC"
```

`sigiro query` is a CLI coordinator: it discovers edges (tailnet peers tagged
`tag:sigiro-edge`, or `SIGIRO_EDGES`/`--edge`) and folds their results —
never a naive average-of-averages. See
[[guides/self-host-edge-federation.md|Self-Host Edge Federation]] for the
required network perimeter, the supported query grammar, and worked
examples.

### What the InvestigationBlock returns

See [[architecture/investigation.md|full schema and field definitions]]. Core fields:

```json
{
  "service": "checkout",
  "window": { "from_ts": 1709294400000000, "to_ts": 1709295900000000 },
  "context": {
    "service_name": "checkout",
    "dependencies": ["payments", "inventory"],
    "dependents": ["api-gateway"]
  },
  "operation_summaries": [{
    "span_name": "POST /checkout",
    "count": 200, "error_count": 24, "error_pct": 12.0,
    "duration_p50_ns": 450000000, "duration_p95_ns": 2100000000,
    "duration_p99_ns": 2500000000
  }],
  "metrics": [{"bucket": "14:10:00", "metric_name": "http.duration", "p95": 1200.0, "avg": 800.0, "count": 342}],
  "spans": [{"trace_id": "abc...", "span_name": "POST /checkout", "duration_ns": 2300000000, "status_code": 2}],
  "span_trees": { "span1": ["span2", "span3"] },
  "logs": [{"template": "connection pool exhausted...", "severity_text": "ERROR", "count": 15}],
  "linked_traces": [{ "trace_id": "abc...", "spans": [...] }],
  "profiles": [{ "function": "runtime.gcBgMarkWorker", "profile_type": "cpu", "total_value": 1847 }],
  "events": [{ "detected_at": "2024-02-29T14:05:00", "event_type": "deploy", "description": "version v2.3.1 first seen" }],
  "coverage": { "has_spans": true, "has_logs": true, "has_metrics": true },
  "truncation": { "spans_total": 342, "spans_returned": 50 }
}
```

---

## Platform Support

### Ports

| Port | Protocol | What | Auth |
|------|----------|------|------|
| 4317 | gRPC | OTLP traces/logs/metrics (protobuf) | Bearer token in `authorization` metadata |
| 4318 | HTTP | OTLP HTTP (protobuf + JSON) | `Authorization: Bearer` header |
| 9999 | HTTP | API: /v1/ (query, diagnose, services), /admin/ (key mgmt), /health | Bearer token (except /health and /admin) |

### eBPF profiling requires privileged Linux

| Platform | eBPF profiling? | Why |
|----------|----------------|-----|
| Kubernetes (EKS, GKE, AKS, k3s) | Yes | `privileged: true` + `hostPID: true` |
| Bare metal / VMs | Yes | Direct kernel access |
| Docker (Linux host) | Yes | `--privileged --pid host` |
| Docker Desktop / OrbStack (macOS) | No | VM isolation |
| Railway / Render / Heroku | No | No privileged containers |
| AWS Fargate / Cloud Run / Lambda | No | No kernel access |

[[designs/future/ebpf-gpu-position.md|eBPF]] profiling is additive. Everything else works everywhere.

### Storage options

**Default: [[architecture/ducklake-integration.md|DuckLake]] (SQLite catalog + local Parquet)**

[[architecture/ducklake-integration.md|DuckLake]] stores a `catalog.sqlite` file (schema, metadata) alongside `catalog.ducklake.files/` ([[architecture/data-model.md|Parquet]] data files) inside your data dir. No PostgreSQL, no external DB.

| Storage | Notes |
|---------|-------|
| Docker volume | Works out of the box |
| Host bind mount | Works out of the box |
| Kubernetes PVC | Works out of the box |

Set a custom data dir with `SIGIRO_DATA_DIR=/path/to/data`.

### Resource usage

| Workload | RAM | Notes |
|----------|-----|-------|
| 1-5 services | ~200 MB | DuckDB + DuckLake + SQLite extensions |
| 10-20 services | ~400 MB | Parquet files + catalog grow |
| 50+ services | ~800 MB | Cap DuckDB buffer pool |
