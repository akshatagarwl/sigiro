# Sigiro Self-Monitoring + AI Self-Improvement Loop

## Goal

Three-layer system:
1. **Self-monitoring**: New Sigiro (vN+1) instruments itself via OTel SDK, sends OTLP to **old Sigiro (vN)** which stores it. No feedback loops.
2. **CI regression detection**: Canary comparison of new Sigiro's performance metrics against old Sigiro's stored data
3. **AI self-improvement loop**: Hermes reads old Sigiro's `/admin/self-investigate`, runs `autoresearch` to fix regressions

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  New Sigiro (vN+1, the one being tested)                   │
│  OTEL_SERVICE_NAME=sigiro-next                              │
│  SIGIRO_OTLP_ENDPOINT=http://old-sigiro:4317               │
└──────────────────┬──────────────────────────────────────────┘
                   │ OTLP (spans + metrics)
                   ↓
┌──────────────────────────────────────────────────────────────┐
│  Old Sigiro (vN, stable production)                          │
│  - Receives and stores all telemetry from new Sigiro         │
│  - DuckDB: sigiro_spans etc. contain "sigiro-next" entries  │
│  - Provides /admin/self-investigate for new Sigiro data      │
└──────────────────────────────────────────────────────────────┘
                   │
                   ↓
         Hermes Agent (outside both)
                   │
                   ├── reads /admin/self-investigate (filtered by service_name=sigiro-next)
                   ├── runs autoresearch loop
                   └── applies fixes → rebuilds → retests
```

**Why this works:**
- Old Sigiro is never tracing itself — no feedback loops possible
- New Sigiro crashes/leaks/has bugs → old Sigiro keeps storing clean telemetry
- Agent reads from old Sigiro which is a stable reference
- Two-instance architecture means no chicken-and-egg on service name detection

**Service name detection:** The `service_name` is set by the **sender** (OTel SDK), not the receiver. `OTEL_SERVICE_NAME=sigiro-next` on new Sigiro → all spans arrive at old Sigiro tagged as `"sigiro-next"`. Old Sigiro reads it from `rp.resource.attributes["service.name"]` (see `ingest.rs:346-367`). No code changes needed.

---

## Phase 1: Self-Monitoring (New Sigiro)

### What to instrument

| Component | Signal | What to capture |
|-----------|--------|-----------------|
| HTTP handlers (`server.rs`) | Spans + metrics | Latency p50/p95/p99, status codes, error rate |
| gRPC handlers | Spans + metrics | Latency, error rate |
| Inserter (`inserter.rs`) | Metrics | Channel depth, flush latency, rows/sec, buffer pressure |
| DuckDB queries | Spans | Per-query duration (tagged: investigate vs raw query) |
| Project pool (`pool.rs`) | Metrics | Active projects, connection wait time |

### OTel SDK integration

**`Cargo.toml` additions:**
```toml
opentelemetry = { version = "0.31", features = ["rt-tokio"] }
opentelemetry-otlp = { version = "0.31", features = ["tonic"] }
opentelemetry_sdk = { version = "0.31", features = ["rt-tokio", "metrics"] }
opentelemetry_api = "0.31"
```

**Instrumentation approach:**

New file `src/telemetry.rs`:
```rust
use opentelemetry::trace::{Tracer, SpanKind};
use opentelemetry_sdk::{runtime, trace as sdktrace};
use opentelemetry_otlp::WithExportConfig;
use std::time::Instant;

