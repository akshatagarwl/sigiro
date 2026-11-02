---
title: 遥测表字段
description: 当前 DuckDB 字段名、类型、单位及 1.0 前的兼容性说明。
sidebar:
  order: 4
---

以下是 `src/schema.rs` 创建的七个可查询的 `sigiro_*` 表。类型为 DuckDB SQL 类型，已声明的 `NOT NULL` 会明确标出。查询中可以使用 `lake.` 前缀或简短表名。这是**当前**结构的参考，不是运行时获取字段的 API。

**1.0 前的兼容性：**字段名、类型及 JSON 结构在 1.0 前可能变化。本文不承诺 DuckLake/Parquet 磁盘格式稳定、自动原地升级或支持降级。存储格式校验和升级策略尚未确定，运行时字段发现也尚未提供。

`TIMESTAMP` 是微秒精度的 DuckDB 时间戳。数值纪元时间（`end_timestamp`、`observed_timestamp`、指标的 `start_timestamp`）和跨度的 `duration` 使用由 OTLP 纳秒转换的**微秒**。但 `events_json` 和 `exemplars_json` 内部的 `time_unix_nano` 仍是**纳秒**。指标值与直方图边界的单位由发送方的 `metric_unit` 决定；剖析值使用 `unit`。

属性和 JSON 数组并非原生 JSON 列，而是**包含 JSON 的 `VARCHAR`**。用 `json_extract_string(column, 'key')` 查询属性键。键由发送方决定，不在此逐一列出。DDL 允许为空不代表当前写入路径必定写入 null。

## `sigiro_spans`

| 字段                       | 类型                 | 单位         | 含义                              |
| -------------------------- | -------------------- | ------------ | --------------------------------- |
| `timestamp`                | `TIMESTAMP NOT NULL` | µs precision | 事件或数据点时间。                |
| `end_timestamp`            | `BIGINT NOT NULL`    | epoch µs     | 跨度结束时间。                    |
| `duration`                 | `BIGINT NOT NULL`    | µs           | 跨度持续时间。                    |
| `trace_id`                 | `VARCHAR`            | —            | 追踪 ID（十六进制文本）。         |
| `span_id`                  | `VARCHAR`            | —            | 跨度 ID（十六进制文本）。         |
| `parent_span_id`           | `VARCHAR`            | —            | 父跨度 ID（十六进制文本）。       |
| `trace_state`              | `VARCHAR`            | —            | W3C 追踪状态。                    |
| `service_name`             | `VARCHAR NOT NULL`   | —            | OTel 资源的 service.name。        |
| `service_namespace`        | `VARCHAR`            | —            | OTel 资源的 service.namespace。   |
| `service_instance_id`      | `VARCHAR`            | —            | OTel 资源的 service.instance.id。 |
| `span_name`                | `VARCHAR NOT NULL`   | —            | 跨度操作名称。                    |
| `span_kind`                | `INTEGER NOT NULL`   | —            | OTLP 跨度类型枚举值。             |
| `status_code`              | `INTEGER NOT NULL`   | —            | OTLP 状态码枚举值。               |
| `status_message`           | `VARCHAR`            | —            | 跨度状态消息。                    |
| `resource_attributes`      | `VARCHAR`            | —            | 资源属性的 JSON 文本。            |
| `scope_name`               | `VARCHAR`            | —            | 插桩范围名称。                    |
| `scope_version`            | `VARCHAR`            | —            | 插桩范围版本。                    |
| `scope_attributes`         | `VARCHAR`            | —            | 范围属性的 JSON 文本。            |
| `span_attributes`          | `VARCHAR`            | —            | 跨度属性的 JSON 文本。            |
| `events_json`              | `VARCHAR`            | —            | 跨度事件的 JSON 数组文本。        |
| `links_json`               | `VARCHAR`            | —            | 跨度链接的 JSON 数组文本。        |
| `dropped_attributes_count` | `INTEGER`            | count        | 被丢弃的跨度属性数量。            |
| `dropped_events_count`     | `INTEGER`            | count        | 被丢弃的跨度事件数量。            |
| `dropped_links_count`      | `INTEGER`            | count        | 被丢弃的跨度链接数量。            |
| `flags`                    | `INTEGER`            | —            | OTLP 标志位字段。                 |

## `sigiro_logs`

