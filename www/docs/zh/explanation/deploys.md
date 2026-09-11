---
title: 关于部署检测，以及你的 span 必须携带的信息
description: >-
  sigiro 只读取一个属性来判断部署——span
  资源上的 service.version。本页说明版本号如何变成部署事件，以及服务不发送版本号时会得到什么。
sidebar:
  order: 4
---
诊断结果可以点出一个可疑部署：_incident ~14:30 on service 'checkout':
… ; suspect deploy: version 1.4.2 first seen at 14:29_。这是一个很强的论断，
读者有理由追问它的依据从何而来。

它来自一个属性、一个位置。本页点明这个属性，说明 sigiro 如何把部署与稳定状态区分开，
解释一次部署如何成为那个可疑部署，并说明该属性缺失时你会得到什么。最后一部分最为重要，
因为这种缺失是无声的。

## 一个属性，一个位置

sigiro 从 **span 的资源属性** 中读取 `service.version`。

这就是全部来源。sigiro 不会为部署读取任何其他字段：

| 不用于部署判断的字段 | 为什么不用 |
| --- | --- |
| `deployment.environment`、`deployment.*` | 没有任何查询读取它们 |
| `git.commit.sha`、`vcs.*` | 没有任何查询读取它们 |
| 日志、指标或 profile 上的 `service.version` | sigiro 只扫描 `sigiro_spans` |
| CI webhook、部署 API、发布信息流 | 除 OTLP 之外，sigiro 没有任何接入通道 |
| Kubernetes API server | sigiro 从不连接它 |

资源属性通过 OTLP 到达，并原样落入 `sigiro_spans` 表的 `resource_attributes` 列，
存为一个 JSON 字符串。有三个资源属性在写入时拥有各自独立的列——`service.name`、
`service.namespace` 和 `service.instance.id`。`service.version` 不在其中，
因此 sigiro 会在每次查询时从 JSON 中把它读出来。

“只看 span”带来两个后果。一个导出日志但不导出 span 的服务不会产生任何部署事件，
无论它的日志有多好。一个只在部分进程中导出 span 的服务，只有当被埋点的那个进程
以新版本重启时，才会报告一次部署。

版本值本身对 sigiro 来说是不透明的。semver 字符串、git SHA、构建号和日期的作用完全相同，
因为 sigiro 只比较两个字符串是否相等，从不解析其中任何一个。sigiro 不会从版本号推导出任何顺序，
所以它无法区分升级和回滚。

## 部署就是基线期没有出现过的版本

sigiro 没有可读取的部署时间戳，所以它自己推导一个。部署事件是这样一个 `service.version` 值：
它出现在诊断时间窗内的某个 span 上，且在基线时间窗内的 **任何** span 上都没有出现过。

基线时间窗的长度是诊断时间窗的四倍，并且它的结束点就是诊断时间窗的起点。
默认的诊断时间窗是最近 15 分钟，因此默认基线是它之前的那一小时。

每个部署事件携带：

- `event_type` —— 字面字符串 `deploy`
- `detected_at` —— 时间窗内携带该版本的最早 span 时间戳，也就是 sigiro 第一次 _看到_
  该版本的时刻，而不是你的部署开始的时刻
- `description` —— `version 1.4.2 first seen`

与基线的比较是其中重要的那一半，它修正了一个真实的失效。如果没有它，“首次看到”就意味着
时间窗的第一行数据，于是每一次对每一个健康服务的诊断都会在窗口起点报告一次部署。
那个事件永远存在，永远在时间上紧邻故障，而读到它的 agent 会把它当成原因。
一条永久的错误线索比没有线索更糟，所以在时间窗开始之前就已在运行的版本算作稳定状态。

由此还直接引出五个后果，每一个都会让某些人意外：

- **恒定的版本号什么也产生不了。** 如果每个构建都发布 `service.version=1.0.0`，
  sigiro 只会报告一次部署事件，之后再也不会。要让部署可见，就给每次发布一个不同的值。
- **多个新版本产生多个事件。** 查询按版本分组，所以一个包含三个新版本的时间窗会返回三个部署事件。
- **回滚会被读作部署。** 旧版本在基线时间窗中不存在，因此按此定义它是新的。
- **遥测数据的空档会被读作部署。** 如果某个服务在基线时间窗内没有发送任何 span，
  基线集合就是空的，于是窗口内的每一个版本都是新的。当服务是新上线的或曾经很安静时，
  要谨慎解读窗口起点处的部署。
- **更宽的时间窗会看得更远。** sigiro 从你请求的时间窗推导基线，所以一次 6 小时的诊断
  会与之前的 24 小时作比较。