// Global tracer — initialized once
pub fn init(service_name: &str, endpoint: &str) {
    let exporter = opentelemetry_otlp::new_exporter()
        .tonic()
        .with_endpoint(endpoint);

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing()
        .with_exporter(exporter)
        .with_trace_config(sdktrace::Config::default().with_resource(
            opentelemetry_sdk::Resource::new(vec![
                opentelemetry_api::KeyValue::new("service.name", service_name),
            ])
        ))
        .install_batch(runtime::Tokio)
        .expect("failed to init OTel tracer");

    tracer
}
```

**No feedback loop risk** — all spans go from new Sigiro → old Sigiro. New Sigiro's own `/admin/self-investigate` reads from old Sigiro via HTTP. It never sends spans to itself.

### `SIGIRO_OTLP_ENDPOINT` env var

New CLI flag on `Serve`:
```rust
Serve {
    /// Endpoint for new Sigiro to send its own telemetry to (e.g. http://old-sigiro:4317)
    #[arg(long, env = "SIGIRO_OTLP_ENDPOINT")]
    otlp_endpoint: Option<String>,
    ...
}
```

When set, init OTel SDK and instrument handlers. When unset, no self-instrumentation (backward compatible).

**Startup sequence:**
1. Parse `otlp_endpoint`
2. If set, call `telemetry::init("sigiro-next", otlp_endpoint)`
3. Start servers normally
4. Every HTTP/gRPC handler gets wrapped in an OTel span

---

## Phase 2: Self-Investigation Endpoint (Old Sigiro)

### New endpoint: `GET /admin/self-investigate?service=sigiro-next`

Query params:
- `service` (required): the OTel `service_name` to investigate — agent passes `sigiro-next`
- `from` / `to` (optional): time range in epoch seconds

This reads from old Sigiro's DuckDB — `WHERE service_name = ?` filter picks up new Sigiro's spans.

### Response type (extends existing `InvestigationBlock`)

New variant or additional field in `InvestigationBlock`:
```rust
pub struct SelfMonitoringBlock {
    // Sigiro's own performance
    pub uptime_seconds: i64,
    pub total_requests: i64,
    pub error_rate: f64,
    pub p50_query_latency_ms: f64,
    pub p95_query_latency_ms: f64,
    pub p99_query_latency_ms: f64,
    pub ingest_throughput_rows_per_sec: f64,
    pub inserter_channel_depth_avg: f64,
    pub active_projects: i64,

    // Aggregated from spans table
    pub span_count_last_15m: i64,
    pub error_span_count_last_15m: i64,

    // Derived
    pub anomalies: Vec<Anomaly>,
}

pub struct Anomaly {
    pub severity: String, // "warning" | "critical"
    pub message: String,
    pub metric: String,
    pub value: f64,
    pub threshold: f64,
}
```

**SQL to compute these (from old Sigiro's DuckDB):**
```sql
-- Query latencies from span_name = '/v1/query' or '/v1/investigate'
SELECT
    percentile_cont(0.50) FILTER (WHERE duration_ns IS NOT NULL) as p50_ns,
    percentile_cont(0.95) FILTER (WHERE duration_ns IS NOT NULL) as p95_ns,
    percentile_cont(0.99) FILTER (WHERE duration_ns IS NOT NULL) as p99_ns
FROM sigiro_spans
WHERE service_name = ? AND span_name IN ('/v1/query', '/v1/investigate')
  AND timestamp > now() - INTERVAL '15 minutes';

-- Error rate
SELECT
    count(*) as total,
    count(*) FILTER (WHERE status_code >= 500) as errors
FROM sigiro_spans
WHERE service_name = ? AND timestamp > now() - INTERVAL '15 minutes';
```

---

## Phase 3: CI Regression Detection

### Setup: Running old Sigiro as sidecar

**In CI, before testing new Sigiro:**
```bash
# Start old Sigiro as observability backend (runs in background, port 9999 old-sigiro)
docker run -d --name old-sigiro \
  -p 9999:9999 -p 4317:4317 \
  sigiroai/sigiro:<previous-stable-tag>

# Wait for ready
until curl -s http://localhost:9999/health; do sleep 1; done

# Run new Sigiro smoke test against old Sigiro
SIGIRO_OTLP_ENDPOINT=http://localhost:4317 \
OTEL_SERVICE_NAME=sigiro-next \
./target/release/sigiro serve --no-ebpf &

