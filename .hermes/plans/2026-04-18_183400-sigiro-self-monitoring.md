# Sigiro Self-Monitoring + AI Self-Improvement Loop

## Goal

Three-layer system:
1. **Self-monitoring**: obi (existing eBPF auto-instrumentation) traces new Sigiro → old Sigiro stores it
2. **CI regression detection**: Compare new Sigiro's investigate data against baseline
3. **AI self-improvement loop**: Hermes reads old Sigiro's investigate output, runs `autoresearch`

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  Old Sigiro (vN, stable)  ←  the observability backend       │
│  :9999 HTTP, :4317 gRPC, :4318 OTLP HTTP                    │
│  Receives and stores all telemetry from new Sigiro           │
│  Agent reads /v1/investigate?service=sigiro-next from here  │
└──────────────────────────────────────────────────────────────┘

┌──────────────────────────────────────────────────────────────┐
│  New Sigiro (vN+1, the one being tested)                    │
│  sigiro serve --ebpf                                        │
│    └─ spawns obi (existing, ebpf.rs:131)                    │
│         ├─ eBPF probes attach at kernel level               │
│         ├─ auto-discovers all processes (including Sigiro)  │
│         └─ OTEL_EXPORTER_OTLP_ENDPOINT=<old-sigiro>:4317  │
└──────────────────────────────────────────────────────────────┘

Hermes Agent (outside both)
  ├─ reads: old Sigiro /v1/investigate?service=sigiro-next
  ├─ runs:  autoresearch loop on regressions
  └─ applies fixes → rebuilds → retests
```

**Key insight:** `obi` already does all the instrumentation. It auto-instruments any language, any framework, zero code changes. Sigiro already bundles it. We just need to point obi's OTLP export at the old Sigiro instead of at self.

**Why this is clean:**
- obi uses eBPF kernel probes — it never modifies Sigiro's code
- obi traces go to old Sigiro, not new Sigiro — no feedback loop
- new Sigiro crashes/leaks → old Sigiro keeps running and storing clean data
- service name is set via `OTEL_SERVICE_NAME=sigiro-next` env var — already a standard OTel convention
- Agent reads existing `/v1/investigate` — no new endpoints needed

---

## Phase 1: Self-Monitoring — One-Line Fix

### What needs to change

**`src/main.rs` — one line:**

```rust
// Before (hardcoded self):
let otlp_endpoint = "http://127.0.0.1:9999".to_string();

// After (env var, obi sends to external receiver instead):
let otlp_endpoint = std::env::var("SIGIRO_OTEL_ENDPOINT")
    .unwrap_or_else(|_| "http://127.0.0.1:9999".to_string());
```

Then passed to `start_obi("obi", &otlp_endpoint)` as before.

That's it. No new flags. No new dependencies. No `telemetry.rs`. No `#[instrument]` macros.

### How obi works

obi is OpenTelemetry eBPF Instrumentation. It:
- Auto-discovers all processes on the host
- Attaches eBPF probes at kernel level (no application code changes)
- Captures HTTP/HTTPS, gRPC, SQL (PostgreSQL, MySQL, SQLite, etc.), DNS
- Exports OTLP to the endpoint set in `OTEL_EXPORTER_OTLP_ENDPOINT`

Currently Sigiro hardcodes `OTEL_EXPORTER_OTLP_ENDPOINT=http://127.0.0.1:9999` — so obi sends to itself. This is the feedback loop. The fix is letting users set it via env var.

### How to use it

```bash
# On the host running new Sigiro:
# Point obi at old Sigiro (the "observability backend")
export SIGIRO_OTEL_ENDPOINT=http://old-sigiro:4317
export OTEL_SERVICE_NAME=sigiro-next   # tells old Sigiro which service this is

sigiro serve --ebpf
```

obi spawns, auto-instruments everything including new Sigiro's HTTP/gRPC handlers, and exports to old Sigiro.

### What obi captures for Sigiro