## Kubernetes 事件来自日志文本

同一个 `events` 数组还承载第二种事件类型 `k8s_event`，它有不同的来源。

sigiro 在时间窗内扫描该服务日志的 `body` 列，查找五个固定子串：`OOMKilled`、
`Unhealthy`、`BackOff`、`FailedScheduling` 和 `ScalingReplicaSet`。匹配区分大小写，
并且是纯粹的子串匹配。description 就是整条日志正文，且 sigiro 每次诊断最多返回 20 条这类事件。

所以这并不是 Kubernetes 集成。sigiro 从不与 API server 通信。
只有当某个组件把集群事件转发进你的 OTLP 日志、并使用与你所诊断服务相同的 `service.name` 时，
你才会看到这些事件。通常的做法是用一个 collector 监听事件并将其作为日志导出。
当没有任何组件转发它们时，这个数组里只有部署事件。

`k8s_event` 永远不会成为可疑部署。只有类型为 `deploy` 的事件才有资格。

## 一次部署如何成为可疑部署

由两条规则决定，两者都源自检测器自身的 5 分钟分桶。

第一，只有 **incident** 才会有可疑部署。一个与其他任何偏移都不相关的单一偏移，
仍然只是单信号条目，完全不携带部署字段。位于同一个桶或相邻桶中的两个或更多偏移会合并成一个 incident，
只有这样的条目才会去寻找部署。[关于用证据取代仪表盘](/docs/explanation/evidence)
解释了偏移为什么会那样关联。

第二，当一次部署所在的桶落在该 incident 自身的桶区间内，或落在第一个偏移之前紧邻的那一个桶中时，
它就符合条件。多出的那个桶是算术结果，而不是容差：一次发生在 14:29 的部署导致了在 14:30 桶中
检测到的偏移，按构造它就是早一个桶。当有多次部署符合条件时，时间上最接近 incident 起点的那一次胜出。

胜出者会出现两次：作为 incident 上的 `deploy` 对象，以及作为条目摘要末尾的一个子句。
其他所有部署事件都留在该服务的 `events` 数组中，sigiro 会按时间对其排序，且从不过滤。
所以在比较中落选的部署仍然在响应里，供你查看。

sigiro 只主张时间上的相关性，仅此而已。用 _suspect_（可疑）这个词是有意的。

## 服务不设置版本号时会发生什么

直白地说：你得不到任何部署事件，而 sigiro 对此什么也不会告诉你。

查询会丢弃 `service.version` 为 null 的行，所以一个从不设置该属性的服务不会贡献任何部署事件。
此时 `events` 数组里只有 Kubernetes 事件，或者干脆是空的。每个 incident 都携带 `deploy: null`，
摘要结尾也没有可疑部署子句。

没有任何条目会提醒你。sigiro 会针对缺失的 `k8s.namespace.name` 和 `k8s.deployment.name`
产生一个 `missing_resource_attributes` 类型的条目，而那个条目对 `service.version` 一言不发。
版本号没有对应的机制。这种降级是无声的。

这是响应中唯一一处你不能照字面理解的部分。一个没有部署事件的时间窗，在两种截然不同的情形下
看起来完全一样：没有发生过部署，以及没有任何进程设置 `service.version`。响应无法区分二者。

有一个查询可以区分它们：

```sql
SELECT DISTINCT resource_attributes::JSON->>'service.version' AS version
FROM sigiro_spans
WHERE service_name = 'checkout'
  AND timestamp > now() - INTERVAL '24 hours';
```

按下面的方式解读结果：

- 只有一行，且为 `NULL` —— 没有任何东西设置该属性。sigiro 永远无法为这个服务报告部署。
- 只有一行且带有值，并且多天不变 —— 有东西设置了该属性，但值从不改变。sigiro 在第一次看到
  这个值时报告过一次部署，此后不再报告。
- 有多行 —— 部署检测对这个服务是有效的。

在你服务的 OpenTelemetry 资源上设置该属性，并为每次发布赋予一个不同的值。
[用 agent 为你的代码埋点](/docs/how-to/install-with-ai) 涵盖资源本身的配置。

## 继续阅读

- [关于用证据取代仪表盘](/docs/explanation/evidence) —— 一个偏移如何成为 incident，
  以及为什么每一行都携带一条查询
- [关于这些表，以及它们各自描述哪台机器](/docs/explanation/tables) ——
  `resource_attributes` 存放在哪里，以及哪张表回答哪个问题
- [API 参考](/docs/reference) —— `DerivedEvent` 与 `Incident` 字段的完整说明
