---
title: 使用智能体为代码添加检测
description: 给智能体一条提示词，它会为你的服务添加 OpenTelemetry 自动插桩并指向 sigiro。适用于 Claude Code、OpenCode 或 Codex。
sidebar:
  order: 1
---

本指南介绍如何让智能体动手修改代码，为一个完全没有 OpenTelemetry 的服务加上它。把下面的提示词粘贴给智能体。智能体会检测你的框架，安装官方的自动插桩，并把导出器指向 sigiro。

它适用于 Claude Code、OpenCode、Codex，或任何能编辑文件并运行包管理器的智能体。

## 安装 sigiro 技能

在启动智能体之前，先安装这个公开技能：

```bash
npx skills add Arnav12114/skills
```

[`debug-with-sigiro` 技能](https://github.com/Arnav12114/skills)教智能体在遥测数据到达后如何使用 sigiro。本指南只关注插桩。

## 开始之前

你需要一台 sigiro 服务器作为目的地。用 `sigiro serve` 启动一台，或者使用[快速开始](/docs/tutorials/quickstart#1-运行它)中的 Docker 命令。如果使用托管版 sigiro，请在启动智能体之前，为新账号运行 `sigiro signup --name "Your Name" --email you@example.com`（已有账号则运行基于浏览器的 `sigiro auth login`）。自托管服务器不需要密钥。

自动插桩支持 Python、Node.js、Java、.NET 和 Ruby。如果你的服务是 Go，不要使用这条提示词，请改看 [Go 一节](#go-检测构建过程而非源码)。

## 1. 把这条提示词交给智能体

替换端点。对于新的托管账号，先运行终端注册命令；对于已有账号，请使用已登录的 CLI 会话，而不要把令牌复制进提示词。

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (self-hosted; for hosted sigiro use
                 https://<your-sigiro-host> with no port)
HOSTED AUTH: sigiro signup --name "Your Name" --email you@example.com  (new account;
             use sigiro auth login for an existing account; omit for self-hosted)

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
- For hosted sigiro only, read a bearer token with `sigiro auth token`; never paste it into source
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- For a new hosted account, use `sigiro signup --name "Your Name" --email you@example.com`;
  for an existing account, use `sigiro auth login`. Never hardcode credentials.
- If a file already initializes the OTel SDK, update it rather than re-initializing
- For automation only, read credentials from the environment; interactive users should
  use the OS credential store via `sigiro signup` or `sigiro auth login`
```

如果智能体检测错了语言，直接告诉它：

```
Language: <python|nodejs|java|dotnet|ruby>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. 告诉智能体不要做什么

在这件事上，智能体做的会比你想要的多。在它开始之前说明这些限制：

- 不要安装 OTel Collector。sigiro 就是采集器。
- 不要为每个端点手动添加 span。自动插桩已经覆盖了它们。
- 不要修改数据库连接字符串或业务逻辑。
- 不要设置仪表盘或告警配置。这不属于这项任务。

完整的任务就是：检测框架，安装自动插桩，并把它指向 sigiro。

## 3. 检查得到的覆盖范围

| 语言    | 方式                                             | 覆盖                                                            |
| ------- | ------------------------------------------------ | --------------------------------------------------------------- |
| Python  | `opentelemetry-instrument` CLI + distro          | HTTP (FastAPI, Flask, Starlette), DB (psycopg2, asyncpg), Redis |
| Node.js | `@opentelemetry/auto-instrumentations-node`      | HTTP (Express, Fastify), DB (pg, mysql2, mongoose)              |
| Java    | `opentelemetry-javaagent.jar`                    | HTTP (Servlets, Spring), DB (JDBC), JVM 指标                    |
| .NET    | `OpenTelemetry.AutoInstrumentation` startup hook | HTTP (ASP.NET Core), DB (EF Core)                               |
| Ruby    | `opentelemetry-instrumentation-all` gem          | HTTP (Rails), DB (ActiveRecord), Sidekiq                        |
| Go      | `otelc` 编译期插桩                               | HTTP 服务端 span、Go 运行时指标、第三方库                       |

如果某个库没有产生 span，只为那个库手动添加 span。

自动插桩覆盖的范围比表格看起来要小，有两个缺口值得预先想到。

**日志。** 自带的日志桥接只覆盖具名的日志库：Node 上的 pino、winston 和 bunyan，以及 Python 上的标准 `logging` 模块。用 `console.log` 或 `print` 输出的服务会产生链路和指标，但**没有日志**。请换用真正的日志库，而不是手动桥接。

**官方插桩不认识的数据库。** 上表的每个 DB 条目都列出了具体的驱动。`instrumentation-pg` 修补的是 `node-postgres`，所以使用 `postgres.js` 的 Node 服务得不到查询 span，ORM 也补不上这个缺口：`drizzle-orm` 在 `drizzle-orm/tracing` 中自带了一个 tracer，但发布的包里它的 `await import('@opentelemetry/api')` 被注释掉了，因此无法发出 span。在相信 DB 这一列之前，先确认你自己的驱动在列表中，并且优先使用该 ORM 有人维护的封装，而不是手写 span。

## Go: 检测构建过程而非源码

Go 不要交给智能体来做。对于 Go，你只需给构建命令加一个前缀，产出的二进制就自带插桩。你不改任何源文件。这个工具是 `otelc`，由 OpenTelemetry 项目构建。它还会为你无法控制的第三方库插桩，而智能体做不到这一点，因为智能体只能编辑你自己的代码。

从 [v1.0.1 版本](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/tag/v1.0.1)下载适合你平台的二进制。该版本为 arm64 和 x86-64 上的 macOS、Linux，以及 x86-64 上的 Windows 发布了 `otelc`：

```bash
curl -fsSL -o otelc https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/download/v1.0.1/otelc-darwin-arm64
chmod +x otelc
./otelc version
```

`go install` 对这个工具不起作用，因为该模块没有发布命令路径。在没有对应资源的平台上，请克隆仓库并运行 `make build`。

然后在你平常的构建命令前加上 `otelc`：

```bash
./otelc go build -o checkout .
```

第一次构建会下载 OpenTelemetry 包，所以比平常的构建更久。你的 `go.mod` 文件保持不变。

用与[快速开始](/docs/tutorials/quickstart#2-向它发送遥测数据)相同的两个变量启动二进制：

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=checkout \
  ./checkout
```

二进制在启动时会创建 trace provider、meter provider 和 logger provider，不需要你写任何 SDK 代码。服务会上报 HTTP 服务端 span 和 Go 运行时指标。

有两条限制。该工具需要 Go 1.25 或更高版本，当前版本是 1.0.1，由项目在 2026 年 7 月 14 日打上标签。它覆盖哪些库，请阅读[上游文档](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation)。

## 4. 确认它能工作

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=my-service \
  <your-app-start-command>
```

然后询问 sigiro 收到了哪些服务：

```bash
curl http://localhost:9999/v1/services
```

有真实流量后，你的服务会在几秒内出现。出现之后，就可以用有界的 `POST /v1/query` 查看它的 span 和日志。

如果服务没有出现，请做以下检查。在自托管服务器上，确认端口：HTTP 用 `:4318`，gRPC 用 `:4317`。在托管服务上则相反，确认端点**不**带端口，并用你的密钥查询 `https://<your-sigiro-host>/v1/services`，而不是 localhost。运行 `sigiro status` 确认服务器健康。阅读你自己服务的日志，查找 OTel 启动错误，因为错误的导出器配置会在启动时报出问题。

在托管服务上，确认请求头恰好是 `Authorization: Bearer <oauth-access-token>`。自托管服务器会忽略所有认证请求头。因此在那里错误的密钥会悄无声息地失败，也不会返回 401。

## 下一步

- [快速开始](/docs/tutorials/quickstart#3-查询最近的错误) —— 用 SQL 查询最近的错误
- [将 agent 自身的遥测数据发送到 sigiro](/docs/how-to/agent-telemetry) —— 也把 Claude Code 或 Codex 指向 sigiro
- [向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding) —— 如果你不想自己运行服务器，请用这个
