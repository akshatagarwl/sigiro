---
title: 将 agent 自身的遥测数据发送到 sigiro
description: 让 Claude Code、Codex、Hermes Agent 或 Pi 把数据发到 sigiro，查看会话、模型延迟、token、工具、错误和重试。
sidebar:
  order: 4
---

本指南记录的是编程智能体自身做了什么，而不是它所编辑的应用做了什么。得到的 span、日志和指标能帮你找出慢的模型调用、重复的工具调用、失败的前置条件、过多的上下文和被串行化的工作。

这与[使用智能体为代码添加检测](/docs/how-to/install-with-ai)不同，后者是把*你的应用*指向 sigiro。两种视角都需要时，两者都用。

## 安装 sigiro 技能

在启动智能体之前，先安装这个公开技能：

```bash
npx skills add Arnav12114/skills
```

[`debug-with-sigiro` 技能](https://github.com/Arnav12114/skills)教智能体如何在 sigiro 中查看遥测数据。它不会改变本指南中的遥测设置。

## 开始之前

启动 sigiro 并验证它的 API：

```bash
sigiro serve
curl http://127.0.0.1:9999/health
```

本地接收器在 `4318` 上接受 OTLP/HTTP，在 `4317` 上接受 OTLP/gRPC。它没有请求认证，所以不要把它暴露在不受信任的网络上。如果使用托管服务，新账号请运行 `sigiro signup --name "Your Name" --email you@example.com`（已有账号运行 `sigiro auth login`），并改为按照[向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding)操作。

遥测数据可能包含提示词、回复、文件内容、命令、用户标识和环境细节。下面的示例选择了保守的采集设置。在启用更丰富的内容之前，请逐一审查每个导出器。

## Claude Code

Claude Code 内置了 OpenTelemetry 导出。把下面这些键添加到 `~/.claude/settings.json` 的 `env` 对象中。这样每个会话都会发送遥测数据，包括由 IDE 启动的会话：

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

如果文件里已经有 `env` 对象，请把这些键合并进去。同样的变量也可以在启动 `claude` 的 shell 中用 `export` 语句设置。

Claude Code 的链路追踪还是 beta 功能。`CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1` 和 `OTEL_TRACES_EXPORTER=otlp` 会开启它。没有链路时，诊断中的 `gen_ai` 和 `operation_summaries` 部分为空，但发送了日志或指标的服务仍会出现在 `/v1/services` 中。你所用版本的情况请参阅 [Claude Code monitoring](https://code.claude.com/docs/en/monitoring-usage)。

开启一个新的 Claude Code 会话，运行一条提示词，等待十秒，然后按照下面的验证步骤操作。

### Claude Code 默认发送的内容

每个指标、日志和 span 都带有 `user.email`、`user.account_uuid`、`user.account_id`、`user.id`、`organization.id`、`session.id` 和 `terminal.type`。在把遥测数据发送到共享或托管服务器之前，请考虑这些身份属性。

不使用下一节中的设置时，遥测数据会记录每个工具的名称，但不记录其参数：

- **工具调用：** 工具名称、是否成功、耗时、输入和结果的字节数，以及权限决定。
- **Bash 命令：** 只有固定列表中的第一个程序（`bash_argv0`，例如 `git`，或 `other`）及其类别（`bash_command_class`）。
- **工具失败：** 只有类别，例如 `ShellError` 或 `TelemetrySafeError`。
- **提示词：** `<REDACTED>` 和提示词长度。
- **模型请求：** `claude_code.api_request` 事件上的模型、token、首个 token 耗时和 `cost_usd`。

### 采集更多细节

下面的每个设置都会导出更多内容，例如命令、文件内容和对话文本。只启用你能接受其暴露程度的设置，然后开启一个新会话。

| 设置                                   | 增加的内容                                                                                                                                                  |
| -------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `OTEL_LOG_TOOL_DETAILS=1`              | 工具 span 上的 `full_command`、`file_path`、`skill_name` 和 `subagent_type`。工具事件上的 `tool_input` 和 `tool_parameters`。完整错误文本。MCP 服务器名称和连接错误。 |
| `OTEL_LOG_TOOL_CONTENT=1`              | 一个包含每个工具输入和输出的 `tool.output` span 事件。需要开启链路。                                                                                       |
| `OTEL_LOG_USER_PROMPTS=1`              | `claude_code.user_prompt` 事件和 `claude_code.interaction` span 上的提示词文本。                                                                            |
| `OTEL_LOG_ASSISTANT_RESPONSES=1`       | `claude_code.assistant_response` 事件上的回复文本。不设置时跟随 `OTEL_LOG_USER_PROMPTS`。设为 `0` 可让回复保持脱敏。                                       |
| `OTEL_LOG_RAW_API_BODIES=1`            | 以 `claude_code.api_request_body` 和 `claude_code.api_response_body` 事件发送完整的 API 请求和响应 JSON。请求体包含对话历史。                              |
| `OTEL_METRICS_INCLUDE_VERSION=true`    | 指标和事件上的 `app.version`。                                                                                                                              |
| `OTEL_METRICS_INCLUDE_ENTRYPOINT=true` | 指标和事件上的 `app.entrypoint`。                                                                                                                           |

失败的工具最能体现差别。默认设置下，失败的 Read 报告 `TelemetrySafeError`，失败的 Bash 命令报告 `ShellError`。设置 `OTEL_LOG_TOOL_DETAILS=1` 后，同样的失败会报告为 `File does not exist. Note: your current working directory is …` 和 `Shell command failed`，并带上失败的命令或路径。

Claude Code 会在 `CLAUDE_CODE_OTEL_CONTENT_MAX_LENGTH`（默认 61,440 个字符）处截断内容属性。长对话的请求体到达时会在这个长度被截断。sigiro 能存储这么大的值。更大的上限未经测试。

### 详细 beta 链路追踪

详细 beta 链路追踪还会记录模型在每一步收到和返回的内容。把下面这些键添加到同一个 `env` 对象中：

```json
"ENABLE_BETA_TRACING_DETAILED": "1",
"BETA_TRACING_ENDPOINT": "http://127.0.0.1:4318"
```

设置这些键后，Claude Code 会把日志和链路发送到 `BETA_TRACING_ENDPOINT`，而不是 `OTEL_EXPORTER_OTLP_ENDPOINT`。指标仍发往 `OTEL_EXPORTER_OTLP_ENDPOINT`。两者请使用同一个 sigiro 基础 URL。

详细 beta 链路追踪会增加：

- 每次运行钩子时的 `claude_code.hook` span，在 `OTEL_LOG_TOOL_DETAILS=1` 时附带钩子配置；
- 交互、模型请求和工具 span 上的 `new_context`：Claude Code 在这一步加入模型上下文的文本；
- 模型请求 span 上的 `response.model_output`、`query_source`、`system_prompt_preview`、`system_prompt_length`、`system_prompt_hash`、`system_reminders` 和 `tools`；
- `claude_code.system_prompt` 日志事件。

`system_reminders` 在每个模型请求上可能达到几十 KB。Claude Code 没有把这些属性纳入稳定的 span 模式，所以它们的名称可能会变。交互式会话只有在你的组织位于 Anthropic 的许可名单中时才会得到这些属性。`claude -p` 和 Agent SDK 会话不需要许可名单。Claude Code 会忽略项目和本地设置文件中的这两个键。

### 来自子进程的链路

Claude Code 会给每条 Bash 命令一个 `TRACEPARENT` 变量。由 Claude Code 的 Bash 工具启动的 `claude -p` 运行会加入父会话的链路。它的 `claude_code.interaction` span 带有 `parent.source: env`，`/v1/traces/{trace_id}` 会返回两个会话的 span。按 `session.id` 过滤即可把它们分开。

### 读取 Claude Code 遥测数据

```text
claude_code.interaction                  one user prompt
├── claude_code.llm_request              one model request
├── claude_code.tool                     permission wait and execution
│   ├── claude_code.tool.blocked_on_user permission wait
│   │   └── claude_code.llm_request      auto mode permission check
│   └── claude_code.tool.execution       execution
└── claude_code.hook                     detailed beta tracing only
```

- `claude_code.tool` 的耗时包含权限等待。运行时间请用 `claude_code.tool.execution`，等待时间请用 `claude_code.tool.blocked_on_user`。
- 在 auto 模式下，Claude Code 会用一次单独的模型请求检查部分工具调用。该请求是 `claude_code.tool.blocked_on_user` 下的一个 `claude_code.llm_request` span，带有 `query_source_safe = 'auto_mode'`。在 Claude Code 2.1.273 中，每次检查大约增加 1.4 秒。这个 span 有 token 计数，但 Claude Code 不会为它发送 `claude_code.api_request` 事件，也不会发送 `claude_code.cost.usage` 数据点。Claude Code 的费用总计不包含这些检查。
- 失败的工具会设置 span 状态，并在其 `claude_code.tool_result` 事件上设置 `success = false`。Claude Code 不会为它发送 ERROR 级别的日志。
- `claude_code.api_request` 事件和 `claude_code.cost.usage` 指标报告的总费用相同。

请把这些查询中的时间戳替换为覆盖你会话的 UTC 字面范围。按天和模型统计费用：

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

每个工具的调用次数、失败次数和 p95 耗时：

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

权限等待与执行时间的对比：

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

不计入 Claude Code 费用总计的 auto 模式权限检查：

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

Codex 从 `~/.codex/config.toml` 读取遥测设置：

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

`exporter` 是日志导出器；它不能替代 `trace_exporter` 或 `metrics_exporter`。Codex 以 `codex_exec` 的身份上报，所以验证时请查找这个服务名。

## Hermes Agent

Hermes 使用社区的 [`briancaffey/hermes-otel`](https://github.com/briancaffey/hermes-otel) 插件：

```bash
hermes plugins install briancaffey/hermes-otel/hermes_otel
```

把插件依赖安装到运行 Hermes 的同一个 Python 环境中。对于虚拟环境位于 `~/git/hermes-agent/venv` 的源码检出：

```bash
~/git/hermes-agent/venv/bin/pip install \
  -r ~/.hermes/plugins/hermes_otel/requirements.txt
```

请使用与你的安装相对应的 `pip` 路径；把这些包装到无关的系统 Python 中，Hermes 是看不到的。

创建 `~/.hermes/hermes_otel.yaml`：

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

安装或修改插件后请重启 Hermes。通用 OTLP 后端会从链路端点推导出 `/v1/metrics` 和 `/v1/logs`。它会发出会话、模型、API、工具、技能、审批和子智能体的嵌套 span，以及 token、耗时、费用、重试和资源指标。

该插件的 `capture_previews` 默认为 `true`；上面显式设置的 `false` 会阻止导出提示词、回复、工具参数和工具结果的预览。`capture_logs: true` 仍可能导出被导入库的 Python 日志记录。需要更窄的日志范围时，请使用插件的 `log_attach_logger` 设置。

## Pi 编程智能体

Pi 官方的 `@earendil-works/pi-telemetry` 包定义了遥测类型，但目前不提供 OTLP 导出器。因此下面的配置使用社区维护的 [`@devkade/pi-opentelemetry`](https://github.com/devkade/pi-opentelemetry) 扩展；采用前请先审查：

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

在 Pi 中，`/otel-status` 报告导出器状态，`/otel-open-trace` 打开当前链路。该扩展的文档涵盖链路和指标，不涉及日志导出。它的 `strict` 隐私配置比默认的 `detailed-with-redaction` 配置暴露得更少。

## 确认遥测数据已到达

在配置好的智能体中运行一条真实的提示词，然后查找服务名：

```bash
curl http://127.0.0.1:9999/v1/services
```

找到智能体对应的条目，查看 `signals.spans`、`signals.logs` 和 `signals.metrics`。span 为零但日志或指标不为零的服务是在发送遥测数据的，并非安装失败。只有当 `signals.spans` 不为零时，才能期待基于 span 的诊断（`gen_ai` 和 `operation_summaries`）。如果你开启了链路却看到零个 span，请检查智能体的链路导出器设置。不要猜测发出的词汇：只查询计数不为零的信号表。把两个时间戳替换为覆盖测试会话的 UTC 字面范围。对于 span：

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

如果 `signals.logs` 不为零，查看日志：

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT service_name, count(*) AS records
          FROM sigiro_logs
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY service_name ORDER BY records DESC"
```

如果 `signals.metrics` 不为零，在各类 OpenTelemetry 表中查找指标名：

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

`TIMESTAMP` 字面范围让 sigiro 能裁剪 Parquet 文件。由 `now()` 构造的条件会读取所有文件，随着本地历史增长会越来越慢。

接下来，请阅读[用 sigiro 调试并优化智能体](/docs/how-to/debug-agent-performance)。
