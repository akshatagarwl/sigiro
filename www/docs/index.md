---
title: sigiro docs
description: sigiro stores OpenTelemetry traces, logs, metrics and profiles and answers SQL over them. Built for agents first, with a dashboard for people on hosted workspaces.
sidebar:
  order: 0
---

sigiro stores OpenTelemetry traces, logs, metrics and profiles, and answers SQL
over them. It is built for agents first: every answer is a query that an agent or
a person can run again. Hosted workspaces also have a read-only
[dashboard](https://sigiro.com/dashboard/) for people.

New here and want your own app's data in sigiro.com? Start with [Send telemetry
to hosted sigiro](/docs/how-to/hosted-onboarding). Want to run it on your own
machine instead? Start with the [Quickstart](/docs/tutorials/quickstart).

These docs have four parts. Each part answers a different kind of question, so
go to the part that matches the question you have now.

## Tutorials

A lesson. Read this part first, because it assumes no sigiro knowledge. You
follow each step, and you see a result at each step.

- **[Quickstart](/docs/tutorials/quickstart)** — run the server on your own
  machine, send telemetry to it, and query recent failures with SQL. This takes
  five minutes.

## How-to guides

Directions for one task. Read this part when you know what you want, and you
need the steps to get it.

- **[Instrument your code with an agent](/docs/how-to/install-with-ai)** — give
  one prompt to an agent. The agent connects OpenTelemetry to your service and
  points it at sigiro.
- **[Onboard an agent with Agent Auth](/docs/how-to/agent-auth)** — register a
  delegated machine identity, approve only the capabilities it needs, then rotate
  or revoke its keys.
- **[Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding)** — sign up
  on the web or with `sigiro signup`, send OTLP from your app to a host that we
  operate, and see it in the dashboard.
- **[Send your agent's own telemetry to sigiro](/docs/how-to/agent-telemetry)** —
  point Claude Code, Codex, Hermes Agent, or Pi at sigiro.
- **[Debug and optimize an agent](/docs/how-to/debug-agent-performance)** — find
  where a session spent time and tokens, remove wasted work, and compare the next
  run with evidence.
- **[Connect sigiro to Executor](/docs/how-to/executor)** — import the OpenAPI API
  or the documentation MCP server into an agent gateway.
- **[Send Pyroscope profiles to sigiro](/docs/how-to/send-profiles)** — configure
  a verified profiling SDK, then read the profile rows back with SQL.

## Reference

Facts about the machine. Read this part while you work, to confirm a name, a
type or a limit.

- **[API reference](/docs/reference)** — the HTTP operations currently described
  by the server's `/openapi.json`. Ingest protocols that are not in that
  document, including the Pyroscope-compatible routes, are documented in the
  how-to guides.
- **[CLI reference](/docs/reference/cli)** — seven subcommands, every flag, and
  every environment variable they read.

An agent that reads `/openapi.json` needs no other information from us.

## Explanation

Why sigiro works as it does. Read this part when you want to know the reasons,
the costs and the alternatives. You do not need a terminal.

- **[About the tables, and which machine they describe](/docs/explanation/tables)**
  — the stored signals and how to avoid answering with data from the wrong machine.
- **[Retired anomaly and diagnosis design](/docs/explanation/evidence)** —
  historical: the ranked-results design, which has been removed. Use SQL instead.
- **[Retired deploy diagnosis design](/docs/explanation/deploys)** — historical:
  automatic deploy detection, which has been removed. `service.version` is still
  queryable.
