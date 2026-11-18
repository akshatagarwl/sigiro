---
title: Investigation Pipeline
created: 2026-03-21
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'comparison', 'data-model', 'deployment', 'duckdb']
---

# Investigation Pipeline

One API call returns a structured investigation block covering metrics, spans, logs,
linked traces, operation summaries, derived events, and service topology. The agent
reads one response and reasons over the full picture.

---

## Why One API Call

### The Old World

```
agent receives alert: "error rate spike on checkout"

round 1: query metrics store for checkout latency/error rate     -> 200ms
         llm reasons: "error rate spiked at 14:10, need traces"  -> 5000ms

round 2: query trace store for error spans on checkout           -> 300ms
         llm reasons: "db calls timing out, need db metrics"     -> 5000ms

round 3: query metrics store for payments-db latency             -> 200ms
         llm reasons: "db latency spiked too, need logs"         -> 5000ms

round 4: query log store for checkout + payments-db errors       -> 400ms
         llm reasons: "connection pool exhausted, check deploy"  -> 5000ms

round 5: query events store for recent deployments               -> 150ms
         llm reasons: "deploy at 14:05 changed pool size"        -> 3000ms

total: 5 queries, 5 llm calls, ~25 seconds, tons of stitching code
```

Each round the agent discovers what to ask next because the data lives in different
stores with different query languages.

### The New World

The agent calls `POST /diagnose` with a service name and time window. The API runs
9 parallel [[architecture/data-model.md|DuckDB]] queries internally, assembles a structured investigation block, and
returns it. The agent sees metric trends, error spans with call trees, log templates,
operation summaries with baseline comparison, derived events (deploys, OOMKills),
service topology, and cross-service linked traces -- all in one response.

The only time a second call is needed is to investigate a *different* service:

```
POST /diagnose/multi  {"services": ["checkout", "payments"], "from_ts": ..., "to_ts": ...}
```

Two rounds max for most incidents. Not five.

| | old world (datadog/splunk/etc) | sigiro |
|---|---|---|
| queries per incident | 4-5 minimum | 1-2 |
| LLM calls per incident | 4-5 | 1-2 |
| stitching code | extensive -- correlate by trace_id across stores | zero -- data arrives pre-correlated |
| wall clock time | ~25s (5 rounds x 5s reasoning) | ~6s (1 round x 5s reasoning) |
| context window usage | fragmented across rounds, loses earlier context | one shot, full picture |
| query languages agent needs | PromQL + LogQL + TraceQL + SQL | SQL only |

---

## API Surface

### POST /diagnose

Single-service investigation. Default window: last 15 minutes.

```json
{
  "service": "checkout",
  "from_ts": 1709294400000000,
  "to_ts": 1709295900000000
}
```

`from_ts` and `to_ts` are epoch **microseconds**. Both optional (defaults: `to_ts` = now, `from_ts` = `to_ts` - 15 min).

Returns: `InvestigationBlock` (see below).

### POST /diagnose/multi

Multi-service investigation. Max 10 services per call.

```json
{
  "services": ["checkout", "payments", "inventory"],
  "from_ts": 1709294400000000,
  "to_ts": 1709295900000000
}
```

Returns: `BTreeMap<service_name, InvestigationBlock>`.

---

## SQL Queries

All queries run against the current [[architecture/data-model.md|DuckDB]] schema: `sigiro_spans`, `sigiro_logs`,
`sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`,
`sigiro_metrics_exp_histogram`. Sort order is `(service_name, timestamp)` on every table.

### 1. Metrics -- bucketed time series (UNION ALL across 4 metric tables)

```sql
WITH scalar AS (
    SELECT timestamp, metric_name, value AS p95_src, value AS avg_src, 1 AS cnt
    FROM sigiro_metrics_gauge
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
    UNION ALL
    SELECT timestamp, metric_name, value, value, 1
    FROM sigiro_metrics_sum
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
),
histo AS (
    SELECT timestamp, metric_name, max AS p95_src,
           CASE WHEN count > 0 THEN sum/count ELSE 0 END AS avg_src, count AS cnt
    FROM sigiro_metrics_histogram
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
    UNION ALL
    SELECT timestamp, metric_name, max,
           CASE WHEN count > 0 THEN sum/count ELSE 0 END, count
    FROM sigiro_metrics_exp_histogram
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
),
combined AS (
    SELECT * FROM scalar UNION ALL SELECT * FROM histo
)
SELECT time_bucket(INTERVAL '{bucket_interval}', timestamp)::VARCHAR AS bucket,
       metric_name,
       approx_quantile(p95_src, 0.95) AS p95,
       avg(avg_src) AS avg,
       sum(cnt) AS n
FROM combined
GROUP BY bucket, metric_name
ORDER BY bucket, metric_name
```

