---
title: Sigiro Pitch
created: 2026-03-21
updated: 2026-05-12
type: summary
tags: ['ai-agent', 'competitor', 'data-model', 'duckdb', 'ducklake', 'investigation']
---

# Sigiro Pitch

## Elevator Pitch

The consumer of observability data changed. It's no longer a human staring at [[designs/future/perses-compat.md|Grafana]] — it's an AI agent running SQL at 3am. Every incumbent (Datadog, Grafana, Splunk) optimized for 50ms dashboard refresh and pretty charts. None of them optimized for the agent workflow: query by structure, follow causal chains, return a diagnosis.

Sigiro is observability rebuilt for this new consumer. Per-signal DuckDB tables (spans, logs, 4 metric types, [[architecture/profiles.md|profiles]]) backed by [[architecture/data-model.md|Parquet]] on S3 via [[architecture/ducklake-integration.md|DuckLake]]. Agent-queryable. Free and self-hosted — your only cost is the S3 bucket you already have.

The wedge: small engineering teams and AI agent builders who want observability that costs less than their Netflix subscription and requires zero ongoing maintenance.

## The Story

Observability for agents, by agents.

### The problem

Observability is complex, confusing, and expensive. Small engineering teams — even solo founders — need to see what's happening in production. But the tools built for that job (Datadog, [[designs/future/perses-compat.md|Grafana]], Splunk) [[company/cost.md|cost]] a fortune, take weeks to set up, and require constant maintenance.

Worse: they optimize for the wrong consumer. Dashboards assume a human is staring at charts. But the on-call "person" in 2026 is increasingly an AI agent. The agent doesn't need pretty graphs. It needs structured data it can query.

### Who we're for

Small engineering teams (1-person teams included) who want to build their product and scale — not manage observability infrastructure. People who don't want to think about setting up boilerplate, don't want to manage things unrelated to their business, and above all don't want to pay a fortune for it.

### The promise

- **Free**: self-hosted, open source, no billing — ever
- **Zero effort**: two env vars, under 5 minutes
- **No dashboards**: a CLI and API, queryable and debuggable by an agent entirely
- **Zero impact**: doesn't hamper your services at all
- **All signals**: logs, traces, [[architecture/profiles.md|profiles]], and metrics out of the box on open technologies

### Core thesis

The consumer changed. Agents, not humans, are the primary reader of telemetry now.

- Current backends optimize for the wrong thing: 50ms dashboard refresh instead of coherent [[architecture/investigation.md|investigation]]
- Agents don't explore dimensions — they follow causal chains (graph traversal, not dimensional query)
- Throughput > latency: agent spends 8-10s on LLM reasoning, 500ms on data fetch — optimize the wrong leg and you save nothing
- Pre-aggregation is the most expensive operation in observability pipelines — agents don't need it, compute on read
- Compression can be 5-10x more aggressive because decompression hides behind LLM inference time
- Per-signal tables (not one mega-table) give zero-copy insert for each metric type, no schema projection overhead

## Dashboards All the Way Down

First there were logs.
Then came dashboards.

Then there were metrics.
Then came more dashboards.

Then there were logs and metrics together.
Then came more and more dashboards.

And then there were traces.
Three pillars, three tools, three tabs, zero correlation.

Then there were a lot more dashboards.
Dashboards that people don't understand.
Dashboards with 47 panels and a Slack thread explaining panel 23.
Dashboards that only one SRE understood who left the company 8 months ago.

So we added alerts.
Then we added alerts for the alerts.
Then we turned off the alerts because nobody could sleep.

So we bought a unified platform.
It unified all three signals into one place.
With three separate tabs.
And more dashboards.

Then came the AI copilot.
It could summarize your dashboards.
You still had to pick the right one first.

So we built two more dashboards.

---

What if instead of building dashboards to stare at, you just asked a question and got the answer?

That's it. That's the product.
