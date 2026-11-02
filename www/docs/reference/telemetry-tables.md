---
title: Telemetry table columns
description: Current DuckDB column names, types, units, and pre-1.0 compatibility limits.
sidebar:
  order: 4
---

These are the seven queryable `sigiro_*` tables created by `src/schema.rs`. Types are DuckDB SQL types; `NOT NULL` is shown where declared. `lake.` qualification and bare table names are both used in queries. This is a reference for the **current** schema, not an API for discovering columns at runtime.

**Pre-1.0 compatibility:** column names, types, and JSON shapes may change before 1.0. This page does not promise stable on-disk DuckLake/Parquet formats, automatic in-place upgrades, or downgrade support. Storage-format validation and upgrade policy are not defined yet, and runtime schema discovery is not available yet.

`TIMESTAMP` columns are DuckDB timestamps at microsecond precision. Numeric epoch timestamps (`end_timestamp`, `observed_timestamp`, and metric `start_timestamp`) and span `duration` use **microseconds**, converted from OTLP nanoseconds. `events_json` and `exemplars_json` contain nested `time_unix_nano` fields that remain **nanoseconds**. Metric values and histogram bounds use the producer's `metric_unit`, not a fixed Sigiro unit; profile values use `unit`. A blank/zero epoch from a producer is not a guarantee that its start time is meaningful.

Attributes and JSON arrays are stored as **`VARCHAR` containing JSON**, not native JSON columns; use `json_extract_string(column, 'key')` for attribute keys. Keys depend on the sender and are not enumerated here. A nullable DDL declaration does not mean every current ingest path writes null.

## `sigiro_spans`

| Column                     | Type                 | Unit         | Meaning                            |
| -------------------------- | -------------------- | ------------ | ---------------------------------- |
| `timestamp`                | `TIMESTAMP NOT NULL` | µs precision | Event or data-point time.          |
| `end_timestamp`            | `BIGINT NOT NULL`    | epoch µs     | Span end time (epoch).             |
| `duration`                 | `BIGINT NOT NULL`    | µs           | Span elapsed time.                 |
| `trace_id`                 | `VARCHAR`            | —            | Trace ID (hex text).               |
| `span_id`                  | `VARCHAR`            | —            | Span ID (hex text).                |
| `parent_span_id`           | `VARCHAR`            | —            | Parent span ID (hex text).         |
| `trace_state`              | `VARCHAR`            | —            | W3C trace state.                   |
| `service_name`             | `VARCHAR NOT NULL`   | —            | OTel resource service.name.        |
| `service_namespace`        | `VARCHAR`            | —            | OTel resource service.namespace.   |
| `service_instance_id`      | `VARCHAR`            | —            | OTel resource service.instance.id. |
| `span_name`                | `VARCHAR NOT NULL`   | —            | Span operation name.               |
| `span_kind`                | `INTEGER NOT NULL`   | —            | OTLP span kind enum.               |
| `status_code`              | `INTEGER NOT NULL`   | —            | OTLP status code enum.             |
| `status_message`           | `VARCHAR`            | —            | Span status message.               |
| `resource_attributes`      | `VARCHAR`            | —            | Resource attributes as JSON text.  |
| `scope_name`               | `VARCHAR`            | —            | Instrumentation scope name.        |
| `scope_version`            | `VARCHAR`            | —            | Instrumentation scope version.     |
| `scope_attributes`         | `VARCHAR`            | —            | Scope attributes as JSON text.     |
| `span_attributes`          | `VARCHAR`            | —            | Span attributes as JSON text.      |
| `events_json`              | `VARCHAR`            | —            | Span events as JSON array text.    |
| `links_json`               | `VARCHAR`            | —            | Span links as JSON array text.     |
| `dropped_attributes_count` | `INTEGER`            | count        | Dropped span attribute count.      |
| `dropped_events_count`     | `INTEGER`            | count        | Dropped span event count.          |
| `dropped_links_count`      | `INTEGER`            | count        | Dropped span link count.           |
| `flags`                    | `INTEGER`            | —            | OTLP flags bitfield.               |

## `sigiro_logs`

