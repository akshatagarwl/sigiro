---
title: CLI
description: sigiro 的每个子命令、参数及其读取的环境变量，包括托管版注册与登录。
sidebar:
  order: 2
---

`sigiro` 有八个子命令。客户端命令成功时输出带缩进的 JSON；服务器命令会一直挂在各自的协议或监听器上运行。

要创建新的托管账号，运行 `sigiro signup --name "Your Name" --email
you@example.com`。它会通过邮件发送六位数 OTP，在终端中安全地提示你输入，创建并激活恰好一个个人工作区，
在内部完成 OAuth 设备授权，并把刷新/访问凭据保存到操作系统的凭据存储中。已有账号请使用基于浏览器的
`sigiro auth login`；加上 `--no-browser` 时会改为打印验证 URL。`sigiro auth status`、
`sigiro auth token` 和 `sigiro auth logout` 用于管理已保存的会话。

全局参数 `--token <TOKEN>` 是供自动化使用的显式覆盖，优先于 `SIGIRO_ACCESS_TOKEN`。开放模式的自托管服务器不需要它；
配置了静态 Bearer 认证的自托管服务器则需要。

所有客户端子命令（`status`、`mcp`）按以下顺序选择服务器：

1. 设置了 `SIGIRO_ENDPOINT` 时，用它；
2. `sigiro auth login` 或 `sigiro signup` 保存了会话时，用 `https://sigiro.com`；
3. 否则用 `http://localhost:9999`。

因此，登录托管版 sigiro 之后，即使本地正在运行 `sigiro serve`，这些命令也会连接 `sigiro.com`。
要改连本地服务器，请设置 `SIGIRO_ENDPOINT=http://localhost:9999`，或运行 `sigiro auth logout`。

## 托管版智能体认证

sigiro 在内部使用官方的 Agent Auth 实现，不提供第二个用于注册的 CLI。关于委托主机注册、一次性批准、
最小权限的 capability 授予、令牌使用、密钥轮换和吊销，请参阅
[使用 Agent Auth 为代理完成注册](/docs/how-to/agent-auth)。

## ingest-keys

托管版 sigiro 上供长期运行服务使用的固定密钥，用来代替 15 分钟有效的 `sigiro auth token`。密钥属于你的登录所绑定的工作区，以 `Authorization: Bearer <key>` 发送，并且只能发送遥测数据。

| 命令                                      | 作用                                         |
| ----------------------------------------- | -------------------------------------------- |
| `sigiro ingest-keys create --name <NAME>` | 创建密钥并只打印一次；只保留其哈希           |
| `sigiro ingest-keys list`                 | 列出 ID、名称、最后使用时间和吊销状态，不含密钥 |
| `sigiro ingest-keys revoke <ID>`          | 吊销密钥；此后它的发送方会收到 `401`          |

只有你自己的登录可以管理密钥；Agent Auth 令牌或接收密钥都不行。

## serve

启动 OTLP gRPC 服务器、OTLP HTTP 服务器和查询 API。

| 参数                      | 覆盖的变量              | 默认值                  |
| ------------------------- | ----------------------- | ----------------------- |
| `--data-dir <PATH>`       | `SIGIRO_DATA_DIR`       | `~/.local/share/sigiro` |
| `--http-port <PORT>`      | `SIGIRO_HTTP_PORT`      | `9999`                  |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318`                  |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT`      | `4317`                  |

首次启动时会把查询扩展下载到数据目录。下载需要 30–90 秒，期间启动会被阻塞。

未启用认证的服务器（自托管的默认方式）还会在 `http://localhost:9999/dashboard/`
提供 Web 仪表盘。仪表盘内置在二进制文件中。若要改用其他静态构建，请把
`SIGIRO_DASHBOARD_DIR` 设为包含该构建的目录（`dashboard/dist`）。

## query

`sigiro query "<SQL>"` 是一个**联邦协调器**，而不是本地客户端。它解析一个或多个边缘节点，把部分查询推送到每个边缘节点，
再把各部分结果合并成一个答案。

