---
title: Testing Playbook
created: 2026-03-20
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'deployment']
---

<!-- Created 2026-03-20: Preserved from Go codebase before Rust migration. -->

# Testing Playbook

How every layer of the pipeline is tested. Preserved from the Go codebase
for the Rust [[architecture/data-model.md|DuckDB]] extension architecture.

---

## Test Tiers

| Tier | What | Time | Needs Docker | Needs Linux |
|---|---|---|---|---|
| Unit | Mapper logic, schema, span trees | seconds | no | no |
| Auth + Pool | Open-mode acceptance and hosted tenant-key isolation | seconds | no | no |
| Integration | OTel → Parquet → DuckDB assertions | seconds | no | no |
| Roundtrip | Writer ↔ query layer ABI agreement | seconds | no | no |
| Query layer | Investigation against fixture Parquet | seconds | no | no |
| Synthetic E2E | Full pipeline with telemetrygen + RustFS | ~3 min | yes | no |
| Real traffic E2E | 14-service OpenTelemetry Demo | ~10 min | yes | no |

| SDK validation | Pyroscope Go/Python/Node/Java SDKs | ~5 min | yes | no |
| OTLP Auth E2E | Open-mode ingest plus hosted TenantKeys on HTTP :4318 + gRPC :4317 | ~1 min | no | no |
| K8s | Full cluster deployment + validation | ~20 min | no (kubectl) | optional |

---

## 1. Unit Tests

### What to test in Rust

Port these exact behaviors from `mapper_test.go`:

- **Service name resolution** (10 cases): SDK name > k8s.deployment > k8s.statefulset >
  k8s.daemonset > k8s.container > process.executable > "unknown_service". Include
  `unknown_service:go` sentinel variant.

- **Metadata merge**: signal attrs override resource attrs. Promoted keys excluded from JSON.
- **Multi-value profile error**: must reject, not silently drop.

- **Span duration ns→ms**: exact float64 division.
- **Span [[plans/STATUS.md|status]] uppercase**: `Error` → `ERROR`.
- **Compression ratio**: ≥7x with zstd on 10K synthetic rows.
- **Span tree builder**: empty input → `[]`, orphan parents → roots, deep nesting.

### How to run (Rust)

```bash
cargo test --lib          # unit tests
cargo test --lib -- --nocapture  # with output
```

---

## 2. Integration Test

### Pattern: construct telemetry → write Parquet → DuckDB assertions

The Go test built realistic OTel pdata, mapped it, wrote [[architecture/data-model.md|Parquet]], then ran
[[architecture/data-model.md|DuckDB]] SQL to assert. The Rust equivalent:

```rust
// 1. Build test telemetry
let traces = build_test_traces();  // 2 services, cross-service traces

// 2. Parse through sigiro_ingest's mapper
let batches = ingest::map_traces(traces);

// 3. Write to temp Parquet (via DuckLake or arrow-rs Parquet writer)
write_parquet(&batches, &temp_path);

// 4. Assert with DuckDB
let conn = duckdb::Connection::open_in_memory()?;
conn.execute(&format!("CREATE VIEW t AS SELECT * FROM read_parquet('{}')", temp_path))?;

// Same SQL assertions from Go:
assert_eq!(query_scalar(&conn, "SELECT count(DISTINCT signal) FROM t"), 4);
assert_eq!(query_scalar(&conn, "SELECT count(DISTINCT service) FROM t"), 2);
// ... all 15+ assertions from integration_test.go
```

### Key assertions to preserve

- All 4 signals present
- Both services present
- Sort order: zero violations on `(service, signal, timestamp)`
- Cross-service trace: both services share a trace_id
- Parent-child spans: parent_span_id references existing span_id
- Span event → log row: matching trace_id + span_id
- Histogram avg: `sum/count` exact value
- Profile top function: GC function dominates
- No NULL in core columns (timestamp, signal, service, name)

---

## 3. Query Layer Tests (reuse fixture)

The `testdata/incident.parquet` fixture is language-agnostic. The Rust
`sigiro_investigate` extension reads the same [[architecture/data-model.md|Parquet]] schema, so all query
assertions work against the same file.

### Incident fixture contents (synthetic OOM incident)

- Memory: starts ~200MB, peaks >500MB
- Latency: baseline `<100ms`, peak `>500ms`
- Error spans: ≥2000ms, both checkout and payments services
- Logs: pool warn → memory warn → OOMKill (exitCode=137) → pool exhausted
- [[architecture/profiles.md|Profiles]]: gcDrain/gcBgMarkWorker dominates
- Cross-service: ≥3 traces span both services
- Deploy: v2.3.0 → v2.3.1, all errors on v2.3.1

### How to test sigiro_investigate

