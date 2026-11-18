---
title: Investigation Workflows
created: 2026-05-12
updated: 2026-05-12
type: guide
tags: ['investigation', 'ai-agent', 'duckdb', 'workflow']
---

# Investigation Workflows

Structured workflows for investigating services, incidents, and anomalies using Sigiro's `POST /v1/diagnose` endpoint and raw SQL fallbacks.

---

## Prerequisites

```sh {"name":"setup"}
export SIGIRO_ENDPOINT="${SIGIRO_ENDPOINT:-http://localhost:9999}"
# Hosted only: export SIGIRO_API_KEY=sk_<tenant>_<random>. Local Open servers need no key.
```

---

## Workflow 1: "Is my service healthy?"

A 30-second health check for any service.

```sh {"name":"wf1-health-check"}
SERVICE="frontend"

# 1. Does the service exist?
echo "=== Services ==="
sigiro query "SELECT DISTINCT service_name FROM lake.sigiro_spans WHERE service_name = '${SERVICE}'"

# 2. Investigate last 15 minutes
# (Default window when --from/--to are omitted)
echo ""
echo "=== Investigation ==="
sigiro diagnose "${SERVICE}" | jq '{
  service,
  window,
  coverage,
  context: .context | {service_name, pod_count, dependencies, dependents}
}'
```

**What to look for:**
- `coverage.has_spans` = true: data is flowing
- `coverage.gaps` = []: no missing data
- `context.dependencies`: downstream services this service calls
- `context.dependents`: upstream services that call this one

---

## Workflow 2: "Something is slow"

Investigate latency regressions with baseline comparison.

```sh {"name":"wf2-slow"}
SERVICE="frontend"
TO_S=$(date +%s)
FROM_S=$((TO_S - 1800))  # last 30 minutes

# Run investigation with explicit window
RESULT=$(sigiro diagnose "${SERVICE}" --from "${FROM_S}" --to "${TO_S}")

# Extract operation summaries with baseline comparison
echo "=== Operation Summaries (current vs baseline) ==="
echo "${RESULT}" | jq '.operation_summaries[] | {
  operation: .span_name,
  count: .count,
  errors: .error_count,
  error_pct: .error_pct,
  p95_ms: (.duration_p95_ns / 1e6),
  baseline_p95_ms: (.baseline_p95_ns / 1e6),
  regression: (if .duration_p95_ns > .baseline_p95_ns * 1.5 then "SLOW" elif .duration_p95_ns > .baseline_p95_ns * 1.2 then "ELEVATED" else "OK" end)
}'
```

**What to look for:**
- `regression` = "SLOW": p95 is >50% above baseline
- `error_pct` spiking: correlated errors often cause slowness
- `exemplar_error_trace_id`: a specific trace to waterfall

### Find the slow trace and waterfall it

```sh {"name":"wf2-waterfall"}
SERVICE="frontend"
# Get the trace_id of the slowest error span
TRACE_ID=$(sigiro query "SELECT trace_id FROM lake.sigiro_spans WHERE service_name = '${SERVICE}' AND status_code = 2 ORDER BY duration DESC LIMIT 1" | grep -oE '[a-f0-9]{32}' | head -1)
echo "Slowest error trace: ${TRACE_ID}"

# Waterfall requires server stopped
pkill -f "sigiro serve" 2>/dev/null || true
sleep 1
sigiro waterfall "${TRACE_ID}"
```

---

## Workflow 3: "Error spike"

Investigate elevated error rates.

```sh {"name":"wf3-errors"}
SERVICE="frontend"
TO_S=$(date +%s)
FROM_S=$((TO_S - 900))  # last 15 minutes

RESULT=$(sigiro diagnose "${SERVICE}" --from "${FROM_S}" --to "${TO_S}")

# Show operations sorted by error count
echo "=== Operations by Error Count ==="
echo "${RESULT}" | jq '.operation_summaries[] | select(.error_count > 0) | {
  operation: .span_name,
  errors: .error_count,
  total: .count,
  error_pct: .error_pct,
  baseline_error_pct: .baseline_error_pct,
  exemplar_trace: .exemplar_error_trace_id
}'

# Show log templates for ERROR severity
echo ""
echo "=== Error Log Templates ==="
echo "${RESULT}" | jq '.logs[] | select(.severity_text == "ERROR") | {template, count, first_seen, last_seen}'
```

