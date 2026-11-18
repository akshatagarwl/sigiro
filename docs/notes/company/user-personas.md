---
title: User Personas, Journeys & Support Matrix
created: 2026-03-03
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'competitor', 'data-model']
---

# User Personas, Journeys & Support Matrix

## Overview

This document defines user personas across different developer types, company sizes, platforms, and tech stacks. Each persona includes a setup journey, what they see after setup, and where they hit product boundaries. Use this document to simulate product testing end-to-end.

---

## Support Matrix

### Signal Collection

| Signal | Collection Method | Languages/Runtimes | Notes |
|--------|------------------|-------------------|-------|
| Metrics | OTLP (gRPC/HTTP) | Any OTel SDK | Port 4317 (gRPC) or 4318 (HTTP) |
| Traces | OTLP (gRPC/HTTP) | Any OTel SDK | Port 4317 (gRPC) or 4318 (HTTP). eBPF auto-instrumentation via OTel eBPF profiler |
| Logs | OTLP (gRPC/HTTP), filelog receiver | Any | Port 4317 (gRPC) or 4318 (HTTP). File-based collection for legacy apps |
| Profiles | OTLP profiling receiver | Go, Java, Python, Node.js, Rust, C/C++ | Port 4317 (OTLP gRPC) or 4318 HTTP at `/ingest/pyroscope` (Pyroscope format). OTel eBPF profiler sends via OTLP. |

### Platform Support

| Platform | Deployment Model | Status |
|----------|-----------------|--------|
| Kubernetes (any distro) | DaemonSet | Primary target |
| Docker Compose | Sidecar container | Supported |
| Bare metal / VMs | Binary or systemd service | Supported |
| AWS ECS/Fargate | Sidecar task | Supported (OTLP push) |
| AWS Lambda | Extension layer | OTLP push only (no eBPF) |
| Google Cloud Run | Sidecar | OTLP push only (no eBPF) |
| Heroku / Railway / Fly.io | OTLP push from app | No collector on platform, push-only |
| Vercel (serverless) | OTLP push from app | No collector, push-only |

### Storage Backend

| Backend | Status | Notes |
|---------|--------|-------|
| AWS S3 | Supported | Primary target |
| MinIO | Supported | Local dev and self-hosted |
| GCS (via S3 compat) | Should work | Untested |
| R2 (Cloudflare) | Should work | Untested |
| Local filesystem | Supported (default) | DuckLake (SQLite catalog + local Parquet) — no S3 or MinIO required |

### Query Access

| Method | Status | Notes |
|--------|--------|-------|
| HTTP API (`POST /v1/query`) | Supported | Returns structured investigation block |
| DuckDB CLI (direct Parquet) | Supported | For power users, ad-hoc SQL |
| Dashboard UI | Not built | Intentional: agent-first, no dashboards |
| Alerting rules | Not built | Planned |
| Grafana datasource | Not built | Not planned (agent-first model) |

---

## Persona 1: Solo SaaS Founder (Platform Engineer Hat)

**Name:** Priya
**Company:** Early-stage SaaS startup, 1-3 engineers
**Stack:** Next.js frontend on Vercel, Go API on a single k8s cluster (DigitalOcean), PostgreSQL managed, Redis managed
**Current observability:** `fmt.Println` and Sentry for crashes. Considered Datadog, couldn't justify $500+/month for 3 services.
**Budget:** $0-50/month for observability
**Why Sigiro:** Price. $0.005/GB means her 5 GB/day costs $0.75/month vs $150+ anywhere else.

### Setup Journey

```
Step 1: Deploy collector (5 min)
  - kubectl apply the DaemonSet manifest
  - Collector starts receiving OTLP from her Go services (already using OTel SDK for Sentry)

Step 2: Add OTel SDK to Go API (10 min)
  - Already has opentelemetry-go for Sentry traces
  - Change exporter endpoint from Sentry to collector's OTLP gRPC (localhost:4317, Arrow-optimized)
  - Traces + metrics start flowing immediately

Step 3: Deploy query server (5 min)
  - Single pod, same cluster, same S3 credentials
  - Verify with: curl -X POST localhost:8080/v1/query \
      -d '{"service":"api","from":"2026-03-03T00:00:00Z","to":"2026-03-03T01:00:00Z"}'

Step 4: Connect to AI agent (5 min)
  - Add /v1/query endpoint to her Claude/GPT agent's tool config
  - Agent can now investigate incidents autonomously
```

**Total setup time:** ~25 minutes
**Ongoing maintenance:** Zero. S3 lifecycle rule for 30-day retention.

### What She Sees

**After 1 hour of traffic:**
- **Metrics:** Request count, latency p95, memory usage per service. ~500 rows/hour for 3 services.
- **Spans:** Every HTTP handler traced with duration. Slow PostgreSQL queries visible as child spans (if pg driver instrumented).
- **Logs:** Structured logs from Go's slog, correlated with trace_ids.
- **[[architecture/profiles.md|Profiles]]:** Not available unless she adds the profiling receiver ([[designs/future/ebpf-gpu-position.md|eBPF]] needs privileged container).
- **[[architecture/investigation.md|Investigation]] block:** Agent asks "why is the API slow?" and gets metrics showing latency spike, spans showing slow DB queries, logs showing connection pool exhaustion.

