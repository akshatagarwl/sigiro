---
title: Send your agent's own telemetry to sigiro
description: Claude Code and Codex both emit OpenTelemetry about their own sessions. Point them at sigiro to read token counts, tool calls and session latency back with SQL.
sidebar:
  order: 3
---

This guide shows you how to record what your agent does, rather than what your
service does. Claude Code and Codex both emit OpenTelemetry about their own
sessions. Point that telemetry at sigiro, and the sessions become rows you can
query.

This is a different task from
[Instrument your code with an agent](/docs/how-to/install-with-ai), which points
_your service_ at sigiro. Do both if you want both.

## Before you start

You need a sigiro server. Start one with `sigiro serve`, or with the Docker
command in the [quickstart](/docs/tutorials/quickstart#1-run-it).

Both agents export over OTLP gRPC on port `4317` in the settings below. A
self-hosted server needs no key.

On the hosted service, use OTLP over **HTTP** instead. Read [Send telemetry to
hosted sigiro](/docs/how-to/hosted-onboarding#2-point-your-opentelemetry-exporter-at-sigiro)
first, because a hosted operator does not always expose the gRPC port. Each
section below ends with the hosted settings.

## Claude Code

Claude Code reads its telemetry settings from the environment. Set these
variables in the environment that starts Claude Code. Use a shell profile, an
`.envrc` file, or a wrapper script:

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

For the hosted service, change the protocol and the endpoint as well as adding
the key. `api.sigiro.com` answers on 443 only, so a `:4317` or `:4318` endpoint
there fails to connect and the agent reports nothing:

```bash
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_ENDPOINT=https://api.sigiro.com
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${SIGIRO_API_KEY}"
```

Keep the key out of every committed file. Your operator tells you which
endpoint and port to use, so use `https://<your-sigiro-host>` when that host is
not `api.sigiro.com`.

## Codex

Codex reads its telemetry settings from its own configuration file, not from the
environment. Edit `~/.codex/config.toml`:

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-grpc]
endpoint = "http://localhost:4317"

[otel.metrics_exporter.otlp-grpc]
endpoint = "http://localhost:4317"

[otel.exporter.otlp-grpc]
endpoint = "http://localhost:4317"
```

Codex names one exporter per signal, and `exporter` is the **log** exporter
alone. Set `trace_exporter` or you get no spans, and set `metrics_exporter` or
you get no metrics. Both mistakes are silent.

For the hosted service, use the HTTP exporter and give each endpoint its **full
signal path**. Codex sends to the URL exactly as written and appends nothing, so
a base URL arrives at the wrong route and sigiro rejects it:

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/traces"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }

[otel.metrics_exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/metrics"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }

[otel.exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/logs"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }
```

`protocol` is required for `otlp-http`, and takes `binary` or `json`. Set
`SIGIRO_API_KEY` in the environment that starts Codex.

Codex reports itself as `codex_exec` rather than `codex`, so look for that name
in the check below.

## One limit applies to both

These settings cover the agent process only. They do not cover the commands that
the agent runs. An application that the agent starts through its shell tool does
not always inherit these variables, so an instrumented service still needs its
own exporter settings. Use
[Instrument your code with an agent](/docs/how-to/install-with-ai) for that.

## Confirm that it works

Start the agent, run one prompt, then ask sigiro which services it received:

```bash
curl http://localhost:9999/v1/services
```

The agent reports itself as a service. After it appears, query the sessions:

```bash
curl -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, span_name, count(*) AS n
          FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2 ORDER BY n DESC"
```

An agent also reports counters and gauges, and those land in the
`sigiro_metrics_*` tables rather than in `sigiro_spans`. [About the
tables](/docs/explanation/tables) says which family holds what, and which
machine each family describes.

## Next

- [Instrument your code with an agent](/docs/how-to/install-with-ai) — point your
  service at sigiro as well
- [CLI reference](/docs/reference/cli) — every subcommand and flag
