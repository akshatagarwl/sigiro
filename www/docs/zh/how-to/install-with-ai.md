---
title: 使用智能体为代码添加检测
description: >-
  只需给智能体一条提示词。智能体会为你的服务添加 OpenTelemetry 自动检测，并将其指向 sigiro。此方法适用于 Claude
  Code、OpenCode 和 Codex。
sidebar:
  order: 1
---

本指南介绍如何为一个尚未接入任何检测的服务添加 OpenTelemetry，具体的代码修改由智能体完成。把下面的提示词粘贴给智能体即可。智能体会检测你的框架、安装官方自动检测组件，并把导出器指向 sigiro。

此方法适用于 Claude Code、OpenCode、Codex，或任何能够编辑文件并运行你的包管理器的智能体。

## 开始之前

你需要一个 sigiro 服务端作为数据目的地。可以用 `sigiro serve` 启动，也可以使用 [快速开始](/docs/tutorials/quickstart#1-运行它) 中的 Docker 命令启动。自托管服务端不需要访问令牌。只有托管服务需要访问令牌，格式为 `<oauth-access-token>`。

Python、Node.js、Java、.NET 和 Ruby 均有自动检测方案。如果你的服务是 Go，请不要使用该提示词，改为查看 [Go 章节](#go检测构建过程而不是源码)。

## 1. 把这条提示词交给智能体

替换其中的 endpoint。除非使用托管服务，否则删除密钥那一行。

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (self-hosted; for hosted sigiro use
                 https://<your-sigiro-host> with no port)
HOSTED OAUTH ACCESS TOKEN: <oauth-access-token>  (omit entirely for a self-hosted server)

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
- For hosted sigiro only, set OTEL_EXPORTER_OTLP_HEADERS with the tenant bearer token
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- For hosted sigiro, do NOT hardcode the OAuth access token — read it from SIGIRO_ACCESS_TOKEN
- If a file already initializes the OTel SDK, update it rather than re-initializing
- Add SIGIRO_ACCESS_TOKEN to .env.example only for a hosted deployment
```

如果智能体识别错了语言，直接告诉它：

```
Language: <python|nodejs|java|dotnet|ruby>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. 告诉智能体不要做什么

在这个场景下，智能体做的事情往往超出你的需要。开始之前先明确这些边界：

- 不要安装 OTel Collector。sigiro 本身就是 collector。
- 不要为每个接口手写 span。自动检测已经覆盖它们。
- 不要修改数据库连接串或业务逻辑。
- 不要配置仪表盘或告警。sigiro 两者都没有。

完整的任务就是：检测框架、安装自动检测、把它指向 sigiro。

## 3. 检查你获得的覆盖范围

| 语言    | 方案                                             | 覆盖范围                                                              |
| ------- | ------------------------------------------------ | --------------------------------------------------------------------- |
| Python  | `opentelemetry-instrument` CLI + distro          | HTTP（FastAPI、Flask、Starlette）、数据库（psycopg2、asyncpg）、Redis |
| Node.js | `@opentelemetry/auto-instrumentations-node`      | HTTP（Express、Fastify）、数据库（pg、mysql2、mongoose）              |
| Java    | `opentelemetry-javaagent.jar`                    | HTTP（Servlets、Spring）、数据库（JDBC）、JVM 指标                    |
| .NET    | `OpenTelemetry.AutoInstrumentation` startup hook | HTTP（ASP.NET Core）、数据库（EF Core）                               |
| Ruby    | `opentelemetry-instrumentation-all` gem          | HTTP（Rails）、数据库（ActiveRecord）、Sidekiq                        |
| Go      | `otelc` 编译期检测                               | HTTP 服务端 span、Go 运行时指标、第三方库                             |

如果某个库没有产生 span，只针对该库手写 span 即可。

有两处缺口值得预先留意，因为自动检测的实际覆盖范围比上表看起来要小。

**日志。** 内置的日志桥接只覆盖具名 logger：Node 上的 pino、winston 和 bunyan，以及 Python 上的标准 `logging` 模块。使用 `console.log` 或 `print` 写日志的服务会产生 trace 和指标，但**没有日志**。请改用真正的 logger，而不是手写桥接。

**官方检测不认识的数据库。** 上表中每一条数据库条目都指明了特定的驱动。`instrumentation-pg` 打补丁的对象是 `node-postgres`，因此使用 `postgres.js` 的 Node 服务不会产生查询 span，而 ORM 也无法弥补这个缺口：`drizzle-orm` 在 `drizzle-orm/tracing` 提供了一个 tracer，但其中的 `await import('@opentelemetry/api')` 在已发布的包里是被注释掉的，所以它根本无法产生 span。在信任数据库那一列之前，先确认你自己的驱动在列表之中；并且优先为该 ORM 选用有人维护的封装，而不是手写 span。

## Go：检测构建过程，而不是源码

不要把这项工作交给智能体处理 Go 项目。对于 Go，你只需在构建命令前加一个前缀，产出的二进制文件就自带检测。你不需要改动任何源文件。相应的工具叫 `otelc`，由 OpenTelemetry 项目构建。该工具还会检测你无法控制的第三方库，而智能体做不到这一点，因为智能体只能编辑你自己的代码。

从 [v1.0.1 release](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/tag/v1.0.1) 下载适用于你平台的二进制文件。该 release 提供了 macOS 与 Linux 的 arm64 和 x86-64 版本，以及 Windows 的 x86-64 版本的 `otelc`：

```bash
curl -fsSL -o otelc https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/download/v1.0.1/otelc-darwin-arm64
chmod +x otelc
./otelc version
```

`go install` 对这个工具不起作用。该模块没有发布命令路径。如果你的平台没有对应的构建产物，请克隆仓库并运行 `make build`。

然后在你平时的构建命令前加上 `otelc`：

```bash
./otelc go build -o checkout .
```

首次构建会下载 OpenTelemetry 相关包，因此比普通构建耗时更长。你的 `go.mod` 文件保持不变。

启动二进制文件时使用与 [快速开始](/docs/tutorials/quickstart#2-向它发送遥测数据) 相同的两个环境变量：

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=checkout \
  ./checkout
```

二进制文件会在启动时创建 trace provider、meter provider 和 logger provider。它不需要你编写任何 SDK 代码。该服务会上报 HTTP 服务端 span 和 Go 运行时指标。

有两点限制。该工具需要 Go 1.25 或更高版本；当前版本为 1.0.1，由项目于 2026 年 7 月 14 日打标签发布。关于它所覆盖的库，请阅读[上游文档](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation)。

## 4. 确认检测生效

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=my-service \
  <your-app-start-command>
```

然后询问 sigiro 收到了哪些服务：

```bash
curl http://localhost:9999/v1/services
```

在有真实流量后的几秒钟内，你的服务就会出现。它出现之后，`sigiro diagnose my-service` 就有数据可读了。

如果服务没有出现，请做以下检查。在自托管服务端上，确认端口：HTTP 用 `:4318`，gRPC 用 `:4317`。在托管服务上则相反，确认 endpoint **不带**端口，并且使用访问令牌查询 `https://<your-sigiro-host>/v1/services`，而不是 localhost。运行 `sigiro status` 确认服务端健康。查看你自己服务的日志，看是否有 OTel 启动错误，因为导出器配置错误会在启动时报告故障。

在托管服务上，确认请求头严格为 `Authorization: Bearer <oauth-access-token>`。自托管服务端会忽略所有认证头。因此令牌错误时不会有任何提示，也不会返回 401。

## 下一步

- [快速开始](/docs/tutorials/quickstart#3-询问发生了什么变化) —— 询问发生了什么变化以及原因
- [把智能体自身的遥测数据发送到 sigiro](/docs/how-to/agent-telemetry) —— 把 Claude Code 或 Codex 也指向 sigiro
- [向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding) —— 如果你不想自己运行服务端，请使用这种方式
