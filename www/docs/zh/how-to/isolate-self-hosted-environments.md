---
title: 隔离自托管环境
description: 使用独立的 Sigiro 实例和存储，隔离开发、预发布与生产环境的遥测数据。
sidebar:
  order: 8
---

如果开发、预发布与生产环境必须隔离，请**为每个环境运行一个自托管 Sigiro 实例，并使用独立的数据目录**。各环境还应分别配置 OTLP 和查询监听端口、网络边界的凭据、存储及备份访问权限，以及保留策略。每个环境的导出器只应向对应的 OTLP 端点发送数据。仅分配不同端口却共用数据目录或存储后端，并不能形成隔离。

使用本地二进制文件时，为每个 `sigiro serve` 进程指定不同的目录和三个不冲突的监听端口。例如，在专供开发使用的主机上：

```bash
SIGIRO_RETENTION_DAYS=2 sigiro serve \
  --data-dir /srv/sigiro/dev \
  --http-port 19999 --otlp-http-port 14318 --otlp-grpc-port 14317
```

预发布和生产环境应以**独立进程**运行，使用不同的路径和端口，最好部署在不同主机或网络区域。上述示例没有配置 TLS 或身份验证；不要向不可信网络开放监听端口。将生产遥测、备份和存储凭据保留在生产安全边界内。通过 `SIGIRO_RETENTION_DAYS` 分别设置各环境的保留期；删除目录不能代替保留策略。容器卷和仅监听 localhost 的端口映射参见[自托管快速入门](/docs/tutorials/quickstart)，设置项参见[CLI 参考](/docs/reference/cli)。

## 共享实例无法隔离什么

在 Open 模式下，Sigiro 不验证请求身份，而是将请求归入合成的 `default` 租户。`default` **不是环境边界**：如果开发和生产请求进入同一个 Open 模式实例，它们会共享租户和查询范围。不要将生产流量与低信任度流量发送到共享的 Open 端点。本文不对未经测试的已认证租户之间的行为作出保证。

`deployment.environment` 是 OpenTelemetry 的**资源属性**，不是 Sigiro 项目 ID，也不是授权规则。在有意共享的实例中，可以用标签过滤查询，但标签缺失、拼写错误或由发送者自行填写的值都可能导致数据混合或遗漏。内置诊断和其他查询不会自动应用这一 SQL 过滤条件。例如，在导出器为每个 span 的资源设置 `deployment.environment=staging` 后，可以将下面的字面量 SQL 发送到该实例的 `/v1/query`：

```sql
SELECT service_name, count(*) AS span_count
FROM sigiro_spans
WHERE timestamp >= TIMESTAMP '2026-09-23 00:00:00'
  AND timestamp < TIMESTAMP '2026-09-24 00:00:00'
  AND (resource_attributes::JSON->>'deployment.environment') = 'staging'
GROUP BY service_name
ORDER BY span_count DESC;
```

请将两个时间改成需要的有限半开区间。该值从 `sigiro_spans.resource_attributes` 列中保存的 JSON 字符串读取，并不是独立的表字段。此过滤条件只是**查询约定**，既不是访问控制或存储隔离，也不能代替独立实例。

## 权衡

如果同时隔离存储和网络访问，独立实例的 Open 模式查询端点就不能读取其他环境的数据。但这也意味着无法用一次本地查询跨开发、预发布和生产环境检索。需要比较时，应分别查询各端点，再由已授权的外部流程合并结果，并明确向该流程授予各环境的访问权限。共享实例使跨环境 SQL 查询更方便，却放弃了这种隔离，并将正确过滤的责任交给操作者。远程 OTLP 或查询访问应通过 TLS 和受访问控制的入口，或可信的私有网络保护。不要通过暴露的明文连接传输凭据或遥测数据。各环境的入口、导出器和存储凭据应分别管理并轮换。