### Drill into a specific error trace

```sh {"name":"wf3-error-trace"}
SERVICE="frontend"
# Get a trace_id that has errors
TRACE_ID=$(sigiro query "SELECT trace_id FROM lake.sigiro_spans WHERE service_name = '${SERVICE}' AND status_code = 2 ORDER BY timestamp DESC LIMIT 1" | grep -oE '[a-f0-9]{32}' | head -1)
echo "Error trace: ${TRACE_ID}"
sigiro trace-tree "${TRACE_ID}"
```

---

## Workflow 4: "Multi-service incident"

When an incident spans multiple services, investigate them together.

```sh {"name":"wf4-multi"}
TO_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((TO_US - 1800 * 1000000))

# Investigate multiple services in one call
SERVICES='["frontend","checkout","payment"]'
echo "Services: ${SERVICES}"

RESULT=$(curl -sf \
  -H "Content-Type: application/json" \
  -X POST "${SIGIRO_ENDPOINT}/v1/diagnose" \
  -d "{\"services\":${SERVICES},\"from_ts\":${FROM_US},\"to_ts\":${TO_US}}")

# Show error counts per service
echo "=== Error Summary by Service ==="
echo "${RESULT}" | jq 'to_entries[] | {
  service: .key,
  span_count: .value.coverage.span_count,
  error_ops: [.value.operation_summaries[] | select(.error_count > 0) | {op: .span_name, errors: .error_count}],
  error_logs: [.value.logs[] | select(.severity_text == "ERROR") | {template: .template, count: .count}]
}'

# Show linked traces (cross-service call chains)
echo ""
echo "=== Cross-Service Linked Traces ==="
echo "${RESULT}" | jq 'to_entries[] | {
  service: .key,
  linked: [.value.linked_traces[] | {trace_id, span_count: (.spans | length)}]
}'
```

---

## Workflow 5: "What changed?"

Correlate incidents with deployments or infrastructure events.

```sh {"name":"wf5-deploys"}
SERVICE="frontend"
TO_S=$(date +%s)
FROM_S=$((TO_S - 7200))  # last 2 hours

RESULT=$(sigiro diagnose "${SERVICE}" --from "${FROM_S}" --to "${TO_S}")

# Show derived events (deploys, K8s events)
echo "=== Events ==="
echo "${RESULT}" | jq '.events[] | {type: .event_type, detected_at, description}'

# Show version changes from spans
# (If your services emit resource_attributes['service.version'])
echo ""
echo "=== Versions Seen ==="
sigiro query "SELECT DISTINCT resource_attributes::JSON->>'service.version' AS version, count(*) AS spans FROM lake.sigiro_spans WHERE service_name = '${SERVICE}' AND timestamp > make_timestamp(${FROM_S}000000) AND resource_attributes::JSON->>'service.version' IS NOT NULL GROUP BY version ORDER BY spans DESC"
```

---

## Workflow 6: "Deep SQL dive"

When the investigation endpoint doesn't have what you need, fall back to raw SQL.

### Per-operation latency distribution

```sh {"name":"wf6-sql-latency"}
SERVICE="frontend"
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 3600 * 1000000))

sigiro query "
SELECT
  span_name,
  count(*) AS n,
  round(min(duration)/1e6, 2) AS min_ms,
  round(approx_quantile(duration, 0.50)/1e6, 2) AS p50_ms,
  round(approx_quantile(duration, 0.95)/1e6, 2) AS p95_ms,
  round(approx_quantile(duration, 0.99)/1e6, 2) AS p99_ms,
  round(max(duration)/1e6, 2) AS max_ms
FROM lake.sigiro_spans
WHERE service_name = '${SERVICE}' AND timestamp > make_timestamp(${FROM_US})
GROUP BY span_name
ORDER BY p95_ms DESC
LIMIT 10
"
```

### Error rate trend (5-minute buckets)

```sh {"name":"wf6-sql-error-trend"}
SERVICE="frontend"
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 3600 * 1000000))

sigiro query "
SELECT
  time_bucket(INTERVAL '5 minutes', timestamp)::VARCHAR AS bucket,
  count(*) AS total,
  count(*) FILTER (WHERE status_code = 2) AS errors,
  round(count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*), 2) AS error_pct
FROM lake.sigiro_spans
WHERE service_name = '${SERVICE}' AND timestamp > make_timestamp(${FROM_US})
GROUP BY bucket
ORDER BY bucket
"
```

