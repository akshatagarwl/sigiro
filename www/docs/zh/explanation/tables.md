---
title: 遥测数据表说明
description: 各 sigiro_* 表存储的内容，以及如何查询发送遥测数据的机器。
sidebar:
  order: 2
---

sigiro 存储通过 OTLP 发送的遥测数据。每一行描述发送该遥测数据的服务和机器。Sigiro 不公开运行服务器的机器的实时主机信息。

## 存储的信号

- `sigiro_spans` — 追踪和 span 时序
- `sigiro_logs` — 日志记录
- `sigiro_metrics_gauge` — 时间点数值
- `sigiro_metrics_sum` — 单调和非单调累加值
- `sigiro_metrics_histogram` — 显式桶直方图
- `sigiro_metrics_exp_histogram` — 指数直方图
- `sigiro_profiles` — 性能剖析样本
- `sigiro_anomalies` — 检测到的状态变化

OTLP 定义了四种不同的指标形态，因此指标保存在四张表中。查询多个指标类型时使用 `UNION ALL`。

## 查询正确的机器

CPU、内存、文件系统和网络问题应查询 `sigiro_metrics_*` 表。结果描述的是导出指标的机器，而不是运行 Sigiro 的机器。

```sql
SELECT service_name, metric_name, value, timestamp
FROM sigiro_metrics_gauge
WHERE service_name = 'checkout'
  AND metric_name LIKE 'system.filesystem%'
  AND timestamp >= TIMESTAMP '2026-09-12 00:00:00'
  AND timestamp <  TIMESTAMP '2026-09-13 00:00:00'
ORDER BY timestamp DESC;
```

空结果表示该服务没有发送匹配的主机指标。应修复检测配置，不要用另一台机器的数据替代。

## 查询规则

原始遥测查询必须包含有限的半开时间范围：`timestamp >= ... AND timestamp < ...`。

## 后续阅读

- [为何使用证据而非仪表板](/zh/docs/explanation/evidence)
- [向托管 sigiro 发送遥测数据](/zh/docs/how-to/hosted-onboarding#4-query-your-data-sql-api)