### Gaps She'll Notice

- No alerting: she can't set "notify me if p95 > 500ms." She must query or have an agent poll.
- No log search UI: can't grep logs from a browser. Must use [[architecture/data-model.md|DuckDB]] CLI or the API.
- [[architecture/profiles.md|Profiles]] require privileged DaemonSet ([[designs/future/ebpf-gpu-position.md|eBPF]]). DigitalOcean DOKS allows it, but she needs to add security context.
- Next.js on Vercel: no collector possible. Browser RUM and serverless function traces would need [[architecture/write-path.md|OTLP]] push directly to her cluster's collector ingress (requires exposing the endpoint).

### Test Scenarios

1. **Happy path:** Go API with OTel SDK, 3 services, k8s, S3. Full metrics+traces+logs.
2. **Partial instrumentation:** Only 1 of 3 services has OTel SDK. Other 2 are dark — no data.
3. **Cold query:** First query after deployment. [[architecture/data-model.md|DuckDB]] loads [[architecture/data-model.md|Parquet]] from S3. Expect 1-3s latency.
4. **High cardinality:** API has 200 unique endpoints. Metrics rows scale linearly. No explosion (no label indexing).
5. **S3 [[company/cost.md|cost]] check:** 5 GB/day raw telemetry, ~0.6 GB after compression. S3 storage: ~$0.50/month. Sigiro ingestion: ~$3/month. Total: ~$3.50/month.

---

## Persona 2: Platform Team at Mid-Size Company

**Name:** Marcus
**Company:** Series B fintech, 40 engineers, 15 microservices
**Stack:** Java (Spring Boot) + Python (ML services) on AWS EKS, PostgreSQL RDS, Redis ElastiCache, Kafka MSK, S3
**Current observability:** [[designs/future/perses-compat.md|Grafana]] Cloud ($1,800/month) with Prometheus, Loki, Tempo. No profiling. Alert fatigue from 200+ Grafana alerts.
**Budget:** Wants to cut to <$200/month
**Why Sigiro:** 10x [[company/cost.md|cost]] reduction. Team spends 3 hours/week maintaining [[designs/future/perses-compat.md|Grafana]] dashboards that agents could replace.

### Setup Journey

```
Step 1: Deploy collector as DaemonSet on EKS (10 min)
  - Helm chart or raw manifest
  - IAM role for S3 access (IRSA) — no static credentials

Step 2: Migrate OTel SDK endpoints (30 min per service, phased)
  - Java services: change OTEL_EXPORTER_OTLP_ENDPOINT from Grafana Cloud to collector
  - Python services: same env var change
  - Phase: start with 3 critical services, expand over 2 weeks
  - Keep Grafana running in parallel during migration

Step 3: Add eBPF profiling (15 min)
  - Enable profiling receiver in collector config
  - Requires privileged security context on DaemonSet
  - Java: JVM profiling via async-profiler (perf events)
  - Python: limited eBPF CPU profiling (no GIL visibility)

Step 4: Deploy query server (10 min)
  - Separate deployment, 2 replicas behind internal ALB
  - Same IAM role for S3 access

Step 5: Agent integration (varies)
  - Internal Slack bot with Claude API
  - Tool: POST /v1/query with service and time window
  - On-call engineer asks bot: "why is payments-service erroring?"
  - Bot queries Sigiro, gets investigation block, returns diagnosis
```

**Total setup time:** ~2 hours for first 3 services, 1-2 weeks for full migration

### What They See

**15 services, ~50 GB/day telemetry:**
- **Metrics:** Request rates, latencies, JVM heap, Python memory, Kafka consumer lag, DB connection pool. ~15K rows/hour.
- **Spans:** Full distributed traces across Java and Python services. Kafka produce/consume spans. DB query spans with duration.
- **Logs:** Structured JSON logs from logback (Java) and structlog (Python). Correlated with trace_ids.
- **[[architecture/profiles.md|Profiles]]:** CPU flame graphs for Java services. Python CPU profiles (limited resolution). Top functions by sample count.
- **[[architecture/investigation.md|Investigation]] block example:** Agent investigates "payments-service latency spike." Block shows: metrics (latency p95 jumped from 50ms to 800ms), spans (PostgreSQL queries taking 600ms, normally 10ms), logs ("connection pool exhausted" messages), [[architecture/profiles.md|profiles]] (top function: `HikariPool.getConnection` at 40% CPU). Agent concludes: DB connection pool too small for traffic spike.

### Gaps They'll Notice

- **No [[designs/future/perses-compat.md|Grafana]] integration:** Marcus's team has 200 Grafana dashboards. No migration path. Must go cold turkey or run both.
- **No alerting:** Their 200+ Prometheus alerting rules can't be migrated. Need external alerting (e.g., polling script or agent-based).
- **Kafka consumer lag:** Sigiro captures it as a metric if the OTel SDK emits it. But no built-in Kafka monitoring like [[designs/future/perses-compat.md|Grafana]] Cloud's.
- **Multi-tenancy:** All 15 services share one S3 bucket. No access control per team. Any engineer with query server access sees all data.
- **Query latency at scale:** 50 GB/day = ~6 GB compressed/day = 180 GB/month on S3. Query over 5-minute window scans ~20 files. Fast. But scanning 24 hours requires 12K files — could be 10-30s.
- **No dashboards for management:** CTO wants a "system health" dashboard. Sigiro doesn't have one. Agent can generate a text summary, but no visual charts.

