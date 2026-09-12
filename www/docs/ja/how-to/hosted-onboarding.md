---
title: ホスト型 sigiro へのテレメトリ送信
description: >-
  テナントキーでホスト型 sigiro に OpenTelemetry を送信し、SQL
  で照会します。インフラの運用は不要で、招待制ベータの期間は当社が各テナントを設定します。
sidebar:
  order: 2
---

このガイドでは、当社が運用する sigiro ホストへ OpenTelemetry を送信する方法と、そのデータを照会する方法を説明します。インフラの運用は不要です。

sigiro はトレース、ログ、メトリクス、プロファイルを読み取ります。招待制ベータの期間中は、各テナントを当社が手作業でセットアップします。以下の `<your-sigiro-host>` は、運用担当者から提供されたホストに置き換えてください。

## 1. API キーを取得する

運用担当者から `sk_<yourtenant>_<random>` という形式のキーが送られます。このキーはパスワードと同様に扱ってください。sigiro がキーを表示するのは 1 回だけです。キーを復元することはできませんが、ローテーションは可能です。このキーは取り込みと照会の**両方**を認証します。

## 2. OpenTelemetry エクスポーターを sigiro に向ける

標準的な OTLP over HTTP を使用します。認証には標準の `Authorization: Bearer` ヘッダーを使用します。

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://<your-sigiro-host>"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sk_<yourtenant>_<random>"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

