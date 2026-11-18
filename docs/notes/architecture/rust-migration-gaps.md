---
title: Rust Migration — Feature Gaps & Edge Cases
created: 2026-03-20
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'data-model', 'decision', 'deployment', 'duckdb']
---

<!-- Created 2026-03-20: Gaps to address when migrating from Go to Rust [[architecture/data-model.md|DuckDB]] extensions. -->
<!-- Updated 2026-03-20: Removed gaps handled by using otlp2records upstream schema as-is. -->

# Rust Migration — Feature Gaps & Edge Cases

Audit of the Go codebase. Behaviors and edge cases relevant to the Rust
[[architecture/data-model.md|DuckDB]] extension architecture.

**Key decision:** Use `otlp2records` upstream as-is. Store data in its native
Arrow schema. Handle presentation concerns ([[plans/STATUS.md|status]] strings, duration units,
span kind labels) at query time in `sigiro_investigate`.

---

## sigiro_ingest — 5 gaps

### Dropped (handled by otlp2records or at query time)

~~1. Service name fallback chain~~ — otlp2records extracts `service_name` from
resource attributes. Any further fallback logic belongs at query time, not ingest.

~~3. Span [[plans/STATUS.md|status]] normalization~~ — otlp2records stores `status_code` as Int32
(0=UNSET, 1=OK, 2=ERROR). Map to string at query time via CASE expression.

~~4. Span duration in ms~~ — otlp2records stores `duration` as Int64 nanos.
Compute `duration / 1e6` at query time.

~~7. Metadata merge / promoted keys~~ — otlp2records stores resource_attributes,
scope_attributes, and signal_attributes as separate JSON columns. Extract
promoted fields at query time via `json_extract_string()`.

### Must preserve

2. **Span events → log rows**: each span event becomes a separate log row correlated via
   `trace_id` + `span_id`. Exception events get `status_code=2` and body=`exception.message`.
   otlp2records stores events as `events_json` on the trace row. For MVP, extract at query
   time in `sigiro_investigate`. Post-MVP: consider a post-ingest step or fork to emit
   separate log RecordBatches.

5. **[[designs/future/ebpf-gpu-position.md|eBPF]] per-sample service name override**: eBPF profiler sets `service.name=unknown_service`
   at resource level but real service name in sample `AttributeIndices`. Sample-level overrides
   resource-level, but only if not itself an `unknown_service` sentinel.
   *Deferred: [[architecture/profiles.md|profiles]] not in MVP.*

6. **[[architecture/profiles.md|Pyroscope]] format routing**: pprof → folded fallback, JFR multi-profile batching, gzip
   auto-detection, multipart form-data, timestamp precision auto-detection.
   *Deferred: [[architecture/profiles.md|profiles]] not in MVP.*

### Must fix (known bugs in Go code)

8. **Span duration unsigned subtraction**: if `EndTimestamp < StartTimestamp` (clock skew,
   unfinished spans with EndTimestamp=0), wraps to huge positive value.
   otlp2records computes `duration` — verify whether it handles this correctly.
   If not, handle at query time: `CASE WHEN duration < 0 THEN 0 ELSE duration END`.

9. **ExponentialHistogram + Summary silently dropped**: Go code dropped these silently.
   otlp2records has full ExponentialHistogram support (separate schema). Summary still
   needs verification — check if otlp2records handles it or silently drops.

### Must plan (operational features)

10. **k8s attributes enrichment**: the Go collector used `k8sattributesprocessor` to auto-add
    `k8s.pod.name`, `k8s.deployment.name`, etc. Without the OTel Collector, `sigiro_ingest`
    needs its own k8s metadata lookup (downward API env vars, or k8s API client).

11. **Durable queue**: the Go collector used `file_storage` extension for disk-backed queue.
    On S3 failure, data spills to disk and replays. The Rust extension needs equivalent
    durability — either [[architecture/ducklake-integration.md|DuckLake]]'s inlined data acts as the buffer, or a WAL.

12. **Batch window**: 60-second batch in Go collector controlled S3 file frequency. [[architecture/ducklake-integration.md|DuckLake]]
    manages flush timing, but the Rust extension should ensure similar batching to avoid
    excessive small files.

---

## sigiro_investigate — 5 gaps

### Must preserve

