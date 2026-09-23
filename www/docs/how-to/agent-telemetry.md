---
title: Send your agent's own telemetry to sigiro
description: Point Claude Code, Codex, Hermes Agent, or Pi at sigiro to inspect sessions, model latency, tokens, tools, errors, and retries.
sidebar:
  order: 4
---

This guide records what your coding agent does, rather than what the application
it edits does. The resulting spans, logs, and metrics let you find slow model
calls, repeated tools, failed prerequisites, excess context, and serialized work.

This is different from
[Instrument your code with an agent](/docs/how-to/install-with-ai), which points
_your application_ at sigiro. Use both when you need both views.

## Install the sigiro skill

Install the public skill before you start the agent:

```bash
npx skills add Arnav12114/skills
```

The [`debug-with-sigiro` skill](https://github.com/Arnav12114/skills) teaches the
agent how to inspect telemetry in sigiro. It does not change the telemetry
settings in this guide.

## Before you start

Start sigiro and verify its API:

```bash
sigiro serve
curl http://127.0.0.1:9999/health
```

The local receiver accepts OTLP/HTTP on `4318` and OTLP/gRPC on `4317`. It has no
request authentication, so do not expose it to an untrusted network. For the
the hosted service, run `sigiro signup --name "Your Name" --email you@example.com`
for a new account (or `sigiro auth login` for an existing account), and follow [Send telemetry to hosted
sigiro](/docs/how-to/hosted-onboarding) instead.

Telemetry can contain prompts, responses, file contents, commands, user
identifiers, and environment details. The examples below choose conservative
capture settings. Review every exporter before enabling richer payloads.

## Claude Code

**Beta tracing is required for span-based diagnosis.** Without both
`CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1` and `OTEL_TRACES_EXPORTER=otlp`,
Claude Code sends no spans: `gen_ai` and `operation_summaries` are empty even
when its logs and metrics arrive. Check `/v1/services` signal counts before
interpreting an empty diagnosis. See [Claude Code monitoring](https://code.claude.com/docs/en/monitoring-usage).

Claude Code has built-in OpenTelemetry export. Add these keys to the `env` object
in `~/.claude/settings.json`. Then every session sends telemetry, including
sessions that an IDE starts:

```json
{
  "env": {
    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
    "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
    "OTEL_TRACES_EXPORTER": "otlp",
    "OTEL_METRICS_EXPORTER": "otlp",
    "OTEL_LOGS_EXPORTER": "otlp",
    "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
    "OTEL_EXPORTER_OTLP_ENDPOINT": "http://127.0.0.1:4318",
    "OTEL_METRIC_EXPORT_INTERVAL": "10000"
  }
}
```

If the file already has an `env` object, merge the keys into it. The same
variables also work as `export` statements in the shell that starts `claude`.

Start a new Claude Code session, run one prompt, wait ten seconds, then use the
verification steps below.

### Correlate a spawned agent with an external task

For one task per Claude Code process, put an **application-owned** task ID in the
child's environment, not in a shared `~/.claude/settings.json` or the parent
worker's global environment. For example, from the launcher that starts `claude`:

```bash
OTEL_RESOURCE_ATTRIBUTES="app.task.id=task-123" claude -p 'Summarize this repository'
```

If you already set `OTEL_RESOURCE_ATTRIBUTES`, append `,app.task.id=task-123`
to the existing value rather than replacing other resource keys. An ordinary
child process inherits its launcher's environment; launchers that filter the
child environment must explicitly pass this variable through. Check the
emitted rows rather than assuming your launcher or Claude Code forwarded it.
[OpenTelemetry resources](https://opentelemetry.io/docs/specs/otel/resource/sdk/)
are process-wide string attributes. Use a new process per task; a long-lived
worker handling multiple tasks must instrument individual spans (or propagate
baggage into span attributes), not change a shared resource per request. Do not
put secrets or sensitive task names in IDs: the ID is exported with telemetry
and may be visible to other query users.

`app.task.id` identifies your **workflow task**, not an agent session.
`gen_ai.conversation.id` is Sigiro's intended canonical session identity;
Claude Code's native `session.id` is producer-specific, not a guaranteed
`gen_ai.conversation.id` projection. Do not invent a `sigiro_task_id`, infer a
session from a trace ID, or assume every usage point carries a session ID.

After a real run, check `/v1/services` for `claude-code` spans and metrics, then
replace the UTC timestamps below with a finite half-open window covering that
run. This single read-only query correlates span trace IDs and raw token/cost
metric points by **the same task resource attribute**; it deliberately does not
sum cumulative metric snapshots or cross-join unrelated spans and points:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT timestamp, 'span' AS signal, span_name AS name,
                 trace_id, CAST(NULL AS DOUBLE) AS value, CAST(NULL AS VARCHAR) AS unit
          FROM sigiro_spans
          WHERE service_name = 'claude-code'
            AND json_extract_string(resource_attributes, 'app.task.id') = 'task-123'
            AND timestamp >= TIMESTAMP '2026-09-23 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-23 13:00:00'
          UNION ALL
          SELECT timestamp, 'metric' AS signal, metric_name AS name,
                 CAST(NULL AS VARCHAR) AS trace_id, value, metric_unit AS unit
          FROM sigiro_metrics_sum
          WHERE service_name = 'claude-code'
            AND metric_name IN ('claude_code.token.usage', 'claude_code.cost.usage')
            AND json_extract_string(resource_attributes, 'app.task.id') = 'task-123'
            AND timestamp >= TIMESTAMP '2026-09-23 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-23 13:00:00'
          ORDER BY timestamp DESC LIMIT 100"
```

If either signal has no matching rows, check the actual metric shape and the
`resource_attributes` on its rows before attributing usage; an empty result is
not proof that the task used zero tokens or cost. For one known trace, inspect
`/v1/traces/{trace_id}`. This is a task-scoped correlation query, **not** a
session-detail API or a trace-ID join onto metrics. Respect access controls on
`/v1/query`; telemetry may include prompts, identities, file paths, and costs.

### What Claude Code sends by default

Every metric, log, and span has `user.email`, `user.account_uuid`,
`user.account_id`, `user.id`, `organization.id`, `session.id`, and
`terminal.type`. Think about these identity attributes before you send telemetry
to a shared or hosted server.

Without the settings in the next section, telemetry names each tool but not its
arguments:

- **Tool calls:** tool name, success, duration, input and result size in bytes,
  and the permission decision.
- **Bash commands:** only the first program, from a fixed list (`bash_argv0`, for
  example `git`, or `other`), and its category (`bash_command_class`).
- **Tool failures:** only a category, such as `ShellError` or
  `TelemetrySafeError`.
- **Prompts:** `<REDACTED>` and the prompt length.
- **Model requests:** model, tokens, time to first token, and `cost_usd` on
  `claude_code.api_request` events.

### Capture more detail

Each setting below exports more content, such as commands, file contents, and
conversation text. Enable only the settings whose exposure you accept, then start
a new session.

| Setting                                | What it adds                                                                                                                                                                              |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `OTEL_LOG_TOOL_DETAILS=1`              | `full_command`, `file_path`, `skill_name`, and `subagent_type` on tool spans. `tool_input` and `tool_parameters` on tool events. Full error text. MCP server names and connection errors. |
| `OTEL_LOG_TOOL_CONTENT=1`              | A `tool.output` span event with the input and output of each tool. Requires traces.                                                                                                       |
| `OTEL_LOG_USER_PROMPTS=1`              | Prompt text on `claude_code.user_prompt` events and `claude_code.interaction` spans.                                                                                                      |
| `OTEL_LOG_ASSISTANT_RESPONSES=1`       | Response text on `claude_code.assistant_response` events. If you do not set it, it follows `OTEL_LOG_USER_PROMPTS`. Set it to `0` to keep responses redacted.                             |
| `OTEL_LOG_RAW_API_BODIES=1`            | The full API request and response JSON as `claude_code.api_request_body` and `claude_code.api_response_body` events. Request bodies include the conversation history.                     |
| `OTEL_METRICS_INCLUDE_VERSION=true`    | `app.version` on metrics and events.                                                                                                                                                      |
| `OTEL_METRICS_INCLUDE_ENTRYPOINT=true` | `app.entrypoint` on metrics and events.                                                                                                                                                   |

A failed tool shows the difference. With the default settings, a failed Read
reports `TelemetrySafeError` and a failed Bash command reports `ShellError`. With
`OTEL_LOG_TOOL_DETAILS=1`, the same failures report
`File does not exist. Note: your current working directory is …` and
`Shell command failed`, with the command or path that failed.

Claude Code truncates content attributes at
`CLAUDE_CODE_OTEL_CONTENT_MAX_LENGTH`, which is 61,440 characters by default. A
request body for a long conversation arrives cut off at that length. Sigiro
stores values of that size. Larger limits are not tested.

### Detailed beta tracing

Detailed beta tracing also records what the model received and returned at each
step. Add these keys to the same `env` object:

```json
"ENABLE_BETA_TRACING_DETAILED": "1",
"BETA_TRACING_ENDPOINT": "http://127.0.0.1:4318"
```

With these keys, Claude Code sends logs and traces to `BETA_TRACING_ENDPOINT`
instead of `OTEL_EXPORTER_OTLP_ENDPOINT`. Metrics still go to
`OTEL_EXPORTER_OTLP_ENDPOINT`. Use the same sigiro base URL for both.

Detailed beta tracing adds:

- a `claude_code.hook` span for each hook run, with the hook configuration if
  `OTEL_LOG_TOOL_DETAILS=1`;
- `new_context` on interaction, model request, and tool spans: the text that
  Claude Code added to the model context at that step;
- `response.model_output`, `query_source`, `system_prompt_preview`,
  `system_prompt_length`, `system_prompt_hash`, `system_reminders`, and `tools`
  on model request spans;
- `claude_code.system_prompt` log events.

`system_reminders` can be tens of kilobytes on each model request. Claude Code
does not include these attributes in its stable span schema, so their names can
change. Interactive sessions get them only if your organization is on Anthropic's
allowlist. `claude -p` and Agent SDK sessions do not need the allowlist. Claude
Code ignores both keys in project and local settings files.

### Traces from a subprocess

Claude Code gives each Bash command a `TRACEPARENT` variable. A `claude -p` run
that a Claude Code Bash tool starts joins the trace of the parent session. Its
`claude_code.interaction` span has `parent.source: env`, and
`/v1/traces/{trace_id}` returns the spans of both sessions. Filter on
`session.id` to separate them.

### Read Claude Code telemetry

```text
claude_code.interaction                  one user prompt
├── claude_code.llm_request              one model request
├── claude_code.tool                     permission wait and execution
│   ├── claude_code.tool.blocked_on_user permission wait
│   │   └── claude_code.llm_request      auto mode permission check
│   └── claude_code.tool.execution       execution
└── claude_code.hook                     detailed beta tracing only
```

- The duration of `claude_code.tool` includes the permission wait. Use
  `claude_code.tool.execution` for run time and `claude_code.tool.blocked_on_user`
  for wait time.
- In auto mode, Claude Code checks some tool calls with a separate model request.
  That request is a `claude_code.llm_request` span under
  `claude_code.tool.blocked_on_user`, with `query_source_safe = 'auto_mode'`. In
  Claude Code 2.1.273, each check added about 1.4 seconds. The span has token
  counts, but Claude Code sends no `claude_code.api_request` event and no
  `claude_code.cost.usage` point for it. Claude Code cost totals do not include
  these checks.
- A failed tool sets span status and `success = false` on its
  `claude_code.tool_result` event. Claude Code sends no ERROR-severity log for
  it.
- `claude_code.api_request` events and the `claude_code.cost.usage` metric report
  the same total cost.

Replace the timestamps in these queries with literal UTC bounds around your
sessions. Cost by day and model:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT strftime(timestamp, '%Y-%m-%d') AS usage_date,
                 json_extract_string(log_attributes, 'model') AS model,
                 count(*) AS requests,
                 round(sum(CAST(json_extract_string(log_attributes, 'cost_usd') AS DOUBLE)), 2) AS cost_usd
          FROM sigiro_logs
          WHERE service_name = 'claude-code'
            AND body = 'claude_code.api_request'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY usage_date, model
          ORDER BY usage_date, cost_usd DESC"
```

Calls, failures, and p95 duration for each tool:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT json_extract_string(log_attributes, 'tool_name') AS tool,
                 count(*) AS calls,
                 count(*) FILTER (WHERE json_extract_string(log_attributes, 'success') = 'false') AS failures,
                 quantile_cont(CAST(json_extract_string(log_attributes, 'duration_ms') AS DOUBLE), 0.95) AS p95_ms
          FROM sigiro_logs
          WHERE service_name = 'claude-code'
            AND body = 'claude_code.tool_result'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY tool
          ORDER BY calls DESC"
```

Permission wait compared with execution time:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT span_name,
                 count(*) AS spans,
                 round(sum(duration) / 60000000.0, 1) AS total_minutes,
                 median(duration) / 1000 AS p50_ms
          FROM sigiro_spans
          WHERE service_name = 'claude-code'
            AND span_name IN ('claude_code.tool.blocked_on_user', 'claude_code.tool.execution')
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY span_name"
```

Auto mode permission checks, which are not in Claude Code cost totals:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT json_extract_string(span_attributes, 'model') AS model,
                 count(*) AS checks,
                 round(sum(duration) / 60000000.0, 1) AS total_minutes,
                 sum(CAST(json_extract_string(span_attributes, 'cache_read_tokens') AS BIGINT)) AS cache_read_tokens,
                 sum(CAST(json_extract_string(span_attributes, 'output_tokens') AS BIGINT)) AS output_tokens
          FROM sigiro_spans
          WHERE service_name = 'claude-code'
            AND span_name = 'claude_code.llm_request'
            AND json_extract_string(span_attributes, 'query_source_safe') = 'auto_mode'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY model"
```

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

Find the agent's entry and inspect `signals.spans`, `signals.logs`, and
`signals.metrics`. A service with zero spans but nonzero logs or metrics is
sending telemetry; it is not a failed install. Only expect span-based diagnosis
(`gen_ai` and `operation_summaries`) when `signals.spans` is nonzero. If you
enabled traces but see zero spans, check the agent's trace exporter settings.
Do not guess the emitted vocabulary: query only the signal tables with nonzero
counts. Replace the two timestamps with literal UTC bounds around your test
session. For spans:

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

If `signals.logs` is nonzero, inspect the logs:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT service_name, count(*) AS records
          FROM sigiro_logs
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY service_name ORDER BY records DESC"
```

If `signals.metrics` is nonzero, discover metric names across the OpenTelemetry
table types:

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