### Top error log templates

```sh {"name":"wf6-sql-log-templates"}
SERVICE="frontend"
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 3600 * 1000000))

sigiro query "
SELECT
  l.template_text AS template,
  count(*) AS count,
  min(l.timestamp)::VARCHAR AS first_seen,
  max(l.timestamp)::VARCHAR AS last_seen
FROM lake.sigiro_logs l
WHERE l.service_name = '${SERVICE}' AND l.timestamp > make_timestamp(${FROM_US})
  AND l.severity_number >= 17
GROUP BY l.template_text
ORDER BY count DESC
LIMIT 10
"
```

### Span attribute analysis

```sh {"name":"wf6-sql-attributes"}
SERVICE="frontend"
NOW_US=$(python3 -c "import time; print(int(time.time()*1e6))")
FROM_US=$((NOW_US - 900 * 1000000))

# Extract a specific attribute (e.g., http.route) and group by it
sigiro query "
WITH extracted AS (
  SELECT
    span_name,
    span_attributes::JSON->>'http.route' AS route,
    span_attributes::JSON->>'http.status_code' AS http_status,
    duration,
    status_code
  FROM lake.sigiro_spans
  WHERE service_name = '${SERVICE}' AND timestamp > make_timestamp(${FROM_US})
)
SELECT
  route,
  count(*) AS calls,
  count(*) FILTER (WHERE status_code = 2) AS errors,
  round(approx_quantile(duration, 0.95)/1e6, 2) AS p95_ms
FROM extracted
WHERE route IS NOT NULL
GROUP BY route
ORDER BY calls DESC
"
```

---

## Workflow 7: "Verify data quality"

Before trusting investigation results, verify data coverage.

```sh {"name":"wf7-coverage"}
SERVICE="frontend"
RESULT=$(sigiro diagnose "${SERVICE}")

# Check truncation flags
echo "=== Truncation ==="
echo "${RESULT}" | jq '.truncation'

# Check coverage
echo ""
echo "=== Coverage ==="
echo "${RESULT}" | jq '.coverage'

# If spans_total > spans_returned, data was truncated
SPANS_TOTAL=$(echo "${RESULT}" | jq -r '.truncation.spans_total')
SPANS_RET=$(echo "${RESULT}" | jq -r '.truncation.spans_returned')
if [ "${SPANS_TOTAL}" -gt "${SPANS_RET}" ]; then
  echo "WARNING: Spans truncated (${SPANS_RET}/${SPANS_TOTAL}). Narrow the time window for full detail."
fi
```

---

## Time Windows Cheat Sheet

| Window | Use case | CLI `--from` |
|--------|----------|-------------|
| Last 5 min | Real-time health | `$(($(date +%s) - 300))` |
| Last 15 min | Incident triage (default) | omit `--from` |
| Last 1 hour | Post-incident review | `$(($(date +%s) - 3600))` |
| Last 4 hours | Pattern analysis | `$(($(date +%s) - 14400))` |
| Custom | Specific incident | Unix epoch seconds |

**Note:** CLI `--from`/`--to` are epoch **seconds**. The API `from_ts`/`to_ts` are epoch **microseconds**.

---

## Agent Decision Tree

```
Agent receives: "checkout is slow"

1. GET /v1/services → confirm "checkout" exists
2. POST /v1/diagnose {"services":["checkout"]}
   a. Check coverage.gaps → if gaps exist, warn about missing data
   b. Check operation_summaries for p95_ms > baseline_p95_ms * 1.5
   c. Note exemplar_error_trace_id if errors exist
3. If errors found:
   a. GET /v1/traces/{exemplar_error_trace_id} → critical path
   b. sigiro waterfall {trace_id} → visual bottleneck (server stopped)
4. If slowness without errors:
   a. Check metrics for latency spikes
   b. Check linked_traces for downstream slowdown
5. Check events for recent deploys
6. If multi-service impact:
   a. POST /v1/diagnose {"services":["checkout","payment","inventory"]}
```