### Test Scenarios

1. **Phased migration:** 3 services on Sigiro, 12 on [[designs/future/perses-compat.md|Grafana]] Cloud. Both run in parallel. Validate data parity.
2. **Java + Python mixed stack:** Verify OTel SDK compatibility for both. Spring Boot auto-instrumentation vs manual Python instrumentation.
3. **High throughput:** 50 GB/day ingestion. Collector memory usage under 200 MB per node. S3 upload latency.
4. **Cross-service trace:** Request flows through 5 services. Linked traces query returns full call chain.
5. **Kafka correlation:** Producer span in service A, consumer span in service B. Verify trace_id propagation through Kafka headers.
6. **Large time window query:** 24-hour query across all services. Measure latency. Expect 10-30s.

---

## Persona 3: DevOps Engineer at a Containerized Startup

**Name:** Alex
**Company:** Developer tools startup, 8 engineers, all-in on containers
**Stack:** Go microservices on k8s (GKE), gRPC between services, CockroachDB, NATS, no legacy
**Current observability:** Jaeger for traces (self-hosted, frequently breaks), Prometheus + AlertManager (self-hosted, 500 GB disk), no logs pipeline, no profiling
**Budget:** $100/month, but mainly wants to stop managing observability infra
**Why Sigiro:** Eliminate self-hosted Prometheus + Jaeger. Zero ops. S3 is the only dependency.

### Setup Journey

```
Step 1: Deploy collector DaemonSet on GKE (10 min)
  - GCS bucket with S3-compatible API (or use MinIO on a dedicated node)
  - OR: use actual S3 in us-east-1 (GKE can reach it, adds ~20ms upload latency)

Step 2: Point OTel SDKs to collector (20 min)
  - Go services already use opentelemetry-go for Jaeger
  - Change exporter from Jaeger to OTLP gRPC
  - Add metrics exporter (currently only traces)
  - Add log bridge (otel log SDK, connect to slog)

Step 3: Enable eBPF profiling (10 min)
  - GKE supports privileged containers
  - Go binaries: eBPF CPU profiling works excellently (DWARF symbols available)
  - Profiles show goroutine contention, GC pressure, hot functions

Step 4: Decommission Jaeger + Prometheus (after 1 week validation)
  - Verify traces appear in Sigiro
  - Verify metrics match Prometheus values
  - Delete Jaeger deployment, Prometheus StatefulSet
  - Free up 500 GB disk + 4 GB RAM

Step 5: Query server + agent (15 min)
  - Deploy query server on GKE
  - Integrate with internal CLI tool that wraps Claude API
```

**Total setup time:** ~1 hour. Decommission old infra after 1 week.

### What They See

**8 Go services, ~10 GB/day telemetry:**
- **Metrics:** gRPC method latencies, NATS message rates, CockroachDB query durations, Go runtime metrics (goroutines, GC pause).
- **Spans:** Full gRPC call chains with metadata. CockroachDB query spans. NATS publish/subscribe spans.
- **Logs:** Structured slog output with trace_id correlation (requires log bridge setup).
- **[[architecture/profiles.md|Profiles]]:** Go CPU profiles with function-level granularity. Can see: mutex contention, channel blocking, GC overhead, hot loops.
- **[[architecture/investigation.md|Investigation]] block example:** "auth-service has high latency." Block shows: metrics (goroutine count spiked to 10K), spans (CockroachDB queries timing out), [[architecture/profiles.md|profiles]] (top function: `runtime.gcBgMarkWorker` at 35% — GC thrashing from allocation pressure), logs ("context deadline exceeded" on DB calls). Root cause: memory leak causing GC pressure causing DB timeout cascade.

### Gaps They'll Notice

- **GCS not natively supported:** Must use S3-compatibility layer or actual AWS S3. Adds friction for GCP-native shops.
- **gRPC server streaming:** Long-lived gRPC streams may not produce per-message spans. Depends on OTel interceptor implementation.
- **CockroachDB-specific metrics:** No built-in CockroachDB receiver. Must use generic [[architecture/write-path.md|OTLP]] or Prometheus remote write (not supported yet).
- **NATS tracing:** Requires manual context propagation through NATS message headers. OTel doesn't auto-instrument NATS.
- **No Prometheus remote write receiver:** Can't use existing Prometheus scrapers as a data source. Must switch to [[architecture/write-path.md|OTLP]].
- **Alerting gap:** Replacing AlertManager means losing pager integration. Need external polling + PagerDuty webhook.

### Test Scenarios

1. **GCS S3-compat:** Test with GCS S3-interop endpoint. Verify [[architecture/data-model.md|Parquet]] upload and [[architecture/data-model.md|DuckDB]] read.
2. **gRPC tracing:** Unary and streaming RPCs. Verify parent-child span relationships across services.
3. **Go profiling accuracy:** Compare [[designs/future/ebpf-gpu-position.md|eBPF]] CPU profile with `go tool pprof`. Verify top functions match.
4. **Decommission validation:** Side-by-side: Prometheus metric value vs Sigiro metric value for same counter. Should match within flush interval tolerance (15s).
5. **NATS message tracing:** Manual trace context propagation. Verify end-to-end trace across publish/subscribe.