**エンドポイントにポートは含まれません。** ホスト型 sigiro は標準の HTTPS ポートで OTLP を終端するため、`https://api.sigiro.com` がエンドポイントの全体です。セルフホストの[クイックスタート](/docs/tutorials/quickstart#2-send-it-telemetry)から `:4318` をコピーしないでください。`api.sigiro.com` ではそのポートは接続を拒否し、接続できなかった SDK は、その失敗をここではなく自身のサービスログに報告します。ホストが異なる場合は運用担当者にお問い合わせください。

**`api.sigiro.com` では OTLP/gRPC は利用できません。** ポート `4317` は閉じており、gRPC のサービスパスはルーティングされていません。HTTP を使用してください。また、他のホストで gRPC が使えると想定する前に、運用担当者に確認してください。

SDK は各シグナルを標準の OTLP パス（`/v1/traces`、`/v1/logs`、`/v1/metrics`、`/v1development/profiles`）に送信し、そのパスは上記のエンドポイントに付加されます。エクスポーターがシグナルごとのエンドポイントを受け取る形式の場合は、フルパスを指定してください。HTTP は gzip をサポートし、エンドポイントは TLS を使用します。1 リクエストは展開後のサイズで **8 MB 以下**である必要があります。SDK のデフォルトのバッチサイズは、この上限を十分に下回ります。

## 3. データが届いていることを確認する

テレメトリを送信します。フラッシュ間隔は約 1 秒のため、数秒待ってください。その後、次のクエリを実行します。

```bash
curl -s "https://<your-sigiro-host>/v1/query" \
  -H "Authorization: Bearer sk_<yourtenant>_<random>" \
  --data "SELECT count(*) AS n FROM sigiro_spans
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'"
```

`n` が 0 以外であれば、トレースが届いています。クエリは最初のバッチを送信した*後*に実行してください。データを一度も送信していない新規テナントには、ストレージが存在しません。

## 4. データを照会する（SQL API）

- **エンドポイント:** メインの HTTPS ポートで `POST /v1/query`。
- **認証:** `Authorization: Bearer sk_<yourtenant>_<random>`。
- **ボディ:** JSON ではなく、生の SQL 文字列。
- **レスポンス:** 行オブジェクトの JSON 配列。ヘッダー `x-sigiro-truncated: true|false` は、sigiro が結果を打ち切ったかどうかを示します。

各テナントは分離されたカタログを持つため、参照できるのは**自分自身の**データのみです。

**テーブル:** `sigiro_spans`、`sigiro_logs`、`sigiro_log_templates`、`sigiro_metrics_gauge`、`sigiro_metrics_sum`、`sigiro_metrics_histogram`、`sigiro_metrics_exp_histogram`、`sigiro_profiles`、および `sigiro_anomalies` テーブル。`sigiro_anomalies` テーブルには、事前計算されたレジームシフトが格納されます。異常検出パスがこれらのシフトを継続的に書き込みます。`GET /v1/anomalies` は同じシフトを型付き JSON として提供します。属性カラム（`*_attributes`、`events_json` など）には JSON テキストが格納されます。フィールドを読み取るには `json_extract(col, '$.key')` または `col ->> 'key'` を使用してください。

各ファミリーが何を測定しているか、また自信に満ちた誤答を生む唯一の名前の衝突については、[テーブルについて](/docs/explanation/tables) をお読みください。

**許可される SQL:** `sigiro_*` テーブルに対する `SELECT` のみです。sigiro は書き込みと DDL を拒否します。また、ファイル、URL、S3 のリーダー（`read_csv`、`read_parquet`、`glob` など）もブロックします。JOIN、ウィンドウ関数、集計関数、`json_extract` はいずれも動作します。

**CTE は拒否されます。** `WITH` 句は、その内容にかかわらず検証に失敗します。派生テーブルのサブクエリとして書き換えてください: `SELECT ... FROM (SELECT ...) t`。ホスト型サービスでは、1 つの共有カタログがすべてのテナントを保持しているため、`information_schema` もブロックされます。

**例:** まずグループキーと件数だけを集計し、その後、選択したグループの詳細を
小さな範囲で取得します。クライアントで UTC の境界を計算し、raw
spans/logs/metrics/profiles には必ず半開区間のリテラル `TIMESTAMP`（`>=` と `<`）を
指定してください。`now()` と `current_timestamp` は
`query_requires_time_bounds`（422）で拒否されます。`LIMIT` は出力を制限しますが、
スキャンや集約ハッシュのメモリは制限しません。

```sql
-- Slowest operations, last hour
SELECT service_name, span_name,
       approx_quantile(duration, 0.95) / 1000.0 AS p95_ms, count(*) AS n
FROM sigiro_spans
WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
  AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
GROUP BY 1, 2 ORDER BY p95_ms DESC LIMIT 10;
```

```sql
-- Error-log rate per route, last 15 min
SELECT service_name, log_attributes ->> 'http.route' AS route, count(*) AS errors
FROM sigiro_logs
WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
  AND timestamp <  TIMESTAMP '2026-09-10 13:00:00' AND severity_number >= 17
GROUP BY 1, 2 ORDER BY errors DESC;
```

`query_timeout`（408）はプロジェクト初期化後のリーダー受付と DuckDB 実行に適用される
設定時間です。リクエスト全体の span には初期化とシリアライズも含まれます。
`query_memory_limit_exceeded`（422）なら範囲・射影を狭めてください。
`query_temp_storage_unavailable`（507）は一時ストレージを直してから、
`query_storage_unavailable`（503）はストレージ復旧後に再試行します。

## 5. 制限

| 制限                       | 値                                  | 超過時の sigiro の動作                                       |
| -------------------------- | ----------------------------------- | ------------------------------------------------------------ |
| クエリのタイムアウト       | 10 秒                               | `query_timeout`（408）                                       |
| 結果行数                   | 100,000                             | レスポンスを打ち切り `x-sigiro-truncated: true` を設定します |
| クエリメモリ               | 設定された共有上限                  | `query_memory_limit_exceeded`（422）                         |
| 取り込みボディ             | 8 MB（展開後）                      | 413（HTTP）/ RESOURCE_EXHAUSTED（gRPC）                      |
| SQL/API リクエストレート   | テナントあたり 5 req/s、バースト 20 | 429                                                          |
| OTLP/HTTP リクエストレート | ソースあたり 50 req/s、バースト 100 | 429                                                          |

sigiro にはセカンダリインデックスがありません。そのため、**すべての raw telemetry クエリを
`timestamp` の明示的な半開区間リテラルで範囲指定してください**。範囲指定は速度をもたらし、
集約を狭くしてから詳細を取得する流れはスキャンと中間メモリを予測しやすくします。

## 6. データ保持

ベータの期間中は、sigiro を記録システムではなく、ライブなクエリサービスとして扱ってください。現在の保持期間については運用担当者にお問い合わせください。長期間保持する必要のあるデータはエクスポートしてください。

## 次のステップ

- [エージェントでコードを計測する](/docs/how-to/install-with-ai) — サービスにまだ OpenTelemetry がない場合
- [API リファレンス](/docs/reference) — 5 つのエンドポイントと、それぞれのリクエストプレイグラウンド
- [ダッシュボードではなくエビデンスについて](/docs/explanation/evidence) — `/v1/diagnose` がチャートではなくランク付けされたリストを返す理由
