---
title: sigiro docs
description: sigiro is the detect and diagnose layer for software that repairs itself. OpenTelemetry goes in; ranked results and runnable SQL come out, for agents.
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
- **[About evidence instead of a dashboard](/docs/explanation/evidence)** — why
  the answer is a ranked list with a runnable query on every row.
- **[About deploy detection, and what your spans must
  carry](/docs/explanation/deploys)** — sigiro reads one attribute to find a
  deploy, and it says nothing when that attribute is absent.
