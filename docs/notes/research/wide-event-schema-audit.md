---
title: Wide Event Schema Audit
created: 2026-03-05
updated: 2026-04-15
type: comparison
tags: ['ai-agent', 'architecture', 'auth', 'comparison', 'competitor', 'data-model']
---

# Wide Event Schema Audit

Comparison of our UnifiedRow against the wide event paradigm from Honeycomb, Axiom, Meta's Scuba, happycontext, and loggingsucks.com. What are we missing, what are we taking for granted, and what should change.

---

## Our Current Schema

```
timestamp        int64           unix nanos
signal           string (dict)   metric / span / log / profile
service          string (dict)
host             string (dict)
pod              string (dict)
deploy_version   string (dict)
trace_id         string?
span_id          string?
parent_span_id   string?
name             string (dict)
value            float64?
status           string? (dict)
message          string?
unit             string? (dict)
kind             string? (dict)
stacktrace       []string?
events           []Event?
metadata         string?          JSON blob
```

17 columns. Sort order: `(service, signal, timestamp)`.

---

## What a Wide Event System Looks Like

Honeycomb, Axiom, and Scuba all converge on the same model:

| Property | Honeycomb | Axiom | Scuba (Meta) |
|----------|-----------|-------|-------------|
| Schema | Schema-less (any JSON) | Schema-less (`map[string]any`) | Schema-less (wide events) |
| Fixed fields | None required | `_time` (auto-set) + `_sysTime` (ingestion time) | Timestamp only |
| Max fields | 2,000 per event | 256-1,024 per dataset | Unbounded |
| Max event size | 1 MB | 1 MB per field | Unknown |
| Cardinality | Unlimited | Unlimited | Unlimited |
| Pre-aggregation | None | None | None |
| Storage | Columnar | Columnar | Columnar |

The core idea: **every event is a flat bag of typed key-value pairs**. There is no separate concept of "metrics" vs "spans" vs "logs" — those are just events with different fields populated.

---

## Gap Analysis: What We're Missing

### Gap 1: `metadata` is a JSON string blob, not queryable columns

**Problem:** Our `metadata` column is `string?` containing JSON. To filter by `http.status_code=500` or `user.id=u_8472`, the query must `JSON_EXTRACT(metadata, '$.http.status_code')` on every row. This makes high-cardinality attribute queries impossible at scale.

**What wide event systems do:** Every attribute is a first-class column. `http.method`, `http.status_code`, `user.id`, `feature.flag` — all separate columns. [[architecture/data-model.md|Parquet]]'s columnar format means unused columns [[company/cost.md|cost]] near-zero storage and [[architecture/data-model.md|DuckDB]] skips them during scans.

**What we should do:** This is the single most important gap. Two options:

1. **Promote known OTel semantic conventions to columns** — `http.method`, `http.status_code`, `http.route`, `rpc.method`, `db.system`, `db.statement`, `net.peer.name`, `error.type`. ~10-15 columns. Keeps schema manageable, covers 80% of queries.

2. **Schema-less approach** — write each attribute as its own [[architecture/data-model.md|Parquet]] column dynamically. Parquet supports schema evolution (columns can appear/disappear across files). [[architecture/data-model.md|DuckDB]]'s `read_parquet` with `union_by_name=true` handles heterogeneous schemas. This is how Axiom works.

**Recommendation:** Start with option 1 (promoted columns) for the query patterns agents actually need. The `metadata` blob stays as a catch-all for everything else. Option 2 is a v2 migration when we have real user attribute patterns.

### Gap 2: No ingestion timestamp

**Problem:** We only store the event's source timestamp. We don't know when we received it.

**What Axiom does:** Two timestamps — `_time` (event time) and `_sysTime` (ingestion time). The gap between them reveals clock skew, delayed delivery, and backfill operations.

**What we should do:** Add `ingest_timestamp int64` (unix nanos, set by the exporter at write time). Costs one int64 per row (~8 bytes). Enables:
- Detecting clock skew (event says 14:00, ingested at 15:00)
- Backfill identification
- Debugging "why didn't my data appear"