# Generate load
./scripts/smoke-test.sh

# Collect metrics
curl -s "http://localhost:9999/admin/self-investigate?service=sigiro-next" \
  > "artifacts/self-internal-${GITHUB_SHA}.json"
```

**Store baseline on main branch:**
```yaml
- name: Upload baseline metrics
  if: github.ref == 'main'
  uses: actions/upload-artifact@v4
  with:
    name: sigiro-baseline
    path: artifacts/self-internal-*.json
    retention-days: 30
```

### Compare script

`scripts/ci-compare.sh`:
```bash
#!/bin/bash
# Compares current run's self-investigate JSON against baseline

BASELINE=$(find artifacts -name 'sigiro-baseline-*.json' | head -1)
CURRENT="artifacts/self-internal-${GITHUB_SHA}.json"

# Extract key metrics with jq
# Compare p99 latency, error rate, throughput
# FAIL if any delta exceeds threshold

THRESHOLD_P99_LATENCY=1.20  # 20% regression tolerance
THRESHOLD_ERROR_RATE=0.05    # 5% absolute

jq -n \
  --slurpfile base "$BASELINE" \
  --slurpfile curr "$CURRENT" \
  -f scripts/ci-compare.jq
```

`scripts/ci-compare.jq`:
```jq
def pct_change($new; $old):
  if ($old == 0) then null
  else (($new - $old) / $old)
  end;

def check($name; $curr; $base; $thresh):
  (pct_change($curr; $base) | if (. == null or . > $thresh) then
    "FAIL: \($name) regressed by \((.*100 | floor))% (threshold: \($thresh * 100)%)"
  else
    "PASS: \($name) stable"
  end);

[ check("p99 latency"; $curr[0].p99_query_latency_ms; $base[0].p99_query_latency_ms; 1.20 ),
  check("error rate";   $curr[0].error_rate;           $base[0].error_rate;           0.05 ) ]
```

---

## Phase 4: AI Self-Improvement Loop

### Architecture

```
Hermes Agent
    │
    ├── reads: old Sigiro /admin/self-investigate (service=sigiro-next)
    ├── reads: CI artifact diff output
    │
    ├── identifies: worst bottleneck (e.g., investigate p99 > baseline by 30%)
    │
    └── runs autoresearch:
        Goal: reduce investigate p99 latency by >20%
        Scope: src/investigate.rs, src/pool.rs, src/schema.rs, src/inserter.rs
        Metric: p99 investigate latency in ms (lower is better)
        Verify: smoke test against old Sigiro, compare before/after
        Direction: minimize
        Iterations: 20
