---
title: 自托管配置
description: 服务端和客户端环境变量、存储限制、认证与直连摄取 TLS。
sidebar:
  order: 3
---

本参考文档介绍当前 `src/config.rs` 中的配置项。请在启动 `sigiro serve` **之前**设置环境变量；更改服务端配置后必须重启（包括按需读取的密钥：虽然它们在请求时读取，但不支持以此方式热重载）。客户端配置由 CLI 读取；更改后请重新运行客户端命令。除非另有说明，下表中的数值类型均为无符号整数。对于标注“未设置”的配置项，空值与**未设置**并不相同。只有四项配置支持 `serve` 命令行参数；其他服务端配置只能通过环境变量设置。

## 服务端监听器、存储和安全

| 变量                                                                 | 默认值、有效值及行为                                                                                                                                                                                                                                 |
| -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SIGIRO_DATA_DIR`                                                    | 平台数据目录（Unix 上为 `~/.local/share/sigiro`）；文件系统路径。可通过 `--data-dir` 覆盖。在 Docker 中应持久化此卷。                                                                                                                                |
| `SIGIRO_HTTP_PORT`                                                   | `9999`；TCP 端口（0–65535，其中 0 表示由操作系统分配端口）。可通过 `--http-port` 覆盖。即使启用了原生 OTLP TLS，主 API 仍使用**明文**。                                                                                                              |
| `SIGIRO_GRPC_PORT`                                                   | `4317`；TCP 端口。可通过 `--otlp-grpc-port` 覆盖。                                                                                                                                                                                                   |
| `SIGIRO_OTLP_HTTP_PORT`                                              | `4318`；TCP 端口。可通过 `--otlp-http-port` 覆盖。                                                                                                                                                                                                   |
| `SIGIRO_TLS_CERT`, `SIGIRO_TLS_KEY`                                  | 两者均未设置时，OTLP 使用明文传输。必须**同时**设置为可读取的 PEM 证书链和私钥文件路径，才能为 OTLP gRPC 和 OTLP HTTP 启用 TLS。缺少其中一项，或文件无效、不可读取，都会导致启动失败。不支持 mTLS。                                                  |
| `SIGIRO_AUTH_ISSUER`, `SIGIRO_AUTH_AUDIENCE`, `SIGIRO_AUTH_JWKS_URL` | 三个 JWT 设置和静态令牌均未配置时为 **Open** 模式，不逐请求认证。启用 JWT 时必须同时配置三者；只配置部分设置会导致启动失败。分布式模式要求 JWT。TLS 本身不会启用认证。                                                                               |
| `SIGIRO_STATIC_BEARER_TOKEN`                                         | 未设置时不启用静态认证。在本地自托管模式下，配置至少 32 字节的 ASCII bearer 令牌可保护查询 API 和两个 OTLP 监听器。不能与 JWT 设置、分布式目录或 `quack` 并用。同一个令牌同时允许摄取和查询。参阅[指南](/docs/zh/how-to/self-hosted-static-bearer)。 |
| `SIGIRO_ADMIN_TOKEN`                                                 | 未设置时，分布式租户/密钥预配的变更端点禁用。设置后作为管理员 bearer 密钥；**不是**通用的摄取/查询令牌。                                                                                                                                             |
| `SIGIRO_CATALOG_URL`                                                 | 未设置时使用本地 DuckLake 文件目录。以 `postgres://` 或 `postgresql://` 开头的 PostgreSQL URL 会选择分布式目录；显式设置为空值无效。应将其中的凭据视为密钥。                                                                                         |
| `SIGIRO_S3_BUCKET`, `SIGIRO_S3_REGION`, `SIGIRO_S3_ENDPOINT`         | 为空时不使用 S3。使用 S3 存储时，需配合 PostgreSQL 目录设置存储桶和区域；兼容 S3 的存储服务还需要绝对 HTTP(S) 源站端点（不得包含路径、查询参数或凭据）。                                                                                             |
| `SIGIRO_S3_KEY_ID`, `SIGIRO_S3_SECRET`                               | 为空时分别回退到 `AWS_ACCESS_KEY_ID` 和 `AWS_SECRET_ACCESS_KEY`。设置 S3 存储桶时必须提供凭据。不要让密钥出现在日志或 shell 历史记录中。                                                                                                             |
| `SIGIRO_EXTENSION_DIR`                                               | 为空时，扩展缓存位于数据目录下；也可设置为可写或已预置 DuckDB 扩展的文件系统目录路径。                                                                                                                                                               |
| `SIGIRO_ALLOW_UNSIGNED_EXTENSIONS`                                   | `false`；布尔值。`true` 允许加载未签名的本地 DuckDB 扩展；对于不可信文件不要启用。                                                                                                                                                                   |
| `SIGIRO_HOSTED_DATA_DIR`, `SIGIRO_DUCKDB_CACHE_DIR`                  | 托管模式的文件系统路径，默认分别为 `/var/lib/sigiro/telemetry` 和 `/var/cache/sigiro/duckdb`；它们不是普通自托管部署的数据目录。                                                                                                                     |
| `SIGIRO_OTEL_AUTH_KEY`                                               | 为空；用于 Sigiro **自身遥测数据导出**的凭据，而不是服务端请求认证凭据。                                                                                                                                                                             |
| `SIGIRO_ALERT_WEBHOOK_URL`, `SIGIRO_ALERT_WEBHOOK_TOKEN`             | URL 为空时禁用 webhook 投递；可选的令牌用于认证发出的告警 webhook 请求，不用于传入的 OTLP/API 请求。                                                                                                                                                 |
| `SIGIRO_PYROSCOPE_ENDPOINT`                                          | 为空时禁用性能分析数据导出；否则为 Pyroscope 端点 URL。                                                                                                                                                                                              |