Bucket interval is adaptive (see below).

### 2. Operation summaries -- per-operation aggregates with baseline comparison

```sql
WITH current AS (
    SELECT span_name, count(*) AS cnt,
           count(*) FILTER (WHERE status_code = 2) AS err_cnt,
           count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*) AS err_pct,
           approx_quantile(duration, 0.50) AS p50,
           approx_quantile(duration, 0.95) AS p95,
           approx_quantile(duration, 0.99) AS p99,
tm|           first(trace_id) FILTER (WHERE status_code = 2) AS exemplar_error_trace_id
    FROM sigiro_spans
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
    GROUP BY span_name
),
baseline AS (
    SELECT span_name,
           count(*) AS bl_cnt,
           count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*) AS bl_err_pct,
           approx_quantile(duration, 0.95) AS bl_p95
    FROM sigiro_spans
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
    GROUP BY span_name
)
SELECT c.*, b.bl_err_pct, b.bl_p95, b.bl_cnt
FROM current c LEFT JOIN baseline b ON c.span_name = b.span_name
ORDER BY c.err_cnt DESC, c.cnt DESC
LIMIT 20
```

Baseline window = 4x the investigation window, immediately preceding it.

### 3. Span stats -- total spans + distinct error traces (one query)

```sql
SELECT count(*),
       count(DISTINCT CASE WHEN status_code = 2 THEN trace_id END)
FROM sigiro_spans
WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
```

### 4. Spans -- error/slow exemplars

```sql
SELECT trace_id, span_id, parent_span_id, span_name, duration,
       status_code, service_name
FROM sigiro_spans
WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
ORDER BY CASE WHEN status_code = 2 THEN 0 ELSE 1 END, duration DESC
LIMIT {span_limit}
```

Parent-child tree assembled in Rust from `parent_span_id`.

### 5. Logs -- deduplicated templates

```sql
SELECT
    regexp_replace(
      regexp_replace(
        regexp_replace(body,
          '[0-9a-f]{8}-...-[0-9a-f]{12}', '{uuid}', 'gi'),
        '\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}\b', '{ip}', 'g'),
      '\b\d{4,}\b', '{id}', 'g'
    ) AS template,
    first(body) AS exemplar_body,
    severity_text,
    count(*) AS count,
    min(timestamp)::VARCHAR AS first_seen,
    max(timestamp)::VARCHAR AS last_seen
FROM sigiro_logs
WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
GROUP BY template, severity_text
ORDER BY count DESC
LIMIT ?
```

### 6. Linked traces -- cross-service call chains

Collect error `trace_id` values from the spans query (capped at 5). Then:

```sql
SELECT trace_id, span_id, parent_span_id, span_name, duration,
       status_code, service_name
FROM sigiro_spans
WHERE trace_id IN (?, ?, ...)
  AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
ORDER BY trace_id, timestamp
LIMIT 200
```

Time range is the investigation window +/- 5 minutes. `trace_id` is not in the sort
order -- this query cannot use row group min/max stats.

### 7. Service context -- metadata + topology

```sql
-- resource attributes (namespace, deployment, pod count)
SELECT ANY_VALUE(resource_attributes::JSON->>'k8s.namespace.name') AS namespace,
       ANY_VALUE(resource_attributes::JSON->>'k8s.deployment.name') AS deployment,
       count(DISTINCT service_instance_id) AS pod_count,
       min(timestamp)::VARCHAR AS first_seen,
       max(timestamp)::VARCHAR AS last_seen
FROM sigiro_spans
WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)

-- downstream dependencies (CLIENT/PRODUCER spans)
SELECT DISTINCT s2.service_name
FROM sigiro_spans s1 JOIN sigiro_spans s2 ON s1.trace_id = s2.trace_id
WHERE s1.service_name = ? AND s2.service_name != ?
  AND s1.span_kind IN (3, 4)
  AND s1.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
LIMIT 20

-- upstream dependents
SELECT DISTINCT s1.service_name
FROM sigiro_spans s1 JOIN sigiro_spans s2 ON s1.trace_id = s2.trace_id
WHERE s2.service_name = ? AND s1.service_name != ?
  AND s1.span_kind IN (3, 4)
  AND s2.timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
LIMIT 20
```

### 8. Derived events -- deploys + K8s events