13. **Three scan modes**: traceID+service (most specific), traceID-only (cross-service trace
    view), service-only (default [[architecture/investigation.md|investigation]]). Priority order matters.

14. **Error-first span sorting**: `CASE WHEN status_code=2 THEN 0 ELSE 1 END, duration DESC`.
    Hard cap: 50 spans. Agents depend on errors appearing first.

15. **Span tree assembly**: parent absent from current span set = root (handles cross-service
    parents). Duplicate SpanID silently overwrites (known, document as gap).

16. **Linked trace collection**: gather error trace_ids from BOTH error spans (`status_code=2`)
    AND error logs. Linked logs exclude primary service (`service_name != $target`), prefix
    message with `[service_name]`.

### Must fix

~~17. **Linked trace IN clause unbounded**~~: RESOLVED (Phase 6) — capped at 5 trace_ids, 200 spans total.

~~18. **Linked span query has no time-range filter**~~: RESOLVED (Phase 6) — ±5 min buffer with `saturating_sub`/`saturating_add`.

### Known technical debt (carry forward)

19. **Profile leaf-only**: current query extracts `stacktrace[len(stacktrace)]` — can't build
    flamegraphs. Need `array_to_string(stacktrace, ';') GROUP BY folded` for flamegraph.
    *Deferred: [[architecture/profiles.md|profiles]] not in MVP.*

20. **Profile type mixing**: cpu + alloc aggregated together. Need `WHERE name='profile.cpu'`
    or parameterized filter. *Deferred: [[architecture/profiles.md|profiles]] not in MVP.*

---

## sigiro_sentinel — 1 gap

~~21. **Sentinel SQL thresholds**~~: RESOLVED (Phase 6) — `sigiro_anomalies` SQL view with error_rate >5%, p95 latency >1s, silence detection. Created in `schema.rs`.

---

## Build / ABI — 1 gap

22. **[[architecture/data-model.md|DuckDB]] extension Linux ABI**: DuckDB's pre-compiled `.a` uses GCC/libstdc++. Rust
    extensions linking against [[architecture/data-model.md|DuckDB]]'s C API must be compiled with compatible ABI. The
    `extension-template-rs` handles this, but cross-compilation (Linux from macOS) may
    need the same musl-cross approach or [[roadmap.md|Docker]]-based builds.

---

## InvestigationBlock JSON contract

The exact JSON structure is the agent-facing API. Field names, nesting, types must match.
Note: `sigiro_investigate` maps from otlp2records native schema at query time.

```json
{
  "service": "checkout",
  "window": "10:00 - 10:15",
  "metrics": [{"bucket": "10:05", "name": "http.duration", "p95": 850.0, "avg": 420.0, "n": 1200}],
  "spans": [{"trace_id": "abc...", "span_id": "def...", "name": "POST /checkout", "duration_ms": 920.0, "status": "ERROR", "kind": "SERVER", "children": [...]}],
  "logs": [{"t": "10:05:23", "status": "ERROR", "message": "connection pool exhausted", "trace_id": "abc..."}],
  "profiles": [{"function": "runtime.gcDrain", "samples": 1847.0}],
  "linked": {
    "traces": [{"trace_id": "abc...", "spans": [...]}],
    "logs": [{"t": "10:05:24", "status": "ERROR", "message": "[payments] charge failed"}]
  }
}
```

Breaking this contract breaks every AI agent trained on `agents.txt`.

---

## Storage schema — otlp2records native

Using `otlp2records` upstream schemas as-is. Per-signal tables match the [[architecture/data-model.md|Arrow]] schemas
output by `transform_traces()`, `transform_logs()`, `transform_metrics()`.

See `otlp2records` source: `src/arrow/schema.rs` for exact field definitions.

Key differences from the old Go unified schema:
- No `signal` column (implicit in table name)
- No promoted `host`/`pod`/`deploy_version` columns (in `resource_attributes` JSON)
- No `metadata` merged blob (separate `resource_attributes`, `scope_attributes`, signal attrs)
- `status_code` is Int32 not string
- `duration` is Int64 nanos not Float64 ms
- `span_kind` is Int32 not string
- Metrics: otlp2records outputs separate RecordBatches per type; sigiro_ingest inserts each into its matching per-type table (`sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`, `sigiro_metrics_exp_histogram`) — zero-copy, no unification step
- Span events embedded as `events_json` not separate rows
