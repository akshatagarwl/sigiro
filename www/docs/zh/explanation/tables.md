---
title: 关于这些表，以及它们描述的是哪台机器
description: >-
  sigiro_* 表存放的是发送 OTLP 的一方产生的遥测数据。sys_* 函数读取的是 sigiro
  自身所在机器。混淆两者，你得到的就是另一台机器的数据。
sidebar:
  order: 2
---
sigiro 提供两类表，在查询语句里它们看起来一模一样。一类存放你的服务发送来的遥测数据，另一类读取
sigiro 自身运行所在的那台机器。两者都能回答关于 CPU、内存或磁盘的问题，
而且都不会告诉你它指的是哪台机器。

本页要划清这条界线。它是本章中最有价值的内容，因为
这种失效模式不会表现为错误信息，而是表现为一个看起来
正确、却描述了你并未询问的那台机器的数字。

## 两个家族，两个主体

| 家族 | 它是什么 | 谁的机器 | 时间 |
| --- | --- | --- | --- |
| `sigiro_spans`、`sigiro_logs`、`sigiro_log_templates`、`sigiro_metrics_gauge`、`sigiro_metrics_sum`、`sigiro_metrics_histogram`、`sigiro_metrics_exp_histogram`、`sigiro_profiles`、`sigiro_anomalies` | 已存储的遥测数据 | 发送 OTLP 的一方——你的服务，在你的主机上，无论它们跑在哪里 | 历史数据，带有 `timestamp` 列 |
| `sys_cpu_info()`、`sys_memory_info()`、`sys_disk_info()`、`sys_network_info()`、`sys_os_info()` | 实时表函数 | 当前 sigiro 进程运行所在的那一台机器 | 此刻的一次测量，没有历史，也没有时间列 |

`sigiro_*` 这些名字是表。`sys_*` 这些名字是函数，调用时要带一对
空括号。这是唯一可见的区别，而这点区别远远不够。

## 陷阱，一个例子说明

假设某个 agent 问：_checkout 服务的磁盘满了吗？_

```sql
-- WRONG for that question
SELECT * FROM sys_disk_info();
```

这条查询会成功执行。它会返回真实的挂载点和真实的可用字节数。其中的每一个
数字描述的都是 sigiro 运行所在的那台机器。如果 checkout 跑在另一个容器里、
另一台主机上，或者另一个区域，那么这个答案跟 checkout 毫无关系。在托管服务上，
这个答案描述的是我们运维的一台机器，而你在上面什么都没跑。

agent 无从察觉。响应里没有错误、没有空列、也没有任何提示信息。于是
agent 会自信地报出一个磁盘数字，而这个数字说的是另一台机器。

关于 checkout 的问题，要用你的服务发送来的指标来回答：

```sql
-- RIGHT for that question
SELECT service_name, metric_name, value, timestamp
FROM sigiro_metrics_gauge
WHERE service_name = 'checkout'
  AND metric_name LIKE 'system.filesystem%'
  AND timestamp > now() - INTERVAL '1 hour'
ORDER BY timestamp DESC;
```

这读取的是 checkout 自己的 OpenTelemetry 主机指标所上报的内容，来自
checkout 自己的机器，覆盖你选定的时间窗口。它是历史数据，所以你能看到
趋势，而不只是某一瞬间。如果结果集为空，那也是一个真实且有用的答案：
没有任何东西在为该服务发送主机指标，该修的是埋点，而不是换一条查询。

## 为什么会有 `sys_*` 存在

`sys_*` 函数不是设计失误。它们能很好地回答一个问题：_这个节点现在是什么状况？_
自托管集群的运维人员会对每个节点问这个问题，而这个答案不需要任何遥测管道，因为
节点读取的是它自己的内核。

它们也是 sigiro SQL 规则的一个有意为之的例外。所有其他会读取数据库之外内容的函数
都被禁用，因为调用方可以把它们指向某个路径：`read_csv('/etc/passwd')` 正是黑名单
要堵上的那个洞。`sys_*` 函数不接受参数，所以调用方没有路径可选，正是这一性质
让它们得以通行。sigiro 会拒绝任何带参数的 `sys_*` 调用。

把后果讲明白。在自托管节点上，边界就是网络，因此任何能触达查询接口的人都可以
读取 `sys_*`。在托管服务上，`sys_*` 描述的是我们的机器，永远不是你的。两种情况下
规则都一样：`sys_*` 说的是那个回答你的进程。

## 指标表有四张，原因在于写入路径

指标进入四张表而不是一张：

- `sigiro_metrics_gauge` —— 某个时间点上的一个值
- `sigiro_metrics_sum` —— 计数器，单调或非单调
- `sigiro_metrics_histogram` —— 显式桶边界
- `sigiro_metrics_exp_histogram` —— 指数桶

单张 `sigiro_metrics` 表的方案曾被提出并被否决。OTLP 本身就以不同字段承载这四种
形态，因此一张表意味着每次插入都要做一次 schema 投影：对不适用的形态填空列，
再加一个决定用哪种形态的映射器。四张表让每种 OTLP 形态都能无需拷贝、无需投影
地插入。代价是当你想一次拿到全部指标时得写 `UNION ALL`，`/v1/diagnose` 内部
干的就是这件事。

`sigiro_anomalies` 是这个家族里的异类。它不存放遥测数据，存放的是某次定时扫描
已经检测出的状态突变，而 `GET /v1/anomalies` 以带类型的 JSON 提供同一批行。
[关于用证据取代仪表盘](/docs/explanation/evidence) 解释了这些行是如何产生的。

## 目前还没有 schema 端点，这正是短板

上面这些是你现在掌握、而 agent 并不掌握的知识。

sigiro 不发布任何 schema 描述。没有 `/v1/schema`，表上也没有列注释，所以
访问 `POST /v1/query` 的 agent 只能靠猜，或者读本站的 `llms-full.txt`。这两条路
成功的次数多到足以变得危险：对一个磁盘问题猜 `sys_disk_info()` 是一个_好_猜测，
而它产出的是一个错误答案。

正因如此，我们把这一点看作 API 中最有价值的短板，而不是一个便利性上的短板。
必须变成机器可读的那个区分不是「本地还是远程」，而是这个：`sys_*` 是对 sigiro
运行所在机器的一次实时测量，而 `sigiro_metrics_*` 是发送 OTLP 的一方产生的
历史遥测数据。

在那个端点出现之前，本页就是机器可读的版本。它以 Markdown 形式提供在
`/docs/explanation/tables.md`，并且列在 `llms.txt` 中。

## 一条每次都能套用的规则

先问这个问题说的是哪台机器。

- 关于你埋了点的某个服务 —— 用 `sigiro_*`，并限定 `timestamp`。
- 关于 sigiro 进程本身 —— 用 `sys_*`，并且只能得到某一瞬间。
- 不确定 —— 用 `sigiro_*`。空结果是诚实的。`sys_*` 的结果自信满满，但可能毫不相关。

## 接着读

- [关于用证据取代仪表盘](/docs/explanation/evidence)
- [向托管版 sigiro 发送遥测数据](/docs/how-to/hosted-onboarding#4-query-your-data-sql-api)
  —— 完整的表清单，以及适用于它的 SQL 规则
