---
title: "How We Use Sigiro to Self-Heal and Self-Optimize Sigiro"
description: Every profiling pipeline loses data. Ours tells us exactly when, where, and why.
---

# How We Use Sigiro to Self-Heal and Self-Optimize Sigiro

*Every profiling pipeline loses data. Ours tells us exactly when, where, and why.*

---

## The Blind Spot

Most profiling pipelines have a blind spot: they collect data from your services but know nothing about themselves. When the pipeline breaks, you don't find out until someone notices their dashboards are empty. When data is lost in transit, there's no record of what happened. When a component fails, you're diagnosing a black box.

Sigiro is a profiling data pipeline built on OpenTelemetry. It receives profiles and traces via OTLP, stores them in DuckDB, and serves investigative queries. From the first commit, it was designed to observe itself.

---

## The Architecture: Sigiro's OTLP Self-Loop

```
Client → /ingest/pyroscope → Sigiro → OTLP → DuckDB
                                         ↑
                                    (self-loop)
```

Sigiro acts as its own OTLP collector. Every operation it performs — every ingest, every flush, every connection open — generates a span stored in the same DuckDB instance. POST `{"services":["sigiro"]}` to `/v1/diagnose` and you get Sigiro's own operational telemetry, queryable with the same tool you'd use for any other service.

---

## How Self-Healing Works: Three Real Bugs

### Bug 1: The Stalled Flusher

During a 300-span bulk load test, we watched the flusher task silently stop. The flusher writes buffered data to DuckDB on a schedule. A race between the shutdown signal and the flusher loop caused it to exit without restarting — data sat in the buffer indefinitely.

**Without self-instrumentation:** we'd have noticed only when queries started returning stale data, potentially days later.

**With self-instrumentation:** the `run_flusher` span exited unexpectedly. The error showed up in investigate output:

```
service=sigiro, operation=run_flusher
error="flusher task dropped without restart"
```

We found and fixed the race in `inserter.rs`: the flusher task was spawned but not owned by the struct, so it couldn't be restarted on drop.

### Bug 2: The Cold-Start Connection Leak

On cold start under load, when multiple connections were being created simultaneously, `pool.rs` had a race in its DashMap usage. Two tasks both saw `Vacant`, both called `open_connection`, and the second insertion won — the first leaked a connection.

**Without self-instrumentation:** one leaked connection per cold start under concurrent load, eventually exhausting file descriptors at scale.

**With self-instrumentation:** we saw duplicate `open_connection` spans for the same project_id, and `get_or_create` spans with abnormally long duration during connection setup. We fixed it by using atomic `or_insert()` semantics instead of `entry()` + load + insert.

### Bug 3: The Silent Channel Drop

During early testing, `Error: channel closed` appeared in logs. The OTLP receiver channel was being dropped while a sender was mid-write — the sender didn't know until the next write failed.

**Without self-instrumentation:** we knew something was wrong but not which span caused it or whether data was lost.

**With self-instrumentation:** every `ingest.send` is a span. The failed send showed as an error span, correlated to the specific bulk loader run and time window. We tracked down the early drop and added explicit sender tracking.

---

## What This Means for You

When Sigiro is part of your infrastructure, you get:
- **Self-observability without configuration** — OTel self-instrumentation is on by default
- **Root cause in one query** — `service=sigiro` gives you Sigiro's own spans, no separate logs
- **Confidence that data is flowing** — gaps in the flusher span tree mean data isn't being written
- **Immediate detection of silent failures** — no more empty dashboards with no explanation

You deploy Sigiro the same way you'd deploy any observability pipeline. We handle the rest.

---

*Sigiro is open source. The self-instrumentation is in `src/otel.rs`.*
