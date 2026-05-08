# Sigiro Microservices Demo Test Results

## 2026-05-07: SDK Validation Apps (Pyroscope Examples)

### Setup
- Sigiro 0.4.0, Docker Compose on macOS
- 4 services: rideshare.go, rideshare.python, rideshare.nodejs, rideshare.java

### Instrumentation Results

| Service | Language | Status | Code Changes | Config Changes | Failures |
|---------|----------|--------|-------------|----------------|----------|
| rideshare.go | Go | NOT WORKING | 4 needed | 2 attempted | 4 |
| rideshare.python | Python | WORKING | 0 | 2 | 1 |
| rideshare.nodejs | Node.js | Not tested | N/A | N/A | 0 |
| rideshare.java | Java | Not tested | N/A | N/A | 0 |

**Success rate: 1/4 (25%)**

### Key Failures (7 total)
1. Go traces: 401 — hardcoded Basic auth vs Sigiro's Bearer
2. Go logs: HTTPS mismatch + wrong path /otlp/v1/logs
3. Go traces: URL parse error from buggy getOTLPUrl()
4. Go metrics: 401 — no auth in otlpmetrichttp.New()
5. Python: gRPC exporter pointed at HTTP port
6. Python: missing OTEL_SERVICE_NAME → "unknown_service"
7. Go metrics: defaults to localhost ignoring OTLP_URL

### Data Quality (Python App)
- 18+ spans ingested
- 0 null timestamps/durations/orphans/corrupted IDs
- 100% service name coverage after fix

### Investigation Effectiveness
- rideshare.python: investigate sufficient, no direct query needed, correct
- sigiro: investigate sufficient, no direct query needed, correct

### Artifacts
```
runs/2026-05-07-sdk-validation/
  config/docker-compose.yml
  artifacts/investigate-rideshare-python.json (77KB)
  artifacts/investigate-sigiro.json (64KB)
  metrics.json
  NOTES.md
```

### Key Takeaways
1. Python Flask auto-instrumentation worked with pure config (2 env vars)
2. Go requires code changes — auth format, URL resolution, logs path
3. Auth is the biggest friction point (Bearer token requirement)
4. Service naming needs explicit OTEL_SERVICE_NAME
5. Investigate correctly surfaced operations for simple single-service traces

### Next Steps
- Test Java with auto-instrumentation agent (promising zero-code)
- Add SIGIRO_OTLP_NO_AUTH for local testing
- Test a real multi-service demo with inter-service calls

---

## 2026-05-07: Google Cloud Microservices Demo (Online Boutique)

### Setup
- Sigiro 0.4.0, Docker Compose on macOS (ARM64)
- 11 services: frontend, checkoutservice, cartservice, productcatalogservice, currencyservice, paymentservice, shippingservice, emailservice, recommendationservice, adservice, redis-cart
- OTel Collector (contrib:latest) bridging services -> Sigiro
- Pre-built GCR images: `gcr.io/google-samples/microservices-demo/*:v0.10.1`

### Instrumentation Results

| Service | Language | Status | Code Changes | Config Changes | Failures |
|---------|----------|--------|-------------|----------------|----------|
| All 11 services | Mixed | WORKING (with config) | 0 | 1-2 per service | 3 |

**Success rate: 11/11 (100%) with config-only changes**

**Required env vars:**
- `ENABLE_TRACING=1` — required on ALL services to enable built-in OTel
- `DISABLE_PROFILER=1` — required on Node.js services (currency, payment) to prevent GCP profiler crash
- `COLLECTOR_SERVICE_ADDR=otel-collector:4317` — points to local collector

### Key Failures (3 categories)
1. **Service naming**: 9/11 Go services default to `unknown_service:server` — no `OTEL_SERVICE_NAME` set
2. **GCP profiler crashes**: Node.js services crash without `DISABLE_PROFILER=1`
3. **Collector gzip incompatibility**: Sigiro gRPC doesn't support gzip — required `compression: none`

### Data Quality

| Metric | Value |
|--------|-------|
| Traces ingested | 1,330 |
| Spans ingested | 5,500+ |
| Null trace IDs | 0 |
| Service name accuracy | 9% (1/11 correct) |
| Distributed trace trees | Working |

### Operations Detected

| Operation | Spans |
|-----------|-------|
| ProductCatalogService/GetProduct | 1,367 |
| frontend (HTTP handler) | 1,061 |
| CurrencyService/GetSupportedCurrencies | 803 |
| CartService/GetCart | 803 |
| ProductCatalogService/ListProducts | 225 |
| CartService/AddItem | 140 |
| CheckoutService/PlaceOrder | 45 |

### Trace Tree Example
```
frontend (unknown_service:server)
  ├── hipstershop.CurrencyService/GetSupportedCurrencies (unknown_service:server)
  └── hipstershop.CartService/GetCart (unknown_service:server)
```
3-span distributed trace with 2 errors (cart Redis timeout).

### Investigation Effectiveness

| Scenario | Service | Investigate Sufficient? | Direct Query Needed? | Correct? |
|----------|---------|------------------------|---------------------|----------|
| Basic visibility | currencyservice | **No** — returned 0 | Yes (SQL) | N/A |
| Trace tree | unknown_service:server | **No** — returned 0 | Yes (/v1/traces/{id}) | Yes |

**Investigate endpoint bug**: Returns empty results for all services despite 5500+ spans in database.
**Trace endpoint works**: Correctly shows 3-span distributed traces with errors.

### Profiling
All services have Google Cloud Profiler but it fails without GCP credentials. No profiles sent to Sigiro.

### Artifacts
```
runs/2026-05-07-online-boutique/
  config/docker-compose.yml
  config/otelcol-config.yml
  artifacts/docker-compose.yml
  artifacts/otelcol-config.yml
  artifacts/sample-trace.json
  metrics.json
```

### Key Takeaways
1. **Config-only instrumentation works** — `ENABLE_TRACING=1` enabled traces on all 11 services
2. **Distributed tracing is functional** — frontend request spans across 3+ services
3. **Service naming is broken for Go** — 9 services merge into `unknown_service:server`
4. **Collector bridging required** — Needed for auth header injection and gzip workaround
5. **Investigate endpoint has a bug** — Returns 0 results; trace endpoint works as fallback
6. **Node.js had best naming** — currencyservice correctly identified itself

### Infrastructure Notes
- Redis port 6379 conflicted with OrbStack — removed external port mapping
- Recommendationservice port 8080 conflicted with OrbStack — remapped to 8082
- AMD64 images on ARM64 Mac cause Redis connection timeouts in cartservice
