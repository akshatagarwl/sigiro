---
title: 用静态 bearer 令牌保护自托管服务
description: 为查询 API、OTLP HTTP 和 OTLP gRPC 设置共用密钥。
sidebar:
  order: 8
---

在本地自托管服务端配置 `SIGIRO_STATIC_BEARER_TOKEN` 后，查询 HTTP、OTLP HTTP 和 OTLP gRPC 监听器均要求同一个令牌。使用 `openssl rand -hex 32` 生成随机值，并通过密钥管理系统注入服务端。令牌必须至少包含 32 字节的 ASCII bearer 令牌字符。空值、过短、不合法或非 UTF-8 值会导致启动失败。如果未设置，服务端仍处于不逐请求认证的 Open 模式；不要向不可信网络暴露 Open 模式监听器。

CLI 和 stdio MCP 客户端使用 `SIGIRO_ACCESS_TOKEN` 或 `--token` 传入相同的值。OTLP 导出器和 HTTP 客户端应在 `Authorization` 请求头中发送 `Bearer ` 前缀及令牌。此模式仅接受大小写完全匹配的 `Bearer`，不接受 `AgentAuth`。所有已认证请求均属于本地 `default` 租户。`/health` 仍公开。`/admin/*` 使用独立的 `SIGIRO_ADMIN_TOKEN`；静态 bearer 令牌不是管理员凭据。

此模式**仅适用于本地存储的自托管实例**。与 `SIGIRO_AUTH_ISSUER`、`SIGIRO_AUTH_AUDIENCE`、`SIGIRO_AUTH_JWKS_URL`、分布式 PostgreSQL 目录或 `SIGIRO_QUERY_SURFACE=quack` 同时配置会导致启动失败。主 HTTP 查询监听器没有原生 TLS；远程访问应使用终止 TLS 的反向代理。OTLP HTTP 和 gRPC 的原生 TLS 可通过 `SIGIRO_TLS_CERT` 和 `SIGIRO_TLS_KEY` 配置，也可在可信代理上终止。不要在跨主机明文连接中发送令牌。轮换令牌需要更新客户端并重启服务端；此初版没有双令牌轮换窗口，也不区分摄取和查询权限。