---

## Persona 4: Full-Stack Developer at an Agency

**Name:** Sofia
**Company:** Web development agency, builds apps for clients
**Stack:** Varies per project. Current project: Ruby on Rails + React, deployed on Render.com. Next project: Django + HTMX on [[company/gtm.md|Fly.io]].
**Current observability:** Render.com built-in logs. No metrics, no traces, no profiling.
**Budget:** $0-20/month per client project
**Why Sigiro:** Cheapest way to add "real" observability to client projects. Differentiator in proposals.

### Setup Journey

```
Step 1: Assess platform constraints
  - Render.com: no DaemonSet, no eBPF. Must push OTLP from app.
  - Fly.io: can run sidecar container. eBPF possible with privileged.

Step 2: For Render.com project (Rails + React)
  - Add opentelemetry-ruby gem to Rails
  - Configure OTLP HTTP exporter to point at Sigiro collector
  - Problem: collector must be externally accessible (not on Render)
  - Options:
    a) Run collector on a cheap VPS ($5/month DigitalOcean droplet)
    b) Run collector on Fly.io (free tier)
    c) Skip collector, push directly to... (not supported, collector is required)

Step 3: For Fly.io project (Django + HTMX)
  - Deploy collector as a Fly.io machine in same region
  - Add opentelemetry-python to Django
  - Configure OTLP gRPC exporter to collector's internal address
  - Profiles: limited for Python (eBPF CPU profiling, no GIL visibility)

Step 4: Query server
  - Run on same VPS/Fly.io machine as collector
  - Share S3 credentials
  - One query server per client, or shared with per-client S3 prefix
```

**Total setup time:** 1-3 hours depending on platform complexity

### What She Sees

**Rails app, ~2 GB/day telemetry:**
- **Metrics:** Request count, response time, ActiveRecord query count per request. Ruby memory (if instrumented).
- **Spans:** Rails controller actions, ActiveRecord queries, Redis cache hits/misses, HTTP calls to external APIs.
- **Logs:** Rails.logger output, but only if bridged to OTel (requires additional gem). Otherwise, logs stay in Render.com.
- **[[architecture/profiles.md|Profiles]]:** Not available on Render (no [[designs/future/ebpf-gpu-position.md|eBPF]]). Available on [[company/gtm.md|Fly.io]] but limited for Ruby (interpreted language).

**Django app, ~1 GB/day telemetry:**
- **Metrics:** Request count, response time, Django ORM query count.
- **Spans:** Django views, ORM queries, template rendering.
- **Logs:** Python logging bridged to OTel.
- **[[architecture/profiles.md|Profiles]]:** [[designs/future/ebpf-gpu-position.md|eBPF]] CPU profiling on [[company/gtm.md|Fly.io]]. Shows C-level hotspots but not Python function names.

### Gaps She'll Notice

- **No managed collector endpoint:** Every deployment needs a collector somewhere. For PaaS platforms without container control, this is friction.
- **Ruby profiling:** Basically non-existent. [[designs/future/ebpf-gpu-position.md|eBPF]] sees the Ruby VM, not Ruby functions.
- **Python profiling:** Sees C extensions and native code. Python function names require py-spy or similar (not integrated).
- **Rails auto-instrumentation:** Works well via opentelemetry-ruby gem, but requires manual setup for each gem (pg, redis, net-http).
- **React frontend:** No browser SDK / RUM. Can't trace frontend-to-backend requests. Only server-side visibility.
- **Multi-client isolation:** Sharing one S3 bucket across clients is a security concern. Need separate buckets or prefixes with access control (not built).
- **No dashboard for client demo:** Can't show clients a pretty chart. Must export data or build custom visualization.

### Test Scenarios

1. **PaaS push-only:** Rails on Render pushing [[architecture/write-path.md|OTLP]] to external collector. Verify data flows despite network hop.
2. **Ruby OTel SDK:** ActiveRecord, Redis, HTTP instrumentation. Verify span accuracy.
3. **Python OTel SDK:** Django middleware, ORM, template spans. Verify trace completeness.
4. **Cross-platform:** Rails on Render + Django on [[company/gtm.md|Fly.io]], both pushing to same collector. Verify service separation in queries.
5. **Low volume:** 1-2 GB/day. Verify S3 costs stay under $1/month.

---

## Persona 5: ML/AI Engineer at a Data Company

**Name:** Raj
**Company:** Series A ML platform company, 20 engineers
**Stack:** Python (FastAPI + PyTorch), GPU workloads on k8s (AWS EKS with GPU nodes), Redis, PostgreSQL, S3 for model storage
**Current observability:** CloudWatch (basic), wandb for ML experiments. No distributed tracing. Debug by reading CloudWatch logs.
**Budget:** $100/month
**Why Sigiro:** Wants to trace ML inference pipelines end-to-end. Traditional APM tools don't understand batch GPU workloads. Price is a bonus.

### Setup Journey

