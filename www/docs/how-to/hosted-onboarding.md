---
title: Send telemetry to hosted sigiro
description: Create a hosted sigiro account from the terminal, send OpenTelemetry, and query it back with SQL.
sidebar:
  order: 3
---

This guide shows you how to send OpenTelemetry to a sigiro host that we operate,
and how to query the data back. You run no infrastructure.

The whole path, start to finish:

1. Sign up at [sigiro.com/sign-in](https://sigiro.com/sign-in), or with
   `sigiro signup` in a terminal. Either way you get one personal workspace.
2. Install the CLI and connect it to that workspace (`sigiro auth login`).
3. Export the `OTEL_*` variables below and start your app. Your app sends to
   `https://sigiro.com`; there is no separate ingest host.
4. See the data at [sigiro.com/dashboard](https://sigiro.com/dashboard/), or
   query it with SQL.

sigiro reads traces, logs, metrics and profiles. Install the CLI first — one
command on macOS, Linux and WSL:

```bash
curl -fsSL https://sigiro.com/install | sh
```

Then connect the CLI to your account. Which command you run depends on where you
started, and [Connect the CLI](#1-connect-the-cli) below has both.

## 1. Connect the CLI

**You already finished signup on the web.** Sign the installed CLI in to the
account you have:

```bash
sigiro auth login
sigiro auth status
```

`sigiro auth login` starts OAuth device authorization and opens a browser for the
confirmation page. Pass `--no-browser` when that machine cannot open one: the CLI
prints the URL and the code to confirm either way.

**You are starting in the terminal, with no account yet.** Create the account
without leaving the shell:

```bash
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

`sigiro signup` emails a six-digit OTP and prompts for it securely in the
terminal. It creates and activates exactly one personal workspace, completes
OAuth device authorization internally without a browser, and stores
refresh/access credentials in your operating system's credential store.

**Every login is bound to one workspace, and the browser names it.** The
approval page shows which workspace the CLI will use. If your account belongs to
more than one, you choose it there, and nothing is approved until you do. The
choice is part of the login: refreshing the session keeps the same workspace even
if you join another one later, and it ends if you are removed from that
workspace. Run `sigiro auth login` again to switch.

Web signup and `sigiro signup` call the same provisioning step, which creates a
personal workspace only when the account has none. So `sigiro signup` is
idempotent for an account that already exists: it signs that account in and does
not create a second account or a second workspace. For an account that already
belongs to several workspaces it stops and asks you to run `sigiro auth login`, so
it never chooses one for you.

`sigiro auth status` prints the account and workspace the CLI is bound to.
`sigiro auth logout` revokes and removes the stored credentials. Both flows keep
credentials out of shell history, and neither needs you to call an OAuth API or
copy a token by hand.

Human signup and OAuth login are covered here. If software needs its own delegated identity and keys, use [Agent Auth onboarding](/docs/how-to/agent-auth).

Hosted authentication is provided by Better Auth. A workspace is the unit of
isolation: its data, its members and its queries are separate from every other
workspace. Some API fields and error messages call it a tenant or an
organization; all three words name the same thing.
Both commands target the hosted service at `https://sigiro.com` by default.

The same token authenticates OTLP ingest and API queries.

## 2. Point your OpenTelemetry exporter at sigiro

Use standard OTLP over HTTP. For authentication, use the standard
`Authorization: Bearer` header.

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_EXPORTER_OTLP_ENDPOINT="https://sigiro.com"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_SERVICE_NAME="checkout"   # the name your service shows up under
```

Set the header first. With `OTEL_EXPORTER_OTLP_ENDPOINT` already exported, the
`sigiro` CLI tries to send its own telemetry there and prints a
`SIGIRO_OTEL_AUTH_KEY is required` warning. The warning is harmless, and the
token is still printed.

**The token expires after 15 minutes.** `sigiro auth token` prints a short-lived
access token and refreshes the stored session before doing so. An app started
with it sends data for 15 minutes; after that its exporter gets HTTP `401`, which
appears only in your app's own log. Restart the app with a fresh token to keep
going. That is enough to try sigiro out.

**A long-running service has no static ingest key yet.** The two options today
are [Agent Auth](/docs/how-to/agent-auth), where your service signs a fresh token
for each export, or [running sigiro yourself](/docs/tutorials/quickstart), where
a local server needs no key at all.

**No OpenTelemetry in your app yet?** Zero-code instrumentation sends traces,
logs and metrics using the variables above, with no code change. Node.js:

```bash
npm install @opentelemetry/api @opentelemetry/auto-instrumentations-node
node --require @opentelemetry/auto-instrumentations-node/register app.js
```

Python:

```bash
pip install opentelemetry-distro opentelemetry-exporter-otlp-proto-http
opentelemetry-bootstrap -a install
export OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED=true  # also send logs
opentelemetry-instrument python app.py
```

For other languages, or to have an agent wire it up, follow [Instrument your code
with an agent](/docs/how-to/install-with-ai).

**The endpoint carries no port.** A hosted sigiro terminates OTLP on the standard
HTTPS port, so `https://sigiro.com` is the whole endpoint. Do not copy the
`:4318` from the self-hosted [quickstart](/docs/tutorials/quickstart#2-send-it-telemetry):
on `sigiro.com` that port refuses the connection, and an SDK that cannot
connect reports the failure in your own service log rather than here.

**OTLP/gRPC is not available on `sigiro.com`.** Port `4317` is closed and the
gRPC service path is not routed. Use HTTP.

**Note — a custom or self-hosted host.** Everything above and below describes
`https://sigiro.com`, the host we operate and the CLI's default. If you or an
operator runs sigiro elsewhere, substitute that host in these examples and set
`SIGIRO_ENDPOINT` so the CLI targets it; ports, gRPC availability and retention
are then whatever that deployment configures, so ask its operator rather than
assuming this page. `sigiro signup` and `sigiro auth login` still authenticate
against the hosted service at `https://sigiro.com`.

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

Send some requests through your app. Wait a few seconds, because the flush
interval is about 1 s. Then open [sigiro.com/dashboard](https://sigiro.com/dashboard/)
and sign in with the same email: your service is listed under its
`OTEL_SERVICE_NAME`, with its traces and logs.

To check from the terminal instead:

```bash
curl -s "https://sigiro.com/v1/services" \
  -H "Authorization: Bearer $(sigiro auth token)"
```

Your service appears in the `services` array, with a count for each signal. If
it is missing, look in your app's own log for the exporter error: `401` means the
token expired or is missing, and a connection error usually means a `:4318` was
left on the endpoint.

**No app instrumented yet?** Post one test span by hand to prove the path;
OTLP/HTTP accepts JSON as well as protobuf:

```bash
now=$(date +%s)000000000
curl -s -w '%{http_code}\n' https://sigiro.com/v1/traces \
  -H "Authorization: Bearer $(sigiro auth token)" \
  -H "content-type: application/json" \
  --data '{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"hello-sigiro"}}]},
    "scopeSpans":[{"spans":[{"traceId":"5b8efff798038103d269b633813fc60c","spanId":"eee19b7ec3c1b174",
    "name":"GET /hello","kind":2,"startTimeUnixNano":"'"$now"'","endTimeUnixNano":"'"$now"'"}]}]}]}'
```

`200` means that sigiro accepted the span; `hello-sigiro` then appears in
`/v1/services` a few seconds later.

`sigiro query` is not the command for this: it federates self-hosted edges and
fails against `sigiro.com`. To query from the terminal or from an agent, use
`curl` as above or [`sigiro mcp`](/docs/reference/cli#mcp), which reads the same
workspace with your stored login:

```bash
claude mcp add sigiro -- sigiro mcp
```

## 4. Query your data (SQL API)

- **Endpoint:** `POST /v1/query` on the main HTTPS port.
- **Auth:** `Authorization: Bearer $(sigiro auth token)`.
- **Body:** the raw SQL string, not JSON.
- **Response:** a JSON array of row objects. The header
  `x-sigiro-truncated: true|false` reports whether sigiro capped the results.

You see **your own** workspace's data only, because each workspace has an isolated catalog.

**Tables:** `sigiro_spans`, `sigiro_logs`, `sigiro_metrics_gauge`, `sigiro_metrics_sum`, `sigiro_metrics_histogram`, `sigiro_metrics_exp_histogram`, and `sigiro_profiles`. Attribute columns contain JSON text; use `json_extract(col, '$.key')` or `col ->> 'key'` to inspect them.

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

| Limit                  | Value                          | What sigiro does when you exceed it                                  |
| ---------------------- | ------------------------------ | -------------------------------------------------------------------- |
| Query timeout          | 10 s                           | sigiro rejects the query (400)                                       |
| Result rows            | 100,000                        | sigiro truncates the response and sets `x-sigiro-truncated: true`    |
| Query memory           | ~400 MB                        | the query fails (400)                                                |
| Ingest body            | 8 MB (decompressed)            | 413                                                                  |
| Concurrent queries     | 4 per tenant                   | extra queries wait up to 5 s for a slot, then 429 with `Retry-After` |
| SQL/API request rate   | 5 req/s per tenant, burst 20   | 429                                                                  |
| OTLP/HTTP request rate | 50 req/s per source, burst 100 | 429                                                                  |

sigiro has no secondary indexes. Therefore **bound every query by `timestamp`**. A
bound gives you speed. A bound also prevents timeouts and truncation.

## 6. Data retention

During the beta, treat sigiro as a live query service, not as a system of record. `sigiro.com`
does not publish a retention window yet; [talk to us](https://cal.com/akshatagarwl/30-minutes) for the
current one. Export any data that you must keep for a long time.

## Next

- [Dashboard](https://sigiro.com/dashboard/) — services, traces and logs in the
  browser, for the workspace you signed in to
- [Instrument your code with an agent](/docs/how-to/install-with-ai) — if your
  service still has no OpenTelemetry
- [API reference](/docs/reference) — the HTTP operations currently described by
  the server's `/openapi.json`
