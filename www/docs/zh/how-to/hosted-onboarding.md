---
title: 向托管版 sigiro 发送遥测数据
description: 在终端中创建托管版 sigiro 账号，发送 OpenTelemetry，并用 SQL 查询回来。
sidebar:
  order: 3
---

本指南介绍如何把 OpenTelemetry 发送到我们运营的 sigiro 主机，以及如何把数据查询回来。你无需运维任何基础设施。

从头到尾的完整流程：

1. 在 [sigiro.com/sign-in](https://sigiro.com/sign-in) 注册，或在终端中运行 `sigiro signup`。两种方式都会给你一个个人工作区。
2. 安装 CLI 并把它连接到该工作区（`sigiro auth login`）。
3. 导出下面的 `OTEL_*` 变量并启动你的应用。应用发送到 `https://sigiro.com`；没有单独的接收主机。
4. 在 [sigiro.com/dashboard](https://sigiro.com/dashboard/) 查看数据，或用 SQL 查询。

sigiro 读取链路、日志、指标和性能剖析数据。先安装 CLI —— 在 macOS、Linux 和 WSL 上只需一条命令：

```bash
curl -fsSL https://sigiro.com/install | sh
```

然后把 CLI 连接到你的账号。运行哪条命令取决于你从哪里开始，下面的[连接 CLI](#1-连接-cli) 两种都有。

## 1. 连接 CLI

**你已经在网页上完成了注册。** 让已安装的 CLI 登录到你已有的账号：

```bash
sigiro auth login
sigiro auth status
```

`sigiro auth login` 会启动 OAuth 设备授权，并在浏览器中打开确认页面。如果那台机器无法打开浏览器，请加上 `--no-browser`：两种情况下 CLI 都会打印需要确认的 URL 和代码。

**你从终端开始，还没有账号。** 不离开 shell 就能创建账号：

```bash
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

`sigiro signup` 会通过邮件发送六位数 OTP，并在终端中安全地提示你输入。它会创建并激活恰好一个个人工作区，无需浏览器即可在内部完成 OAuth 设备授权，并把刷新/访问凭据保存到操作系统的凭据存储中。

**每次登录都绑定到一个工作区，浏览器会显示它的名字。** 批准页面会显示 CLI 将使用的工作区。如果你的账号属于多个工作区，就在那里选择，选定之前不会批准任何东西。这个选择是登录的一部分：刷新会话时工作区保持不变，即使你之后加入了另一个工作区也是如此；如果你被移出该工作区，登录就会结束。要切换，请重新运行 `sigiro auth login`。

网页注册和 `sigiro signup` 调用的是同一个开通步骤，只有在账号还没有工作区时才会创建个人工作区。因此，对已存在的账号运行 `sigiro signup` 是幂等的：它只会登录该账号，不会创建第二个账号或第二个工作区。如果账号已经属于多个工作区，它会停下来请你运行 `sigiro auth login`，因此绝不会替你选择。

`sigiro auth status` 打印 CLI 绑定的账号和工作区。`sigiro auth logout` 吊销并删除已保存的凭据。这两个流程都不会把凭据留在 shell 历史中，也都不需要你调用 OAuth API 或手动复制令牌。

这里介绍的是人的注册和 OAuth 登录。如果软件需要自己的委托身份和密钥，请使用 [Agent Auth 注册](/docs/how-to/agent-auth)。

托管版的认证由 Better Auth 提供。工作区是隔离的单位：它的数据、成员和查询与其他所有工作区相互隔离。有些 API 字段和错误消息称它为租户或组织；这三个词指的是同一个东西。
两个命令默认都以托管服务 `https://sigiro.com` 为目标。

同一个令牌既用于 OTLP 接收的认证，也用于 API 查询的认证。

## 2. 将 OpenTelemetry 导出器指向 sigiro

使用标准的 OTLP over HTTP。认证使用标准的 `Authorization: Bearer` 请求头。

**OTLP/HTTP：**

```bash
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_EXPORTER_OTLP_ENDPOINT="https://sigiro.com"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_SERVICE_NAME="checkout"   # the name your service shows up under
```

请先设置请求头。如果已经导出了 `OTEL_EXPORTER_OTLP_ENDPOINT`，`sigiro` CLI 会尝试把自己的遥测数据发到那里，并打印 `SIGIRO_OTEL_AUTH_KEY is required` 警告。这个警告无害，令牌照常输出。

**令牌 15 分钟后过期。** `sigiro auth token` 会先刷新已保存的会话，再打印一个短期有效的访问令牌。用它启动的应用会发送 15 分钟的数据；之后导出器会收到带有原因 "The access token has expired" 的 HTTP `401`，而这只会出现在你应用自己的日志里。用来试用 sigiro 已经足够。

**长期运行的服务使用接收密钥发送。** 为你的 CLI 当前登录的工作区创建一个：

```bash
sigiro ingest-keys create --name checkout
```

它会打印密钥 `sigiro_<id>_<secret>` 以及对应的 `otel_exporter_otlp_headers` 值。请立即保存密钥：它只显示一次，sigiro 只保留其哈希。然后用它代替令牌：

```bash
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sigiro_<id>_<secret>"
```

密钥不会过期。它只能发送遥测数据：用它发起的查询会得到 `403`。`sigiro ingest-keys list` 显示每个密钥的 ID 和最后一次使用时间，`sigiro ingest-keys revoke <id>` 会立即停用它。

**应用里还没有 OpenTelemetry？** 零代码插桩只用上面的变量就能发送链路、日志和指标，无需改动代码。Node.js：

```bash
npm install @opentelemetry/api @opentelemetry/auto-instrumentations-node
node --require @opentelemetry/auto-instrumentations-node/register app.js
```

Python：

```bash
pip install opentelemetry-distro opentelemetry-exporter-otlp-proto-http
opentelemetry-bootstrap -a install
export OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED=true  # also send logs
opentelemetry-instrument python app.py
```

其他语言，或者想让智能体来接入，请参阅[使用智能体为代码添加检测](/docs/how-to/install-with-ai)。

**端点不带端口。** 托管版 sigiro 在标准 HTTPS 端口上终止 OTLP，因此 `https://sigiro.com` 就是完整的端点。不要照搬自托管[快速开始](/docs/tutorials/quickstart#2-向它发送遥测数据)里的 `:4318`：在 `sigiro.com` 上该端口会拒绝连接，而连接失败的 SDK 会把错误报告在你自己的服务日志里，而不是这里。

**`sigiro.com` 不提供 OTLP/gRPC。** 端口 `4317` 已关闭，gRPC 服务路径也没有路由。请使用 HTTP。

**注意 —— 自定义或自托管主机。** 上下文所有内容描述的都是 `https://sigiro.com`，即我们运营的、也是 CLI 默认使用的主机。如果你或运维人员在其他地方运行 sigiro，请在这些示例中替换为那台主机，并设置 `SIGIRO_ENDPOINT` 让 CLI 以它为目标；此时端口、gRPC 是否可用以及数据留存都取决于该部署的配置，请询问其运维人员，而不要以本页为准。`sigiro signup` 和 `sigiro auth login` 仍然向 `https://sigiro.com` 上的托管服务进行认证。

SDK 会把每种信号发送到它的标准 OTLP 路径（`/v1/traces`、`/v1/logs`、`/v1/metrics`、`/v1development/profiles`），并把该路径追加到上面的端点之后。如果你的导出器接受的是按信号区分的端点，请给出完整路径。HTTP 支持 gzip，端点使用 TLS。单个请求解压后必须**不超过 8 MB**。SDK 默认的批大小远低于这个限制。

性能剖析数据使用同一个端点。如何从 Go 程序、OpenTelemetry eBPF 剖析器或 Collector 发送，请参阅[向 sigiro 发送性能剖析数据](/docs/how-to/send-profiles)。

## 3. 确认数据已到达

通过你的应用发送一些请求。刷新间隔约为 1 秒，所以请等几秒钟。然后打开 [sigiro.com/dashboard](https://sigiro.com/dashboard/)，用同一个邮箱登录：你的服务会以 `OTEL_SERVICE_NAME` 的名字出现在列表中，并带有它的链路和日志。

如果想在终端中检查：

```bash
curl -s "https://sigiro.com/v1/services" \
  -H "Authorization: Bearer $(sigiro auth token)"
```

你的服务会出现在 `services` 数组中，并附有每种信号的数量。如果没有出现，请在应用自己的日志中查找导出器错误：`401` 会带上原因（令牌过期、密钥已吊销或未知、请求头缺失），连接错误通常意味着端点上还留着 `:4318`。

**还没有插桩好的应用？** 手动发送一个测试 span 来验证链路；OTLP/HTTP 除了 protobuf 也接受 JSON：

```bash
now=$(date +%s)000000000
curl -s -w '%{http_code}\n' https://sigiro.com/v1/traces \
  -H "Authorization: Bearer $(sigiro auth token)" \
  -H "content-type: application/json" \
  --data '{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"hello-sigiro"}}]},
    "scopeSpans":[{"spans":[{"traceId":"5b8efff798038103d269b633813fc60c","spanId":"eee19b7ec3c1b174",
    "name":"GET /hello","kind":2,"startTimeUnixNano":"'"$now"'","endTimeUnixNano":"'"$now"'"}]}]}]}'
```

`200` 表示 sigiro 接受了该 span；几秒钟后 `hello-sigiro` 就会出现在 `/v1/services` 中。

这里不能用 `sigiro query`：它用于联邦查询自托管边缘节点，对 `sigiro.com` 会失败。要在终端或智能体中查询，请像上面那样使用 `curl`，或者使用 [`sigiro mcp`](/docs/reference/cli#mcp)，它会用你已保存的登录读取同一个工作区：

```bash
claude mcp add sigiro -- sigiro mcp
```

## 4. 查询你的数据（SQL API）

- **端点：** 主 HTTPS 端口上的 `POST /v1/query`。
- **认证：** `Authorization: Bearer $(sigiro auth token)`。
- **请求体：** 原始 SQL 字符串，而不是 JSON。
- **响应：** 由行对象组成的 JSON 数组。响应头 `x-sigiro-truncated: true|false` 表示 sigiro 是否截断了结果。

每个工作区都有隔离的目录，所以你只能看到**你自己**工作区的数据。

**数据表：** `sigiro_spans`、`sigiro_logs`、`sigiro_metrics_gauge`、`sigiro_metrics_sum`、`sigiro_metrics_histogram`、`sigiro_metrics_exp_histogram` 和 `sigiro_profiles`。属性列存放 JSON 文本；使用 `json_extract(col, '$.key')` 或 `col ->> 'key'` 查看。

每类表测量的是什么，以及那个会导致自信却错误答案的命名冲突，请阅读[数据表说明](/docs/explanation/tables)。

**允许的 SQL：** 仅限针对 `sigiro_*` 表的 `SELECT`。sigiro 会拒绝写入和 DDL。sigiro 还会屏蔽文件、URL 和 S3 读取函数（`read_csv`、`read_parquet`、`glob` 等）。连接、窗口函数、聚合和 `json_extract` 都可以使用。

**CTE 会被拒绝。** 无论 `WITH` 子句里写了什么，它都无法通过校验。请改写为派生表子查询：`SELECT ... FROM (SELECT ...) t`。由于一个共享目录存放了所有租户，托管服务也会屏蔽 `information_schema`。

**示例：**

```sql
-- Slowest operations, last hour
SELECT service_name, span_name,
       approx_quantile(duration, 0.95) / 1000.0 AS p95_ms, count(*) AS n
FROM sigiro_spans
WHERE timestamp > now() - INTERVAL '1 hour'
GROUP BY 1, 2 ORDER BY p95_ms DESC LIMIT 10;
```

```sql
-- Error-log rate per route, last 15 min
SELECT service_name, log_attributes ->> 'http.route' AS route, count(*) AS errors
FROM sigiro_logs
WHERE timestamp > now() - INTERVAL '15 minutes' AND severity_number >= 17
GROUP BY 1, 2 ORDER BY errors DESC;
```

## 5. 限制

| 限制                  | 值                           | 超出时 sigiro 的行为                                           |
| --------------------- | ---------------------------- | -------------------------------------------------------------- |
| 查询超时              | 10 秒                        | sigiro 拒绝该查询（400）                                       |
| 结果行数              | 100,000                      | sigiro 截断响应并设置 `x-sigiro-truncated: true`               |
| 查询内存              | 约 400 MB                    | 查询失败（400）                                                |
| 接收请求体            | 8 MB（解压后）               | 413                                                            |
| 并发查询              | 每个租户 4 个                | 多出的查询最多等待 5 秒空位，之后返回带 `Retry-After` 的 429   |
| SQL/API 请求速率      | 每个租户 5 req/s，突发 20    | 429                                                            |
| OTLP/HTTP 请求速率    | 每个来源 50 req/s，突发 100  | 429                                                            |

sigiro 没有二级索引。因此**每条查询都要用 `timestamp` 限定范围**。限定范围能带来速度，也能避免超时和截断。

## 6. 数据留存

在测试期间，请把 sigiro 当作实时查询服务，而不是记录系统。`sigiro.com` 尚未公布留存期限；当前期限请[与我们联系](https://cal.com/akshatagarwl/30-minutes)。需要长期保存的数据请自行导出。

## 下一步

- [仪表盘](https://sigiro.com/dashboard/) —— 在浏览器中查看你所登录工作区的服务、链路和日志
- [使用智能体为代码添加检测](/docs/how-to/install-with-ai) —— 如果你的服务还没有接入 OpenTelemetry
- [API 参考](/docs/reference) —— 服务器的 `/openapi.json` 当前描述的 HTTP 操作