```sql
-- Deploy detection method 1: CI/CD pipeline spans (preferred when available)
SELECT timestamp::VARCHAR AS detected_at,
       span_attributes::JSON->>'cicd.pipeline.result' AS result,
       span_attributes::JSON->>'cicd.pipeline.name' AS pipeline,
       resource_attributes::JSON->>'service.version' AS version
FROM sigiro_spans
WHERE span_name LIKE 'cicd.pipeline%'
  AND resource_attributes::JSON->>'service.name' = ?
  AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
ORDER BY detected_at

-- Deploy detection method 2: distinct service.version values (fallback)
WITH versioned AS (
    SELECT timestamp, resource_attributes::JSON->>'service.version' AS version
    FROM sigiro_spans
    WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
)
SELECT min(timestamp)::VARCHAR AS detected_at, version
FROM versioned WHERE version IS NOT NULL
GROUP BY version ORDER BY detected_at

-- K8s events from log body patterns
SELECT timestamp::VARCHAR AS detected_at, body AS description
FROM sigiro_logs
WHERE service_name = ? AND timestamp BETWEEN make_timestamp(?) AND make_timestamp(?)
  AND (body LIKE '%OOMKilled%' OR body LIKE '%Unhealthy%'
    OR body LIKE '%BackOff%' OR body LIKE '%FailedScheduling%'
    OR body LIKE '%ScalingReplicaSet%')
LIMIT 20
```

### 9. Coverage -- what data exists

Derived from the counts returned by queries 3 and 5, plus the length of the metrics
vector. No separate SQL query.

---

## InvestigationBlock JSON

This is the actual Rust struct serialized as JSON. One canonical copy.

```json
{
  "service": "checkout",
  "window": {
    "from_ts": 1709294400000000,
    "to_ts": 1709295900000000,
    "from_iso": "2024-02-29T14:00:00+00:00",
    "to_iso": "2024-02-29T14:25:00+00:00"
  },
  "truncation": {
    "spans_total": 342,
    "spans_returned": 50,
    "logs_total": 87,
    "logs_returned": 30,
    "error_traces_total": 12,
    "error_traces_returned": 5,
    "linked_spans_total": 45,
    "linked_spans_returned": 45
  },
  "coverage": {
    "has_spans": true,
    "has_logs": true,
    "has_metrics": true,
    "span_count": 342,
    "log_template_count": 87,
    "metric_series_count": 5,
    "gaps": []
  },
  "context": {
    "service_name": "checkout",
    "first_seen": "2024-02-29T14:00:01.234+00:00",
    "last_seen": "2024-02-29T14:24:58.567+00:00",
    "namespace": "production",
    "deployment": "checkout",
    "pod_count": 3,
    "dependencies": ["payments", "inventory"],
    "dependents": ["api-gateway"]
  },
  "events": [
    {"detected_at": "2024-02-29T14:05:00+00:00", "event_type": "deploy", "description": "version v2.3.1 first seen"},
    {"detected_at": "2024-02-29T14:10:02+00:00", "event_type": "k8s_event", "description": "OOMKilled by kubelet"}
  ],
  "operation_summaries": [
    {
      "span_name": "POST /checkout",
      "count": 200,
      "error_count": 24,
      "error_pct": 12.0,
      "duration_p50_ns": 450000000,
      "duration_p95_ns": 2100000000,
      "duration_p99_ns": 2500000000,
      "exemplar_error_trace_id": "abc123",
      "baseline_error_pct": 0.5,
      "baseline_p95_ns": 180000000,
      "baseline_count": 800
    }
  ],
  "metrics": [
    {"bucket": "2024-02-29 14:10:00", "metric_name": "http.duration", "p95": 1200.0, "avg": 800.0, "count": 342}
  ],
  "spans": [
    {
      "trace_id": "abc123",
      "span_id": "span1",
      "parent_span_id": "",
      "span_name": "POST /checkout",
      "duration_ns": 2300000000,
      "status_code": 2,
      "service_name": "checkout"
    }
  ],
  "span_trees": {
    "span1": ["span2", "span3"]
  },
  "logs": [
    {
      "template": "connection pool exhausted, cannot acquire connection",
      "body": "connection pool exhausted, cannot acquire connection",
      "severity_text": "ERROR",
      "count": 15,
      "first_seen": "2024-02-29 14:10:03",
      "last_seen": "2024-02-29 14:24:55"
    }
  ],
  "linked_traces": [
    {
      "trace_id": "abc123",
      "spans": [
        {"trace_id": "abc123", "span_id": "s1", "parent_span_id": "", "span_name": "POST /checkout", "duration_ns": 2300000000, "status_code": 2, "service_name": "checkout"},
        {"trace_id": "abc123", "span_id": "s2", "parent_span_id": "s1", "span_name": "POST /pay", "duration_ns": 2100000000, "status_code": 2, "service_name": "payments"}
      ]
    }
  ]
}
```

**Agent MUST check `truncation`:** if any `_total > _returned`, the data is incomplete.
Acknowledge this or request a narrower window.

**Agent MUST check `coverage.gaps`:** if gaps is non-empty, absence of data does not
mean absence of problems.

---