| 字段                  | 类型                 | 单位         | 含义                              |
| --------------------- | -------------------- | ------------ | --------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | 事件或数据点时间。                |
| `observed_timestamp`  | `BIGINT NOT NULL`    | epoch µs     | 日志观测时间。                    |
| `trace_id`            | `VARCHAR`            | —            | 追踪 ID（十六进制文本）。         |
| `span_id`             | `VARCHAR`            | —            | 跨度 ID（十六进制文本）。         |
| `event_name`          | `VARCHAR`            | —            | 日志事件名称。                    |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel 资源的 service.name。        |
| `service_namespace`   | `VARCHAR`            | —            | OTel 资源的 service.namespace。   |
| `service_instance_id` | `VARCHAR`            | —            | OTel 资源的 service.instance.id。 |
| `severity_number`     | `INTEGER NOT NULL`   | —            | OTLP 严重程度编号。               |
| `severity_text`       | `VARCHAR NOT NULL`   | —            | 严重程度标签。                    |
| `body`                | `VARCHAR`            | —            | 日志正文文本。                    |
| `resource_attributes` | `VARCHAR`            | —            | 资源属性的 JSON 文本。            |
| `scope_name`          | `VARCHAR`            | —            | 插桩范围名称。                    |
| `scope_version`       | `VARCHAR`            | —            | 插桩范围版本。                    |
| `scope_attributes`    | `VARCHAR`            | —            | 范围属性的 JSON 文本。            |
| `log_attributes`      | `VARCHAR`            | —            | 日志属性的 JSON 文本。            |
| `template_id`         | `UBIGINT`            | —            | DRAIN 日志模板哈希。              |
| `template_values`     | `VARCHAR`            | —            | 原始变量词元的 JSON 数组文本。    |
| `template_text`       | `VARCHAR`            | —            | DRAIN 分类的日志模板。            |

## `sigiro_metrics_gauge`

| 字段                  | 类型                 | 单位         | 含义                              |
| --------------------- | -------------------- | ------------ | --------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | 事件或数据点时间。                |
| `start_timestamp`     | `BIGINT`             | epoch µs     | 指标数据点的开始时间。            |
| `metric_name`         | `VARCHAR NOT NULL`   | —            | 指标名称。                        |
| `metric_description`  | `VARCHAR`            | —            | 指标描述。                        |
| `metric_unit`         | `VARCHAR`            | —            | 发送方提供的单位。                |
| `value`               | `DOUBLE NOT NULL`    | metric_unit  | 仪表数据点的值。                  |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel 资源的 service.name。        |
| `service_namespace`   | `VARCHAR`            | —            | OTel 资源的 service.namespace。   |
| `service_instance_id` | `VARCHAR`            | —            | OTel 资源的 service.instance.id。 |
| `resource_attributes` | `VARCHAR`            | —            | 资源属性的 JSON 文本。            |
| `scope_name`          | `VARCHAR`            | —            | 插桩范围名称。                    |
| `scope_version`       | `VARCHAR`            | —            | 插桩范围版本。                    |
| `scope_attributes`    | `VARCHAR`            | —            | 范围属性的 JSON 文本。            |
| `metric_attributes`   | `VARCHAR`            | —            | 数据点属性的 JSON 文本。          |
| `flags`               | `INTEGER`            | —            | OTLP 标志位字段。                 |
| `exemplars_json`      | `VARCHAR`            | —            | 范例值的 JSON 数组文本。          |

## `sigiro_metrics_sum`

| 字段                      | 类型                 | 单位         | 含义                              |
| ------------------------- | -------------------- | ------------ | --------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | 事件或数据点时间。                |
| `start_timestamp`         | `BIGINT`             | epoch µs     | 指标数据点的开始时间。            |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | 指标名称。                        |
| `metric_description`      | `VARCHAR`            | —            | 指标描述。                        |
| `metric_unit`             | `VARCHAR`            | —            | 发送方提供的单位。                |
| `value`                   | `DOUBLE NOT NULL`    | metric_unit  | 求和数据点的值。                  |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel 资源的 service.name。        |
| `service_namespace`       | `VARCHAR`            | —            | OTel 资源的 service.namespace。   |
| `service_instance_id`     | `VARCHAR`            | —            | OTel 资源的 service.instance.id。 |
| `resource_attributes`     | `VARCHAR`            | —            | 资源属性的 JSON 文本。            |
| `scope_name`              | `VARCHAR`            | —            | 插桩范围名称。                    |
| `scope_version`           | `VARCHAR`            | —            | 插桩范围版本。                    |
| `scope_attributes`        | `VARCHAR`            | —            | 范围属性的 JSON 文本。            |
| `metric_attributes`       | `VARCHAR`            | —            | 数据点属性的 JSON 文本。          |
| `flags`                   | `INTEGER`            | —            | OTLP 标志位字段。                 |
| `exemplars_json`          | `VARCHAR`            | —            | 范例值的 JSON 数组文本。          |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 聚合时间性枚举值。           |
| `is_monotonic`            | `BOOLEAN NOT NULL`   | —            | 该求和是否单调递增。              |

## `sigiro_metrics_histogram`