| Signal | What obi captures | Stored in old Sigiro as |
|--------|------------------|------------------------|
| HTTP spans | Method, path, status, duration, headers | `sigiro_spans` with `span_name=/v1/query`, `/v1/investigate` |
| gRPC spans | Service, method, status, duration | `sigiro_spans` |
| SQL spans | Query text, duration, rows | `sigiro_spans` with `span_name=<SQL>` |
| Logs | `service.name`, severity, body | `sigiro_logs` |

The `span_name` and `service_name` fields let the agent filter exactly what it needs.

---

## Phase 2: CI Regression Detection

### Setup

Old Sigiro runs as a sidecar in CI. New Sigiro sends its telemetry there.

```bash
# Start old Sigiro (previous stable tag) as observability backend
docker run -d --name old-sigiro \
  -p 9999:9999 -p 4317:4317 -p 4318:4318 \
  sigiroai/sigiro:<previous-stable-tag>

# Wait for ready
until curl -s http://localhost:9999/health; do sleep 1; done

# Run new Sigiro with eBPF pointing to old Sigiro
SIGIRO_OTEL_ENDPOINT=http://localhost:4317 \
OTEL_SERVICE_NAME=sigiro-next \
./target/release/sigiro serve --no-ebpf &
NEW_PID=$!

# Generate load (smoke test)
./scripts/smoke-test.sh

# Collect metrics from old Sigiro
curl -s "http://localhost:9999/v1/investigate?service=sigiro-next&from=${FROM}&to=${TO}" \
  > "artifacts/sigiro-next-${GITHUB_SHA}.json"

kill $NEW_PID 2>/dev/null; docker stop old-sigiro
```

### Baseline management

On `main` branch, upload the metrics artifact as a baseline:

```yaml
- name: Upload baseline
  if: github.ref == 'main'
  uses: actions/upload-artifact@v4
  with:
    name: sigiro-baseline
    path: artifacts/sigiro-next-*.json
    retention-days: 30
```

On PRs, download the baseline and compare:

```yaml
- name: Download baseline
  uses: actions/download-artifact@v4
  with:
    name: sigiro-baseline
    path: artifacts/baseline/

- name: Compare with baseline
  run: ./scripts/ci-compare.sh
```

### Compare script

`scripts/ci-compare.sh`:
```bash
#!/bin/bash
set -e

BASELINE=$(ls artifacts/baseline/sigiro-next-*.json | head -1)
CURRENT=$(ls artifacts/sigiro-next-${GITHUB_SHA}.json)

THRESHOLD_P99=1.20   # 20% regression tolerance
THRESHOLD_ERR=0.05  # 5% absolute error rate

# Run jq comparison; exits non-zero if regression detected
exec ./scripts/ci-compare.jq "$BASELINE" "$CURRENT" "$THRESHOLD_P99" "$THRESHOLD_ERR"
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

check("p99 latency";   $curr.p99_latency_ms;   $base.p99_latency_ms;   1.20),
check("error rate";    $curr.error_rate;        $base.error_rate;        0.05)
```

### Metrics extracted from investigate JSON

The agent reads `sigiro-next` investigate data and computes:

| Metric | Source field in investigate JSON |
|--------|----------------------------------|
| `p99_latency_ms` | `span_latencies.p99` from spans where `span_name IN ('/v1/query', '/v1/investigate')` |
| `error_rate` | `error_count / total_spans` |
| `throughput_rows_per_sec` | `span_count / time_window_seconds` |
| `span_count` | `span_count` |
| `anomalies` | `anomalies[]` array |

These are all derived client-side from the existing `/v1/investigate` response. No new endpoint.

---

## Phase 3: AI Self-Improvement Loop

### Architecture

```
Hermes Agent
    │
    ├── reads CI compare output (FAIL/PASS + delta)
    ├── reads old Sigiro /v1/investigate?service=sigiro-next (filtered)
    │
    ├── identifies worst bottleneck
    │     e.g., p99 latency regressed 35% vs baseline
    │
    └── runs autoresearch:
        Goal: reduce p99 latency by >20%
        Scope: src/investigate.rs, src/pool.rs, src/schema.rs, src/inserter.rs
        Metric: p99 latency from investigate JSON (lower is better)
        Verify: rebuild → smoke test → re-compare
        Direction: minimize
        Iterations: 20
```

