---
title: User Story
created: 2026-03-01
updated: 2026-04-15
type: summary
tags: ['ai-agent', 'architecture', 'data-model', 'duckdb', 'ducklake', 'investigation']
---

## User Story

Who uses this, how, and why no CLI yet.

---

### The User

Small team SRE. Could be one person running a SaaS product on Kubernetes. Could be a founding engineer who is also the on-call. They don't have time to learn PromQL, configure [[designs/future/perses-compat.md|Grafana]] dashboards, or manage a Datadog bill. They want to ship code, get alerted when something breaks, and fix it fast.

The other user — increasingly the primary one — is the agent itself. An LLM agent handling the on-call rotation, investigating alerts autonomously, and escalating with a diagnosis rather than a raw page.

Both users speak SQL.

---

### The Workflow

**1. Alert fires.**
Edge alert at the OTel collector detects [[designs/future/anomaly-view.md|anomaly]] (error rate spike, latency p95 jump, OOMKill event). Sends webhook with: service name, timestamp, short description.

**2. Agent receives alert.**
The agent (Claude, or any LLM with tool use) gets the alert payload. It now has two things: a service name and a time window.

**3. Agent queries S3 [[architecture/data-model.md|Parquet]] via DuckDB.**
The agent connects [[architecture/data-model.md|DuckDB]] to S3 (httpfs extension, credentials from environment). It knows the schema — every column, what each signal type means, the partition layout. It writes SQL directly.

First query: the structured [[architecture/investigation.md|investigation]] block — 4 parallel queries against the unified table:
- Metrics: bucketed time series for the service in the window
- Spans: error and slow spans with trace_ids
- Logs: recent logs, capped at 100
- [[architecture/profiles.md|Profiles]]: top functions by sample count

Second query: linked traces — follow error trace_ids across services to see the full call chain.

**4. Agent reasons over structured data.**
The agent now has: metric trends showing when things degraded, error spans showing which operations failed, logs with error messages, profile hotspots showing resource contention, and cross-service traces showing where the failure propagated.

One round of data, one round of reasoning. The agent produces a diagnosis.

**5. Agent reports.**
The agent writes a structured incident summary: root cause, affected services, evidence (specific spans, log lines, metric inflections), and suggested remediation. This goes to Slack, PagerDuty, or wherever the team gets notifications.

---

### Example: OOMKill on Checkout

```
Alert: OOMKill on checkout pod, 14:10 UTC
```

Agent queries:

```sql
-- Metrics: memory trending up?
SELECT strftime(to_timestamp(timestamp/1e9)::TIMESTAMP, '%H:%M') AS bucket,
       name, approx_quantile(value, 0.95) AS p95, avg(value) AS avg, count(*) AS n
FROM read_parquet('s3://sigiro/1772373600/*.parquet')
WHERE service='checkout' AND signal='metric'
GROUP BY bucket, name ORDER BY bucket

-- Spans: what was checkout doing?
SELECT trace_id, span_id, parent_span_id, name, value AS duration_ms, status, kind
FROM read_parquet('s3://sigiro/1772373600/*.parquet')
WHERE service='checkout' AND signal='span' AND (status='ERROR' OR value > 1000)
ORDER BY value DESC LIMIT 50

-- Logs: any OOM or memory errors?
SELECT timestamp, status, message, trace_id
FROM read_parquet('s3://sigiro/1772373600/*.parquet')
WHERE service='checkout' AND signal='log'
ORDER BY timestamp DESC LIMIT 100

-- Profiles: what's eating memory?
SELECT stacktrace[len(stacktrace)] AS leaf, sum(value) AS samples
FROM read_parquet('s3://sigiro/1772373600/*.parquet')
WHERE service='checkout' AND signal='profile'
GROUP BY leaf ORDER BY samples DESC LIMIT 10
```

Agent sees: memory metric climbing from 14:05, db.pool.active hitting limit at 14:08, error spans on `POST /checkout` at 14:10 with 2.3s duration, logs showing "connection pool exhausted", [[architecture/profiles.md|profiles]] showing `runtime.gcBgMarkWorker` dominating.

Follow-up — linked traces across services:

```sql
SELECT trace_id, span_id, parent_span_id, service, name, value AS duration_ms, status
FROM read_parquet('s3://sigiro/1772373600/*.parquet')
WHERE trace_id IN ('abc123', 'def456') AND signal IN ('span', 'log')
ORDER BY trace_id, timestamp
```

Agent sees: checkout → payments → db.query chain, payments also showing connection timeouts. Root cause: connection pool leak in checkout causing cascading failure to payments service.

Total: 2 SQL rounds, ~1.5s of query time, 1 LLM reasoning step. Diagnosis complete.

---

### Why No CLI Yet

The agent speaks SQL directly against [[architecture/data-model.md|DuckDB]] + S3 [[architecture/data-model.md|Parquet]]. It doesn't need a CLI wrapper.

More importantly: we want to sigiro what queries agents actually produce when given raw SQL access. The queries above are our hypothesis for the 5 core patterns. Before locking these into a CLI interface, we need to validate:

- Do agents actually use these patterns, or do they write different queries?
- Do agents need narrowing queries (drill-down) more often than we expect?
- What follow-up queries do agents produce after the first [[architecture/investigation.md|investigation]] block?
- Are there query patterns we haven't anticipated?

Building a CLI now would constrain agent behavior before we understand it. The [[architecture/investigation.md|investigation pipeline]] docs (see `docs/architecture/investigation.md`) gives agents everything they need to query independently. We watch, learn, then build the right interface.

CLI comes later, informed by real agent query logs.

---

### Why No Dashboards

Dashboards are for humans scanning visual patterns. Our user is an agent reading structured data. The [[architecture/investigation.md|investigation]] block is the dashboard — metrics trends, error exemplars, log context, profile hotspots, cross-service traces — all in one JSON response the agent can reason over.

If a human needs to look, they run the SQL themselves or read the agent's incident report. The data is in S3 [[architecture/data-model.md|Parquet]] — any tool that reads Parquet works ([[architecture/data-model.md|DuckDB]] CLI, Python, DataGrip).

Dashboards are a presentation layer. We're building a data layer.
