---
title: 与 Jaeger 并行试用 sigiro
description: 无需修改应用的发送目标，把现有 Jaeger v2 Collector 的 OTLP 链路镜像到一个只监听回环地址的 sigiro 实例。
sidebar:
  order: 8
---

这是一次**仅限链路的评估**，不是迁移。让应用继续发送到 `4317`/`4318` 上现有的 Jaeger v2 OTLP 接收器。在该接收器的 **traces** 管道上把 sigiro 添加为第二个导出器；保留 Jaeger 的存储导出器和其他所有管道。Jaeger 接收的是链路数据，不接收 OTLP 日志或指标。如果 Jaeger 前面还有一个独立的 OpenTelemetry Collector，请改在那里做同样的 traces 管道修改，并保留它现有的 Jaeger 目标。

## 1. 用不同的主机端口启动 sigiro

在与 Jaeger **同一台主机**上，在 `14317`、`14318` 和 `9999` 空闲的情况下运行：

```bash
docker run -d --name sigiro-trial --restart unless-stopped \
  -p 127.0.0.1:14317:4317 \
  -p 127.0.0.1:14318:4318 \
  -p 127.0.0.1:9999:9999 \
  -v sigiro-trial-data:/var/lib/sigiro \
  ghcr.io/sigiroai/sigiro
curl --fail http://127.0.0.1:9999/health
```

`14317` 是重新映射后的 gRPC 端口；本指南只使用 `14318` 上的 OTLP HTTP。如果 `9999` 已被占用，也要重新映射它的**主机**侧（例如 `-p 127.0.0.1:19999:9999`），并在下面的检查中使用那个端口。等待健康检查成功，或查看 `docker logs sigiro-trial`。不要修改任何应用导出器的端点。

**网络边界：** 本示例假设 Jaeger/Collector 能从自己的网络命名空间访问主机的 `127.0.0.1:14318`。位于其他容器或其他主机上的 Collector 无法使用这个回环地址；请通过一个 TLS/认证代理（或同等的网络控制）安排一条私有路由，并改用它可达的地址。sigiro 本身不提供认证。不要仅仅为了让它跑通就把 Docker 绑定改成 `0.0.0.0`。
自托管 sigiro 不对 OTLP 或查询请求做认证：绝不要把这些端口直接暴露在不受信任的网络上。仅绑定回环地址并不意味着本地进程就是可信的。

## 2. 在现有 traces 管道中加入有上限的镜像

备份你的 Jaeger v2/Collector YAML。把下面的条目**合并**进去；这是一个片段，不是用来替换的完整配置。保持现有的接收器、处理器、导出器、扩展、Jaeger 存储以及非链路管道不变。如果 traces 管道还有其他导出器或处理器，请按原有顺序保留它们。本示例使用 Jaeger **2.0.0** 的组件 ID：该版本把 OTLP HTTP 导出器称为 `otlphttp`。较新的 Collector 发行版称它为 `otlp_http`（旧的拼写是已弃用的别名）。请用你安装的二进制运行 `validate` 检查，并在两处一致地使用它接受的名称。

```yaml
exporters:
  # Add alongside the existing jaeger_storage_exporter; do not replace it.
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
      # Preserve the actual receiver and processor lists from your config.
      receivers: [otlp]
      processors: [batch]
      exporters: [jaeger_storage_exporter, otlphttp/sigiro]
```

`endpoint` 是 OTLP/HTTP 的**基础** URL：导出器会追加 `/v1/traces`。不要在这里加上 `/v1/traces`，也不要在没有完整路径的情况下使用按信号区分的 `traces_endpoint`。这里使用明文 HTTP 只适用于同主机回环；`http://` 端点不需要 `tls: { insecure: true }`。如果目标是远程私有地址，请改为在两端都配置 TLS 和认证。

队列位于**内存中**，最多容纳 1,000 个请求，而不是 1,000 个 span。每个批次的重试在五分钟后结束；队列满、重试过期或 Collector 重启都可能丢失镜像的 span。这**不是**保证无损的扇出。请监控 Collector 导出器队列的容量/大小，以及发送失败和入队失败的指标；根据负载和内存设置队列大小。如果这些失败或 Jaeger 的接收错误上升，请停止试用并恢复原配置。不要把 sigiro 导出器加到 Jaeger 的指标或日志管道中。