## Token Budget

### Adaptive summarization

Detail level varies by window size to stay under token budget:

| Window | Metric bucket interval | Span exemplar limit | Log template limit |
|--------|------------------------|--------------------|--------------------|
| < 15m  | 1 minute               | 50                 | 30                 |
| 15m-2h | 5 minutes              | 20                 | 30                 |
| > 2h   | 1 hour                 | 0 (summaries only) | 20                 |

### Typical response size

For a 25-minute window, single service: ~10-50K tokens of structured evidence.
Fits comfortably in a single LLM context window. The agent reasons over the full
picture in one call.

If the window is too large, the adaptive config automatically coarsens:
- Aggregate metrics into wider buckets
- Return operation summaries only (no individual spans)
- Deduplicate log templates

The agent can always drill down into a narrower window if the summary suggests
something interesting.

### Hard caps

| Field | Cap |
|-------|-----|
| Span exemplars | 50 (< 15m), 20 (15m-2h), 0 (> 2h) |
| Log templates | 30 (< 2h), 20 (> 2h) |
| Error trace_ids for linking | 5 |
| Linked spans total | 200 |
| Operation summaries | 20 |
| Dependency/dependent services | 20 each |
| K8s events | 20 |

---

## Agent SQL Guide

[[architecture/data-model.md|DuckDB]] gotchas and patterns useful when agents issue raw SQL via `/query`.

### DuckDB-specific gotchas

- **`resource_attributes` is a JSON column.** Use the `->>` operator for flat keys:
  `resource_attributes::JSON->>'k8s.namespace.name'`. `json_extract_string` fails on
  keys containing dots.

- **`time_bucket` returns TIMESTAMP.** Must cast to VARCHAR for duckdb-rs String reads:
  `time_bucket(INTERVAL '1 minute', timestamp)::VARCHAR`.

- **`status_code` is integer, not string.** `0` = UNSET, `1` = OK, `2` = ERROR.
  Map at query time: `CASE status_code WHEN 2 THEN 'ERROR' WHEN 1 THEN 'OK' ELSE 'UNSET' END`.

- **`span_kind` is integer, not string.** `1` = INTERNAL, `2` = SERVER, `3` = CLIENT,
  `4` = PRODUCER, `5` = CONSUMER.

- **`duration` on spans is nanoseconds (i64).** Divide by `1e6` for milliseconds,
  `1e9` for seconds.

- **Timestamps use `make_timestamp(epoch_microseconds)`.** The `from_ts`/`to_ts` API
  parameters are epoch microseconds.

- **CTE approach for JSON extraction.** Extract JSON fields in a CTE first, then GROUP BY
  the extracted value. Avoids [[architecture/data-model.md|DuckDB]] casting issues with repeated `::JSON` casts in
  GROUP BY clauses.

### Table schema (current)

| Table | Key columns | Notes |
|-------|------------|-------|
| `sigiro_spans` | timestamp, trace_id, span_id, parent_span_id, service_name, span_name, duration, status_code, span_kind, resource_attributes, span_attributes | Sort: (service_name, timestamp) |
| `sigiro_logs` | timestamp, trace_id, span_id, service_name, severity_number, severity_text, body, resource_attributes, log_attributes | Sort: (service_name, timestamp) |
| `sigiro_metrics_gauge` | timestamp, metric_name, service_name, value | Sort: (service_name, timestamp) |
| `sigiro_metrics_sum` | timestamp, metric_name, service_name, value | Sort: (service_name, timestamp) |
| `sigiro_metrics_histogram` | timestamp, metric_name, service_name, count, sum, min, max | Sort: (service_name, timestamp) |
| `sigiro_metrics_exp_histogram` | timestamp, metric_name, service_name, count, sum, min, max | Sort: (service_name, timestamp) |

### Historical note (stale)

The original design used a single flat [[architecture/data-model.md|Parquet]] table with a `signal` column
(`"metric"`, `"span"`, `"log"`, `"profile"`) and sort order `(service, signal,
timestamp)`. This was replaced by the per-signal DuckDB tables above when the project
moved from [[architecture/data-model.md|Parquet]]-on-S3 to DuckDB with otlp2records native Arrow schemas.

---

## CLI

```
sigiro diagnose <service> [--from <epoch_seconds>] [--to <epoch_seconds>]
```

`--from` and `--to` are epoch **seconds** (the CLI multiplies by 1,000,000 to convert
to the microsecond timestamps the API expects). Both optional -- defaults to the last
15 minutes.

Output: pretty-printed InvestigationBlock JSON to stdout.

Note: the CLI opens the [[architecture/data-model.md|DuckDB]] file directly. If `sigiro serve` is running, it holds
the file lock -- use `POST /diagnose` via HTTP instead.