| 参数                      | 类型   | 默认值 |
| ------------------------- | ------ | ------ |
| `<SQL>`                   | 字符串 | 无     |
| `--edge <NAME=HOST:PORT>` | 可重复 | 无     |
| `--full-scan`             | 开关   | 关     |

边缘节点按以下顺序解析。`SIGIRO_EDGES`（逗号分隔的 `name=host:port`）和 `--edge` 会被原样使用，
任一项存在都会跳过发现。两者都未设置时，sigiro 会运行 `tailscale status --json`，并使用所有带
`tag:sigiro-edge` 标签的节点。一个边缘节点都没解析到时，命令会直接失败，而不是对空目标运行。

因此，`sigiro query` 对托管版 `sigiro.com` 或普通的本地服务器**不起作用**。边缘节点必须以
`SIGIRO_QUERY_SURFACE=quack` 提供数据，并且你必须用 `--edge` 指定它，或者能在 tailnet 上找到它。
要对托管版 sigiro 或单台本地服务器运行 SQL，请使用 [`sigiro mcp`](#mcp) 的 `run_query` 工具，或者把 SQL
POST 到 `/v1/query`：

```bash
curl -s https://sigiro.com/v1/query \
  -H "Authorization: Bearer $(sigiro auth token)" \
  --data "SELECT service_name, count(*) AS spans FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour' GROUP BY 1"
```

`--full-scan` 允许运行没有在 `timestamp` 上设置 `WHERE` 范围的查询。默认情况下，sigiro 会拒绝这类查询。

## status

`sigiro status` 读取按上述方式选出的服务器的 `/health` 端点，输出健康状态块，以及包含最近五分钟 span 和日志数量的
`recent_ingest` 块。`endpoint` 字段表示它探测的服务器。`/health` 不需要密钥。

## healthcheck

`sigiro healthcheck` 探测本地的 `/health` 端点，只有在服务器就绪时才以 `0` 退出。distroless 镜像里没有 shell，
也没有 `curl`，所以它就是容器的 `HEALTHCHECK`。

## mcp

`sigiro mcp` 启动一个 stdio 模式的 Model Context Protocol 服务器。它提供 `list_services` 和
`run_query`，以及文档工具（`search_docs`、`get_page`、`list_pages`、`get_navigation`），
这样分析用的智能体无需自己构造 HTTP 请求就能查看 sigiro。它使用按上述方式选出的服务器，所以在
`sigiro auth login` 之后，它会用已保存的会话读取你的托管工作区，并自行刷新该会话。

把它作为 stdio 服务器注册到你的智能体：

```bash
claude mcp add sigiro -- sigiro mcp     # Claude Code
codex mcp add sigiro -- sigiro mcp      # Codex
```

如果智能体启动时的 `PATH` 比你的 shell 更窄，请使用 `sigiro` 的绝对路径。在 `run_query` 中，请用时间戳值来限定
`timestamp`，例如 `timestamp > now() - INTERVAL '1 hour'`；结果行把时间戳和 `duration` 显示为自 Unix
纪元以来的微秒数，但把 `timestamp` 和整数比较会失败。

文档工具使用 `https://sigiro.com/mcp/docs` 上的托管文档 MCP；如果设置了 `SIGIRO_DOCS_MCP_URL`，
则使用该地址。当无法访问该端点时（机器离线、端点几秒内没有响应或返回错误），这些工具会改用内置在
二进制文件中的这份文档副本来回答。工具名称、参数和回答格式保持不变，但内置副本是你所运行版本发布时的
文档。要跳过网络、始终使用内置副本，请把 `SIGIRO_DOCS_MCP_URL` 设为空值。

托管版 sigiro 还在 `https://sigiro.com/mcp` 通过 streamable HTTP 提供同样的工具，供那些在浏览器中用 OAuth
登录、而不是使用 CLI 会话的智能体使用。无需登录、只含文档工具的版本位于 `https://sigiro.com/mcp/docs`。