## 摄取、保留期限和查询限制

| 变量                                                           | 默认值、有效值及行为                                                                                                                                                                                                                                                                                                                         |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `SIGIRO_RETENTION_DAYS`                                        | `7` 天；无符号 32 位整数，`0` 禁用自动保留期限清理。每小时执行的清理会删除写入器各信号表中带时间戳的过期遥测行；之后由 DuckLake 检查点和快照过期处理回收物理文件（单独执行 DELETE 不一定会释放磁盘空间）。不保证派生数据和管理数据遵循这一截止时间。                                                                                         |
| `SIGIRO_MAX_DISK_BYTES`                                        | `0`（禁用）；以原始字节数表示的无符号 64 位整数。此项是定期采样的**本地**数据目录软上限；达到或超过上限时，以 HTTP **507** 或 gRPC **resource_exhausted** 拒绝新的摄取。采样结果显示占用**低于上限的 90%** 后恢复。队列中及处理中的数据可能使占用超出上限，请预留磁盘空间。S3 Parquet 文件和外部 PostgreSQL 目录占用的字节数**不**计入其中。 |
| `SIGIRO_DISK_SAMPLE_SECONDS`                                   | `30` 秒；大于零的无符号 64 位整数。仅在启用保留期限清理或磁盘上限时进行采样。达到磁盘上限后，如果启用了保留期限清理，会触发保留期限清理，同一时间只运行一次；是否恢复由下一次采样决定。                                                                                                                                                      |
| `SIGIRO_MAX_CHECKPOINT_FAILURES`                               | 每个项目连续失败 `3` 次；无符号 32 位整数。达到阈值时，以 HTTP 507 / gRPC resource_exhausted 拒绝该项目的摄取，并报告健康状态降级；成功完成一次检查点操作会重置计数器并恢复摄取。`0` 禁用拒绝摄取，**不会**禁用故障报告。当 PostgreSQL 目录增长而本地磁盘采样器无法计入其占用时，此设置尤为重要。                                            |
| `SIGIRO_MAX_INGEST_BODY_BYTES`                                 | `8388608` 字节（8 MiB）；正整数。解码后的 gRPC 消息及解压后的 OTLP HTTP 请求体的大小上限。                                                                                                                                                                                                                                                   |
| `SIGIRO_BACKPRESSURE_POLICY`                                   | 默认 `block`；可选 `block` 或 `drop-newest`。`block` 等待每个项目有界队列出现空位，可能使客户端变慢；`drop-newest` 在队列已满时丢弃新传入的消息、记录数据丢失，且**返回成功**（而非 429）。两种策略都不提供持久化缓冲；请为故障期间的采集器配置发送队列和重试。                                                                              |
| `SIGIRO_FLUSH_INTERVAL_SECS`                                   | `1` 秒；无符号 64 位整数表示的间隔。用于定时刷写缓冲的行；达到行数或字节数阈值时可能提前刷写。                                                                                                                                                                                                                                               |
| `SIGIRO_FLUSH_ROW_THRESHOLD`                                   | `10000` 行；无符号整数，按项目统计其各信号表中的行数。                                                                                                                                                                                                                                                                                       |
| `SIGIRO_FLUSH_BYTE_THRESHOLD`                                  | `8388608` 字节（8 MiB）；无符号整数，按项目统计缓冲的数据。                                                                                                                                                                                                                                                                                  |
| `SIGIRO_SHUTDOWN_TIMEOUT_SECS`                                 | 每个项目排空队列的超时时间为 `5` 秒；大于零的无符号 64 位整数。存储速度较慢时可能超时，导致缓冲数据未刷写。                                                                                                                                                                                                                                  |
| `SIGIRO_DUCKDB_THREADS`                                        | 主机可用的并行度；正整数，表示每个项目数据库的 DuckDB 线程数。                                                                                                                                                                                                                                                                               |
| `SIGIRO_DUCKDB_MEMORY_LIMIT`                                   | `400MB`；非空的 DuckDB 内存大小字符串（例如 `8GB`），按项目数据库设置。当诊断或查询遇到内存不足时，可在可用 RAM 允许的范围内提高该值。                                                                                                                                                                                                       |
| `SIGIRO_QUERY_TIMEOUT_SECONDS`                                 | `30` 秒；大于零的无符号 64 位整数，表示单次查询的看门狗超时时间。                                                                                                                                                                                                                                                                            |
| `SIGIRO_MAX_RESULT_ROWS`                                       | `100000` 行；正整数，表示 `/v1/query` 响应被截断前允许的最大行数。                                                                                                                                                                                                                                                                           |
| `SIGIRO_QUERY_ROW_CAP`                                         | `1000000` 行；正整数，表示联邦查询中每个边缘节点的部分结果行数上限。                                                                                                                                                                                                                                                                         |
| `SIGIRO_QUERY_SURFACE`                                         | 默认 `none`；可选 `none` 或 `quack`。`quack` 会在边缘节点开放跨项目 DuckDB 查询服务。                                                                                                                                                                                                                                                        |
| `SIGIRO_QUACK_BIND`, `SIGIRO_QUACK_PORT`, `SIGIRO_QUACK_TOKEN` | 分别默认为 `127.0.0.1`、`7777`、空令牌。绑定地址必须是 IP 地址；端口为 TCP 端口。令牌**仅用于 quack 查询接口**，不用于 OTLP 或主 API。不要将明文 quack 接口暴露给不可信网络。                                                                                                                                                                |

