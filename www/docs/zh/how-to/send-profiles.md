---
title: 向 sigiro 发送 Pyroscope 性能剖析数据
description: 通过兼容 Pyroscope 的 HTTP，把经过验证的 Pyroscope Rust SDK 产生的 pprof 剖析数据发送到 sigiro，再用 SQL 确认剖析数据行。
sidebar:
  order: 7
---

本指南介绍如何把应用的 CPU 性能剖析数据发送到 sigiro。它使用经过验证的官方 Pyroscope Rust SDK 及其 pprof 后端。

这与[发送智能体自身的遥测数据](/docs/how-to/agent-telemetry)不同，后者用 OpenTelemetry 记录智能体会话。当你要查看的进程会产生 pprof 剖析数据时，请使用本指南。

## 开始之前

你需要一台 sigiro 服务器。用 `sigiro serve` 启动一台，或者使用[快速开始](/docs/tutorials/quickstart#1-运行它)中的 Docker 命令。

兼容 Pyroscope 的 HTTP 在 sigiro 的主端口 `9999` 和 OTLP HTTP 端口 `4318` 上都可用。除非你有意把接收流量分开，否则请使用主端口。sigiro 不在端口 `4040` 上接收剖析数据。

Open 认证模式的自托管服务器不需要密钥。Better Auth OAuth/JWT 模式的服务器（包括托管版 sigiro）需要 `Authorization: Bearer YOUR_TENANT_KEY`。

## 配置 pyroscope-rs

sigiro 验证的是官方 `pyroscope` crate 及其 pprof 后端。本仓库目前锁定版本 `2.0.4`：

```toml
[dependencies]
pyroscope = { version = "=2.0.4", features = ["backend-pprof-rs"] }
```

对于自托管 sigiro，使用以下目标，不加任何请求头：

```rust
let endpoint = "http://localhost:9999";
let http_headers = None;
```

对于托管版 sigiro，使用不带端口的 HTTPS 源，并加上 OAuth 访问令牌：

```rust
use std::collections::HashMap;

let endpoint = "https://sigiro.com";
let mut headers = HashMap::new();
headers.insert(
    "Authorization".to_string(),
    format!("Bearer {}", std::env::var("SIGIRO_ACCESS_TOKEN")?),
);
let http_headers = Some(headers);
```

在应用启动时，把该目标和可选的请求头映射传给 SDK：

```rust
use pyroscope::backend::pprof::PprofConfig;
use pyroscope::backend::{BackendConfig, pprof_backend};
use pyroscope::pyroscope::PyroscopeAgentBuilder;

let backend = pprof_backend(PprofConfig::default(), BackendConfig::default());
let mut builder = PyroscopeAgentBuilder::new(
    endpoint,
    "checkout",
    100,
    "pyroscope-rs",
    env!("CARGO_PKG_VERSION"),
    backend,
);
if let Some(headers) = http_headers {
    builder = builder.http_headers(headers);
}
let _profile_agent = builder.build()?.start()?;
```

在进程的整个生命周期内都要让 `_profile_agent` 保持在作用域中。SDK 每 10 秒发送一批数据。如果句柄被提前释放，之后的批次都不会到达 sigiro。

不要把 OAuth 访问令牌放进版本控制。请在启动进程的环境中设置 `SIGIRO_ACCESS_TOKEN`。

## HTTP 约定

SDK 可以向以下任一等价路径发送：

- `/`
- `/push.v1.PusherService/Push`
- `/ingest`
- `/ingest/pyroscope`

Rust SDK 会在上面的目标后追加 `/push.v1.PusherService/Push`。因此，如果目标本身已包含该路径，数据可能会发到错误的 URL，而 sigiro 中不会出现明显的错误。只把源地址交给这个 SDK。

sigiro 接受以下请求形式：

- gzip 压缩的 Pyroscope `PushRequest`：`application/proto`、`binary/octet-stream` 或不带内容类型
- `multipart/form-data`，其中文件字段名必须恰好是 `profile`，内容为原始 pprof 字节或 `PushRequest`

查询参数 `name` 会改变 sigiro 解码请求体的方式。存在 `name` 时，sigiro 把 `profile` 字节当作原始 pprof，并把第一个 `{` 之前的文本作为服务名。没有 `name` 时，请求体必须能解码为 `PushRequest`，并且每个序列都必须带有非空的 `service_name` 标签。不带 `name` 发送原始 pprof，或发送没有 `service_name` 的 `PushRequest`，都会返回 `400` 且不存储任何数据。

sigiro 接受 gzip，默认把每个请求限制为解压后 8 MiB。超过 HTTP 请求体上限的请求返回 `413`。格式错误或不受支持的剖析数据返回 `400`，缺少或无效的 Better Auth OAuth/JWT Bearer 令牌返回 `401`，触发容量限制返回 `507`。`200` 表示 sigiro 已接受该批数据。

只发送 pprof 数据。sigiro 不解码 Java Flight Recorder 数据，因此本指南不声称兼容 Java 或 JFR。其他语言 SDK 的配置在有可运行的端到端验证之前暂不列出。

## 确认它能工作

启动应用，让它至少运行 10 秒。然后通过主 HTTP API 读回剖析数据行：

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

对于托管版 sigiro，请更换源地址并加上 `-H "Authorization: Bearer YOUR_TENANT_KEY"`。结果非空，即确认了 SDK 推送、剖析数据解码和 HTTP 读回都正常。如果过了一个完整的 SDK 周期结果仍为空，通常是剖析句柄被释放了、目标包含多余的路径，或者查询中的服务名与 SDK 配置不一致。

## 下一步

- [向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding) — 配置托管版认证及其他遥测信号
- [数据表说明](/docs/explanation/tables) — 查看每张表存储了哪些遥测数据
