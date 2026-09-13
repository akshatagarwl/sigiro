---
title: About the telemetry tables
description: What each sigiro_* table stores and how to query the machine that emitted the telemetry.
sidebar:
  order: 2
---

sigiro stores telemetry sent over OTLP. Every row describes the service and
machine that emitted it; Sigiro does not expose live host information from the
machine running the server.

## Stored signals

- `sigiro_spans` — traces and span timing
- `sigiro_logs` — log records
- `sigiro_log_templates` — deduplicated log templates
- `sigiro_metrics_gauge` — point-in-time values
- `sigiro_metrics_sum` — monotonic and non-monotonic sums
- `sigiro_metrics_histogram` — explicit-bucket histograms
- `sigiro_metrics_exp_histogram` — exponential histograms
- `sigiro_profiles` — profile samples
- `sigiro_anomalies` — detected regime shifts

Metrics use four tables because OTLP defines four different metric shapes.
Keeping them separate avoids null-heavy rows and write-time projections. Use
`UNION ALL` when a query needs several metric types.

## Ask about the right machine

Host CPU, memory, filesystem and network questions belong in the
`sigiro_metrics_*` tables. The answer is about the machine that exported those
metrics, not the machine running Sigiro.

```sql
SELECT service_name, metric_name, value, timestamp
FROM sigiro_metrics_gauge
WHERE service_name = 'checkout'
  AND metric_name LIKE 'system.filesystem%'
  AND timestamp >= TIMESTAMP '2026-09-12 00:00:00'
  AND timestamp <  TIMESTAMP '2026-09-13 00:00:00'
ORDER BY timestamp DESC;
```

An empty result means that service did not send matching host metrics. Fix the
instrumentation rather than substituting data from another machine.

## Query rule

Raw telemetry queries must include a finite half-open timestamp range:
`timestamp >= ... AND timestamp < ...`.

## Read next

- [About evidence instead of a dashboard](/docs/explanation/evidence)
- [Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding#4-query-your-data-sql-api)
