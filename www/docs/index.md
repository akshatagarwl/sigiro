---
title: sigiro docs
description: sigiro is the detect and diagnose layer for software that repairs itself. OpenTelemetry goes in. Ranked results and runnable SQL come out, for agents and not for dashboards.
sidebar:
  order: 0
---

sigiro is an observability backend for agents, not for dashboards. It reads
OpenTelemetry. It answers the two questions that an automatic loop must answer
before the loop repairs a fault: **what changed**, and **why**.

These docs have four parts. Each part answers a different kind of question, so
go to the part that matches the question you have now.

## Tutorials

A lesson. Read this part first, because it assumes no sigiro knowledge. You
follow each step, and you see a result at each step.

- **[Quickstart](/docs/tutorials/quickstart)** — run the server, send telemetry
  to it, and ask it what changed and why. This takes five minutes.

## How-to guides

Directions for one task. Read this part when you know what you want, and you
need the steps to get it.

- **[Instrument your code with an agent](/docs/how-to/install-with-ai)** — give
  one prompt to an agent. The agent connects OpenTelemetry to your service and
  points it at sigiro.
- **[Send telemetry to hosted sigiro](/docs/how-to/hosted-onboarding)** — run no
  server yourself. Send OTLP to a host that we operate, with a tenant key.
- **[Send your agent's own telemetry to sigiro](/docs/how-to/agent-telemetry)** —
  point Claude Code or Codex at sigiro, then read the sessions back with SQL.

## Reference

Facts about the machine. Read this part while you work, to confirm a name, a
type or a limit.

- **[API reference](/docs/reference)** — five endpoints. Each endpoint has a
  request playground. sigiro builds these pages from the same `/openapi.json`
  document that the server publishes, so the pages always agree with the API.
- **[CLI reference](/docs/reference/cli)** — seven subcommands, every flag, and
  every environment variable they read.

An agent that reads `/openapi.json` needs no other information from us.

## Explanation

Why sigiro works as it does. Read this part when you want to know the reasons,
the costs and the alternatives. You do not need a terminal.

- **[About the tables, and which machine they describe](/docs/explanation/tables)**
  — why `sigiro_metrics_*` and `sys_*` answer questions about different
  machines, and how to avoid a confident wrong answer.
- **[About evidence instead of a dashboard](/docs/explanation/evidence)** — why
  the answer is a ranked list with a runnable query on every row.
