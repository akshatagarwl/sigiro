---
title: テレメトリテーブルの列
description: 現在の DuckDB 列名、型、単位、1.0 前の互換性。
sidebar:
  order: 4
---

以下は `src/schema.rs` が作成する、問い合わせ可能な 7 個の `sigiro_*` テーブルです。型は DuckDB SQL 型で、宣言された `NOT NULL` を表示します。クエリでは `lake.` 修飾名と短いテーブル名の両方を使用できます。これは**現在**のスキーマの説明であり、実行時のスキーマ検出 API ではありません。

**1.0 前の互換性:** 列名・型・JSON の形は 1.0 までに変更される可能性があります。このページは DuckLake/Parquet のディスク形式の安定性、自動インプレース更新、ダウングレード対応を保証しません。ストレージ形式の検証と更新方針はまだ決まっておらず、実行時のスキーマ検出もまだ提供していません。

`TIMESTAMP` はマイクロ秒精度の DuckDB 時刻です。数値エポック時刻（`end_timestamp`、`observed_timestamp`、メトリクスの `start_timestamp`）とスパンの `duration` は OTLP のナノ秒から変換した**マイクロ秒**です。一方、`events_json` と `exemplars_json` 内の `time_unix_nano` は**ナノ秒**のままです。メトリクス値とヒストグラム境界の単位は送信元の `metric_unit`、プロファイル値は `unit` に従います。

属性と JSON 配列はネイティブ JSON 型ではなく、**JSON を含む `VARCHAR`** です。属性キーは `json_extract_string(column, 'key')` で参照してください。キーの集合は送信元ごとに異なります。DDL で nullable と宣言されていても、現在の取り込み処理が常に null を書くわけではありません。

## `sigiro_spans`

| 列                         | 型                   | 単位         | 意味                                  |
| -------------------------- | -------------------- | ------------ | ------------------------------------- |
| `timestamp`                | `TIMESTAMP NOT NULL` | µs precision | イベントまたはデータ点の時刻。        |
| `end_timestamp`            | `BIGINT NOT NULL`    | epoch µs     | スパン終了時刻。                      |
| `duration`                 | `BIGINT NOT NULL`    | µs           | スパンの経過時間。                    |
| `trace_id`                 | `VARCHAR`            | —            | トレース ID（16 進文字列）。          |
| `span_id`                  | `VARCHAR`            | —            | スパン ID（16 進文字列）。            |
| `parent_span_id`           | `VARCHAR`            | —            | 親スパン ID（16 進文字列）。          |
| `trace_state`              | `VARCHAR`            | —            | W3C トレース状態。                    |
| `service_name`             | `VARCHAR NOT NULL`   | —            | OTel リソースの service.name。        |
| `service_namespace`        | `VARCHAR`            | —            | OTel リソースの service.namespace。   |
| `service_instance_id`      | `VARCHAR`            | —            | OTel リソースの service.instance.id。 |
| `span_name`                | `VARCHAR NOT NULL`   | —            | スパン操作名。                        |
| `span_kind`                | `INTEGER NOT NULL`   | —            | OTLP スパン種別の列挙値。             |
| `status_code`              | `INTEGER NOT NULL`   | —            | OTLP ステータスコードの列挙値。       |
| `status_message`           | `VARCHAR`            | —            | スパンのステータスメッセージ。        |
| `resource_attributes`      | `VARCHAR`            | —            | リソース属性の JSON 文字列。          |
| `scope_name`               | `VARCHAR`            | —            | 計装スコープ名。                      |
| `scope_version`            | `VARCHAR`            | —            | 計装スコープのバージョン。            |
| `scope_attributes`         | `VARCHAR`            | —            | スコープ属性の JSON 文字列。          |
| `span_attributes`          | `VARCHAR`            | —            | スパン属性の JSON 文字列。            |
| `events_json`              | `VARCHAR`            | —            | スパンイベントの JSON 配列文字列。    |
| `links_json`               | `VARCHAR`            | —            | スパンリンクの JSON 配列文字列。      |
| `dropped_attributes_count` | `INTEGER`            | count        | 破棄されたスパン属性の数。            |
| `dropped_events_count`     | `INTEGER`            | count        | 破棄されたスパンイベントの数。        |
| `dropped_links_count`      | `INTEGER`            | count        | 破棄されたスパンリンクの数。          |
| `flags`                    | `INTEGER`            | —            | OTLP フラグのビットフィールド。       |

