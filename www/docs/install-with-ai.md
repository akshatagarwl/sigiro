---
title: Instrument your code with an AI agent
description: Give one prompt to an agent. The agent adds OpenTelemetry auto-instrumentation to your service and points it at sigiro. This works with Claude Code, OpenCode, and Codex.
sidebar:
  order: 3
---

If your code has no OpenTelemetry yet, do not add it by hand. Paste the prompt
below into an agent. The agent detects your framework, installs the official
auto-instrumentation, and points the exporter at sigiro.

This works with Claude Code, OpenCode, Codex, or any agent that edits files and
runs your package manager.

## Before you start

You need a sigiro server as the destination. Start one with `sigiro serve`, or with
the Docker command in the [quickstart](/quickstart). A self-hosted server needs
no key. Only the hosted service needs a key, and that key has the form
`sk_<tenant>_<random>`.

Auto-instrumentation exists for Python, Node.js, Java, .NET and Ruby. Go has no
runtime agent. For Go you write a few lines of SDK setup instead.

## 1. Give the agent this prompt

Replace the endpoint. Delete the key line unless you use the hosted service.

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (replace with your OTLP ingest address)
HOSTED API KEY: sk_<tenant>_<random>  (omit entirely for a self-hosted server)

Goal: add OpenTelemetry auto-instrumentation so this service emits traces,
metrics, and logs to sigiro. Make the minimum change that works — prefer
auto-instrumentation over hand-written spans.

Rules:
- Detect the language and framework automatically
- Use official OTel auto-instrumentation where it exists (Python opentelemetry-instrument,
  Java javaagent, Node.js auto-instrumentations-node, etc.)
- If auto-instrumentation is not available for this language/framework, add
  manual OTel SDK spans for HTTP handlers and database calls
- Set OTEL_EXPORTER_OTLP_ENDPOINT to the sigiro OTLP HTTP endpoint (:4318)
- For hosted sigiro only, set OTEL_EXPORTER_OTLP_HEADERS with the tenant bearer token
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- For hosted sigiro, do NOT hardcode the tenant key — read it from SIGIRO_API_KEY
- If a file already initializes the OTel SDK, update it rather than re-initializing
- Add SIGIRO_API_KEY to .env.example only for a hosted deployment
```

If the agent detects the wrong language, tell it directly:

```
Language: <python|nodejs|java|dotnet|ruby|go>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. Coverage for each language

| Language | Approach | Covers |
|----------|----------|--------|
| Python | `opentelemetry-instrument` CLI + distro | HTTP (FastAPI, Flask, Starlette), DB (psycopg2, asyncpg), Redis |
| Node.js | `@opentelemetry/auto-instrumentations-node` | HTTP (Express, Fastify), DB (pg, mysql2, mongoose) |
| Java | `opentelemetry-javaagent.jar` | HTTP (Servlets, Spring), DB (JDBC), JVM metrics |
| .NET | `OpenTelemetry.AutoInstrumentation` startup hook | HTTP (ASP.NET Core), DB (EF Core) |
| Ruby | `opentelemetry-instrumentation-all` gem | HTTP (Rails), DB (ActiveRecord), Sidekiq |
| Go | Manual SDK setup | HTTP spans via middleware, DB spans via OTel hooks |

For Go, you create the tracer by hand and you wrap every handler:

```go
import (
    "context"

    "go.opentelemetry.io/contrib/instrumentation/net/http/otelhttp"
    "go.opentelemetry.io/otel"
    "go.opentelemetry.io/otel/exporters/otlp/otlptrace/otlptracehttp"
    sdktrace "go.opentelemetry.io/otel/sdk/trace"
)

func initTracer(ctx context.Context) (func(context.Context) error, error) {
    exporter, err := otlptracehttp.New(ctx,
        otlptracehttp.WithEndpoint("localhost:4318"),
        otlptracehttp.WithInsecure(),
    )
    if err != nil {
        return nil, err
    }
    tp := sdktrace.NewTracerProvider(sdktrace.WithBatcher(exporter))
    otel.SetTracerProvider(tp)
    return tp.Shutdown, nil
}

// Wrap each handler so its spans carry the route name:
http.Handle("/checkout", otelhttp.NewHandler(
    http.HandlerFunc(checkoutHandler), "checkout",
))
```

`WithInsecure()` makes the exporter use plain HTTP with a local server. Delete
`WithInsecure()` for the hosted service, which uses TLS. Then add the bearer token
with `otlptracehttp.WithHeaders(map[string]string{"Authorization": "Bearer " + key})`.

## 3. Confirm that it works

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=my-service \
  <your-app-start-command>
```

Then ask sigiro which services it received:

```bash
curl http://localhost:9999/v1/services
```

Your service appears within a few seconds of real traffic. After it appears,
`sigiro investigate my-service` has data to read.

If the service does not appear, make these checks. Confirm the port: `:4318` for
HTTP and `:4317` for gRPC. Run `sigiro status` to confirm that the server is
healthy. Read the logs of your own service for OTel startup errors, because a wrong
exporter configuration reports the fault at boot.

On the hosted service, confirm that the header is exactly
`Authorization: Bearer sk_<tenant>_<random>`. A self-hosted server ignores all
authentication headers. Therefore a wrong key fails there without a message and
without a 401 response.

## Tell the agent what not to do

An agent does more than you want here. State these limits before the agent starts:

- Do not install an OTel Collector. sigiro is the collector.
- Do not add manual spans for each endpoint. Auto-instrumentation covers them.
- Do not change database connection strings or business logic.
- Do not set up dashboards or alert configuration. sigiro has neither.

The full task is this: detect the framework, install auto-instrumentation, and point
it at sigiro.

## Point Claude Code itself at sigiro

Claude Code emits its own OpenTelemetry. To monitor your sessions, set these
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

For the hosted service, add
`OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${SIGIRO_API_KEY}"`. Keep the key
out of every committed file.

These variables instrument Claude Code, not the commands that Claude Code runs. An
application that Claude Code starts through its shell tool does not always inherit
these variables. Therefore an instrumented service still needs its own exporter
settings.

## Point Codex itself at sigiro

Codex reads its telemetry settings from its own configuration file, not from the
environment. Edit `~/.codex/config.toml`:

```toml
[otel]
environment = "dev"
log_user_prompt = false
exporter = "otlp-grpc"

[otel.exporter."otlp-grpc"]
endpoint = "http://localhost:4317"
```

For the hosted service, add
`headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }`. Set `SIGIRO_API_KEY` in
the environment that starts Codex. The same limit applies: these settings cover
Codex, not the services that Codex edits.

## Next

- [Quickstart](/quickstart) — ask what changed and why
- [Hosted beta](/hosted-onboarding) — use this if you do not want to run the
  server yourself
