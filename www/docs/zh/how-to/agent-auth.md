---
title: 使用 Agent Auth 为代理完成注册
description: 通过 Sigiro 的 Agent Auth API 注册代理主机、授予最小能力，并轮换或撤销其凭据。
sidebar:
  order: 2
---

当软件而不是人员需要向托管版 Sigiro 进行身份验证时，请使用 Agent Auth。Sigiro
支持委托身份：用户只需批准一次代理并将其绑定到工作区，之后代理即可使用自己的密钥和
短期令牌无人值守运行。

无需安装另一个 Sigiro 可执行文件。请在代理主机中集成官方 `@auth/agent` SDK。
Sigiro 不支持自主代理身份。

## 开始之前

先创建或登录托管版 Sigiro 账户：

```bash
sigiro signup --name "Your Name" --email you@example.com
# 已有账户：
sigiro auth login
```

该账户必须仅属于一个 Sigiro 工作区。Agent Auth 会把获批代理绑定到此工作区。

请为 SDK 使用持久化密钥存储。SDK 会保存主机和代理的 Ed25519 私钥以及连接记录。
默认内存存储只适用于短期示例；一旦丢失，就必须注册新身份。

## 1. 发现 Sigiro

读取在线提供方文档，不要硬编码生命周期路由：

```bash
curl -fsS https://sigiro.com/.well-known/agent-configuration | jq
```

确认 issuer 为 `https://sigiro.com/api/auth`，受保护资源为
`https://sigiro.com`，且 `modes` 仅包含 `delegated`。

列出可用能力：

```bash
curl -fsS https://sigiro.com/api/auth/capability/list | jq
```

只请求进程必需的能力。OTLP 导出器只需要 `telemetry:ingest`，不需要读取或告警管理能力。

## 2. 添加官方 SDK

```bash
aube add @auth/agent@0.6.2
```

使用 Sigiro 服务源创建客户端。生产环境必须传入持久化 `storage`，例如由密钥存储支持的
`KVStorage`。不要把私钥保存到源代码、日志、截图或未加密的应用状态中。

```ts
import { AgentAuthClient, MemoryStorage } from '@auth/agent';

const client = new AgentAuthClient({
  urls: ['https://sigiro.com'],
  allowDirectDiscovery: true,
  hostName: 'production-telemetry-host',
  storage: new MemoryStorage(), // 生产环境请替换为持久化密钥存储。
  onApprovalRequired: (approval) => {
    const url = approval.verification_uri_complete ?? approval.verification_uri;
    approvalUi.show({ url, code: approval.user_code });
  },
});

const providers = await client.init();
if (providers.length !== 1) throw new Error('Sigiro 发现失败');
```

`approvalUi.show` 表示经过身份验证且不会持久保存内容的操作员界面。该界面不得将批准 URL
或代码写入应用日志。

`connectAgent` 会生成主机和代理的 Ed25519 密钥。Sigiro 的发现文档和生产配置允许动态
主机注册，因此此流程不需要单独的主机注册命令或令牌。管理员预先配置的主机则使用 SDK 的
`enrollHost` 流程及其一次性注册令牌；不要混用这两条路径。

## 3. 注册并批准代理

以委托模式连接，并且只请求遥测写入能力：

```ts
const connection = await client.connectAgent({
  provider: 'https://sigiro.com',
  mode: 'delegated',
  name: 'production telemetry exporter',
  capabilities: ['telemetry:ingest'],
  reason: "Send this service's OpenTelemetry data to Sigiro",
  preferredMethod: 'device_authorization',
});

if (connection.status !== 'active') {
  throw new Error(`代理未激活：${connection.status}`);
}
```

打开显示的验证 URL，登录 Sigiro，确认代码并批准请求的能力。SDK 会轮询，直到批准成功或
请求过期。批准链接和代码有效期很短，但仍不得写入日志或支持工单。

批准后，请将 `connection.agentId` 与 SDK 的持久化状态一起保存。不要导出或复制私钥。

## 4. 验证遥测请求

在发送遥测请求之前立即签发新的能力限定令牌：

```ts
const authorization = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

const headers = {
  Authorization: `Bearer ${authorization.token}`,
};
```

将此请求头附加到 `https://sigiro.com/v1/traces`、`/v1/metrics`、
`/v1/logs` 或 `/v1development/profiles` 的 OTLP/HTTP 请求。代理令牌有效期很短；
导出器传输层必须在过期前获取新令牌，不要把单个令牌放入长期环境变量。

Sigiro 会先验证代理状态、工作区绑定、audience、重放状态和 `telemetry:ingest` 授权，
再接收遥测数据。不要对另一个请求重复使用已签名令牌。

## 5. 轮换代理密钥

通过 SDK 轮换密钥：

```ts
await client.rotateAgentKey(connection.agentId);
```

该操作会替换已注册的公钥，并更新配置存储中的私钥。轮换后，请丢弃旧密钥签发的所有令牌。
请确保 SDK 存储在重启后仍然可用，以便主机继续管理代理。

## 6. 撤销并验证

仅为负向验证创建一个令牌，然后撤销代理：

```ts
const tokenBeforeRevocation = await client.signJwt({
  agentId: connection.agentId,
  audience: 'https://sigiro.com',
  capabilities: ['telemetry:ingest'],
});

await client.disconnectAgent(connection.agentId);
```

`disconnectAgent` 会先撤销远程身份及其能力授权，然后删除本地连接。此后使用
`tokenBeforeRevocation` 发出的请求必须失败。Sigiro 的 OTLP/HTTP 端点会为已撤销的代理
令牌返回 HTTP `401`。如果请求仍被接受，应将其视为撤销失败并停止导出器。

如需停用整个主机而不是单个代理，请从批准该主机的 Sigiro 账户撤销主机。撤销主机也会
撤销其下的所有代理。

## 故障排除

- **发现失败：** 提供方 URL 应为 `https://sigiro.com`，而不是 issuer 路径。使用
  `curl` 检查 well-known 文档。
- **批准过期：** 再次调用 `connectAgent`，并在新请求超时前完成批准。
- **缺少能力：** 明确请求 `telemetry:ingest`，不要通过增加无关能力来规避问题。
- **重启后身份消失：** 将 `MemoryStorage` 替换为持久化 `KVStorage` 后端，并按密钥材料
  保护该后端。
- **签名令牌因重放被拒绝：** 每个请求都签发新令牌。Sigiro 的认证实例共享重放保护。
- **已撤销代理仍能发送数据：** 停止进程，保留失败响应并联系 Sigiro。在查明撤销失败原因
  之前，不要创建替代身份。

有关人员注册、OAuth 登录和普通访问令牌，请参阅
[向托管版 Sigiro 发送遥测](/docs/how-to/hosted-onboarding)。有关 Sigiro CLI 行为，请参阅
[CLI 参考](/docs/reference/cli)。
