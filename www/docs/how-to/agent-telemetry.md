---
title: Send your agent's own telemetry to sigiro
description: Point Claude Code, Codex, Hermes Agent, or Pi at sigiro to inspect sessions, model latency, tokens, tools, errors, and retries.
sidebar:
  order: 3
---

This guide records what your coding agent does, rather than what the application
it edits does. The resulting spans, logs, and metrics let you find slow model
calls, repeated tools, failed prerequisites, excess context, and serialized work.

This is different from
[Instrument your code with an agent](/docs/how-to/install-with-ai), which points
_your application_ at sigiro. Use both when you need both views.

## Before you start

Start sigiro and verify its API:

```bash
sigiro fetch-extensions
sigiro serve
curl http://127.0.0.1:9999/health
```

The local receiver accepts OTLP/HTTP on `4318` and OTLP/gRPC on `4317`. It has no
request authentication, so do not expose it to an untrusted network. For the
hosted service, follow [Send telemetry to hosted
sigiro](/docs/how-to/hosted-onboarding) instead.

Telemetry can contain prompts, responses, file contents, commands, user
identifiers, and environment details. The examples below choose conservative
capture settings. Review every exporter before enabling richer payloads.

## Claude Code

Claude Code has built-in OpenTelemetry export. Configure the process that starts
`claude`:

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1

export OTEL_METRICS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_METRICS_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=http://127.0.0.1:4318/v1/metrics

export OTEL_LOGS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_LOGS_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_LOGS_ENDPOINT=http://127.0.0.1:4318/v1/logs

# Short intervals are useful while proving a new local setup.
export OTEL_METRIC_EXPORT_INTERVAL=10000
export OTEL_LOGS_EXPORT_INTERVAL=5000

claude
```

Run one prompt, wait ten seconds, then use the verification steps below. Claude
Code omits prompt text, assistant text, tool details, tool contents, and raw API
bodies by default. Do not enable `OTEL_LOG_USER_PROMPTS`,
`OTEL_LOG_ASSISTANT_RESPONSES`, `OTEL_LOG_TOOL_CONTENT`, or
`OTEL_LOG_RAW_API_BODIES` unless the additional exposure is intentional.

Claude Code documents distributed traces as beta, but its current public setup
guide does not give a complete stable trace configuration. The settings above
therefore export the documented metrics and events without relying on an old beta
flag. See [Claude Code monitoring](https://docs.anthropic.com/en/docs/claude-code/monitoring-usage)
for the version you run.

## Codex

Codex reads telemetry settings from `~/.codex/config.toml`:

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"

[otel.metrics_exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"

[otel.exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"
```

`exporter` is the log exporter; it does not replace `trace_exporter` or
`metrics_exporter`. Codex reports itself as `codex_exec`, so look for that
service name during verification.

## Hermes Agent

Hermes uses the community
[`briancaffey/hermes-otel`](https://github.com/briancaffey/hermes-otel) plugin:

```bash
hermes plugins install briancaffey/hermes-otel/hermes_otel
```

Install the plugin requirements into the same Python environment that runs
Hermes. For a source checkout whose virtual environment is
`~/git/hermes-agent/venv`:

```bash
~/git/hermes-agent/venv/bin/pip install \
  -r ~/.hermes/plugins/hermes_otel/requirements.txt
```

Use the equivalent `pip` path for your installation; installing these packages
into an unrelated system Python does not make them visible to Hermes.

Create `~/.hermes/hermes_otel.yaml`:

```yaml
backends:
  - type: otlp
    name: sigiro
    endpoint: http://127.0.0.1:4318/v1/traces
    metrics: true
    logs: true

capture_logs: true
capture_previews: false
capture_sender_id: false
capture_conversation_history: false

# Optional host-level evidence for local performance work.
host_metrics: true
host_metrics_gpu: auto
host_metrics_interval_ms: 1000
```

Restart Hermes after installing or changing the plugin. The generic OTLP backend
derives `/v1/metrics` and `/v1/logs` from the trace endpoint. It emits nested
session, model, API, tool, skill, approval, and subagent spans plus token,
duration, cost, retry, and resource metrics.

The plugin defaults `capture_previews` to `true`; the explicit `false` above
prevents prompt, response, tool argument, and tool result previews from being
exported. `capture_logs: true` can still export Python records from imported
libraries. Use the plugin's `log_attach_logger` setting when you need a narrower
logger boundary.

## Pi coding agent

Pi's official `@earendil-works/pi-telemetry` package defines telemetry types but
currently provides no OTLP exporter. The following setup therefore uses the
community-maintained
[`@devkade/pi-opentelemetry`](https://github.com/devkade/pi-opentelemetry)
extension; review it before adopting it:

```bash
pi install npm:@devkade/pi-opentelemetry

export PI_OTEL_ENABLE=1
export OTEL_SERVICE_NAME=pi-coding-agent

export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:4318/v1/traces

export OTEL_METRICS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=http://127.0.0.1:4318/v1/metrics
export OTEL_METRIC_EXPORT_INTERVAL=10000

export PI_OTEL_PRIVACY_PROFILE=strict
export PI_OTEL_PAYLOAD_MAX_BYTES=32768

pi
```

Inside Pi, `/otel-status` reports exporter state and `/otel-open-trace` opens the
current trace. The extension documents traces and metrics, not log export. Its
`strict` privacy profile exposes less than the default
`detailed-with-redaction` profile.

## Confirm that telemetry arrived

Run one real prompt in the configured agent, then discover the service name:

```bash
curl http://127.0.0.1:9999/v1/services
```

Do not guess the emitted vocabulary. Inspect span and metric names first. Replace
the two timestamps with literal UTC bounds around your test session:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT service_name, span_name, count(*) AS occurrences
          FROM sigiro_spans
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY service_name, span_name
          ORDER BY occurrences DESC"
```

Metrics can use several OpenTelemetry table types, so discover all of them:

```bash
for table in gauge sum histogram exp_histogram; do
  curl -sS -X POST http://127.0.0.1:9999/v1/query \
    -H 'content-type: text/plain' \
    --data "SELECT metric_name, count(*) AS points
            FROM sigiro_metrics_${table}
            WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
              AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
            GROUP BY metric_name ORDER BY points DESC"
done
```

Literal `TIMESTAMP` bounds let sigiro prune Parquet files. A predicate built from
`now()` reads every file and becomes progressively slower as local history grows.

Next, [debug and optimize an agent with sigiro](/docs/how-to/debug-agent-performance).
