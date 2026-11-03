---
title: 将 sigiro 接入 Executor
description: 把 sigiro 的 OpenAPI API 或文档 MCP 服务器添加到 Executor，再把生成的工具提供给你的智能体。
sidebar:
  order: 6
---

[Executor](https://executor.sh/) 是面向智能体的集成层。它把 OpenAPI 规范、MCP 服务器和 GraphQL 端点导入同一个工具目录。你无需编写 Executor 插件就能添加 sigiro。

Executor 还会读取 [integrations.sh](https://integrations.sh/) 注册表来填充目录，而 sigiro 在那里发布了自己的接口，所以在你粘贴任何东西之前，Executor 内的搜索就可能找到 sigiro。下面的步骤是显式导入，两种情况下都适用。

## 添加 sigiro API

公开的 OpenAPI 文档是：

```
https://sigiro.com/openapi.json
```

在 Executor 网页界面中：

1. 打开 **Add Integration**。
2. 粘贴 OpenAPI URL。
3. 让 Executor 检测并索引这些操作。
4. 为该集成创建一个连接。
5. 如果 API 要求，把托管版 sigiro 的 OAuth 访问令牌添加到该连接上。
6. 为每个工具设置策略。对运行 SQL 或查看遥测数据的工具使用审批。

该规范是公开的，读取它不需要密钥。托管版 API 本身仍然需要 OAuth 访问令牌。自托管 sigiro 也可以用你自己实例提供的 OpenAPI URL 以同样的方式导入，而且完全不需要密钥，因为自托管服务器运行在开放模式下，不对任何请求做认证。

Executor 也提供 CLI。等价的导入命令如下：

```bash
executor call executor openapi addIntegration '{
  "spec": "https://sigiro.com/openapi.json",
  "namespace": "sigiro",
  "baseUrl": "https://sigiro.com"
}'
```

如果你导入的规范中的服务器 URL 不是你希望 Executor 调用的 URL，请使用 `baseUrl` 字段。认证应配置在生成的连接上，而不是写进公开规范的 URL。

确认它已生效：

```bash
executor tools integrations
```

## 添加 sigiro 文档 MCP 服务器

文档站点还提供一个公开的只读 MCP 服务器：

```
https://sigiro.com/mcp/docs
```

在 Executor 中把它作为 MCP 集成添加，并选择 streamable HTTP 传输。这样智能体就能使用搜索、获取页面等文档工具。它无法访问工作区的遥测数据。去掉 `/docs` 的同一 URL `https://sigiro.com/mcp` 会额外提供遥测工具，但需要 OAuth 登录。

实时遥测操作请使用 OpenAPI 集成。智能体需要查找或阅读 sigiro 文档时请使用 MCP 集成。两者可以添加到同一个 Executor 工作区。

## 连接你的智能体

Executor 索引完集成后，通过 MCP 把你的智能体连接到 Executor。智能体使用的是 Executor 的 MCP 端点，不需要单独的 sigiro 插件。你的部署所用的端点和传输方式，请参考 [Executor 的 MCP 连接指南](https://executor.sh/docs)。

## 验证导入

在让智能体使用之前，先在 Executor 中检查该集成。sigiro API 的导入应提供服务、链路、告警和查询相关的操作。文档的导入应提供文档搜索和页面获取。

然后先试一次只读调用。对于托管版 API 连接，请确认请求带上了 OAuth 访问令牌，并且返回的数据属于预期的 sigiro 租户。不要把 OAuth 访问令牌写进本页、OpenAPI URL，或提交到仓库中的智能体提示词里。

## 下一步

- [API 参考](/docs/reference) — 查看生成的操作详情
- [向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding) — 创建托管端点和租户连接
- [快速开始](/docs/tutorials/quickstart) — 在本地运行 sigiro 并发送测试数据
