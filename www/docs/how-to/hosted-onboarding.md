---
title: Send telemetry to hosted sigiro
description: Send OpenTelemetry to hosted sigiro with a tenant key, then query it back with SQL. You run no infrastructure; we set up each tenant by hand.
sidebar:
  order: 2
---

This guide shows you how to send OpenTelemetry to a sigiro host that we operate,
and how to query the data back. You run no infrastructure.

sigiro reads traces, logs, metrics and profiles. During the invited beta, we set
up each tenant by hand. Replace `<your-sigiro-host>` below with the host that
your operator gives you.

## 1. Get your API key

Your operator sends you a key with the form `sk_<yourtenant>_<random>`. Treat the key
like a password. sigiro shows the key one time only. You cannot recover the key, but
you can rotate it. The key authenticates **both** ingest and queries.

## 2. Point your OpenTelemetry exporter at sigiro

Use standard OTLP over HTTP. For authentication, use the standard
`Authorization: Bearer` header.

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://<your-sigiro-host>"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sk_<yourtenant>_<random>"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

**The endpoint carries no port.** A hosted sigiro terminates OTLP on the standard
HTTPS port, so `https://api.sigiro.com` is the whole endpoint. Do not copy the
`:4318` from the self-hosted [quickstart](/docs/tutorials/quickstart#2-send-it-telemetry):
on `api.sigiro.com` that port refuses the connection, and an SDK that cannot
connect reports the failure in your own service log rather than here. Ask your
operator if your host differs.

**OTLP/gRPC is not available on `api.sigiro.com`.** Port `4317` is closed and the
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
  -H "Authorization: Bearer sk_<yo...dom>" \
  --data "SELECT count(*) AS n FROM sigiro_spans
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'"
```

A non-zero `n` means that traces arrive. Query only _after_ your first batch. A new
tenant that never sent data has no storage.

## 4. Query your data (SQL API)

- **Endpoint:** `POST /v1/query` on the main HTTPS port.
- **Auth:** `Authorization: Bearer sk_<yourtenant>_<random>`.
- **Body:** the raw SQL string, not JSON.
- **Response:** a JSON array of row objects. The header
  `x-sigiro-truncated: true|false` reports whether sigiro capped the results.

You see **your own** data only, because each tenant has an isolated catalog.

**Tables:** `sigiro_spans`, `sigiro_logs`, `sigiro_log_templates`,
`sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`,
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

**Examples:** Start with a narrow aggregate, then fetch selected detail. Compute
literal UTC bounds in your client and use a half-open range on every raw
spans/logs/metrics/profiles query. Do not use `now()` or `current_timestamp`;
sigiro rejects those predicates with `query_requires_time_bounds` (422).

```sql
-- First: narrow aggregate
SELECT service_name, span_name, count(*) AS occurrences
FROM sigiro_spans
WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
  AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
GROUP BY service_name, span_name
ORDER BY occurrences DESC
LIMIT 20;
```

```sql
-- Then: selected bounded detail, including wide JSON only for the selected group
SELECT timestamp, trace_id, service_name, span_name, span_attributes
FROM sigiro_spans
WHERE service_name = 'checkout' AND span_name = 'HTTP GET'
  AND timestamp >= TIMESTAMP '2026-09-10 12:00:00'
  AND timestamp <  TIMESTAMP '2026-09-10 12:05:00'
ORDER BY timestamp DESC LIMIT 100;
```

`LIMIT` caps returned rows and some output/sort work; it does not cap scan work
or aggregate hash memory. `query_timeout` (408) means the configured timeout
expired after project initialization and covers reader admission plus DuckDB
execution; the total request span also includes initialization and serialization.
For valid but too-expensive SQL, narrow the range/projection after
`query_memory_limit_exceeded` (422). For `query_temp_storage_unavailable`
(507), free or repair temporary storage before retrying. For
`query_storage_unavailable` (503), retry after storage availability is restored.

## 5. Limits

| Limit                  | Value                          | What sigiro does when you exceed it                               |
| ---------------------- | ------------------------------ | ----------------------------------------------------------------- |
| Query timeout          | 10 s                           | `query_timeout` (408)                                             |
| Result rows            | 100,000                        | sigiro truncates the response and sets `x-sigiro-truncated: true` |
| Query memory           | configured shared cap          | `query_memory_limit_exceeded` (422)                               |
| Ingest body            | 8 MB (decompressed)            | 413 (HTTP) / RESOURCE_EXHAUSTED (gRPC)                            |
| SQL/API request rate   | 5 req/s per tenant, burst 20   | 429                                                               |
| OTLP/HTTP request rate | 50 req/s per source, burst 100 | 429                                                               |

sigiro has no secondary indexes. Therefore **bound every raw telemetry query by
`timestamp` with explicit half-open literal UTC bounds**. A bound gives you
speed, while the narrow-aggregate-then-detail workflow keeps scans and
intermediate memory predictable.

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