```sql
-- Load fixture
SELECT * FROM sigiro_investigate(
  service := 'checkout',
  from_ts := (SELECT min(epoch_ms(timestamp/1e6)) FROM read_parquet('testdata/incident.parquet')),
  to_ts := (SELECT max(epoch_ms(timestamp/1e6)) FROM read_parquet('testdata/incident.parquet'))
);

-- Assert: error spans sorted first
-- Assert: OOMKill log present
-- Assert: GC in top profile
-- Assert: linked traces contain both services
```

**Note**: fixture must be regenerated if schema changes (per-signal tables vs unified).

---

## 4. E2E Tests (shell scripts — language-agnostic, reuse as-is)

### Synthetic E2E (`test/e2e/run.sh`)

Services: RustFS + collector + telemetrygen (checkout, payments) + profilegen.

```bash
bash test/e2e/run.sh
```

**For Rust migration**: replace `collector/Dockerfile` with [[architecture/data-model.md|DuckDB]] + extensions image.
Everything else (RustFS, telemetrygen, profilegen, [[architecture/data-model.md|DuckDB]] assertions) stays the same.

Wait logic: 120s for first `flushed parquet` log from collector.

[[architecture/data-model.md|DuckDB]] checks:
1. All 4 signals present
2. Both services present
3. Metrics have time-series shape
4. Spans have trace_id
5. Spans have parent_span_id
6. [[architecture/profiles.md|Profiles]] have non-empty stacktrace
7. [[architecture/profiles.md|Profiles]] from both services
8. [[architecture/investigation.md|InvestigationBlock]] returns all 4 signals for checkout
9. Self-observability: collector's own metrics visible

### Real traffic E2E (`test/e2e/run-real.sh`)

14-service [[architecture/write-path.md|OpenTelemetry]] Demo (v2.2.0). Real HTTP traffic via Locust load gen.

```bash
bash test/e2e/run-real.sh
```

Phase 1: infra (RustFS, Kafka, Postgres, 10s wait)
Phase 2: 12 app services (30s wait)
Phase 3: load generator
Phase 4: 180s telemetry accumulation

Additional checks: ≥5 services, cross-service traces, span depth ≥3, ≥3 metric names.


## 5. SDK Validation (`tests/sdk/run.sh`)

Tests [[architecture/profiles.md|Pyroscope]] SDK → sigiro_ingest pathway for 4 languages:

| SDK | Format | Push interval | Min rows |
|---|---|---|---|
| Go | pprof (cpu, alloc, goroutines) | ~10s | ≥5 |
| Python | pprof (cpu) | ~10s | ≥5 |
| Node.js | pprof (cpu) | ~60s | ≥1 |
| Java | JFR (cpu, alloc, lock) | ~10s | ≥5 |

Total wait: 90s (Node.js is the bottleneck).

```bash
bash tests/sdk/run.sh
```

**Reuse as-is** — these test the SDK→receiver pathway, not the implementation language.
Just update the collector image.

---

## 6. K8s Tests (`test/k8s/run.sh`)

Full cluster deployment: DaemonSet collector, 13 demo services, RustFS in-cluster.

- Auto-detects Lima VM for image loading
- Port-forwards RustFS for [[architecture/data-model.md|DuckDB]] verification
- Checks k8s.namespace.name in metadata JSON


```bash
bash test/k8s/run.sh
```

---

## 7. profilegen

Custom Go tool at `test/e2e/profilegen/` that generates synthetic [[architecture/write-path.md|OTLP]] [[architecture/profiles.md|profiles]].

- 16 function names, 5 stack patterns (GC, HTTP, DB, JSON, malloc)
- Sends to [[architecture/write-path.md|OTLP]] gRPC :4319 (not [[architecture/data-model.md|Arrow]] :4317 — [[architecture/profiles.md|profiles]] not supported on Arrow yet)
- **Never use in real E2E tests** — only for synthetic E2E

Keep this tool — it fills the gap that `telemetrygen` doesn't support [[architecture/profiles.md|profiles]].
Port to Rust or keep as Go binary alongside the Rust extensions.

---

## Adaptation Summary

| Test | Rewrite needed? | Why |
|---|---|---|
| Unit (mapper) | **Yes — port to Rust** | Tests mapper internals |
| Auth + Pool | **Done** | 34 unit tests: key create/verify/revoke, bearer extract, pool isolation |
| Integration | **Yes — port to Rust** | Tests write+query roundtrip |
| Roundtrip | **Yes — port to Rust** | Tests ns/ms ABI contract |
| Query fixture | **No** — reuse `incident.parquet` | Language-agnostic |
| Synthetic E2E | **No** — update Dockerfile only | Shell script + DuckDB |
| Real E2E | **No** — update Dockerfile only | Shell script + DuckDB |

| SDK validation | **No** — update Dockerfile only | Shell script + DuckDB |
| K8s | **No** — update manifests only | Shell script + kubectl |
| profilegen | **Keep as Go** or port | Synthetic profile generator |
