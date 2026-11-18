---
title: 将 agent 自身的遥测数据发送到 sigiro
description: >-
  Claude Code 和 Codex 都会发出关于自身会话的 OpenTelemetry 数据。将它们指向 sigiro，即可用 SQL 读取 token
  数量、工具调用和会话延迟。
sidebar:
  order: 3
---
本指南介绍如何记录 agent 的行为，而不是你的服务的行为。Claude Code 和 Codex
都会发出关于自身会话的 OpenTelemetry 数据。将这些遥测数据指向 sigiro，会话就会
变成可供查询的数据行。

这与[用 agent 为你的代码埋点](/docs/how-to/install-with-ai)是两项不同的任务，后者
是把 _你的服务_ 指向 sigiro。如果两者都需要，就都做一遍。

## 开始之前

你需要一台 sigiro 服务器。用 `sigiro serve` 启动，或使用
[快速上手](/docs/tutorials/quickstart#1-run-it)中的 Docker 命令启动。

在下面的配置中，两个 agent 都通过 OTLP gRPC 从 `4317` 端口导出数据。自托管服务器
不需要密钥。

## Claude Code

Claude Code 从环境变量中读取遥测配置。请在启动 Claude Code 的环境中设置这些
变量。可以使用 shell 配置文件、`.envrc` 文件或包装脚本：

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

如果使用托管服务，请额外添加
`OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${SIGIRO_API_KEY}"`。切勿将密钥
提交到任何文件中。

## Codex

Codex 从自己的配置文件读取遥测配置，而不是从环境变量读取。请编辑
`~/.codex/config.toml`：

```toml
[otel]
environment = "dev"
log_user_prompt = false
exporter = "otlp-grpc"

[otel.exporter."otlp-grpc"]
endpoint = "http://localhost:4317"
```

如果使用托管服务，请额外添加
`headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }`。并在启动 Codex 的环境
中设置 `SIGIRO_API_KEY`。

## 两者共有的一个限制

以上配置只覆盖 agent 进程本身，不覆盖 agent 运行的命令。agent 通过其 shell 工具
启动的应用程序并不总能继承这些变量，因此已埋点的服务仍需要自己的导出器配置。这种
情况请参阅[用 agent 为你的代码埋点](/docs/how-to/install-with-ai)。

## 确认配置生效

启动 agent，运行一次提示词，然后查询 sigiro 收到了哪些服务：

```bash
curl http://localhost:9999/v1/services
```

agent 会把自己上报为一个服务。等它出现之后，再查询会话：

```bash
curl -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, span_name, count(*) AS n
          FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2 ORDER BY n DESC"
```

agent 还会上报计数器和仪表值，这些数据会落到 `sigiro_metrics_*` 系列表中，而不是
`sigiro_spans` 表。[关于这些表](/docs/explanation/tables)说明了各系列表分别存放
什么数据，以及每个系列描述的是哪一台机器。

## 下一步

- [用 agent 为你的代码埋点](/docs/how-to/install-with-ai) —— 把你的服务也指向
  sigiro
- [CLI 参考](/docs/reference/cli) —— 全部子命令和参数