## `sigiro_logs`

| 列                    | 型                   | 単位         | 意味                                  |
| --------------------- | -------------------- | ------------ | ------------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | イベントまたはデータ点の時刻。        |
| `observed_timestamp`  | `BIGINT NOT NULL`    | epoch µs     | ログ観測時刻。                        |
| `trace_id`            | `VARCHAR`            | —            | トレース ID（16 進文字列）。          |
| `span_id`             | `VARCHAR`            | —            | スパン ID（16 進文字列）。            |
| `event_name`          | `VARCHAR`            | —            | ログイベント名。                      |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel リソースの service.name。        |
| `service_namespace`   | `VARCHAR`            | —            | OTel リソースの service.namespace。   |
| `service_instance_id` | `VARCHAR`            | —            | OTel リソースの service.instance.id。 |
| `severity_number`     | `INTEGER NOT NULL`   | —            | OTLP 重大度番号。                     |
| `severity_text`       | `VARCHAR NOT NULL`   | —            | 重大度ラベル。                        |
| `body`                | `VARCHAR`            | —            | ログ本文の文字列。                    |
| `resource_attributes` | `VARCHAR`            | —            | リソース属性の JSON 文字列。          |
| `scope_name`          | `VARCHAR`            | —            | 計装スコープ名。                      |
| `scope_version`       | `VARCHAR`            | —            | 計装スコープのバージョン。            |
| `scope_attributes`    | `VARCHAR`            | —            | スコープ属性の JSON 文字列。          |
| `log_attributes`      | `VARCHAR`            | —            | ログ属性の JSON 文字列。              |
| `template_id`         | `UBIGINT`            | —            | DRAIN ログテンプレートのハッシュ。    |
| `template_values`     | `VARCHAR`            | —            | 元の変数トークンの JSON 配列文字列。  |
| `template_text`       | `VARCHAR`            | —            | DRAIN が分類したログテンプレート。    |

## `sigiro_metrics_gauge`

| 列                    | 型                   | 単位         | 意味                                  |
| --------------------- | -------------------- | ------------ | ------------------------------------- |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | イベントまたはデータ点の時刻。        |
| `start_timestamp`     | `BIGINT`             | epoch µs     | メトリクスデータ点の開始時刻。        |
| `metric_name`         | `VARCHAR NOT NULL`   | —            | メトリクス名。                        |
| `metric_description`  | `VARCHAR`            | —            | メトリクスの説明。                    |
| `metric_unit`         | `VARCHAR`            | —            | 送信元が指定した単位。                |
| `value`               | `DOUBLE NOT NULL`    | metric_unit  | ゲージのデータ点値。                  |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel リソースの service.name。        |
| `service_namespace`   | `VARCHAR`            | —            | OTel リソースの service.namespace。   |
| `service_instance_id` | `VARCHAR`            | —            | OTel リソースの service.instance.id。 |
| `resource_attributes` | `VARCHAR`            | —            | リソース属性の JSON 文字列。          |
| `scope_name`          | `VARCHAR`            | —            | 計装スコープ名。                      |
| `scope_version`       | `VARCHAR`            | —            | 計装スコープのバージョン。            |
| `scope_attributes`    | `VARCHAR`            | —            | スコープ属性の JSON 文字列。          |
| `metric_attributes`   | `VARCHAR`            | —            | データ点属性の JSON 文字列。          |
| `flags`               | `INTEGER`            | —            | OTLP フラグのビットフィールド。       |
| `exemplars_json`      | `VARCHAR`            | —            | エグザンプラーの JSON 配列文字列。    |

## `sigiro_metrics_sum`