### Gap 3: No duration column separate from value

**Problem:** `value` means different things per signal: metric value, span duration (ms), profile sample count. For spans, duration is the most important field — but it's overloaded into the generic `value` column with a comment contract.

**What happycontext does:** Explicit `duration_ms` field on every request event.

**What we should do:** Add `duration_ms float64?` for spans, keep `value` for metric values and profile sample counts. This removes the stringly-typed contract between mapper and query layer (the `UnitSpanDuration` / `value > 1000` fragility noted in the code). Query layer can use `duration_ms` directly instead of relying on `unit = 'ms'` to interpret `value`.

### Gap 4: No HTTP semantics as first-class columns

**Problem:** Agent investigating "why is checkout slow" almost always needs: HTTP method, [[plans/STATUS.md|status]] code, route, and the downstream service. These are buried in `metadata` JSON.

**What happycontext captures automatically:** `http.method`, `http.path`, `http.route`, `http.status` as top-level fields.

**What we should do:** Promote the highest-value OTel semantic conventions:

```
http_method       string? (dict)     # http.request.method
http_status_code  int32?             # http.response.status_code
http_route        string? (dict)     # http.route
```

These three columns cover the majority of "what happened" questions for HTTP services. Dict-encoded, they compress to near-zero for non-HTTP signals.

### Gap 5: No error classification column

**Problem:** `status` stores span status (OK/ERROR/UNSET) and log severity (INFO/WARN/ERROR). There's no field for error type/class.

**What wide event systems recommend:** An `error.type` or `exception.type` field so agents can group errors by class (e.g., "NullPointerException" vs "TimeoutException" vs "ConnectionRefused").

**What we should do:** Add `error_type string? (dict)`. Populated from:
- Span events with name `exception`: extract `exception.type` attribute
- Log records: extract from attributes if present
- Metrics: null

### Gap 6: Logs don't carry attributes in metadata

**Problem:** `mapLogs` never populates `metadata`. Log record attributes (which can contain structured context like `user.id`, `request.id`, `feature.flag`) are silently dropped.

**What loggingsucks.com recommends:** Every wide event should carry 50+ fields of context. For logs, the attributes ARE the context.

**What we should do:** Populate `metadata` for logs the same way we do for spans:
```go
if lr.Attributes().Len() > 0 {
    attrs := map[string]string{}
    lr.Attributes().Range(func(k string, v pcommon.Value) bool {
        attrs[k] = v.AsString()
        return true
    })
    metadata = mustJSONString(attrs)
}
```

This is a simple mapper fix with zero schema change.

### Gap 7: Resource attributes beyond service/host/pod are dropped

**Problem:** The mapper extracts only 4 resource attributes: `service.name`, `host.name`, `k8s.pod.name`, `service.version`. Everything else (cloud provider, region, namespace, container name, OS, SDK version, etc.) is silently dropped.

**What wide event systems do:** Keep all resource attributes as fields on the event.

**What we should do:** Merge remaining resource attributes into `metadata`. Add to each map* function:
```go
res.Attributes().Range(func(k string, v pcommon.Value) bool {
    if k != "service.name" && k != "host.name" && k != "k8s.pod.name" && k != "service.version" {
        attrs[k] = v.AsString()
    }
    return true
})
```

This captures `cloud.provider`, `cloud.region`, `k8s.namespace.name`, `k8s.deployment.name`, `telemetry.sdk.language`, etc. without schema changes.

---

## What We're Doing Right (Keep)

### Unified table
Every wide event system converges on "one table, all events." We're already there. Axiom, Honeycomb, and Scuba all store everything in one place.

### Sort order (service, signal, timestamp)
Optimal for the query pattern "what happened to service X in time window Y." Row group pruning by service+signal is effectively free filtering.

### Nullable columns cost nothing in Parquet
A log row with null stacktrace, null span_id, null events costs zero storage. [[architecture/data-model.md|Parquet]] columnar format is perfectly suited for sparse wide events.

### Trace correlation via trace_id/span_id
First-class trace IDs enable linked trace queries. This is something Axiom doesn't have natively (they rely on field [[company/naming.md|naming]] conventions).