| Column                | Type                 | Unit         | Meaning                                      |
| --------------------- | -------------------- | ------------ | -------------------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | Event or data-point time.                    |
| `observed_timestamp`  | `BIGINT NOT NULL`    | epoch µs     | Log observation time (epoch).                |
| `trace_id`            | `VARCHAR`            | —            | Trace ID (hex text).                         |
| `span_id`             | `VARCHAR`            | —            | Span ID (hex text).                          |
| `event_name`          | `VARCHAR`            | —            | Log event name.                              |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel resource service.name.                  |
| `service_namespace`   | `VARCHAR`            | —            | OTel resource service.namespace.             |
| `service_instance_id` | `VARCHAR`            | —            | OTel resource service.instance.id.           |
| `severity_number`     | `INTEGER NOT NULL`   | —            | OTLP severity number.                        |
| `severity_text`       | `VARCHAR NOT NULL`   | —            | Severity label.                              |
| `body`                | `VARCHAR`            | —            | Log body as text.                            |
| `resource_attributes` | `VARCHAR`            | —            | Resource attributes as JSON text.            |
| `scope_name`          | `VARCHAR`            | —            | Instrumentation scope name.                  |
| `scope_version`       | `VARCHAR`            | —            | Instrumentation scope version.               |
| `scope_attributes`    | `VARCHAR`            | —            | Scope attributes as JSON text.               |
| `log_attributes`      | `VARCHAR`            | —            | Log attributes as JSON text.                 |
| `template_id`         | `UBIGINT`            | —            | DRAIN log-template hash.                     |
| `template_values`     | `VARCHAR`            | —            | Original variable tokens as JSON array text. |
| `template_text`       | `VARCHAR`            | —            | DRAIN-classified log template.               |

## `sigiro_metrics_gauge`

| Column                | Type                 | Unit         | Meaning                               |
| --------------------- | -------------------- | ------------ | ------------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | Event or data-point time.             |
| `start_timestamp`     | `BIGINT`             | epoch µs     | Metric data-point start time (epoch). |
| `metric_name`         | `VARCHAR NOT NULL`   | —            | Metric name.                          |
| `metric_description`  | `VARCHAR`            | —            | Metric description.                   |
| `metric_unit`         | `VARCHAR`            | —            | Unit supplied by the metric producer. |
| `value`               | `DOUBLE NOT NULL`    | metric_unit  | Gauge data-point value.               |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel resource service.name.           |
| `service_namespace`   | `VARCHAR`            | —            | OTel resource service.namespace.      |
| `service_instance_id` | `VARCHAR`            | —            | OTel resource service.instance.id.    |
| `resource_attributes` | `VARCHAR`            | —            | Resource attributes as JSON text.     |
| `scope_name`          | `VARCHAR`            | —            | Instrumentation scope name.           |
| `scope_version`       | `VARCHAR`            | —            | Instrumentation scope version.        |
| `scope_attributes`    | `VARCHAR`            | —            | Scope attributes as JSON text.        |
| `metric_attributes`   | `VARCHAR`            | —            | Data-point attributes as JSON text.   |
| `flags`               | `INTEGER`            | —            | OTLP flags bitfield.                  |
| `exemplars_json`      | `VARCHAR`            | —            | Exemplars as JSON array text.         |

## `sigiro_metrics_sum`

| Column                    | Type                 | Unit         | Meaning                               |
| ------------------------- | -------------------- | ------------ | ------------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | Event or data-point time.             |
| `start_timestamp`         | `BIGINT`             | epoch µs     | Metric data-point start time (epoch). |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | Metric name.                          |
| `metric_description`      | `VARCHAR`            | —            | Metric description.                   |
| `metric_unit`             | `VARCHAR`            | —            | Unit supplied by the metric producer. |
| `value`                   | `DOUBLE NOT NULL`    | metric_unit  | Sum data-point value.                 |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel resource service.name.           |
| `service_namespace`       | `VARCHAR`            | —            | OTel resource service.namespace.      |
| `service_instance_id`     | `VARCHAR`            | —            | OTel resource service.instance.id.    |
| `resource_attributes`     | `VARCHAR`            | —            | Resource attributes as JSON text.     |
| `scope_name`              | `VARCHAR`            | —            | Instrumentation scope name.           |
| `scope_version`           | `VARCHAR`            | —            | Instrumentation scope version.        |
| `scope_attributes`        | `VARCHAR`            | —            | Scope attributes as JSON text.        |
| `metric_attributes`       | `VARCHAR`            | —            | Data-point attributes as JSON text.   |
| `flags`                   | `INTEGER`            | —            | OTLP flags bitfield.                  |
| `exemplars_json`          | `VARCHAR`            | —            | Exemplars as JSON array text.         |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP aggregation temporality enum.    |
| `is_monotonic`            | `BOOLEAN NOT NULL`   | —            | Whether the sum is monotonic.         |

## `sigiro_metrics_histogram`