| 列                        | 型                   | 単位         | 意味                                  |
| ------------------------- | -------------------- | ------------ | ------------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | イベントまたはデータ点の時刻。        |
| `start_timestamp`         | `BIGINT`             | epoch µs     | メトリクスデータ点の開始時刻。        |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | メトリクス名。                        |
| `metric_description`      | `VARCHAR`            | —            | メトリクスの説明。                    |
| `metric_unit`             | `VARCHAR`            | —            | 送信元が指定した単位。                |
| `value`                   | `DOUBLE NOT NULL`    | metric_unit  | Sum のデータ点値。                    |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel リソースの service.name。        |
| `service_namespace`       | `VARCHAR`            | —            | OTel リソースの service.namespace。   |
| `service_instance_id`     | `VARCHAR`            | —            | OTel リソースの service.instance.id。 |
| `resource_attributes`     | `VARCHAR`            | —            | リソース属性の JSON 文字列。          |
| `scope_name`              | `VARCHAR`            | —            | 計装スコープ名。                      |
| `scope_version`           | `VARCHAR`            | —            | 計装スコープのバージョン。            |
| `scope_attributes`        | `VARCHAR`            | —            | スコープ属性の JSON 文字列。          |
| `metric_attributes`       | `VARCHAR`            | —            | データ点属性の JSON 文字列。          |
| `flags`                   | `INTEGER`            | —            | OTLP フラグのビットフィールド。       |
| `exemplars_json`          | `VARCHAR`            | —            | エグザンプラーの JSON 配列文字列。    |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 集約時間性の列挙値。             |
| `is_monotonic`            | `BOOLEAN NOT NULL`   | —            | Sum が単調増加かどうか。              |

## `sigiro_metrics_histogram`

| 列                        | 型                   | 単位         | 意味                                                  |
| ------------------------- | -------------------- | ------------ | ----------------------------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | イベントまたはデータ点の時刻。                        |
| `start_timestamp`         | `BIGINT`             | epoch µs     | メトリクスデータ点の開始時刻。                        |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | メトリクス名。                                        |
| `metric_description`      | `VARCHAR`            | —            | メトリクスの説明。                                    |
| `metric_unit`             | `VARCHAR`            | —            | 送信元が指定した単位。                                |
| `count`                   | `BIGINT NOT NULL`    | samples      | サンプル数。                                          |
| `sum`                     | `DOUBLE`             | metric_unit  | 記録値の合計。                                        |
| `min`                     | `DOUBLE`             | metric_unit  | 記録値の最小値。                                      |
| `max`                     | `DOUBLE`             | metric_unit  | 記録値の最大値。                                      |
| `bucket_counts`           | `VARCHAR NOT NULL`   | samples      | 明示的ヒストグラムのバケット計数（JSON 配列文字列）。 |
| `explicit_bounds`         | `VARCHAR NOT NULL`   | metric_unit  | 明示的ヒストグラムの境界（JSON 配列文字列）。         |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel リソースの service.name。                        |
| `service_namespace`       | `VARCHAR`            | —            | OTel リソースの service.namespace。                   |
| `service_instance_id`     | `VARCHAR`            | —            | OTel リソースの service.instance.id。                 |
| `resource_attributes`     | `VARCHAR`            | —            | リソース属性の JSON 文字列。                          |
| `scope_name`              | `VARCHAR`            | —            | 計装スコープ名。                                      |
| `scope_version`           | `VARCHAR`            | —            | 計装スコープのバージョン。                            |
| `scope_attributes`        | `VARCHAR`            | —            | スコープ属性の JSON 文字列。                          |
| `metric_attributes`       | `VARCHAR`            | —            | データ点属性の JSON 文字列。                          |
| `flags`                   | `INTEGER`            | —            | OTLP フラグのビットフィールド。                       |
| `exemplars_json`          | `VARCHAR`            | —            | エグザンプラーの JSON 配列文字列。                    |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 集約時間性の列挙値。                             |

## `sigiro_metrics_exp_histogram`

