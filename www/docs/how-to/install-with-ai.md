---
title: Instrument your code with an agent
description: Give one prompt to an agent. It adds OpenTelemetry auto-instrumentation to your service and points it at sigiro — in Claude Code, OpenCode, or Codex.
sidebar:
  order: 1
---

This guide shows you how to add OpenTelemetry to a service that has none, with
an agent doing the edits. Paste the prompt below into an agent. The agent detects
your framework, installs the official auto-instrumentation, and points the
exporter at sigiro.

This works with Claude Code, OpenCode, Codex, or any agent that edits files and
runs your package manager.

## Install the sigiro skill

Install the public skill before you start the agent:

```bash
npx skills add sigiroai/skills
```

The [`debug-with-sigiro` skill](https://github.com/sigiroai/skills) teaches the
agent how to use sigiro after telemetry arrives. This guide stays focused on
instrumentation.

## Before you start

You need a sigiro server as the destination. Start one with `sigiro serve`, or with
the Docker command in the [quickstart](/docs/tutorials/quickstart#1-run-it). A
self-hosted server needs no key. Only the hosted service needs a key, and that key
has the form `<oauth-access-token>`.

Auto-instrumentation exists for Python, Node.js, Java, .NET and Ruby. If your
service is Go, do not use the prompt. Go to
[the Go section](#go-instrument-the-build-not-the-source) instead.

## 1. Give the agent this prompt

Replace the endpoint. Delete the key line unless you use the hosted service.

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (self-hosted; for hosted sigiro use
                 https://<your-sigiro-host> with no port)
HOSTED OAUTH ACCESS TOKEN: <oauth-access-token>  (omit entirely for a self-hosted server)

Goal: add OpenTelemetry auto-instrumentation so this service emits traces,
metrics, and logs to sigiro. Make the minimum change that works — prefer
auto-instrumentation over hand-written spans.

Rules:
- Detect the language and framework automatically
- Use official OTel auto-instrumentation where it exists (Python opentelemetry-instrument,
  Java javaagent, Node.js auto-instrumentations-node, etc.)
- If auto-instrumentation is not available for this language/framework, add
  manual OTel SDK spans for HTTP handlers and database calls
- Set OTEL_EXPORTER_OTLP_ENDPOINT to the SIGIRO ENDPOINT above exactly as given.
  A self-hosted server listens on :4318; a hosted one carries no port. Do not add
  or remove a port
- For hosted sigiro only, set OTEL_EXPORTER_OTLP_HEADERS with the tenant bearer token
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- For hosted sigiro, do NOT hardcode the OAuth access token — read it from SIGIRO_ACCESS_TOKEN
- If a file already initializes the OTel SDK, update it rather than re-initializing
- Add SIGIRO_ACCESS_TOKEN to .env.example only for a hosted deployment
```

If the agent detects the wrong language, tell it directly:

```
Language: <python|nodejs|java|dotnet|ruby>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. Tell the agent what not to do

An agent does more than you want here. State these limits before the agent starts:

- Do not install an OTel Collector. sigiro is the collector.
- Do not add manual spans for each endpoint. Auto-instrumentation covers them.
- Do not change database connection strings or business logic.
- Do not set up dashboards or alert configuration. sigiro has neither.

The full task is this: detect the framework, install auto-instrumentation, and point
it at sigiro.

## 3. Check the coverage you get

| Language | Approach                                         | Covers                                                          |
| -------- | ------------------------------------------------ | --------------------------------------------------------------- |
| Python   | `opentelemetry-instrument` CLI + distro          | HTTP (FastAPI, Flask, Starlette), DB (psycopg2, asyncpg), Redis |
| Node.js  | `@opentelemetry/auto-instrumentations-node`      | HTTP (Express, Fastify), DB (pg, mysql2, mongoose)              |
| Java     | `opentelemetry-javaagent.jar`                    | HTTP (Servlets, Spring), DB (JDBC), JVM metrics                 |
| .NET     | `OpenTelemetry.AutoInstrumentation` startup hook | HTTP (ASP.NET Core), DB (EF Core)                               |
| Ruby     | `opentelemetry-instrumentation-all` gem          | HTTP (Rails), DB (ActiveRecord), Sidekiq                        |
| Go       | `otelc` compile-time instrumentation             | HTTP server spans, Go runtime metrics, third-party libraries    |

If a library gets no spans, add the spans by hand for that library only.

Two gaps are worth expecting, because auto-instrumentation covers less than the
table suggests.

**Logs.** The bundled log bridges cover named loggers: pino, winston and bunyan
on Node, and the standard `logging` module on Python. A service that writes with
`console.log` or `print` produces traces and metrics and **no logs**. Move to a
real logger rather than bridging by hand.

**Databases the official instrumentation does not know.** Each DB entry above
names specific drivers. `instrumentation-pg` patches `node-postgres`, so a Node
service on `postgres.js` gets no query spans, and an ORM does not close the gap:
`drizzle-orm` ships a tracer at `drizzle-orm/tracing` whose
`await import('@opentelemetry/api')` is commented out in the published package,
so it cannot emit a span. Check that your own driver is on the list before you
trust the DB column, and prefer a maintained wrapper for that ORM over
hand-written spans.

## Go: instrument the build, not the source

Do not give the agent this job for Go. For Go you add a prefix to the build
command, and the binary ships instrumented. You change no source file. The tool is
`otelc`, and the OpenTelemetry project builds it. The tool also instruments the
third-party libraries that you do not control, which an agent cannot do, because
an agent edits only your own code.

Download the binary for your platform from the
[v1.0.1 release](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/tag/v1.0.1).
The release publishes `otelc` for macOS and Linux on arm64 and x86-64, and for
Windows on x86-64:

```bash
curl -fsSL -o otelc https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/download/v1.0.1/otelc-darwin-arm64
chmod +x otelc
./otelc version
```

`go install` does not work for this tool. The module publishes no command path.
On a platform without an asset, clone the repository and run `make build`.

Then add `otelc` in front of your normal build command:

```bash
./otelc go build -o checkout .
```

The first build downloads the OpenTelemetry packages, so it takes longer than a
normal build. Your `go.mod` file stays unchanged.

Start the binary with the same two variables that the
[quickstart](/docs/tutorials/quickstart#2-send-it-telemetry) uses:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=checkout \
  ./checkout
```

The binary creates the trace provider, the meter provider, and the logger
provider at startup. It needs no SDK code from you. The service reports HTTP
server spans and Go runtime metrics.

Two limits apply. The tool needs Go 1.25 or later, and the current version is
1.0.1, which the project tagged on 14 July 2026. Read the
[upstream documentation](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation)
for the libraries that it covers.

## 4. Confirm that it works

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
`sigiro diagnose my-service` has data to read.

If the service does not appear, make these checks. On a self-hosted server,
confirm the port: `:4318` for HTTP and `:4317` for gRPC. On the hosted service,
confirm the opposite, that the endpoint carries **no** port, and query
`https://<your-sigiro-host>/v1/services` with your key rather than localhost. Run
`sigiro status` to confirm that the server is healthy. Read the logs of your own
service for OTel startup errors, because a wrong exporter configuration reports
the fault at boot.

On the hosted service, confirm that the header is exactly
`Authorization: Bearer <oauth-access-token>`. A self-hosted server ignores all
authentication headers. Therefore a wrong key fails there without a message and
without a 401 response.

## Next

- [Quickstart](/docs/tutorials/quickstart#3-ask-what-changed) — ask what changed
  and why
- [Send your agent's own telemetry to sigiro](/docs/how-to/agent-telemetry) —
  point Claude Code or Codex at sigiro too
- [Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding) — use this if
  you do not want to run the server yourself
