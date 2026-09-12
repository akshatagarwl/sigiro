---
title: About the tables, and which machine they describe
description: The sigiro_* tables hold telemetry from whatever sent OTLP. The sys_* functions read the machine sigiro runs on. Confuse them and you get the wrong machine.
sidebar:
  order: 2
---

sigiro gives you two kinds of table, and they look alike in a query. One kind
holds the telemetry that your services sent. The other kind reads the machine
that sigiro itself runs on. Both answer a question about CPU, memory or disk, and
neither one tells you which machine it means.

This page draws the line. It is the most valuable thing in this section, because
the failure mode is not an error message. The failure mode is a number that looks
correct and describes a machine you did not ask about.

## Two families, two subjects

| Family                                                                                                                                                                                                 | What it is           | Whose machine                                                        | Time                                                                |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------- | -------------------------------------------------------------------- | ------------------------------------------------------------------- |
| `sigiro_spans`, `sigiro_logs`, `sigiro_log_templates`, `sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`, `sigiro_metrics_exp_histogram`, `sigiro_profiles`, `sigiro_anomalies` | Stored telemetry     | Whatever sent OTLP — your services, on your hosts, wherever they run | Historical, with a `timestamp` column                               |
| `sys_cpu_info()`, `sys_memory_info()`, `sys_disk_info()`, `sys_network_info()`, `sys_os_info()`                                                                                                        | Live table functions | The one machine this sigiro process runs on                          | One measurement at this instant, with no history and no time column |

The `sigiro_*` names are tables. The `sys_*` names are functions, and you call
them with empty parentheses. That is the only visible difference, and it is not
enough of a difference.

## The trap, in one example

Suppose an agent asks: _is the disk full on the checkout service?_

```sql
-- WRONG for that question
SELECT * FROM sys_disk_info();
```

That query succeeds. It returns real mount points with real free bytes. Every
number in it describes the machine that sigiro runs on. If checkout runs in
another container, on another host, or in another region, the answer has nothing
to do with checkout. On the hosted service the answer describes a machine that we
operate, and you do not run anything on it.

The agent has no way to notice. There is no error, no null column and no message
in the response. So the agent reports a disk figure with confidence, and the
figure is about the wrong machine.

The question about checkout is answered from the metric your service sent:

```sql
-- RIGHT for that question
SELECT service_name, metric_name, value, timestamp
FROM sigiro_metrics_gauge
WHERE service_name = 'checkout'
  AND metric_name LIKE 'system.filesystem%'
  AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
  AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
ORDER BY timestamp DESC;
```

This reads what checkout's own OpenTelemetry host metrics reported, from
checkout's own machine, over a window you chose. It is historical, so you can see
a trend rather than one instant. If the row set is empty, that is a real and
useful answer too: nothing sends host metrics for that service, and the fix is
instrumentation rather than a different query.

## Why `sys_*` exists at all

The `sys_*` functions are not a mistake. They answer one question well: _what
does this node look like right now?_ An operator with a self-hosted fleet asks
that about each node, and the answer needs no telemetry pipeline, because the
node reads its own kernel.

They also make a deliberate exception to sigiro's SQL rules. Every other function
that reads outside the database is blocked, because a caller could point it at a
path: `read_csv('/etc/passwd')` is exactly the hole that the blocklist closes.
The `sys_*` functions take no arguments, so there is no path for a caller to
choose, and that is the property that let them through. sigiro rejects any
`sys_*` call that carries arguments.

State the consequence plainly. On a self-hosted node the perimeter is the network,
so `sys_*` is readable by anyone who can reach the query surface. On the hosted
service, `sys_*` describes our machine and never yours. In both cases the rule is
the same: `sys_*` is about the process that answers you.

## The metric tables are four, and the reason is the write path

Metrics arrive in four tables rather than one:

- `sigiro_metrics_gauge` — a value at a point in time
- `sigiro_metrics_sum` — a counter, monotonic or not
- `sigiro_metrics_histogram` — explicit bucket boundaries
- `sigiro_metrics_exp_histogram` — exponential buckets

A single `sigiro_metrics` table was proposed and rejected. OTLP already carries
these four shapes with different fields, so one table would mean a schema
projection on every insert: null columns for the shape that does not apply, and a
mapper that decides which. Four tables let each OTLP shape insert without a copy
and without a projection. You pay for that with a `UNION ALL` when you want all
metrics at once, which is what `/v1/diagnose` does internally.

`sigiro_anomalies` is the odd one in the family. It holds no telemetry. It holds
the regime shifts that a scheduled pass has already detected, and
`GET /v1/anomalies` serves the same rows as typed JSON. [About evidence instead
of a dashboard](/docs/explanation/evidence) explains how those rows are produced.

## There is no schema endpoint yet, and that is the gap

Everything above is knowledge you now hold and an agent does not.

sigiro publishes no schema description. There is no `/v1/schema`, and the tables
carry no column comments, so an agent that reaches `POST /v1/query` makes a guess,
or reads `llms-full.txt` on this site. Both work often enough to be dangerous: a
guess of `sys_disk_info()` for a disk question is a _good_ guess, and it produces
a wrong answer.

We treat this as the most valuable gap in the API rather than a convenience gap,
for exactly that reason. The distinction that has to become machine-readable is
not "local or remote". It is this: `sys_*` is a live measurement of the machine
sigiro runs on, and `sigiro_metrics_*` is historical telemetry from whatever sent
OTLP.

Until that endpoint exists, this page is the machine-readable version. It is
served as Markdown at `/docs/explanation/tables.md`, and it is in `llms.txt`.

## A rule you can apply every time

Ask which machine the question is about.

- About a service you instrumented — use `sigiro_*`, and bound `timestamp`.
- About the sigiro process itself — use `sys_*`, and expect one instant.
- Not sure — use `sigiro_*`. An empty result is honest. A `sys_*` result is
  confident and possibly irrelevant.

## Read next

- [About evidence instead of a dashboard](/docs/explanation/evidence)
- [Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding#4-query-your-data-sql-api)
  — the full table list with the SQL rules that apply to it