| Column                    | Type                 | Unit         | Meaning                                                  |
| ------------------------- | -------------------- | ------------ | -------------------------------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | Event or data-point time.                                |
| `start_timestamp`         | `BIGINT`             | epoch µs     | Metric data-point start time (epoch).                    |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | Metric name.                                             |
| `metric_description`      | `VARCHAR`            | —            | Metric description.                                      |
| `metric_unit`             | `VARCHAR`            | —            | Unit supplied by the metric producer.                    |
| `count`                   | `BIGINT NOT NULL`    | samples      | Number of samples.                                       |
| `sum`                     | `DOUBLE`             | metric_unit  | Sum of recorded values.                                  |
| `min`                     | `DOUBLE`             | metric_unit  | Minimum recorded value.                                  |
| `max`                     | `DOUBLE`             | metric_unit  | Maximum recorded value.                                  |
| `bucket_counts`           | `VARCHAR NOT NULL`   | samples      | Explicit histogram bucket counts as JSON array text.     |
| `explicit_bounds`         | `VARCHAR NOT NULL`   | metric_unit  | Explicit histogram bucket boundaries as JSON array text. |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel resource service.name.                              |
| `service_namespace`       | `VARCHAR`            | —            | OTel resource service.namespace.                         |
| `service_instance_id`     | `VARCHAR`            | —            | OTel resource service.instance.id.                       |
| `resource_attributes`     | `VARCHAR`            | —            | Resource attributes as JSON text.                        |
| `scope_name`              | `VARCHAR`            | —            | Instrumentation scope name.                              |
| `scope_version`           | `VARCHAR`            | —            | Instrumentation scope version.                           |
| `scope_attributes`        | `VARCHAR`            | —            | Scope attributes as JSON text.                           |
| `metric_attributes`       | `VARCHAR`            | —            | Data-point attributes as JSON text.                      |
| `flags`                   | `INTEGER`            | —            | OTLP flags bitfield.                                     |
| `exemplars_json`          | `VARCHAR`            | —            | Exemplars as JSON array text.                            |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP aggregation temporality enum.                       |

## `sigiro_metrics_exp_histogram`

| Column                    | Type                 | Unit         | Meaning                                    |
| ------------------------- | -------------------- | ------------ | ------------------------------------------ |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | Event or data-point time.                  |
| `start_timestamp`         | `BIGINT`             | epoch µs     | Metric data-point start time (epoch).      |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | Metric name.                               |
| `metric_description`      | `VARCHAR`            | —            | Metric description.                        |
| `metric_unit`             | `VARCHAR`            | —            | Unit supplied by the metric producer.      |
| `count`                   | `BIGINT NOT NULL`    | samples      | Number of samples.                         |
| `sum`                     | `DOUBLE`             | metric_unit  | Sum of recorded values.                    |
| `min`                     | `DOUBLE`             | metric_unit  | Minimum recorded value.                    |
| `max`                     | `DOUBLE`             | metric_unit  | Maximum recorded value.                    |
| `scale`                   | `INTEGER NOT NULL`   | —            | Exponential histogram scale.               |
| `zero_count`              | `BIGINT NOT NULL`    | samples      | Count in zero bucket.                      |
| `zero_threshold`          | `DOUBLE`             | metric_unit  | Zero-bucket threshold.                     |
| `positive_offset`         | `INTEGER`            | —            | Positive bucket index offset.              |
| `positive_bucket_counts`  | `VARCHAR`            | samples      | Positive bucket counts as JSON array text. |
| `negative_offset`         | `INTEGER`            | —            | Negative bucket index offset.              |
| `negative_bucket_counts`  | `VARCHAR`            | samples      | Negative bucket counts as JSON array text. |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel resource service.name.                |
| `service_namespace`       | `VARCHAR`            | —            | OTel resource service.namespace.           |
| `service_instance_id`     | `VARCHAR`            | —            | OTel resource service.instance.id.         |
| `resource_attributes`     | `VARCHAR`            | —            | Resource attributes as JSON text.          |
| `scope_name`              | `VARCHAR`            | —            | Instrumentation scope name.                |
| `scope_version`           | `VARCHAR`            | —            | Instrumentation scope version.             |
| `scope_attributes`        | `VARCHAR`            | —            | Scope attributes as JSON text.             |
| `metric_attributes`       | `VARCHAR`            | —            | Data-point attributes as JSON text.        |
| `flags`                   | `INTEGER`            | —            | OTLP flags bitfield.                       |
| `exemplars_json`          | `VARCHAR`            | —            | Exemplars as JSON array text.              |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP aggregation temporality enum.         |

## `sigiro_profiles`

| Column                | Type                 | Unit         | Meaning                           |
| --------------------- | -------------------- | ------------ | --------------------------------- |
| `profile_id`          | `VARCHAR`            | —            | Profile ID.                       |
| `trace_id`            | `VARCHAR`            | —            | Trace ID (hex text).              |
| `span_id`             | `VARCHAR`            | —            | Span ID (hex text).               |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel resource service.name.       |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | Event or data-point time.         |
| `profile_type`        | `VARCHAR`            | —            | Profile sample type.              |
| `stack_trace`         | `VARCHAR[]`          | —            | Resolved stack frames in order.   |
| `value`               | `BIGINT`             | unit         | Sample value for this value type. |
| `unit`                | `VARCHAR`            | —            | Unit of the profile sample type.  |
| `resource_attributes` | `VARCHAR`            | —            | Resource attributes as JSON text. |