**Auto-approvable changes** (no human review):
- SQL query INDEX hints
- Buffer size tunables (inserter batch, channel depth)
- LIMIT clauses added to prevent unbounded queries
- Comment/formatting fixes

**Human-required changes**:
- API changes (new endpoints, changed response shapes)
- Schema migrations
- New dependencies
- Architecture changes

### Metric → Scope mapping (for skill)

```
investigate p99 latency
  → investigate.rs (SQL query efficiency)
  → pool.rs (connection mutex contention)
  → schema.rs (missing indexes)

inserter flush latency
  → inserter.rs (batch size, flush cadence)

error rate
  → server.rs (handler errors)
  → schema.rs (invalid SQL)

query error count
  → investigate.rs (SQL errors in investigation queries)
```

---

## Files to Change

| File | Change |
|------|--------|
| `src/main.rs` | Wire `SIGIRO_OTEL_ENDPOINT` env var to `start_obi()` — one line |
| `scripts/ci-compare.sh` | **NEW** |
| `scripts/ci-compare.jq` | **NEW** |
| `scripts/smoke-test.sh` | **NEW** — generate load against new Sigiro |
| `.github/workflows/build.yml` | Add old Sigiro sidecar, baseline upload/compare |
| `.hermes/skills/sigiro-self-improvement/` | **NEW** — metric→scope skill |

**Nothing else.** No new Rust modules, no OTel SDK additions, no new endpoints, no Cargo.toml changes.

---

## Testing

1. **Local two-instance smoke test**
   ```bash
   docker run -d --name old-sigiro -p 9999:9999 sigiroai/sigiro:latest
   SIGIRO_OTEL_ENDPOINT=http://localhost:4317 OTEL_SERVICE_NAME=sigiro-next \
     ./target/release/sigiro serve --no-ebpf &
   ./scripts/smoke-test.sh
   curl -s "http://localhost:9999/v1/investigate?service=sigiro-next" | jq '.span_count'
   # Should be > 0
   ```

2. **CI compare regression detection**
   - Simulate regression: add `thread::sleep(Duration::from_millis(100))` in investigate handler
   - Run CI, verify `ci-compare.sh` exits non-zero

3. **Feedback loop test**
   - Query old Sigiro's investigate endpoint repeatedly
   - Verify old Sigiro's own spans (if any) are separate from `sigiro-next` service data

---

## Risks and Tradeoffs

| Risk | Mitigation |
|------|-----------|
| obi can't attach (wrong platform/caps) | `--no-ebpf` fallback; obi already degrades gracefully |
| old Sigiro fills disk | TTL retention job; data is append-only until GC |
| Timing variance causes flaky CI | Smoke test runs 3x; use median; require >15% delta |
| Self-improvement destabilizes code | Scope limits which files autoresearch can touch; human gate for risky changes |
| Breaking schema change blocks ingest | Feature: old Sigiro rejects new Sigiro's data → CI catches schema regressions early |

---

## Open Questions

1. **Which old Sigiro tag to use?** Pin to the last release tag. When cutting a new release, update the pinned tag in CI.
2. **obi traces old Sigiro too** — should we exclude old Sigiro from obi's probes? Could add `discovery.exclude_services: ["sigiro"]` in obi config, but it's harmless — agent filters by `service_name = sigiro-next`.
3. **What if old Sigiro is not available in CI?** Fail fast — if the observability backend is down, CI should fail loudly rather than run blind.
4. **Retention policy?** Old Sigiro needs a TTL. Add a cron job: `DELETE FROM sigiro_spans WHERE timestamp < now() - INTERVAL '7 days'`. Or use DuckDB's built-in expiration.
5. **Can customers use this for their own apps?** Yes — they run `sigiro serve --ebpf` pointing at their own old Sigiro instance as the backend. obi instruments their entire stack automatically.
