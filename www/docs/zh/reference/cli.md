---
title: CLI
description: sigiro 的每个子命令、参数以及读取的环境变量。sigiro 有七个子命令，每个都输出带缩进的 JSON。
sidebar:
  order: 2
---
`sigiro` 有七个子命令。每个子命令在执行成功时都会输出带缩进的 JSON。没有第二种
输出格式，也不做 TTY 检测。

全局标志 `--key <TOKEN>` 用于设置访问托管端点的 bearer token。它会覆盖
`SIGIRO_API_KEY`。自托管服务器会忽略该 token。

所有客户端子命令都会访问 `SIGIRO_ENDPOINT` 指定的服务器，默认值为
`http://localhost:9999`。

## serve

启动 OTLP gRPC 服务器、OTLP HTTP 服务器和查询 API。

| 标志 | 覆盖 | 默认值 |
| --- | --- | --- |
| `--data-dir <PATH>` | `SIGIRO_DATA_DIR` | `~/.local/share/sigiro` |
| `--http-port <PORT>` | `SIGIRO_HTTP_PORT` | `9999` |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318` |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT` | `4317` |

首次启动时会将查询扩展下载到数据目录中。该下载耗时 30–90 秒，并会阻塞启动过程。

## diagnose

`sigiro diagnose <SERVICE>` 针对一个服务返回一个诊断块。

| 标志 | 类型 | 默认值 |
| --- | --- | --- |
| `--from <EPOCH_SECONDS>` | 整数 | 当前时间之前 15 分钟 |
| `--to <EPOCH_SECONDS>` | 整数 | 当前时间 |

`--from` 和 `--to` 的单位是 Unix 纪元**秒**。对应的 HTTP 字段名为 `from_ts` 和
`to_ts`，它们的单位是 Unix 纪元**微秒**。这两个 HTTP 字段都是可选的。如果两者都
省略，服务器会读取最近 15 分钟的数据，与 CLI 使用的时间窗口一致。

该命令输出 JSON。若要缩小输出范围，可通过管道传给 `jq`。CLI 自身不做任何过滤，
因此 CLI 和 API 返回的是同一份文档。

## anomalies

`sigiro anomalies` 列出检测到的状态突变（regime shift），按突变幅度从大到小排列，
每一行都带有跨信号的 `incident_id`。

| 标志 | 类型 | 默认值 |
| --- | --- | --- |
| `--service <NAME>` | 字符串 | 所有服务 |
| `--from <EPOCH_SECONDS>` | 整数 | 无下界 |
| `--to <EPOCH_SECONDS>` | 整数 | 无上界 |

这些行来自 `sigiro_anomalies` 表，由一个定时任务写入。请求时不会进行任何计算。

## query

`sigiro query "<SQL>"` 是一个**联邦协调器**，而不是本地客户端。它会解析一个或多个
边缘节点，向每个边缘节点下推一个局部查询，并把这些局部结果合并成一个答案。

| 标志 | 类型 | 默认值 |
| --- | --- | --- |
| `<SQL>` | 字符串 | 无 |
| `--edge <NAME=HOST:PORT>` | 可重复 | 无 |
| `--full-scan` | 标志 | 关闭 |

边缘节点的解析顺序如下。`SIGIRO_EDGES`（以逗号分隔的 `name=host:port`）和
`--edge` 会被逐字采用，其中任意一个都会跳过自动发现。两者都未设置时，sigiro 会运行
`tailscale status --json`，并取用所有带 `tag:sigiro-edge` 标签的对端节点。如果解析
到的边缘节点为零，该命令会直接失败，而不是在没有任何目标的情况下执行。

因此 `sigiro query` **不能**用于普通的本地服务器。边缘节点必须以
`SIGIRO_QUERY_SURFACE=quack` 提供数据，并且你必须用 `--edge` 指定它，或者在
tailnet 上找到它。若要针对单个本地服务器执行 SQL，请改为把 SQL POST 到
`/v1/query`。

`--full-scan` 允许执行在 `timestamp` 上没有 `WHERE` 约束的查询。默认情况下
sigiro 会拒绝这类查询。

## status

`sigiro status` 读取服务器的 `/health` 端点并输出健康状况块。`/health` 不需要
key。

## fetch-extensions

`sigiro fetch-extensions` 在首次执行 `sigiro serve` 之前把查询扩展下载到数据
目录中。安装 sigiro 之后运行一次，或在部署到没有出站网络访问权限的主机之前
运行。

## healthcheck

`sigiro healthcheck` 探测本地 `/health` 端点，只有在服务器就绪时才以 `0` 退出。
这就是容器的 `HEALTHCHECK`，因为 distroless 镜像既没有 shell 也没有 `curl`。
