# Microservices Demo — Agent Playbook

> **Purpose:** This directory contains real-world telemetry validation setups.
> **Target:** macOS ARM64 (Apple Silicon) with Docker (OrbStack/Desktop).
>
> **CRITICAL:** If you are on ARM64, the Google Online Boutique `cartservice` WILL CRASH
> with a .NET JIT assertion failure unless you add `platform: linux/amd64`.
> This has been encountered multiple times — do not skip this step.

---

## Quick Decision Tree

```
Need a working demo RIGHT NOW?
├── Yes, and I want Online Boutique (10 services, familiar)
│   └── Add `platform: linux/amd64` to cartservice (Section 1.2)
├── Yes, and I want maximum services with zero config
│   └── Use OpenTelemetry Demo (Section 2) — multi-arch, works on ARM64
└── No, just documenting
    └── Read Section 3 (Known Issues) and update this file
```

---

## 1. Online Boutique (Google Microservices Demo)

### 1.1 The ARM64 Cartservice Problem

**Symptom:** `cartservice` container exits with code 133 or crashes with:
```
assertion failed [block != nullptr]: BasicBlock requested for unrecognized address
(BuilderBase.h:550 block_for_offset)
```

**Root Cause:** The upstream `gcr.io/google-samples/microservices-demo/cartservice:v0.10.1`
image is built for AMD64. On macOS ARM64 with Docker, the .NET JIT compiler crashes
under Rosetta emulation.

**Impact:**
- `cartservice` is down
- `frontend` cannot load carts → 100% error rate on cart operations
- `checkoutservice` cannot place orders → checkout cascade fails
- `paymentservice`, `shippingservice`, `emailservice`, `recommendationservice` never get called

### 1.2 Fix: Add `platform: linux/amd64` to docker-compose

**The one-line fix** (already applied to `docker-compose.yml`):

```yaml
  cartservice:
    image: gcr.io/google-samples/microservices-demo/cartservice:v0.10.1
    platform: linux/amd64          # <-- ADD THIS LINE
    environment:
      - PORT=7070
      ...
```

Docker Desktop / OrbStack on macOS ARM64 has Rosetta 2 support for AMD64 containers.
Contrary to earlier reports, the `cartservice` runs stably under Rosetta as of 2026-05-28.

**After adding `platform: linux/amd64`, restart the stack:**

```bash
cd tests/microservices-demo/runs/2026-05-09-online-boutique/config
docker compose up -d

# IMPORTANT: Also restart frontend to clear its DNS cache
# (Go's gRPC client caches "no such host" errors)
docker compose restart frontend

# Verify all containers are healthy
docker compose ps
```

### 1.3 Services That Still Won't Emit OTLP (Instrumentation Limitation)

Even with the cartservice fix, **3 services use Stackdriver/Cloud Trace exporters**
instead of OTLP and will NOT produce spans in Sigiro:

| Service | Language | Issue | Fix Required |
|---------|----------|-------|--------------|
| `cartservice` | .NET | Uses Stackdriver exporter | Needs source patch to switch to OTLP |
| `shippingservice` | Go | Uses Google Cloud Trace exporter | Needs source patch |
| `adservice` | Java | Uses Stackdriver exporter | Needs source patch |

**Workaround:** These services still function as part of the trace tree — their
parent spans (from `frontend` or `checkoutservice`) show the calls being made.
The missing spans are an instrumentation gap, not a Sigiro bug.

### 1.4 Expected Service Coverage (with ARM64 fix)

| Service | Spans | Status |
|---------|-------|--------|
| frontend | ~15K | ✅ Working |
| productcatalogservice | ~6K | ✅ Working |
| currencyservice | ~10K | ✅ Working |
| recommendationservice | ~3K | ✅ Working |
| checkoutservice | ~800 | ✅ Working |
| paymentservice | ~200 | ✅ Working |
| emailservice | ~50 | ✅ Working |
| cartservice | 0 | ⚠️ Running but no OTLP spans |
| shippingservice | 0 | ⚠️ No OTLP spans |
| adservice | 0 | ⚠️ No OTLP spans |

---

## 2. OpenTelemetry Demo (Recommended for Quick Start)

The OTel Demo uses multi-arch images and works on ARM64 **without any config changes**.

```bash
cd tests/microservices-demo/runs/2026-05-09-otel-demo/config
# Follow the run-e2e.sh script or:
docker compose up -d
```

**Advantages:**
- All services emit OTLP natively
- Multi-arch images (AMD64 + ARM64)
- More services (~20) than Online Boutique
- No custom builds needed

**Disadvantages:**
- More complex topology (Kafka, Postgres, etc.)
- Higher resource usage

---

## 3. Known Issues & Previous Fixes

### Issue: pod_count = 0 in investigate
**Fixed in:** `src/investigate/mod.rs`
**Fix:** Fall back to 1 when `service_instance_id` is NULL (Docker deployments don't set `service.instance.id`)

### Issue: TraceService/Export spans in results
**Fixed in:** `src/investigate/mod.rs`
**Fix:** Filter out `opentelemetry.proto.collector.trace.v1.TraceService/Export` spans from all investigate queries

### Issue: Recursive federation query loops
**Fixed in:** `src/server.rs`
**Fix:** Exclude self from worker forwarding in `execute_sql` and `investigate_handler`

### Issue: Duplicate operations in merged investigate blocks
**Fixed in:** `src/server.rs`
**Fix:** Deduplicate `operation_summaries` by `span_name` when merging results from multiple workers

---

## 4. Files Reference

```
tests/microservices-demo/
├── AGENTS.md                          <-- YOU ARE HERE
├── runs/2026-05-09-online-boutique/
│   ├── config/docker-compose.yml      # Online Boutique stack
│   ├── config/otelcol-config.yml      # Collector → Sigiro
│   └── RESULTS.md                     # Previous run results
├── runs/2026-05-09-otel-demo/
│   ├── config/docker-compose.yml      # OTel Demo stack
│   └── run-e2e.sh                     # Orchestration script
└── online-boutique/                   # Git submodule (Google repo)
```

---

## 5. Checklist for Future Agents

When setting up Online Boutique on ARM64:

- [ ] Verify `docker-compose.yml` has `platform: linux/amd64` under `cartservice:`
- [ ] Start stack with `docker compose up -d`
- [ ] Restart frontend: `docker compose restart frontend` (clears DNS cache)
- [ ] Verify `config-cartservice-1` is `Up` (not `Exited`)
- [ ] Verify `config-frontend-1` logs show successful cart retrievals (status 200/302)
- [ ] Run load generator for 60+ seconds
- [ ] Query Sigiro for service list — expect 7+ services with spans
- [ ] Run `/v1/investigate` — verify `pod_count >= 1` and no `TraceService/Export` ops
