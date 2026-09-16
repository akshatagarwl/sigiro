---
title: Send telemetry to hosted sigiro
description: Create a hosted sigiro account from the terminal, send OpenTelemetry, and query it back with SQL.
sidebar:
  order: 2
---

This guide shows you how to send OpenTelemetry to a sigiro host that we operate,
and how to query the data back. You run no infrastructure.

sigiro reads traces, logs, metrics and profiles. For a new hosted account, run:

```bash
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

The command emails a six-digit OTP and prompts for it securely in the terminal.
It creates and activates exactly one personal workspace, completes OAuth device
authorization internally without a browser, and stores refresh/access credentials
in your operating system's credential store. It uses the hosted endpoint by default.
Use `sigiro auth logout` to revoke and remove the stored credentials.

## 1. Sign up and sign in

Hosted authentication is provided by Better Auth. Organizations are the tenants.
Use `sigiro signup` for first-time onboarding rather than provisioning a token
manually. For an existing account, use browser-based `sigiro auth login`; use
`--no-browser` when that machine cannot open a browser. Both flows keep credentials
out of shell history.

The same token authenticates OTLP ingest and API queries.

## 2. Point your OpenTelemetry exporter at sigiro

Use standard OTLP over HTTP. For authentication, use the standard
`Authorization: Bearer` header.

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://<your-sigiro-host>"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

`sigiro auth token` prints a short-lived access token and refreshes the stored
session before doing so. Use this form for an interactive test, not as a
long-running collector secret.

**The endpoint carries no port.** A hosted sigiro terminates OTLP on the standard
HTTPS port, so `https://sigiro.com` is the whole endpoint. Do not copy the
`:4318` from the self-hosted [quickstart](/docs/tutorials/quickstart#2-send-it-telemetry):
on `sigiro.com` that port refuses the connection, and an SDK that cannot
connect reports the failure in your own service log rather than here. Ask your
operator if your host differs.

**OTLP/gRPC is not available on `sigiro.com`.** Port `4317` is closed and the
gRPC service path is not routed. Use HTTP, and ask your operator before assuming
gRPC on any other host.

Your SDK posts each signal to its standard OTLP path (`/v1/traces`, `/v1/logs`,
`/v1/metrics`, `/v1development/profiles`), which it appends to the endpoint above.
If your exporter takes a per-signal endpoint instead, give it the full path.
HTTP supports gzip and the endpoint uses TLS. One request must be **8 MB or
smaller** after decompression. Default SDK batch sizes stay well below this
limit.

A Pyroscope SDK does not use the OTLP profiles path. Follow [Send Pyroscope
profiles to sigiro](/docs/how-to/send-profiles) for its target, request format
and service-name rules.

## 3. Confirm that the data arrives

Send some telemetry. Wait a few seconds, because the flush interval is about 1 s.
Then run this query:

```bash
curl -s "https://<your-sigiro-host>/v1/query" \
  -H "Authorization: Bearer <oauth-access-token>" \
  --data 'SELECT count(*) AS n FROM sigiro_spans'
```

A non-zero `n` means that traces arrive. Query only _after_ your first batch. A new
tenant that never sent data has no storage.

## 4. Query your data (SQL API)

- **Endpoint:** `POST /v1/query` on the main HTTPS port.
- **Auth:** `Authorization: Bearer <oauth-access-token>`.
- **Body:** the raw SQL string, not JSON.
- **Response:** a JSON array of row objects. The header
  `x-sigiro-truncated: true|false` reports whether sigiro capped the results.

You see **your own** data only, because each tenant has an isolated catalog.

**Tables:** `sigiro_spans`, `sigiro_logs`, `sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`,
`sigiro_metrics_exp_histogram`, `sigiro_profiles`, and the `sigiro_anomalies`
table. The `sigiro_anomalies` table holds precomputed regime shifts. The anomaly pass
writes these shifts continuously. `GET /v1/anomalies` serves the same shifts as typed
JSON. Attribute columns (`*_attributes`, `events_json`, and similar) hold JSON text.
Read a field with `json_extract(col, '$.key')` or `col ->> 'key'`.

For what each family measures, and for the one name collision that produces a
confident wrong answer, read [About the tables](/docs/explanation/tables).

**Allowed SQL:** `SELECT` only, against the `sigiro_*` tables. sigiro rejects writes
and DDL. sigiro also blocks the file, URL and S3 readers (`read_csv`,
`read_parquet`, `glob` and similar). Joins, window functions, aggregates and
`json_extract` all work.

**CTEs are rejected.** A `WITH` clause fails validation, whatever the clause
contains. Rewrite it as a derived-table subquery: `SELECT ... FROM (SELECT ...) t`.
The hosted service blocks `information_schema` too, because one shared catalog holds
every tenant.

**Examples:**

```sql
-- Slowest operations, last hour
SELECT service_name, span_name,
       approx_quantile(duration, 0.95) / 1000.0 AS p95_ms, count(*) AS n
FROM sigiro_spans
WHERE timestamp > now() - INTERVAL '1 hour'
GROUP BY 1, 2 ORDER BY p95_ms DESC LIMIT 10;
```

```sql
-- Error-log rate per route, last 15 min
SELECT service_name, log_attributes ->> 'http.route' AS route, count(*) AS errors
FROM sigiro_logs
WHERE timestamp > now() - INTERVAL '15 minutes' AND severity_number >= 17
GROUP BY 1, 2 ORDER BY errors DESC;
```

## 5. Limits

| Limit                  | Value                          | What sigiro does when you exceed it                               |
| ---------------------- | ------------------------------ | ----------------------------------------------------------------- |
| Query timeout          | 10 s                           | sigiro rejects the query (400)                                    |
| Result rows            | 100,000                        | sigiro truncates the response and sets `x-sigiro-truncated: true` |
| Query memory           | ~400 MB                        | the query fails (400)                                             |
| Ingest body            | 8 MB (decompressed)            | 413 (HTTP) / RESOURCE_EXHAUSTED (gRPC)                            |
| SQL/API request rate   | 5 req/s per tenant, burst 20   | 429                                                               |
| OTLP/HTTP request rate | 50 req/s per source, burst 100 | 429                                                               |

sigiro has no secondary indexes. Therefore **bound every query by `timestamp`**. A
bound gives you speed. A bound also prevents timeouts and truncation.

## 6. Data retention

During the beta, treat sigiro as a live query service, not as a system of record. Ask
your operator for the current retention window. Export any data that you must keep
for a long time.

## Next

- [Instrument your code with an agent](/docs/how-to/install-with-ai) — if your
  service still has no OpenTelemetry
- [API reference](/docs/reference) — the HTTP operations currently described by
  the server's `/openapi.json`
- [About evidence instead of a dashboard](/docs/explanation/evidence) — why
  `/v1/diagnose` returns a ranked list rather than a chart
