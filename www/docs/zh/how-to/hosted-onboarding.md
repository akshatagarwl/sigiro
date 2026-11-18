---
title: 向托管版 sigiro 发送遥测数据
description: >-
  使用 OAuth 访问令牌将 OpenTelemetry 数据发送到托管版 sigiro，然后用 SQL 查询回来。你无需运行任何基础设施。在邀请制 Beta
  期间，我们为每个租户手动完成配置。
sidebar:
  order: 3
---

本指南介绍如何将 OpenTelemetry 数据发送到由我们运维的 sigiro 主机，
以及如何把数据查询回来。你无需运行任何基础设施。

sigiro 可读取链路追踪、日志、指标和性能剖析数据。请先安装 CLI —— 在 macOS、Linux 和 WSL 上都是同一条命令：

```bash
curl -fsSL https://sigiro.com/install | sh
```

然后把 CLI 连接到你的账号。具体运行哪条命令，取决于你是从哪里开始的；下面的
[连接 CLI](#1-连接-cli) 两种情况都有说明。

## 1. 连接 CLI

**你已经在网页端完成注册。** 让已安装的 CLI 登录到你现有的账号：

```bash
sigiro auth login
sigiro auth status
```

`sigiro auth login` 会发起 OAuth 设备授权，并打开浏览器显示确认页面。如果这台机器无法打开浏览器，
请加上 `--no-browser`：无论哪种方式，CLI 都会打印出 URL 和需要确认的验证码。

**你还没有账号，完全从终端开始。** 不离开命令行即可创建账号：

```bash
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

`sigiro signup` 会发送六位数一次性验证码到邮箱，并在终端安全地提示输入，创建并激活唯一的个人工作区，
在内部完成无需浏览器的 OAuth 设备授权，并将刷新令牌和访问令牌存入操作系统凭据存储。

**两条路径都会把 CLI 绑定到同一个、唯一的个人工作区。** 网页注册和 `sigiro signup` 调用的是同一个
预配步骤，而该步骤只在账号还没有任何工作区时才创建个人工作区。因此对已存在的账号来说，
`sigiro signup` 是幂等的：对一个已注册的邮箱执行它，只会让该账号登录，不会再创建一个账号或
第二个工作区。

`sigiro auth status` 会打印 CLI 当前绑定的账号和工作区。`sigiro auth logout` 会吊销并删除已保存的凭据。
两条路径都不会把凭据留在 shell 历史中，也都不需要你手动调用 OAuth API 或复制令牌。

本页介绍人员注册和 OAuth 登录。如果软件需要自己的委托身份和密钥，请使用 [Agent Auth 注册指南](/docs/how-to/agent-auth)。

托管认证由 Better Auth 提供。Organizations 就是租户。这两条命令默认都指向托管服务
`https://sigiro.com`。

同一个令牌同时用于 OTLP 数据摄取和 API 查询的身份认证。

## 2. 将 OpenTelemetry 导出器指向 sigiro

使用标准的 OTLP over HTTP。身份认证请使用标准的
`Authorization: Bearer` 请求头。

**OTLP/HTTP：**

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://sigiro.com"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

`sigiro auth token` 会先刷新已存储的会话，再输出一个短期访问令牌。此方式仅用于交互式测试，不应作为长期运行的收集器密钥。

**该端点不带端口号。** 托管版 sigiro 在标准 HTTPS 端口上接收 OTLP，
因此 `https://sigiro.com` 就是完整的端点。请不要照搬自托管[快速上手](/docs/tutorials/quickstart#2-向它发送遥测数据)中的
`:4318`：在 `sigiro.com` 上，该端口会拒绝连接，而无法建立连接的 SDK
只会把这一失败记录在你自己服务的日志里，而不会记录在这里。

**`sigiro.com` 上不提供 OTLP/gRPC。** 端口 `4317` 已关闭，gRPC
服务路径也未做路由。请使用 HTTP。

**注意 —— 自定义主机或自托管。** 本页上下文所描述的都是 `https://sigiro.com`，也就是由我们运维、
并且是 CLI 默认使用的主机。如果你或运维方把 sigiro 部署在别处，请把示例中的主机替换成那个地址，
并设置 `SIGIRO_ENDPOINT` 让 CLI 指向它；此时端口、是否支持 gRPC 以及数据留存都取决于那套部署的
配置，因此请咨询它的运维方，而不要以本页为准。`sigiro signup` 和 `sigiro auth login` 仍然是对托管
服务 `https://sigiro.com` 进行认证。

你的 SDK 会将每类信号发送到其标准 OTLP 路径（`/v1/traces`、`/v1/logs`、
`/v1/metrics`、`/v1development/profiles`），并将该路径追加到上面的端点之后。如果你的
导出器要求为每类信号分别配置端点，请填写完整路径。HTTP
支持 gzip，端点使用 TLS。单个请求解压后必须**不超过 8 MB**。
SDK 的默认批处理大小远低于此上限。

## 3. 确认数据已到达

发送一些遥测数据。等待几秒钟，因为刷写间隔约为 1 秒。
然后执行以下查询：

```bash
curl -s "https://sigiro.com/v1/query" \
  -H "Authorization: Bearer <oauth-access-token>" \
  --data 'SELECT count(*) AS n FROM sigiro_spans'
```

`n` 为非零值即表示链路追踪数据已到达。请在发送第一批数据*之后*再查询。
从未发送过数据的新租户没有任何存储。

## 4. 查询你的数据（SQL API）

- **端点：** 主 HTTPS 端口上的 `POST /v1/query`。
- **认证：** `Authorization: Bearer <oauth-access-token>`。
- **请求体：** 原始 SQL 字符串，而非 JSON。
- **响应：** 由行对象组成的 JSON 数组。响应头
  `x-sigiro-truncated: true|false` 表示 sigiro 是否对结果做了截断。

你只能看到**自己的**数据，因为每个租户都拥有独立的目录（catalog）。

**数据表：** `sigiro_spans`、`sigiro_logs`、
`sigiro_metrics_gauge`、`sigiro_metrics_sum`、`sigiro_metrics_histogram`、
`sigiro_metrics_exp_histogram`、`sigiro_profiles`，以及 `sigiro_anomalies`
表。`sigiro_anomalies` 表保存预先计算好的状态突变（regime shift）。异常检测过程
会持续写入这些突变。`GET /v1/anomalies` 以带类型的 JSON 形式提供同样的突变数据。
属性列（`*_attributes`、`events_json` 等）保存的是 JSON 文本。
可通过 `json_extract(col, '$.key')` 或 `col ->> 'key'` 读取其中的字段。

关于每个指标族各自度量的内容，以及那个会导致「看似正确其实错误」的同名冲突，
请阅读[关于数据表](/docs/explanation/tables)。

**允许的 SQL：** 仅支持针对 `sigiro_*` 表的 `SELECT`。sigiro 会拒绝写入
和 DDL。sigiro 还会屏蔽文件、URL 和 S3 读取器（`read_csv`、
`read_parquet`、`glob` 等）。JOIN、窗口函数、聚合函数和
`json_extract` 均可正常使用。

**CTE 会被拒绝。** 无论 `WITH` 子句包含什么内容，它都无法通过校验。
请改写为派生表子查询：`SELECT ... FROM (SELECT ...) t`。
托管服务同样屏蔽了 `information_schema`，因为一个共享目录中保存着所有租户的信息。

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

| 限制项             | 取值                      | 超出时 sigiro 的处理方式                         |
| ------------------ | ------------------------- | ------------------------------------------------ |
| 查询超时           | 10 秒                     | sigiro 拒绝该查询（400）                         |
| 结果行数           | 100,000                   | sigiro 截断响应并设置 `x-sigiro-truncated: true` |
| 查询内存           | 约 400 MB                 | 查询失败（400）                                  |
| 摄取请求体         | 8 MB（解压后）            | 413（HTTP）/ RESOURCE_EXHAUSTED（gRPC）          |
| SQL/API 请求速率   | 每租户 5 req/s，突发 20   | 429                                              |
| OTLP/HTTP 请求速率 | 每来源 50 req/s，突发 100 | 429                                              |

sigiro 没有二级索引。因此**请为每个查询都加上 `timestamp` 范围限定**。
范围限定能带来速度，也能避免超时和结果截断。

## 6. 数据留存

在 Beta 期间，请把 sigiro 当作实时查询服务，而不是记录系统（system of record）。
请向运维方询问当前的留存窗口。需要长期保存的数据请自行导出。

## 下一步

- [用智能体为你的代码埋点](/docs/how-to/install-with-ai) —— 如果你的
  服务尚未接入 OpenTelemetry
- [API 参考](/docs/reference) —— 五个端点，每个都配有请求调试台
- [关于「证据」而非仪表盘](/docs/explanation/evidence) —— 为什么
  `/v1/diagnose` 返回的是一个排序列表而不是图表
