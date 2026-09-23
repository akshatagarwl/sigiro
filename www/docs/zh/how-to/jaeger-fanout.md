---
title: 与 Jaeger 并行试用 sigiro
description: 不修改应用的目标地址，把 Jaeger v2 的 OTLP 链路追踪同时发送给 sigiro。
sidebar:
  order: 8
---

这是**仅针对链路追踪**的评估，不是迁移。应用继续向 Jaeger v2 现有的 OTLP 接收器（`4317`/`4318`）发送数据。保留 Jaeger 原有存储 exporter 和其他管道，只在 traces 管道增加第二个 exporter。Jaeger 不接收日志或指标。如果 Jaeger 前面另有 OpenTelemetry Collector，就修改那个 Collector 的 traces 管道，并保留原有 Jaeger 输出。

## 1. 在不同的宿主机端口启动 sigiro

在与 Jaeger **相同的宿主机**上，确认 `14317`、`14318`、`9999` 空闲：

```bash
docker run -d --name sigiro-trial --restart unless-stopped \
  -p 127.0.0.1:14317:4317 \
  -p 127.0.0.1:14318:4318 \
  -p 127.0.0.1:9999:9999 \
  -v sigiro-trial-data:/var/lib/sigiro \
  ghcr.io/sigiroai/sigiro
curl --fail http://127.0.0.1:9999/health
```

`14317` 是重映射的 gRPC 端口；本文只使用 HTTP `14318`。如果 `9999` 已被占用，也将其**宿主机侧**改成例如 `19999`，并在下文检查中使用新端口。健康检查失败时查看 `docker logs sigiro-trial`。不要修改应用的 exporter 地址。

**安全边界：**这里的 `127.0.0.1` 只适用于 Collector 可以访问宿主机环回地址的情况。另一个容器或主机不能用这个地址访问宿主机；应使用可达的私有网络路径并配置 TLS/认证。不要为了连通而把 Docker 绑定改成 `0.0.0.0`。自托管 sigiro 的 OTLP 和查询接口不进行认证，不得直接暴露在不可信网络上；也不要默认信任本机所有进程。

## 2. 在原有 traces 管道增加镜像输出

先备份原始 YAML。以下是需要**合并的片段**，不是完整配置。保留原有接收器、处理器、exporter、Jaeger 存储、其他管道及其顺序。此示例使用 Jaeger 2.0.0 的组件名 `otlphttp`。较新的 Collector 使用 `otlp_http`，旧名是已弃用的别名。以实际运行的二进制文件验证，并确保定义和列表使用同一名称。

```yaml
exporters:
  otlphttp/sigiro:
    endpoint: http://127.0.0.1:14318
    sending_queue:
      enabled: true
      queue_size: 1000
    retry_on_failure:
      enabled: true
      initial_interval: 5s
      max_interval: 30s
      max_elapsed_time: 300s
service:
  pipelines:
    traces:
      # 保留实际配置中的接收器和处理器。
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger_storage_exporter, otlphttp/sigiro]
```

`endpoint` 是基础 URL；OTLP HTTP exporter 会追加 `/v1/traces`，不要重复写入路径。明文 HTTP 仅用于同机环回地址；跨主机时配置 TLS 和认证。内存队列最多容纳 **1000 个请求**，不是 1000 个 span。单批重试在五分钟后结束。队列满、重试到期或 Collector 重启都可能丢失镜像数据，**不保证无损**。监控 Collector 队列大小/容量、发送及入队失败指标和 Jaeger 接收错误；异常增加时回滚。不要把这个 exporter 加进 Jaeger 的 logs 或 metrics 管道。

## 3. 在重启现有服务前验证

用**相同版本**的 Jaeger v2/Collector 可执行文件对编辑后的文件执行 `validate --config=<编辑后的文件>`。这只检查配置，不证明网络可达或数据已送达。再用编辑后的**副本**启动独立的试验实例：将副本中**所有**监听端口改为空闲端口，包括 OTLP `4317`/`4318`、查询界面 `16686`、Prometheus `8888`（如有 `8889`）、健康检查 `13133`，以及所有其他扩展、接收器、查询监听端口。隔离试验存储，禁止试验实例写入生产存储。确认试验 trace 到达两个试验目标后，才按照正常变更流程重启现有实例。仅修改 Docker 的 `-p` 无法防止共享网络命名空间中的监听或存储冲突。如果无法隔离，不要把生产重启当成首次配置检查。

## 4. 验证两个后端

通过**原有**应用或 OTLP 地址产生一个新的、易识别的 span，记录 trace ID、服务名和时间。在 Jaeger UI（通常是 `16686`）查找该 ID；在 sigiro 执行 `curl --fail http://127.0.0.1:9999/v1/services` 查看服务，再查询同一 ID。请将起止 UTC 时间替换为覆盖测试 span 的实际值；`now()` 会妨碍 DuckLake 文件裁剪，也不符合有界时间查询策略：

```bash
curl --fail -X POST http://127.0.0.1:9999/v1/query \
  --data "SELECT trace_id, span_name, timestamp FROM sigiro_spans
          WHERE trace_id = '<your-trace-id>'
            AND timestamp >= TIMESTAMP '<start-utc:YYYY-MM-DD HH:MM:SS>'
            AND timestamp < TIMESTAMP '<end-utc:YYYY-MM-DD HH:MM:SS>'"
```

如果任一后端缺少 span，检查 Jaeger/Collector 日志、队列和发送失败指标、sigiro 日志、网络命名空间及目标地址。对比实际 trace ID 而非总量，并考虑上游采样与过滤。

## 回滚

恢复备份的 YAML（从 `traces.exporters` 列表**和** `exporters` 映射中删除 `otlphttp/sigiro`），验证后按正常流程重启，并确认新的 trace 仍可在 Jaeger 中查询。然后移除试验容器：

```bash
docker rm -f sigiro-trial
# 仅当确认要丢弃试验数据时执行（不可恢复）：
docker volume rm sigiro-trial-data
```

要保留数据则省略最后一条命令。无需停用 Jaeger 或修改应用配置。

配置依据：[Collector 配置及验证](https://opentelemetry.io/docs/collector/configuration/)、[OTLP HTTP exporter](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/README.md)、[队列及重试](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/exporterhelper/README.md)、[故障排查](https://opentelemetry.io/docs/collector/troubleshooting/)、[Jaeger v2 API](https://www.jaegertracing.io/docs/2.0/architecture/apis/)。
