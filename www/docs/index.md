---
title: sigiro docs
description: sigiro is the detect and diagnose layer for software that repairs itself. OpenTelemetry goes in. Ranked results and runnable SQL come out, for agents and not for dashboards.
sidebar:
  order: 1
---

sigiro is an observability backend for agents, not for dashboards. It reads
OpenTelemetry. It answers the two questions that an automatic loop must answer
before the loop repairs a fault:

1. **What changed?** — `sigiro anomalies` returns the shifts that deviate from
   the statistical baseline of the service. sigiro ranks these shifts. sigiro
   also groups them into incidents across all signals.
2. **Why?** — `sigiro investigate` returns one structured evidence block for
   each service. The block holds ranked findings, error breakdowns for each
   operation, LLM statistics for each model, sampled spans, log patterns, and a
   ready-to-run SQL query on every row.

Both commands are also endpoints: `GET /v1/anomalies` and
`POST /v1/investigate`. An agent uses these endpoints to ask the same two
questions. Refer to the [API reference](/api).

sigiro is one static binary. You do not run a collector. You learn no query
language other than SQL.

## Start here

- **[Quickstart](/quickstart)** — run the binary, send telemetry to it, and
  ask it a question. This takes five minutes.
- **[Instrument your code with an AI agent](/install-with-ai)** — give one
  prompt to an agent. The agent connects OpenTelemetry to sigiro for you.
- **[Hosted beta](/hosted-onboarding)** — host nothing yourself. Send OTLP
  to `api.sigiro.com` with a tenant key.

## API reference

sigiro serves a document at `/openapi.json`. The reference comes from that same
document. Therefore the reference always agrees with the API.

- **[API reference](/api)** — five endpoints. Each endpoint has a request
  playground.

An agent that reads `/openapi.json` needs no other information from us.