| 字段                      | 类型                 | 单位         | 含义                               |
| ------------------------- | -------------------- | ------------ | ---------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | 事件或数据点时间。                 |
| `start_timestamp`         | `BIGINT`             | epoch µs     | 指标数据点的开始时间。             |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | 指标名称。                         |
| `metric_description`      | `VARCHAR`            | —            | 指标描述。                         |
| `metric_unit`             | `VARCHAR`            | —            | 发送方提供的单位。                 |
| `count`                   | `BIGINT NOT NULL`    | samples      | 样本数量。                         |
| `sum`                     | `DOUBLE`             | metric_unit  | 记录值之和。                       |
| `min`                     | `DOUBLE`             | metric_unit  | 记录值的最小值。                   |
| `max`                     | `DOUBLE`             | metric_unit  | 记录值的最大值。                   |
| `bucket_counts`           | `VARCHAR NOT NULL`   | samples      | 显式直方图桶计数的 JSON 数组文本。 |
| `explicit_bounds`         | `VARCHAR NOT NULL`   | metric_unit  | 显式直方图桶边界的 JSON 数组文本。 |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel 资源的 service.name。         |
| `service_namespace`       | `VARCHAR`            | —            | OTel 资源的 service.namespace。    |
| `service_instance_id`     | `VARCHAR`            | —            | OTel 资源的 service.instance.id。  |
| `resource_attributes`     | `VARCHAR`            | —            | 资源属性的 JSON 文本。             |
| `scope_name`              | `VARCHAR`            | —            | 插桩范围名称。                     |
| `scope_version`           | `VARCHAR`            | —            | 插桩范围版本。                     |
| `scope_attributes`        | `VARCHAR`            | —            | 范围属性的 JSON 文本。             |
| `metric_attributes`       | `VARCHAR`            | —            | 数据点属性的 JSON 文本。           |
| `flags`                   | `INTEGER`            | —            | OTLP 标志位字段。                  |
| `exemplars_json`          | `VARCHAR`            | —            | 范例值的 JSON 数组文本。           |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 聚合时间性枚举值。            |

## `sigiro_metrics_exp_histogram`

| 字段                      | 类型                 | 单位         | 含义                              |
| ------------------------- | -------------------- | ------------ | --------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | 事件或数据点时间。                |
| `start_timestamp`         | `BIGINT`             | epoch µs     | 指标数据点的开始时间。            |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | 指标名称。                        |
| `metric_description`      | `VARCHAR`            | —            | 指标描述。                        |
| `metric_unit`             | `VARCHAR`            | —            | 发送方提供的单位。                |
| `count`                   | `BIGINT NOT NULL`    | samples      | 样本数量。                        |
| `sum`                     | `DOUBLE`             | metric_unit  | 记录值之和。                      |
| `min`                     | `DOUBLE`             | metric_unit  | 记录值的最小值。                  |
| `max`                     | `DOUBLE`             | metric_unit  | 记录值的最大值。                  |
| `scale`                   | `INTEGER NOT NULL`   | —            | 指数直方图的尺度。                |
| `zero_count`              | `BIGINT NOT NULL`    | samples      | 零值桶计数。                      |
| `zero_threshold`          | `DOUBLE`             | metric_unit  | 零值桶阈值。                      |
| `positive_offset`         | `INTEGER`            | —            | 正值桶索引偏移。                  |
| `positive_bucket_counts`  | `VARCHAR`            | samples      | 正值桶计数的 JSON 数组文本。      |
| `negative_offset`         | `INTEGER`            | —            | 负值桶索引偏移。                  |
| `negative_bucket_counts`  | `VARCHAR`            | samples      | 负值桶计数的 JSON 数组文本。      |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel 资源的 service.name。        |
| `service_namespace`       | `VARCHAR`            | —            | OTel 资源的 service.namespace。   |
| `service_instance_id`     | `VARCHAR`            | —            | OTel 资源的 service.instance.id。 |
| `resource_attributes`     | `VARCHAR`            | —            | 资源属性的 JSON 文本。            |
| `scope_name`              | `VARCHAR`            | —            | 插桩范围名称。                    |
| `scope_version`           | `VARCHAR`            | —            | 插桩范围版本。                    |
| `scope_attributes`        | `VARCHAR`            | —            | 范围属性的 JSON 文本。            |
| `metric_attributes`       | `VARCHAR`            | —            | 数据点属性的 JSON 文本。          |
| `flags`                   | `INTEGER`            | —            | OTLP 标志位字段。                 |
| `exemplars_json`          | `VARCHAR`            | —            | 范例值的 JSON 数组文本。          |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 聚合时间性枚举值。           |

## `sigiro_profiles`

| 字段                  | 类型                 | 单位         | 含义                       |
| --------------------- | -------------------- | ------------ | -------------------------- |
| `profile_id`          | `VARCHAR`            | —            | 剖析 ID。                  |
| `trace_id`            | `VARCHAR`            | —            | 追踪 ID（十六进制文本）。  |
| `span_id`             | `VARCHAR`            | —            | 跨度 ID（十六进制文本）。  |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel 资源的 service.name。 |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | 事件或数据点时间。         |
| `profile_type`        | `VARCHAR`            | —            | 剖析样本类型。             |
| `stack_trace`         | `VARCHAR[]`          | —            | 按顺序解析的堆栈帧。       |
| `value`               | `BIGINT`             | unit         | 该值类型的样本值。         |
| `unit`                | `VARCHAR`            | —            | 剖析样本类型的单位。       |
| `resource_attributes` | `VARCHAR`            | —            | 资源属性的 JSON 文本。     |