### Profile stacktraces as `list<string>`
SQL-queryable stacks without a separate profiling backend. `GROUP BY stacktrace[leaf]` is elegant.

### No pre-aggregation
Metrics are raw data points, not pre-aggregated. [[architecture/data-model.md|DuckDB]] computes p95/avg at query time. This is the Scuba/Honeycomb/Axiom approach — pre-aggregation is a write-time decision that loses information.

---

## What We Should Reconsider

### The `signal` column as a type discriminator

In a pure wide event model, there is no "signal type." A request event might have a duration (span-like), a message (log-like), metrics (gauge-like), and a stacktrace (profile-like) — all on the same row. The signal column forces a single classification.

**For now: keep it.** The sort order depends on `signal` for row group clustering, and the query layer runs 4 parallel queries filtered by signal. Removing it would require a fundamentally different query architecture. But as we add promoted attribute columns (Gap 1), the line between signals blurs — a span row with `http_status_code=500` and `message="timeout"` and `duration_ms=2300` is already a wide event.

### The `name` column is overloaded

`name` means: metric name (`http.duration`), span operation (`POST /checkout`), log scope (`com.example.auth`), profile type (`profile.cpu`). Four different semantics in one column.

**For now: keep it.** Dict encoding makes it efficient, and the query layer always filters by signal first, so name semantics are unambiguous within a query. But consider renaming to something more generic if we ever blur signals.

---

## Proposed Schema Changes (Priority Order)

### P0: Fix data loss (no schema change)

1. **Populate log metadata** — `mapLogs` should serialize log attributes into `metadata` (Gap 6)
2. **Preserve resource attributes** — merge non-promoted resource attributes into `metadata` for all signals (Gap 7)

### P1: Add high-value promoted columns

```
http_method       string? (dict)    # from span/log attributes
http_status_code  int32?            # from span/log attributes
http_route        string? (dict)    # from span/log attributes
error_type        string? (dict)    # from span exception events
duration_ms       float64?          # span duration (currently overloaded in value)
```

5 new columns. All nullable, all dict-encoded (except status_code and duration_ms). Near-zero storage overhead for signals that don't use them.

### P2: Add ingestion timestamp

```
ingest_timestamp  int64             # unix nanos, set by exporter at write time
```

1 new column. Fixed 8 bytes per row.

### P3: Consider dynamic attribute columns (v2)

[[architecture/data-model.md|Parquet]] supports schema evolution. [[architecture/data-model.md|DuckDB]]'s `union_by_name=true` reads heterogeneous schemas. We could write each high-cardinality attribute as its own column, letting the schema grow organically per service. This is the Axiom model. Defer until we have real usage patterns.

---

## Sources

- [loggingsucks.com](https://loggingsucks.com/) — wide events thesis, 50+ fields per event
- [nominal.dev](https://nominal.dev/) — agent-first incident resolution
- [Boris Tane: Wide Events 101](https://boristane.com/blog/observability-wide-events-101/) — practical schema guidance
- [Ivan Burmistrov: All You Need Is Wide Events](https://isburmistrov.substack.com/p/all-you-need-is-wide-events-not-metrics) — Scuba at Meta, unified event model
- [Charity Majors: Observability 2.0](https://charity.wtf/tag/observability-2-0/) — single source of truth, no pre-aggregation
- [Axiom docs: event data](https://axiom.co/docs/[[getting-started.md|getting-started]]-guide/event-data) — schema-less event model, `_time` + `_sysTime`
- [Axiom docs: limits](https://axiom.co/docs/reference/limits) — 1,024 fields, 1 MB per field
- [Axiom: Designing MCP Servers for Wide Events](https://axiom.co/blog/designing-mcp-servers-for-wide-events) — agent query optimization for wide schemas
- [Honeycomb: Structured Events Are the Basis of Observability](https://www.honeycomb.io/blog/structured-events-basis-observability) — 2,000 fields, schema-less, dynamic sampling
- [happycontext](https://github.com/happytoolin/happycontext) — Go library, one event per request, automatic HTTP fields
