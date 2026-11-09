---
title: 向 sigiro 发送性能剖析数据
description: 通过 OTLP 从 Go 的 otelprofile、OpenTelemetry eBPF 剖析器或 OpenTelemetry Collector 向 sigiro 发送持续性能剖析数据，然后用 SQL 确认剖析数据行。
sidebar:
  order: 7
---

本指南介绍如何把持续性能剖析数据（CPU、堆以及其他 pprof 剖析类型）发送到 sigiro。sigiro 只接受一种剖析格式：通过 HTTP 或 gRPC 传输的 OTLP 剖析数据，即 OpenTelemetry 的 profiles 信号。

这与[发送智能体自身的遥测数据](/docs/how-to/agent-telemetry)不同，后者用 OpenTelemetry 记录智能体会话。当你想查看一个进程把 CPU 时间和内存花在哪里时，请使用本指南。

## 开始之前

你需要一台 sigiro 服务器。用 `sigiro serve` 启动一台，或者使用[快速开始](/docs/tutorials/quickstart#1-运行它)中的 Docker 命令。

自托管服务器在端口 `4318` 和主端口 `9999` 上接收 OTLP/HTTP，在端口 `4317` 上接收 OTLP/gRPC。托管版 sigiro 只接收 OTLP/HTTP，地址为不带端口的 `https://sigiro.com`。

处于 Open 认证模式的自托管服务器不需要密钥。处于 Better Auth OAuth/JWT 模式的服务器（包括托管版 sigiro）需要 `Authorization: Bearer YOUR_TENANT_KEY`。

OpenTelemetry 的 profiles 信号在上游仍处于开发阶段，其传输格式在不同版本之间发生过变化。sigiro 读取 `opentelemetry-proto` v1.10 的布局。基于其他版本构建的生产者发送的剖析数据，可能会被 sigiro 以 `400` 拒绝。

## Go：otelprofile

[otelprofile](https://github.com/akshatagarwl/otelprofile-go) 使用 Go 自带的运行时剖析器从内部剖析 Go 程序，并导出 OTLP 剖析数据。把它加入你的模块：

```bash
go get github.com/akshatagarwl/otelprofile-go@v0.3.0
```

在程序启动时启动一次，在退出时停止：

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "http://localhost:4318",
})
if err != nil {
    log.Fatal(err)
}
defer p.Stop()
```

默认情况下，它每 15 秒发送一次堆剖析数据，每 30 秒发送一次 CPU 剖析数据。设置 `ProfileTypes`（或 `OTEL_GO_X_PROFILES_TYPES=all`）即可加入 goroutine、mutex 和 block 剖析数据。

对于托管版 sigiro，把端点设为 `https://sigiro.com` 并加上密钥：

```go
p, err := otelprofile.Start(otelprofile.Config{
    ServiceName: "checkout",
    Endpoint:    "https://sigiro.com",
    Headers: map[string]string{
        "Authorization": "Bearer " + os.Getenv("SIGIRO_ACCESS_TOKEN"),
    },
})
```

当对应字段留空时，标准的 `OTEL_EXPORTER_OTLP_ENDPOINT`、`OTEL_EXPORTER_OTLP_HEADERS` 和 `OTEL_SERVICE_NAME` 环境变量同样有效。不要把访问令牌放进源代码管理。

如果程序还通过 OpenTelemetry Go SDK 发送链路追踪，otelprofile 会给 CPU 样本标上它所运行的 span，仪表盘可以从链路视图打开某个 span 的剖析数据。设置方法见 otelprofile 的 README。

## 任意 Linux 进程：OpenTelemetry eBPF 剖析器

[OpenTelemetry eBPF 剖析器](https://github.com/open-telemetry/opentelemetry-ebpf-profiler)无需修改程序，就能对 Linux 主机上所有进程的 CPU 进行采样。它作为 `profiling` 接收器运行在 Collector 发行版 `otelcol-ebpf-profiler` 中，需要 root 权限（或 `CAP_SYS_ADMIN`、`CAP_PERFMON` 和 `CAP_BPF`）以及对 `/proc` 的访问。

该剖析器会给每个进程加上 `process.executable.name`，但不会设置 `service.name`，而 sigiro 会拒绝没有服务名的剖析数据。下面的 `resource` 处理器用可执行文件名补上服务名：

```yaml
receivers:
  profiling: {}

processors:
  resource:
    attributes:
      - key: service.name
        from_attribute: process.executable.name
        action: insert

exporters:
  otlp_http:
    endpoint: http://localhost:4318
    # For hosted sigiro:
    # endpoint: https://sigiro.com
    # headers:
    #   Authorization: "Bearer ${env:SIGIRO_ACCESS_TOKEN}"

service:
  pipelines:
    profiles:
      receivers: [profiling]
      processors: [resource]
      exporters: [otlp_http]
```

启用 profiles 特性开关来启动 Collector：

```bash
sudo otelcol-ebpf-profiler --feature-gates=+service.profilesSupport --config config.yaml
```

在出现 `otlp_http` 这个名称之前的 Collector 版本中，同一个导出器叫做 `otlphttp`。

## 通过 OpenTelemetry Collector

已经在接收 OTLP 的 Collector 可以把剖析数据转发给 sigiro。添加一条从 `otlp` 接收器到 `otlp_http` 导出器的 `profiles` 流水线，并用 `--feature-gates=+service.profilesSupport` 启动 Collector：

```yaml
receivers:
  otlp:
    protocols:
      http:
      grpc:

exporters:
  otlp_http/sigiro:
    endpoint: http://localhost:4318

service:
  pipelines:
    profiles:
      receivers: [otlp]
      exporters: [otlp_http/sigiro]
```

导出器会把 `/v1development/profiles` 追加到端点之后。如果需要给出完整 URL，请改为设置 `profiles_endpoint`。

## HTTP 约定

- **路径：** `POST /v1development/profiles`。
- **请求体：** protobuf 格式的 `ExportProfilesServiceRequest`，内容类型为 `application/x-protobuf`。剖析数据不接受 JSON。
- **服务：** 每个资源都必须带有 `service.name` 属性。缺少时返回 `400`，且不存储任何数据。
- **大小：** 接受 gzip。默认情况下，单个请求解压后不得超过 8 MiB，超出时返回 `413`。
- **错误：** 格式错误的剖析数据，或没有任何非零样本的剖析数据，返回 `400`。缺少或无效的 Better Auth OAuth/JWT bearer 令牌返回 `401`，容量闸门返回 `507`。`2xx` 响应表示 sigiro 已接受该批数据。

## 确认它能工作

让剖析器至少运行一个上传间隔。然后从主 HTTP API 读回剖析数据行：

```bash
curl -s -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, profile_type, unit, count(*) AS samples
          FROM sigiro_profiles
          WHERE service_name = 'checkout'
            AND timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2, 3
          ORDER BY samples DESC"
```

对于托管版 sigiro，请更换源地址并加上 `-H "Authorization: Bearer YOUR_TENANT_KEY"`。结果非空即可确认导出、剖析数据解码和 HTTP 读回都已生效。在仪表盘的 **Profiles** 页面可以把同样的数据看作火焰图。如果过了一个完整间隔结果仍为空，通常是端点有误、查询中的服务名与生产者不一致，或者生产者日志里出现了 sigiro 不读取的格式所返回的 `400`。

## 下一步

- [向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding) — 配置托管版认证及其他遥测信号
- [数据表说明](/docs/explanation/tables) — 查看每张表存储了哪些遥测数据