```
Step 1: Deploy collector on EKS (10 min)
  - Standard DaemonSet, S3 credentials via IRSA
  - GPU nodes need eBPF kernel support (Amazon Linux 2 / Bottlerocket: yes)

Step 2: Instrument FastAPI services (20 min)
  - opentelemetry-python with FastAPI auto-instrumentation
  - Add custom spans for model inference:
    with tracer.start_as_current_span("model.inference") as span:
        span.set_attribute("model.name", "gpt-4-classifier")
        span.set_attribute("model.version", "v2.3")
        result = model.predict(input)
        span.set_attribute("inference.latency_ms", elapsed)
        span.set_attribute("inference.tokens", token_count)

Step 3: Add GPU metrics (15 min)
  - Use NVIDIA DCGM exporter → Prometheus format
  - Problem: Sigiro doesn't have a Prometheus remote write receiver
  - Workaround: Use OTel Collector's prometheus receiver to scrape DCGM exporter
  - Add to collector config:
    receivers:
      prometheus:
        config:
          scrape_configs:
            - job_name: 'dcgm'
              static_configs:
                - targets: ['dcgm-exporter:9400']

Step 4: Profile GPU workloads
  - eBPF CPU profiling captures CPU-side code (data loading, preprocessing, postprocessing)
  - GPU profiling (CUDA kernels) NOT supported — eBPF doesn't see GPU execution
  - For GPU profiling, still need NVIDIA Nsight or PyTorch profiler

Step 5: Query server + agent integration (15 min)
  - Deploy query server
  - Integrate with their existing Claude-based ML ops agent
  - Agent investigates: "why is inference latency high for gpt-4-classifier?"
```

**Total setup time:** ~1 hour

### What They See

**5 FastAPI services + GPU workers, ~20 GB/day telemetry:**
- **Metrics:** Request latency, GPU utilization (via Prometheus scrape), memory usage, inference latency, token throughput.
- **Spans:** Full request lifecycle: HTTP receive → preprocessing → model load → GPU inference → postprocessing → response. Batch processing spans with item counts.
- **Logs:** FastAPI structured logs, model loading events, OOM warnings, CUDA errors.
- **[[architecture/profiles.md|Profiles]]:** CPU profiles showing data loading bottlenecks, GIL contention, preprocessing hotspots. No GPU kernel profiles.
- **[[architecture/investigation.md|Investigation]] block example:** "inference-service latency spiked to 5s." Block shows: metrics (GPU utilization dropped to 20%, CPU at 95%), spans (preprocessing taking 4.5s, inference only 0.3s), [[architecture/profiles.md|profiles]] (top CPU function: `tokenizer.encode` at 60%), logs ("CUDA OOM, falling back to CPU" messages). Root cause: tokenizer running on CPU is the bottleneck, not the GPU.

### Gaps They'll Notice

- **No GPU profiling:** [[designs/future/ebpf-gpu-position.md|eBPF]] sees CPU, not CUDA kernels. Must keep NVIDIA Nsight for GPU debugging.
- **No Prometheus remote write:** Must use OTel Collector's prometheus scrape receiver as a workaround for DCGM exporter.
- **Batch job semantics:** ML training jobs run for hours. 15s flush interval means lots of small files. 8-hour training run = 1,920 files just for that job.
- **Large payloads:** ML services may log large tensors or embeddings in metadata. The `metadata string` column could bloat.
- **No experiment tracking:** wandb handles this. Sigiro is for production ops, not training experiments. Complementary, not replacement.
- **Python profiling limitations:** [[designs/future/ebpf-gpu-position.md|eBPF]] sees CPython interpreter internals, not Python function names. `py-spy`-level integration not available.

### Test Scenarios

1. **GPU metrics via Prometheus scrape:** DCGM exporter → OTel prometheus receiver → Sigiro. Verify GPU utilization, temperature, memory metrics.
2. **Custom ML spans:** Inference latency, model version, token count as span attributes. Verify they appear in [[architecture/investigation.md|investigation]] block.
3. **Batch processing:** 1000-item batch job. Verify spans don't explode (one parent span with metadata, not 1000 child spans).
4. **CUDA OOM recovery:** Service falls back to CPU. Verify logs + metrics + spans capture the event coherently.
5. **Large time window:** Query 8-hour training job window. Measure query latency with 1,920+ files.

---

## Persona 6: Enterprise Platform Architect