除无符号类型本身的限制外，刷写行数和字节数阈值没有统一的最小值要求；文档**未将 `0` 定义为这些设置的关闭开关**。即使数值符合类型要求，底层子系统仍可能拒绝该值。表格中未明确列出禁用值的配置项，不存在受支持的统一禁用约定。

## 客户端与自身遥测配置（不用于 `serve` 认证）

| 变量                                               | 默认值、有效值及行为                                                                                                                   |
| -------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| `SIGIRO_ENDPOINT`                                  | 为空时选择本地主机（`http://localhost:9999`），除非通过 OAuth 登录选择了托管服务；该 URL 用于客户端命令和 MCP 运维调用。               |
| `SIGIRO_ACCESS_TOKEN`                              | 为空；客户端发出请求时使用的 bearer 凭据。全局参数 `--token <TOKEN>` 会覆盖此值。**不会**配置传入请求的认证。                          |
| `SIGIRO_EDGES`                                     | 为空时发现 Tailscale 对等节点；否则使用逗号分隔的 `name=host:port` 边缘节点覆盖项，供联邦 `sigiro query` 查询使用，也可配合 `--edge`。 |
| `SIGIRO_DOCS_MCP_URL`                              | `https://sigiro.com/mcp/docs`；公开文档的 MCP URL，不会转发 Sigiro 凭据。                                                              |
| `OTEL_SERVICE_NAME`, `OTEL_EXPORTER_OTLP_ENDPOINT` | 分别默认为 `sigiro` 和空值；用于 Sigiro 自身遥测采集的服务名称和导出器端点。虽然不以 `SIGIRO_*` 开头，但也在 `src/config.rs` 中声明。  |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`       | 为空；当对应的 `SIGIRO_S3_*` 值为空时，分别作为 S3 凭据的回退值。                                                                      |

## Docker：直连 OTLP 摄取的 TLS

请使用您自己的证书，并确保其中的 SAN 与客户端使用的主机名匹配。下面的示例只将 OTLP 端口映射到主机，而明文主 API 仅绑定到环回地址；请根据部署情况调整镜像标签和数据卷。

```sh
docker run --name sigiro \
  -v sigiro-data:/var/lib/sigiro \
  -v "$PWD/certs:/certs:ro" \
  -e SIGIRO_TLS_CERT=/certs/server.pem \
  -e SIGIRO_TLS_KEY=/certs/server-key.pem \
  -p 127.0.0.1:9999:9999 -p 4317:4317 -p 4318:4318 \
  ghcr.io/sigiroai/sigiro:latest serve
```

将 gRPC 导出器配置为使用 `https://your-host:4317`，OTLP HTTP 导出器配置为使用 `https://your-host:4318`（显式指定 HTTP 端点时，需包含 `/v1/traces` 等信号路径）。客户端必须信任签发证书的 CA，或明确将自签名证书设为可信，并验证与之匹配的 DNS SAN。**不要**禁用证书验证。主机内 `9999` 端口上的主 API 仍使用 `http://`；如需供远程查询/API 客户端使用 HTTPS，请在反向代理处终止 TLS。TLS 只保护传输，**不**控制访问：Open 模式仍会接受任何能够到达监听端口的请求。请配置全部三项 JWT 设置，或将这些端口隔离在可信网络内。`tests/grpc_tls_test.rs` 覆盖真实服务端的 TLS gRPC 通信路径、TLS HTTP 握手和明文主 API。上述 Docker 命令和镜像标签尚未经过独立的集成测试。
