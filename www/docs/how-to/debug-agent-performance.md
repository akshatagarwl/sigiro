---
title: Debug and optimize an agent with sigiro
description: Record a coding-agent session, find where it spent time and tokens, and turn the evidence into a smaller and faster workflow.
sidebar:
  order: 5
---

This guide uses an agent's own OpenTelemetry to answer four questions:

1. Where did the session spend wall-clock time?
2. Which model, tool, or operation consumed the most tokens or calls?
3. Which failures, retries, and repeated reads did no useful work?
4. Did a proposed optimization improve a comparable second run?

Do not optimize from one slow span or a model's self-report. Use a bounded session
window, rank the complete operation summaries, and preserve the SQL behind every
claim.

## 1. Start an isolated local sigiro

Install the latest release and start the server:

```bash
curl -fsSL https://sigiro.com/install | sh
export PATH="$HOME/.local/bin:$PATH"

export SIGIRO_DATA_DIR="$HOME/.local/share/sigiro-agent-debug"
unset SIGIRO_CATALOG_URL
unset SIGIRO_S3_BUCKET SIGIRO_S3_ENDPOINT SIGIRO_S3_REGION
unset SIGIRO_S3_KEY_ID SIGIRO_S3_SECRET
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN

sigiro serve
```

With `SIGIRO_CATALOG_URL` unset, sigiro stores telemetry in local Parquet files
and keeps their metadata in a local DuckDB catalog. It does not use PostgreSQL or
S3. The explicit data directory keeps this experiment separate from other local
telemetry.

In another terminal, wait for readiness:

```bash
sigiro healthcheck
curl http://127.0.0.1:9999/v1/services
```

Then follow [Send your agent's own telemetry to
sigiro](/docs/how-to/agent-telemetry) for Claude Code, Codex, Hermes Agent, or Pi.

## 2. Record a useful test session

Choose a task with a clear completion check. Record these before starting:

- the exact prompt;
- repository revision and working-tree state;
- agent, model, and configuration;
- UTC start and end timestamps;
- whether caches and dependencies were warm;
- the expected artifact and verification command.

Run the task once without intervening. A benchmark that changes prompt, model,
repository state, and cache warmth at the same time cannot identify why it got
faster.

After the run, confirm the exporter produced data:

```bash
curl http://127.0.0.1:9999/v1/services
```

The service name is exporter-defined. Discover it here rather than assuming that
it equals the agent's product name.

## 3. Query a bounded session window

Use `/v1/query` with literal UTC timestamps covering the run. Aggregate the
operation counts and error spans first, then inspect the matching trace IDs.
The result is raw evidence, not a diagnosis or causal verdict. Check the
`x-sigiro-truncated` response header before treating returned rows as complete.

## 4. Rank wall-clock cost

Group spans by `span_name` (or a route/address attribute where available),
then inspect these patterns:

- one model call dominates p95 and total duration;
- many sequential tool spans could have run independently;
- setup, authentication, or dependency checks repeat in one session;
- a failed tool is retried without changing its inputs;
- the same file, URL, or status endpoint is fetched repeatedly;
- child agents repeat discovery already done by the parent;
- an approval or user wait is counted as execution time;
- a long parent span merely contains child time and must not be added to it.

Span durations are inclusive. Do not sum a parent session span and its child model
and tool spans as if they were independent costs. Use the parent to measure
end-to-end latency, then use children to explain it.

When you write a custom query, use literal UTC bounds so Parquet statistics can
skip files:

```bash
curl -sS -D /tmp/sigiro-headers -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT span_name,
                 count(*) AS calls,
                 sum(duration) / 1000000.0 AS total_seconds,
                 max(duration) / 1000000.0 AS slowest_seconds
          FROM sigiro_spans
          WHERE service_name = '<service-name>'
            AND timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY span_name
          ORDER BY total_seconds DESC"
```

Check the `x-sigiro-truncated` response header. If it is `true`, narrow the query
or aggregate further before making a completeness claim. The query endpoint
accepts one read-only `SELECT`; it does not accept CTEs.

## 5. Turn evidence into changes

Apply the smallest change that addresses the dominant measured cost:

- **Serialized independent tools:** request them together or delegate independent
  investigations concurrently.
