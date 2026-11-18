---
title: CLI
description: sigiro 的每个子命令、参数以及读取的环境变量。sigiro 有七个子命令，客户端命令成功时输出带缩进的 JSON，服务端命令保持连接不退出。
sidebar:
  order: 2
---

`sigiro` 有七个子命令。每个子命令在执行成功时都会输出带缩进的 JSON。没有第二种
输出格式，也不做 TTY 检测。

新用户请运行 `sigiro signup --name "Your Name" --email you@example.com`。命令会发送六位数
的一次性验证码，在终端安全地提示输入，创建并激活唯一的个人工作区，在内部完成 OAuth
设备授权，并将刷新令牌和访问令牌存入操作系统凭据存储。已有账号请使用浏览器登录
`sigiro auth login`；`sigiro auth status`、`sigiro auth token` 和 `sigiro auth logout` 管理登录状态。

全局标志 `--token <TOKEN>` 为自动化显式设置 bearer token。它会覆盖
`SIGIRO_ACCESS_TOKEN`。Open 模式的自托管服务器不需要 token；启用静态 bearer
认证的自托管服务器则需要。

所有客户端子命令都会访问 `SIGIRO_ENDPOINT` 指定的服务器，默认值为
`http://localhost:9999`。

## 托管式代理认证

Sigiro 在内部使用官方 Agent Auth 实现，且不提供第二个注册 CLI。有关委托主机注册、
一次性批准、最小权限能力授权、令牌使用、密钥轮换和撤销，请参阅
[使用 Agent Auth 为代理完成注册](/docs/how-to/agent-auth)。

## serve

启动 OTLP gRPC 服务器、OTLP HTTP 服务器和查询 API。

| 标志                      | 覆盖                    | 默认值                  |
| ------------------------- | ----------------------- | ----------------------- |
| `--data-dir <PATH>`       | `SIGIRO_DATA_DIR`       | `~/.local/share/sigiro` |
| `--http-port <PORT>`      | `SIGIRO_HTTP_PORT`      | `9999`                  |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318`                  |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT`      | `4317`                  |

首次启动时会将查询扩展下载到数据目录中。该下载耗时 30–90 秒，并会阻塞启动过程。

## query

`sigiro query "<SQL>"` 是一个**联邦协调器**，而不是本地客户端。它会解析一个或多个
边缘节点，向每个边缘节点下推一个局部查询，并把这些局部结果合并成一个答案。

| 标志                      | 类型   | 默认值 |
| ------------------------- | ------ | ------ |
| `<SQL>`                   | 字符串 | 无     |
| `--edge <NAME=HOST:PORT>` | 可重复 | 无     |
| `--full-scan`             | 标志   | 关闭   |

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

## healthcheck

`sigiro healthcheck` 探测本地 `/health` 端点，只有在服务器就绪时才以 `0` 退出。
这就是容器的 `HEALTHCHECK`，因为 distroless 镜像既没有 shell 也没有 `curl`。

## mcp

`sigiro mcp` 启动 stdio Model Context Protocol 服务器。它提供
`list_services` 和 `run_query`，
让分析代理无需自行构造 HTTP 请求即可检查 Sigiro。
