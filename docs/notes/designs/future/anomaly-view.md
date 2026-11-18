---
title: Anomaly View — Design
created: 2026-03-28
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'comparison', 'competitor', 'data-model', 'decision']
---

# Anomaly View — Design

## Status

v1 DONE in schema.rs (lines 172-201): flat thresholds for error_rate, high_latency, silence.
v2 NEEDED: relative anomalies against service baselines, cross-signal.

## Problem with v1

Flat thresholds (>5% errors, >1s p95) are naive. A service with normal 2s p95
won't trigger. A service with normal 0.01% error rate won't catch a jump to 3%.
Every service has its own baseline.

## v2: Relative Anomaly Detection (still pure SQL)

Core idea: compare current window vs rolling baseline using [[architecture/data-model.md|DuckDB]] window functions.

### Approach

```sql
-- For each service+operation, compute:
--   baseline = avg + stddev over trailing 1h (in 5-min buckets)
--   current  = value in last 5 min
--   anomaly  = current > baseline_mean + 3 * baseline_stddev

WITH buckets AS (
  SELECT service_name, span_name,
         time_bucket(INTERVAL '5 minutes', timestamp) AS bucket,
         approx_quantile(duration, 0.95) AS p95_ns,
         count(*) FILTER (WHERE status_code = 2) * 100.0 / count(*) AS error_pct,
         count(*) AS span_count
  FROM sigiro_spans
  WHERE timestamp > now()::TIMESTAMP - INTERVAL '1 hour'
  GROUP BY 1, 2, 3
),
baselines AS (
  SELECT service_name, span_name,
         avg(p95_ns) AS mean_p95,
         stddev(p95_ns) AS std_p95,
         avg(error_pct) AS mean_err,
         stddev(error_pct) AS std_err,
         avg(span_count) AS mean_count
  FROM buckets
  WHERE bucket < time_bucket(INTERVAL '5 minutes', now()::TIMESTAMP)
  GROUP BY 1, 2
),
current AS (
  SELECT * FROM buckets
  WHERE bucket = time_bucket(INTERVAL '5 minutes', now()::TIMESTAMP)
)
-- Anomaly: current value > mean + 3σ of baseline
```

### Anomaly Types (all relative)

FROM SPANS:
  - latency_spike:  p95 > mean_p95 + 3 * std_p95
  - error_burst:    error_pct > mean_err + 3 * std_err
  - traffic_spike:  span_count > mean_count + 3 * std_count
  - silence:        service had baseline traffic but current = 0

FROM LOGS:
  - error_log_surge: ERROR severity count vs baseline
  - new_pattern:     log body template not seen in trailing 24h

FROM METRICS:
  - gauge_anomaly:   value outside 3σ of rolling mean
  - counter_stall:   monotonic counter delta = 0 vs baseline > 0

CROSS-SIGNAL:
  - correlated:      latency_spike AND error_burst on same service (strongest signal)

### Key Decisions

- 3σ default, configurable per service later
- 1h rolling baseline (12 x 5-min buckets) — enough history, not too stale
- Still a VIEW — no background job, [[architecture/data-model.md|DuckDB]] handles the computation
- One unified view: SELECT * FROM sigiro_anomalies WHERE anomaly_type = 'latency_spike'

### Files to Change

- `src/schema.rs` — replace v1 view with v2
- Tests: insert known anomalous data, verify detection
