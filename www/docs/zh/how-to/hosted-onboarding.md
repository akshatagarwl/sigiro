---
title: 向托管版 sigiro 发送遥测数据
description: >-
  使用租户密钥将 OpenTelemetry 数据发送到托管版 sigiro，然后用 SQL 查询回来。你无需运行任何基础设施。在邀请制 Beta
  期间，我们为每个租户手动完成配置。
sidebar:
  order: 2
---
本指南介绍如何将 OpenTelemetry 数据发送到由我们运维的 sigiro 主机，
以及如何把数据查询回来。你无需运行任何基础设施。

sigiro 可读取链路追踪、日志、指标和性能剖析数据。在邀请制 Beta 期间，我们为每个租户
手动完成配置。请把下文中的 `<your-sigiro-host>` 替换为运维方提供给你的主机地址。

## 1. 获取你的 API 密钥

运维方会向你发送一个形如 `sk_<yourtenant>_<random>` 的密钥。请像对待密码一样对待该密钥。
sigiro 只会展示密钥一次。密钥无法恢复，但可以轮换。该密钥**同时**用于数据摄取和查询的身份认证。

## 2. 将 OpenTelemetry 导出器指向 sigiro

使用标准的 OTLP over HTTP。身份认证请使用标准的
`Authorization: Bearer` 请求头。

**OTLP/HTTP：**

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://<your-sigiro-host>"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sk_<yourtenant>_<random>"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

**该端点不带端口号。** 托管版 sigiro 在标准 HTTPS 端口上接收 OTLP，
因此 `https://api.sigiro.com` 就是完整的端点。请不要照搬自托管[快速上手](/docs/tutorials/quickstart#2-send-it-telemetry)中的
`:4318`：在 `api.sigiro.com` 上，该端口会拒绝连接，而无法建立连接的 SDK
只会把这一失败记录在你自己服务的日志里，而不会记录在这里。如果你的主机有所不同，请咨询运维方。

**`api.sigiro.com` 上不提供 OTLP/gRPC。** 端口 `4317` 已关闭，gRPC
服务路径也未做路由。请使用 HTTP；对于其他任何主机，也请先咨询运维方，不要想当然地认为 gRPC 可用。

你的 SDK 会将每类信号发送到其标准 OTLP 路径（`/v1/traces`、`/v1/logs`、
`/v1/metrics`、`/v1/profiles`），并将该路径追加到上面的端点之后。如果你的
导出器要求为每类信号分别配置端点，请填写完整路径。HTTP
支持 gzip，端点使用 TLS。单个请求解压后必须**不超过 8 MB**。
SDK 的默认批处理大小远低于此上限。

## 3. 确认数据已到达

发送一些遥测数据。等待几秒钟，因为刷写间隔约为 1 秒。
然后执行以下查询：

```bash
curl -s "https://<your-sigiro-host>/v1/query" \
  -H "Authorization: Bearer sk_<yourtenant>_<random>" \
  --data 'SELECT count(*) AS n FROM sigiro_spans'
```

`n` 为非零值即表示链路追踪数据已到达。请在发送第一批数据*之后*再查询。
从未发送过数据的新租户没有任何存储。

## 4. 查询你的数据（SQL API）

- **端点：** 主 HTTPS 端口上的 `POST /v1/query`。
- **认证：** `Authorization: Bearer sk_<yourtenant>_<random>`。
- **请求体：** 原始 SQL 字符串，而非 JSON。
- **响应：** 由行对象组成的 JSON 数组。响应头
  `x-sigiro-truncated: true|false` 表示 sigiro 是否对结果做了截断。

你只能看到**自己的**数据，因为每个租户都拥有独立的目录（catalog）。

**数据表：** `sigiro_spans`、`sigiro_logs`、`sigiro_log_templates`、
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

| 限制项 | 取值 | 超出时 sigiro 的处理方式 |
|-------|-------|-----------|
| 查询超时 | 10 秒 | sigiro 拒绝该查询（400） |
| 结果行数 | 100,000 | sigiro 截断响应并设置 `x-sigiro-truncated: true` |
| 查询内存 | 约 400 MB | 查询失败（400） |
| 摄取请求体 | 8 MB（解压后） | 413（HTTP）/ RESOURCE_EXHAUSTED（gRPC） |
| SQL/API 请求速率 | 每租户 5 req/s，突发 20 | 429 |
| OTLP/HTTP 请求速率 | 每来源 50 req/s，突发 100 | 429 |

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