```

**No feedback loop** — the agent is completely outside Sigiro. It reads from old Sigiro, builds new Sigiro, tests it, measures it.

### Self-improvement skill

New skill: `sigiro-self-improvement` (saved via `skill_manage`):

```
Metric → Scope mapping:
- investigate p99 → investigate.rs (SQL queries), pool.rs (connection contention)
- inserter flush latency → inserter.rs (buffer sizes, flush cadence)
- query error rate → server.rs (handler errors), schema.rs (invalid SQL)
- memory pressure → pool.rs (connection count), inserter.rs (channel depth)
```

**Auto-approvable changes** (no human review needed):
- SQL query hints (INDEX, ANALYZE, LIMIT)
- Buffer size tunables (inserter batch size, channel depth)
- Feature flags that don't change API contract
- Comment/cleanup improvements

**Human-required changes**:
- API changes (new endpoints, changed response shapes)
- Schema migrations (CREATE TABLE, ALTER TABLE)
- New dependencies (Cargo.toml additions)
- Architecture changes (multi-process, caching layer)

### Trigger conditions

The self-improvement loop runs when:
1. CI detects a regression: `ci-compare.sh` exits non-zero → Hermes reads diff, runs `autoresearch`
2. Nightly: no regression but p99 > 90th percentile of last 7 runs → proactive optimization
3. Manual: `sigiro self-improve --target=p99` via CLI

---

## Files Likely to Change

| File | Changes |
|------|---------|
| `Cargo.toml` | Add `opentelemetry`, `opentelemetry_otlp`, `opentelemetry_sdk`, `opentelemetry_api` |
| `src/telemetry.rs` | **NEW** — OTel tracer + meter init, instrumentation helpers |
| `src/main.rs` | Add `--otlp-endpoint` flag, wire `telemetry::init()` in serve |
| `src/server.rs` | Wrap handlers in OTel spans (add `#[instrument]` or manual span) |
| `src/inserter.rs` | Add OTel metrics for channel depth, flush latency |
| `src/investigate.rs` | Add `SelfMonitoringBlock` struct + `query_self_monitoring()` SQL |
| `src/admin.rs` | New `GET /admin/self-investigate` handler |
| `scripts/ci-compare.sh` | **NEW** — CI baseline comparison |
| `scripts/ci-compare.jq` | **NEW** — jq filter for regression detection |
| `scripts/smoke-test.sh` | **NEW** — load generator for CI self-monitoring |
| `.github/workflows/build.yml` | Add baseline upload (main) + compare step (PR) |
| `.hermes/skills/sigiro-self-improvement/` | **NEW** — skill encoding metric→scope mapping |

---

## Testing

1. **Local two-instance test**: Start old Sigiro on `:9999`, new Sigiro with `SIGIRO_OTLP_ENDPOINT=http://localhost:4317 OTEL_SERVICE_NAME=sigiro-next ./sigiro serve`, generate load, verify `curl localhost:9999/admin/self-investigate?service=sigiro-next` returns data
2. **Feedback loop test**: Verify that querying `/admin/self-investigate` from new Sigiro does NOT create new spans (it shouldn't — new Sigiro's admin endpoint calls old Sigiro over HTTP, not through its own OTel tracer)
3. **CI compare test**: Upload two JSON artifacts with different p99 values, verify script detects regression
4. **Regression simulation**: Intentionally add `thread::sleep(Duration::from_millis(50))` in investigate handler, verify CI catches it

---

## Risks and Tradeoffs

| Risk | Mitigation |
|------|-----------|
| Old Sigiro version mismatch | Pin old Sigiro image tag in CI; store baseline keyed by old version |
| Timing variance in benchmarks | Run smoke test 3x, use median; require >15% delta for regression |
| Self-improvement destabilizes code | Autoresearch only touches files in scope; human review for high-risk changes |
| Old Sigiro disk fills up | TTL: `DELETE FROM sigiro_spans WHERE timestamp < now() - INTERVAL '7 days'` |
| Two-instance memory overhead | Old Sigiro is sidecar only during CI; scale to zero when idle |

---

## Open Questions

1. **Which old Sigiro version to use as baseline?** Tag the current production release. When cutting a new release, the release commit becomes the new baseline. Or: always use the last successful main-branch build as baseline.
2. **What if new Sigiro has breaking schema changes?** If new Sigiro changes `sigiro_spans` table schema (e.g., new column), old Sigiro's ingest will reject it. This is a feature — it blocks schema regressions. Fix: only compare metrics that both versions support.
3. **How to isolate new Sigiro's data in old Sigiro's DB?** Service name filter (`WHERE service_name = 'sigiro-next'`) is sufficient. Projects are isolated via `project_id` in the API key — new Sigiro uses its own API key, data stays separate.
4. **Can the agent run self-improvement automatically on every PR?** Yes, but expensive. Better: run on regression only, with a manual trigger for proactive optimization.
5. **What about logs and metrics signals?** Same pattern — OTel logs/metrics from new Sigiro → old Sigiro stores them. The agent reads `sigiro_logs` filtered by `service_name = 'sigiro-next'` too.