- **Repeated discovery:** gather prerequisites once and pass the result to child
  agents instead of making each child rediscover it.
- **Oversized tool results:** filter at the source, request fewer fields, use
  narrower file ranges, and avoid feeding entire logs back to the model.
- **Excess model context:** remove duplicated instructions and stale history;
  retrieve only the files and records needed for the current decision.
- **Repeated failures:** fix the missing path, credential, dependency, or invalid
  argument before retrying. A retry with unchanged inputs is not recovery.
- **Too many model calls:** combine dependent micro-questions into one bounded
  request, while keeping independent work parallel.
- **Cost concentrated in one model:** use a cheaper model for mechanical
  discovery and reserve the capable model for decisions that require it.
- **Long approval waits:** separate human-wait time from compute time before
  claiming an execution-speed improvement.

Do not infer causality from token count alone. More tokens can accompany either
useful reasoning or repeated context. Read the associated spans, errors, finish
reasons, and tool sequence.

## 6. Verify the optimization

Run the same prompt again against the same repository state and comparable cache
conditions. Compare equal UTC windows and report:

- end-to-end session duration;
- model-call count and p95 latency;
- input, output, cache, and reasoning tokens when emitted;
- tool-call count and cumulative child duration;
- failed and retried operation counts;
- whether the requested artifact passed the same verification command.

A faster run that fails the task is not an optimization. Preserve the bounded
SQL and query results used for the comparison.

## Give an analysis agent direct access

Sigiro exposes two machine interfaces:

- `GET http://127.0.0.1:9999/openapi.json` for the HTTP API;
- `sigiro mcp` for stdio MCP tools: `list_services` and `run_query`.

For Hermes Agent, add this to `~/.hermes/config.yaml`, ensure the Python `mcp`
package is installed in Hermes's environment, and restart Hermes:

```yaml
mcp_servers:
  sigiro:
    command: sigiro
    args: [mcp]
```

Other MCP-capable agents can register the same `sigiro mcp` command using their
own MCP configuration. Use an absolute path to `sigiro` if the agent service has
a narrower `PATH` than your interactive shell.

## Prompts for an analysis agent

### Find the session bottleneck

```text
Use Sigiro to analyze service <service-name> from <UTC start> to <UTC end>.
Use list_services, then run_query with literal UTC bounds to rank where the
session spent wall-clock time by operation and model. Distinguish inclusive
parent duration from child duration; do not double-count nested spans. Cite
trace IDs or SQL results for every conclusion. Check truncation before
claiming completeness. End with the three smallest changes most likely to reduce
end-to-end latency without weakening the task's verification.
```

### Find wasted work

```text
Audit service <service-name> from <UTC start> to <UTC end> for work that did not
advance the task: repeated file or URL reads, unchanged retries, failed setup or
authentication calls, duplicate parent/child discovery, oversized tool results,
and serial calls that were independent. Query bounded raw telemetry, then inspect
complete rows for suspicious operations. Report counts, duration,
and trace IDs. Separate evidence from inference.
```

### Analyze model and token cost

```text
Analyze GenAI telemetry for service <service-name> from <UTC start> to <UTC end>.
Break down calls, p95 latency, errors, finish reasons, and available input,
output, cache, and reasoning tokens by provider and model. Identify where token
volume is concentrated and correlate it with the surrounding operation or tool
sequence. Do not recommend a cheaper model unless the evidence shows which calls
are mechanical enough to move safely.
```

### Compare before and after

```text
Compare service <service-name> in baseline window <UTC start/end> with optimized
window <UTC start/end>. Use equal query logic and literal TIMESTAMP bounds. Compare
end-to-end duration, model calls and p95, token categories, tool calls, failures,
and retries. Verify both runs completed the same acceptance check. Cite SQL and
rows for every delta, state missing telemetry plainly, and reject improvements
that only moved time into an unmeasured child process.
```

### Separate setup from delivery

```text
Analyze this agent session as two phases: prerequisite/setup work and delivery
work. Derive the boundary from span names and timestamps rather than assuming it.
Report duration, failures, retries, and model/tool calls in each phase. Identify
which setup checks can be cached or performed once, and which are required for
correctness. Include trace IDs and runnable drill-down SQL.
```