**Name:** Chen
**Company:** Large enterprise, 500+ engineers, 200+ microservices
**Stack:** Mixed: Java (Spring Boot), .NET (C#), Node.js, some Go. Kubernetes on-prem (OpenShift), Oracle DB, RabbitMQ, internal service mesh (Istio)
**Current observability:** Dynatrace ($50K/month), but leadership wants [[company/cost.md|cost]] reduction. Some teams also run Splunk for logs.
**Budget:** $5K/month (90% reduction from Dynatrace)
**Why Sigiro:** 90% [[company/cost.md|cost]] reduction. But needs to validate enterprise readiness.

### Setup Journey

```
Step 1: Pilot scope (1 week)
  - Choose 10 services across 2 teams (Java + Node.js)
  - Deploy collector DaemonSet on OpenShift
  - OpenShift: may need SecurityContextConstraints (SCC) for eBPF privileged containers
  - On-prem S3: MinIO cluster or existing Ceph with S3 gateway

Step 2: OTel SDK rollout for pilot services (1 week)
  - Java: Spring Boot + opentelemetry-java-agent (auto-instrumentation JAR, zero code changes)
  - Node.js: @opentelemetry/auto-instrumentations-node
  - .NET: not in pilot phase
  - Change OTEL_EXPORTER_OTLP_ENDPOINT to collector address

Step 3: Enterprise requirements check
  - Authentication on query server: NOT AVAILABLE → blocker for enterprise
  - Multi-tenancy (per-team isolation): NOT AVAILABLE → blocker
  - RBAC: NOT AVAILABLE → blocker
  - Audit log: NOT AVAILABLE → blocker
  - SSO/SAML: NOT AVAILABLE → blocker
  - Data residency (on-prem S3): SUPPORTED via MinIO
  - Encryption at rest: S3-side encryption (SSE-S3 or SSE-KMS)
  - Encryption in transit: OTLP over gRPC with TLS, S3 over HTTPS

Step 4: If pilot approved despite gaps
  - Deploy query server behind internal API gateway (adds auth externally)
  - Separate S3 buckets per team (manual isolation)
  - Proceed with limited blast radius
```

**Total setup time:** 2-3 weeks for pilot

### What They See

**10 services, ~100 GB/day telemetry (pilot):**
- **Metrics:** JVM heap, GC pauses, HTTP latencies, RabbitMQ queue depth, thread pool usage.
- **Spans:** Full distributed traces across Java and Node.js. Istio sidecar spans (if OTel-instrumented). Oracle DB query spans (JDBC auto-instrumentation).
- **Logs:** logback (Java) and winston (Node.js) structured logs.
- **[[architecture/profiles.md|Profiles]]:** Java: excellent [[designs/future/ebpf-gpu-position.md|eBPF]] CPU + async-profiler integration. Node.js: V8 CPU profiling (limited resolution with eBPF).

### Gaps They'll Notice (Enterprise Blockers)

- **No authentication:** Query server is wide open. Enterprise non-starter without API gateway in front.
- **No multi-tenancy:** Team A can query Team B's data. Must use separate buckets + separate query server instances.
- **No [[designs/auth-billing.md|RBAC]]:** No role-based access. All or nothing.
- **No audit logging:** No record of who queried what.
- **No SSO:** No SAML/OIDC integration.
- **No data masking:** PII in logs/spans not redacted. Must rely on OTel Collector processors (redaction processor).
- **No SLA:** Self-hosted only. No vendor support contract.
- **No Dynatrace migration tool:** 200+ auto-discovered services in Dynatrace. Must manually instrument each for OTel.
- **OpenShift SCC:** Privileged containers for [[designs/future/ebpf-gpu-position.md|eBPF]] require cluster-admin approval. Political friction.
- **Oracle DB:** JDBC auto-instrumentation works, but no Oracle-specific metrics receiver.
- **.NET support:** OTel .NET SDK exists but is less mature than Java. Auto-instrumentation coverage is narrower.

### Test Scenarios

1. **OpenShift SCC:** Deploy privileged DaemonSet. Verify [[designs/future/ebpf-gpu-position.md|eBPF]] probes attach successfully.
2. **MinIO on-prem:** Ceph S3 gateway as storage. Verify [[architecture/data-model.md|Parquet]] upload + [[architecture/data-model.md|DuckDB]] read.
3. **Java auto-instrumentation:** Spring Boot + OTel Java agent. Verify zero-code-change traces.
4. **Scale test:** 100 GB/day ingestion. Collector memory under 200 MB. S3 upload latency stable.
5. **Cross-mesh tracing:** Service A (Istio sidecar) → Service B (Istio sidecar). Verify trace_id propagation through Envoy.
6. **[[designs/auth-billing.md|Auth]] workaround:** API gateway (Kong/Nginx) in front of query server. Verify JWT validation pass-through.
7. **Team isolation:** Separate S3 bucket per team. Separate query server per team. Verify no cross-team data leakage.

---

## Persona 7: Indie Hacker / Side Project Developer

**Name:** Jake
**Company:** Solo developer, side project
**Stack:** Single Go binary on a $5 VPS (Hetzner), SQLite, no containers
**Current observability:** SSH into server, `tail -f app.log`, `htop`
**Budget:** $0. Literally zero.
**Why Sigiro:** Curiosity. Saw it on Hacker News. Wants to try "real" observability without [[company/cost.md|cost]].

### Setup Journey

```
Step 1: Assess feasibility
  - No Kubernetes → no DaemonSet
  - $5 VPS with 1 GB RAM → collector + query server + app must fit in 1 GB
  - Collector: ~100-200 MB RAM
  - Query server: ~50-100 MB (DuckDB is embedded)
  - App: ~50 MB
  - Total: 200-350 MB → fits, but tight

Step 2: Install collector as binary (15 min)
  - Download pre-built collector binary (if published)
  - OR build from source: make collector
  - Run as systemd service
  - Storage: MinIO on same VPS (or local filesystem hack)
  - Problem: local filesystem not supported by writer. Must run MinIO.
  - MinIO: adds ~100 MB RAM. Now at 350-450 MB. Still fits in 1 GB.

Step 3: Add OTel SDK to Go app (10 min)
  - Add opentelemetry-go SDK
  - Point OTLP to localhost:4317
  - Wrap HTTP handlers with OTel middleware
  - Add slog→OTel log bridge

Step 4: Run query server on same VPS (5 min)
  - S3_ENDPOINT=localhost:9000 (MinIO)
  - Single process, no replicas

Step 5: Query via DuckDB CLI (no agent)
  - Install duckdb CLI
  - Query Parquet files directly from MinIO
  - Or curl the query server API
```

**Total setup time:** ~30-45 minutes

### What He Sees

**1 Go service, ~0.5 GB/day telemetry:**
- **Metrics:** Request count, latency, memory usage, goroutine count. ~100 rows/hour.
- **Spans:** HTTP handler spans, SQLite query spans (if instrumented).
- **Logs:** slog output with trace_id correlation.
- **[[architecture/profiles.md|Profiles]]:** [[designs/future/ebpf-gpu-position.md|eBPF]] CPU profiling works on bare metal (needs root). Shows Go function hotspots.
- **Monthly [[company/cost.md|cost]]:** S3/MinIO storage: $0 (local disk). Sigiro: $0 (self-hosted). Total: $0.

### Gaps He'll Notice

- **MinIO overhead:** Running MinIO on a $5 VPS just for S3 compatibility feels heavy. A local filesystem backend would eliminate this.
- **No pre-built binaries:** Must build collector from source (requires Go 1.25 toolchain).
- **Memory pressure:** 1 GB VPS is tight. Under load, [[architecture/data-model.md|DuckDB]] query could OOM the server.
- **No web UI:** Can't check observability from phone. Must SSH in.
- **Single point of failure:** VPS goes down → lose everything (telemetry, app, MinIO).

### Test Scenarios

1. **Minimal VPS:** 1 GB RAM, all components co-located. Verify no OOM under normal load.
2. **Local MinIO:** Single-node MinIO. Verify [[architecture/data-model.md|Parquet]] upload + read cycle.
3. **Build from source:** Clone repo, `make collector`, verify binary works.
4. **SQLite tracing:** Custom OTel wrapper around SQLite. Verify query spans.
5. **Memory limit:** Send burst traffic. Monitor RSS of collector + query server. Identify OOM threshold.

---

## Persona 8: SRE at a Cloud-Native Company

**Name:** Kim
**Company:** B2B SaaS, 100 engineers, fully cloud-native
**Stack:** Go + Rust microservices on AWS EKS, DynamoDB, SQS, Lambda functions, CloudFront CDN
**Current observability:** Mix of Datadog ($8K/month) and CloudWatch. Want to consolidate and reduce [[company/cost.md|cost]].
**Budget:** $500/month
**Why Sigiro:** Datadog at $8K/month is their 3rd largest AWS expense. At $0.005/GB, same data costs ~$50/month. Plus, agent-native query model aligns with their "AI ops" initiative.

### Setup Journey

```
Step 1: EKS collector deployment (10 min)
  - DaemonSet with IRSA for S3 access
  - Dedicated S3 bucket with lifecycle rules (30-day retention = auto-delete)
  - Existing OTel SDKs in Go + Rust services

Step 2: Migrate from Datadog (phased, 2-3 weeks)
  Week 1: Switch 5 Go services from Datadog agent to OTel collector
  Week 2: Switch 10 Rust services
  Week 3: Lambda functions (see below) + validation

Step 3: Lambda instrumentation
  - Lambda can't run collector (no sidecar in Lambda)
  - Options:
    a) OTel Lambda layer → push OTLP to collector's external endpoint
    b) OTel Lambda layer → push OTLP to API Gateway → collector
  - Must expose collector's OTLP HTTP endpoint externally (with auth!)
  - Problem: a self-hosted Open collector has no per-request authentication. Keep it on a private network or place it behind a reverse proxy; hosted deployments use tenant keys.

Step 4: CloudWatch metrics
  - CloudWatch has metrics for DynamoDB, SQS, Lambda
  - No CloudWatch receiver in OTel Collector (community contrib has one)
  - Add awscloudwatchmetrics receiver from otel-contrib

Step 5: Rust profiling
  - eBPF CPU profiling works excellently for Rust (native binary, DWARF symbols)
  - Best profiling experience of any language

Step 6: Deploy query server with external access
  - Internal ALB + API Gateway for Lambda's OTLP push
  - Agent integration: internal Slack bot for incident investigation
```

**Total setup time:** 2-3 weeks for full migration

### What They See

**30 services (Go + Rust + Lambda), ~200 GB/day telemetry:**
- **Metrics:** HTTP latencies, DynamoDB consumed capacity, SQS queue depth, Lambda invocations/errors/duration, Go/Rust memory.
- **Spans:** Full distributed traces including Lambda invocations as spans. DynamoDB operation spans. SQS send/receive spans.
- **Logs:** Structured logs from all services. Lambda logs via OTel log bridge.
- **[[architecture/profiles.md|Profiles]]:** Excellent Rust and Go CPU profiles. Function-level granularity with symbol names. Lambda: no profiling (no [[designs/future/ebpf-gpu-position.md|eBPF]]).
- **[[architecture/investigation.md|Investigation]] block example:** "order-processor Lambda timing out." Block shows: metrics (Lambda duration at max 15s, DynamoDB consumed RCU spike), spans (DynamoDB GetItem taking 12s, batch size 500 items), logs ("provisioned throughput exceeded" from DynamoDB), [[architecture/profiles.md|profiles]] (N/A for Lambda). Root cause: DynamoDB throttling due to hot partition key.

### Gaps They'll Notice

- **Lambda profiling:** Impossible with [[designs/future/ebpf-gpu-position.md|eBPF]]. Must use AWS X-Ray profiling or Lambda Insights as complement.
- **Lambda collector ingestion:** Requires exposing the collector endpoint externally. Use hosted TenantKeys or a private/reverse-proxied Open deployment.
- **CloudWatch integration:** Requires contrib receiver (`awscloudwatchmetrics`). Must add to OCB manifest. Not included by default.
- **Datadog migration:** No migration tool. 200+ Datadog monitors must be recreated (or abandoned for agent-based [[architecture/investigation.md|investigation]]).
- **200 GB/day at scale:** 200 GB/day raw = ~25 GB compressed = 750 GB/month on S3. S3 [[company/cost.md|cost]]: ~$17/month. Sigiro ingestion: $120/month. Query server: may need 3+ replicas for concurrent queries. Total: ~$200/month (vs $8K Datadog).
- **DynamoDB specifics:** OTel auto-instrumentation captures DynamoDB calls as spans. But DynamoDB-specific metrics (consumed capacity, throttle events) require CloudWatch integration.
- **No custom dashboards for on-call:** SRE team expects a "service overview" dashboard. Must build custom or accept text-only agent summaries.

### Test Scenarios

1. **Lambda [[architecture/write-path.md|OTLP]] push:** Lambda → API Gateway → Collector. Verify traces flow end-to-end.
2. **Rust [[designs/future/ebpf-gpu-position.md|eBPF]] profiling:** Verify symbol resolution for Rust binaries. Top functions should show Rust function names, not mangled symbols.
3. **DynamoDB tracing:** GetItem, PutItem, Query, Scan operations as spans with table name, consumed capacity.
4. **SQS correlation:** Producer span → SQS → consumer span. Verify trace_id propagation through SQS message attributes.
5. **200 GB/day load test:** Sustained ingestion. Collector fleet memory usage. S3 upload rate. Query latency under load.
6. **Datadog parity check:** Same service instrumented with both Datadog and Sigiro. Compare metric values, trace completeness.
7. **[[company/cost.md|Cost]] validation:** Measure actual S3 storage, data transfer, and compute costs over 1 week. Extrapolate to monthly.

---

## Cross-Persona Comparison Matrix

| Dimension | Priya (Solo) | Marcus (Mid) | Alex (DevOps) | Sofia (Agency) | Raj (ML) | Chen (Enterprise) | Jake (Indie) | Kim (SRE) |
|-----------|-------------|-------------|--------------|---------------|---------|-------------------|-------------|-----------|
| **Services** | 3 | 15 | 8 | 2-3 per client | 5 | 200+ | 1 | 30 |
| **Daily volume** | 5 GB | 50 GB | 10 GB | 1-2 GB | 20 GB | 500 GB+ | 0.5 GB | 200 GB |
| **Monthly cost** | $3 | $30 | $6 | $1-2 | $12 | $300+ | $0 | $120 |
| **Platform** | k8s (DO) | k8s (EKS) | k8s (GKE) | PaaS (mixed) | k8s (EKS+GPU) | OpenShift | Bare metal | k8s (EKS)+Lambda |
| **Languages** | Go | Java+Python | Go | Ruby+Python | Python | Java+.NET+Node | Go | Go+Rust |
| **Setup time** | 25 min | 2 hours | 1 hour | 1-3 hours | 1 hour | 2-3 weeks | 30 min | 2-3 weeks |
| **Biggest gap** | No alerting | No dashboards | No GCS native | No managed endpoint | No GPU profiling | No auth/RBAC | No local FS backend | No Lambda profiling |
| **Blocker?** | No | No | No | Mild | No | Yes (auth) | No | No |
| **Replaces** | Nothing | Grafana Cloud | Jaeger+Prometheus | Nothing | CloudWatch | Dynatrace | tail -f | Datadog |

## Universal Gaps (All Personas)

These limitations affect every persona and should be prioritized:

1. **No alerting engine:** Every persona must build external polling or rely on agent-based monitoring.
2. **No authentication on query server:** Enterprise blocker. Others work around it with network isolation.
3. **No web UI / dashboards:** Agent-first is the thesis, but humans still want visual confirmation.
4. **No Prometheus remote write receiver:** Cuts off existing Prometheus ecosystems.
5. **No managed collector endpoint (SaaS ingest):** PaaS and serverless users must run their own collector.
6. **No browser RUM / frontend SDK:** Full-stack visibility stops at the backend.
7. **No local filesystem storage backend:** Forces MinIO even for single-node setups.

## What Every Persona Gets Right (Universal Wins)

1. **Price:** 15-25x cheaper than alternatives. No persona pays more than $300/month.
2. **Setup speed:** Under 1 hour for standard k8s deployments.
3. **Zero maintenance:** No databases, no indices, no compaction jobs, no version upgrades.
4. **Agent-native queries:** 1 API call returns everything needed for incident [[architecture/investigation.md|investigation]].
5. **[[designs/future/ebpf-gpu-position.md|eBPF]] profiling:** Free CPU profiling for Go, Rust, Java, C/C++ without code changes.
6. **Unified correlation:** Metrics + traces + logs + [[architecture/profiles.md|profiles]] in one query, auto-correlated by service and time.