## 3. 在改动现有实例之前先验证

首先，用**相同版本的 Jaeger v2 或 Collector 二进制/镜像**对编辑后的文件运行 `validate --config=<edited-file>` 命令。验证检查的是配置，**不**检查网络访问、投递或存储。然后用编辑后配置的副本启动一个同版本的独立临时实例：在临时副本中把**所有**监听器重新映射到未使用的端口，包括 OTLP `4317`/`4318`、Jaeger 查询/UI `16686`、内部 Prometheus 指标 `8888`（以及任何 `8889`）、健康检查 `13133`，以及其他所有启用的扩展、接收器和查询监听器。把它的存储与生产隔离，或使用只读的测试后端；不要让临时实例的导出器写入生产存储。在重启现有实例之前，确认临时进程能启动，并能把一条测试链路发送到两个临时目标。仅靠 Docker 的 `-p` 重映射**不会**改变共享网络命名空间内的监听器，也无法防止存储冲突。如果无法安排临时隔离，就不要把生产重启当作第一次语法测试。

临时检查通过后，用你常规的受控 Jaeger/Collector 重启或发布流程应用编辑后的配置。立即检查启动日志和现有的 Jaeger 查询 UI。保留原来的 YAML 以便回滚。

## 4. 验证两个后端

通过**现有**应用或其原来的 OTLP 端点（不要直接发给 sigiro）生成一个**新的**、名称唯一的测试 span。记录 trace ID、服务名和时间。等批处理/导出延迟过去后：

1. 在现有 Jaeger UI（通常在端口 `16686`）中查找该 trace ID，确认其 span 存在。这证明现有的存储路径仍然可用；Collector 成功启动并不能证明这一点。
2. 在 sigiro 上，用 `curl --fail http://127.0.0.1:9999/v1/services` 检查该服务。用 `POST /v1/query` 在测试时间窗口内查询这个确切的 trace ID，或在其中找到对应的 span。例如，替换为你的 trace ID 和覆盖测试 span 的 UTC 时间窗口。请使用字面范围：`now()` 可能妨碍 DuckLake 的文件裁剪，而有界查询策略要求半开的字面区间。

   ```bash
   curl --fail -X POST http://127.0.0.1:9999/v1/query \
     --data "SELECT trace_id, span_name, timestamp FROM sigiro_spans
             WHERE trace_id = '<your-trace-id>'
               AND timestamp >= TIMESTAMP '<start-utc:YYYY-MM-DD HH:MM:SS>'
               AND timestamp < TIMESTAMP '<end-utc:YYYY-MM-DD HH:MM:SS>'"
   ```

如果任一后端缺少该 span，请在下结论之前检查 Jaeger/Collector 日志、导出器失败/队列指标、sigiro 日志、网络命名空间和端点。比较实际的 trace ID，而不只是总数；上游的采样或过滤可能会同时影响两个目标。

## 回滚

恢复备份的 YAML（**从 traces 导出器列表和导出器映射中**都删除 `otlphttp/sigiro`），验证它，并使用同样的受控重启/发布流程。Jaeger 原有的接收器、存储导出器、应用设置和其他管道都保持不变。在 Jaeger 中确认一条新链路。然后停止并删除试用实例：

```bash
docker rm -f sigiro-trial
# Only after deciding to discard the trial's stored telemetry:
docker volume rm sigiro-trial-data
```

删除数据卷不可撤销；如果想保留试用数据，请省略最后一条命令。回滚过程中不要停止 Jaeger，也不要修改应用的发送目标。

## 配置参考

- [Collector configuration and validation](https://opentelemetry.io/docs/collector/configuration/)
- [Jaeger 2.0.0 exporter factory](https://github.com/jaegertracing/jaeger/blob/v2.0.0/cmd/jaeger/internal/components.go) 和 [Collector 0.112.0 component ID](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/internal/metadata/generated_status.go)
- [OTLP HTTP exporter settings (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/otlphttpexporter/README.md)
- [Exporter queue/retry behavior (Collector 0.112.0)](https://github.com/open-telemetry/opentelemetry-collector/blob/v0.112.0/exporter/exporterhelper/README.md)
- [Collector troubleshooting and exporter metrics](https://opentelemetry.io/docs/collector/troubleshooting/)
- [Jaeger v2 API/port reference](https://www.jaegertracing.io/docs/2.0/architecture/apis/)