| 列                        | 型                   | 単位         | 意味                                  |
| ------------------------- | -------------------- | ------------ | ------------------------------------- |
| `timestamp`               | `TIMESTAMP NOT NULL` | µs precision | イベントまたはデータ点の時刻。        |
| `start_timestamp`         | `BIGINT`             | epoch µs     | メトリクスデータ点の開始時刻。        |
| `metric_name`             | `VARCHAR NOT NULL`   | —            | メトリクス名。                        |
| `metric_description`      | `VARCHAR`            | —            | メトリクスの説明。                    |
| `metric_unit`             | `VARCHAR`            | —            | 送信元が指定した単位。                |
| `count`                   | `BIGINT NOT NULL`    | samples      | サンプル数。                          |
| `sum`                     | `DOUBLE`             | metric_unit  | 記録値の合計。                        |
| `min`                     | `DOUBLE`             | metric_unit  | 記録値の最小値。                      |
| `max`                     | `DOUBLE`             | metric_unit  | 記録値の最大値。                      |
| `scale`                   | `INTEGER NOT NULL`   | —            | 指数ヒストグラムのスケール。          |
| `zero_count`              | `BIGINT NOT NULL`    | samples      | ゼロバケットの計数。                  |
| `zero_threshold`          | `DOUBLE`             | metric_unit  | ゼロバケットの閾値。                  |
| `positive_offset`         | `INTEGER`            | —            | 正側のバケット番号オフセット。        |
| `positive_bucket_counts`  | `VARCHAR`            | samples      | 正側バケット計数の JSON 配列文字列。  |
| `negative_offset`         | `INTEGER`            | —            | 負側のバケット番号オフセット。        |
| `negative_bucket_counts`  | `VARCHAR`            | samples      | 負側バケット計数の JSON 配列文字列。  |
| `service_name`            | `VARCHAR NOT NULL`   | —            | OTel リソースの service.name。        |
| `service_namespace`       | `VARCHAR`            | —            | OTel リソースの service.namespace。   |
| `service_instance_id`     | `VARCHAR`            | —            | OTel リソースの service.instance.id。 |
| `resource_attributes`     | `VARCHAR`            | —            | リソース属性の JSON 文字列。          |
| `scope_name`              | `VARCHAR`            | —            | 計装スコープ名。                      |
| `scope_version`           | `VARCHAR`            | —            | 計装スコープのバージョン。            |
| `scope_attributes`        | `VARCHAR`            | —            | スコープ属性の JSON 文字列。          |
| `metric_attributes`       | `VARCHAR`            | —            | データ点属性の JSON 文字列。          |
| `flags`                   | `INTEGER`            | —            | OTLP フラグのビットフィールド。       |
| `exemplars_json`          | `VARCHAR`            | —            | エグザンプラーの JSON 配列文字列。    |
| `aggregation_temporality` | `INTEGER NOT NULL`   | —            | OTLP 集約時間性の列挙値。             |

## `sigiro_profiles`

| 列                    | 型                   | 単位         | 意味                                     |
| --------------------- | -------------------- | ------------ | ---------------------------------------- |
| `profile_id`          | `VARCHAR`            | —            | プロファイル ID。                        |
| `trace_id`            | `VARCHAR`            | —            | トレース ID（16 進文字列）。             |
| `span_id`             | `VARCHAR`            | —            | スパン ID（16 進文字列）。               |
| `service_name`        | `VARCHAR NOT NULL`   | —            | OTel リソースの service.name。           |
| `timestamp`           | `TIMESTAMP NOT NULL` | µs precision | イベントまたはデータ点の時刻。           |
| `profile_type`        | `VARCHAR`            | —            | プロファイルのサンプル種別。             |
| `stack_trace`         | `VARCHAR[]`          | —            | 解決済みスタックフレームの順序付き配列。 |
| `value`               | `BIGINT`             | unit         | この値種別のサンプル値。                 |
| `unit`                | `VARCHAR`            | —            | プロファイルのサンプル種別の単位。       |
| `resource_attributes` | `VARCHAR`            | —            | リソース属性の JSON 文字列。             |
