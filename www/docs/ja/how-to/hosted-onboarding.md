---
title: ホスト型 sigiro へのテレメトリ送信
description: >-
  OAuth アクセストークンでホステッド sigiro に OpenTelemetry を送信し、SQL
  で照会します。インフラの運用は不要で、招待制ベータの期間は当社が各テナントを設定します。
sidebar:
  order: 3
---

このガイドでは、当社が運用する sigiro ホストへ OpenTelemetry を送信する方法と、そのデータを照会する方法を説明します。インフラの運用は不要です。

sigiro はトレース、ログ、メトリクス、プロファイルを読み取ります。まず CLI をインストールします。macOS、Linux、WSL のいずれでも次の 1 コマンドです。

```bash
curl -fsSL https://sigiro.com/install | sh
```

次に CLI をアカウントへ接続します。実行するコマンドはどこから始めたかによって変わります。両方を下の [CLI を接続する](#1-cli-を接続する) で説明します。

## 1. CLI を接続する

**すでに Web でサインアップを完了している場合。** インストール済みの CLI を、既存のアカウントにサインインさせます。

```bash
sigiro auth login
sigiro auth status
```

`sigiro auth login` は OAuth デバイス認証を開始し、確認ページをブラウザーで開きます。ブラウザーを開けないマシンでは `--no-browser` を指定してください。いずれの場合も、CLI は URL と確認用コードを表示します。

**アカウントがまだなく、ターミナルから始める場合。** シェルから離れずにアカウントを作成します。

```bash
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

`sigiro signup` は 6 桁の OTP をメールで送り、ターミナルで安全に入力を求めます。個人用ワークスペースを 1 つだけ作成・有効化し、ブラウザーなしで OAuth デバイス認証を内部で完了して、リフレッシュ／アクセス資格情報を OS の資格情報ストアに保存します。

**どちらの経路でも、CLI は同一の単一の個人用ワークスペースに紐づきます。** Web のサインアップと `sigiro signup` は同じプロビジョニング処理を呼び出し、その処理はアカウントにワークスペースが 1 つもない場合にのみ個人用ワークスペースを作成します。したがって `sigiro signup` は既存アカウントに対して冪等です。すでに登録済みのメールアドレスに対して実行しても、そのアカウントにサインインするだけで、アカウントもワークスペースも二重には作成されません。

`sigiro auth status` は CLI が紐づいているアカウントとワークスペースを表示します。`sigiro auth logout` は保存済みの資格情報を失効させて削除します。どちらの経路も資格情報をシェル履歴に残さず、OAuth API を手動で呼び出したりトークンをコピーしたりする必要もありません。

このページでは人のサインアップと OAuth ログインを扱います。ソフトウェア固有の委任 ID と鍵が必要な場合は、[Agent Auth オンボーディング](/docs/how-to/agent-auth)を使用してください。

ホステッド環境の認証は Better Auth が提供します。Organizations はテナントです。どちらのコマンドも、既定でホスト型サービス `https://sigiro.com` を対象とします。

同じトークンで OTLP の取り込みと API クエリを認証します。

## 2. OpenTelemetry エクスポーターを sigiro に向ける

標準的な OTLP over HTTP を使用します。認証には標準の `Authorization: Bearer` ヘッダーを使用します。

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://sigiro.com"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
```

`sigiro auth token` は保存済みセッションを更新してから、短期間有効なアクセストークンを出力します。この方法は対話的なテスト専用であり、長時間動作するコレクターのシークレットには使用しないでください。

**エンドポイントにポートは含まれません。** ホスト型 sigiro は標準の HTTPS ポートで OTLP を終端するため、`https://sigiro.com` がエンドポイントの全体です。セルフホストの[クイックスタート](/docs/tutorials/quickstart#2-テレメトリを送信する)から `:4318` をコピーしないでください。`sigiro.com` ではそのポートは接続を拒否し、接続できなかった SDK は、その失敗をここではなく自身のサービスログに報告します。

**`sigiro.com` では OTLP/gRPC は利用できません。** ポート `4317` は閉じており、gRPC のサービスパスはルーティングされていません。HTTP を使用してください。

**注 — カスタムホストやセルフホストの場合。** 本ページの内容は、当社が運用し CLI が既定で使用するホスト `https://sigiro.com` を前提としています。あなたや運用担当者が別の場所で sigiro を運用している場合は、各例のホストを置き換え、CLI がそこを対象とするように `SIGIRO_ENDPOINT` を設定してください。その場合、ポート、gRPC の可否、データ保持はそのデプロイの設定次第ですので、本ページを前提にせず運用担当者にご確認ください。なお `sigiro signup` と `sigiro auth login` は、引き続きホスト型サービス `https://sigiro.com` に対して認証します。

SDK は各シグナルを標準の OTLP パス（`/v1/traces`、`/v1/logs`、`/v1/metrics`、`/v1development/profiles`）に送信し、そのパスは上記のエンドポイントに付加されます。エクスポーターがシグナルごとのエンドポイントを受け取る形式の場合は、フルパスを指定してください。HTTP は gzip をサポートし、エンドポイントは TLS を使用します。1 リクエストは展開後のサイズで **8 MB 以下**である必要があります。SDK のデフォルトのバッチサイズは、この上限を十分に下回ります。

## 3. データが届いていることを確認する

テレメトリを送信します。フラッシュ間隔は約 1 秒のため、数秒待ってください。その後、次のクエリを実行します。

```bash
curl -s "https://sigiro.com/v1/query" \
  -H "Authorization: Bearer <oauth-access-token>" \
  --data 'SELECT count(*) AS n FROM sigiro_spans'
```

`n` が 0 以外であれば、トレースが届いています。クエリは最初のバッチを送信した*後*に実行してください。データを一度も送信していない新規テナントには、ストレージが存在しません。

## 4. データを照会する（SQL API）

- **エンドポイント:** メインの HTTPS ポートで `POST /v1/query`。
- **認証:** `Authorization: Bearer <oauth-access-token>`。
- **ボディ:** JSON ではなく、生の SQL 文字列。
- **レスポンス:** 行オブジェクトの JSON 配列。ヘッダー `x-sigiro-truncated: true|false` は、sigiro が結果を打ち切ったかどうかを示します。

各テナントは分離されたカタログを持つため、参照できるのは**自分自身の**データのみです。

**テーブル:** `sigiro_spans`、`sigiro_logs`、`sigiro_metrics_gauge`、`sigiro_metrics_sum`、`sigiro_metrics_histogram`、`sigiro_metrics_exp_histogram`、`sigiro_profiles`。属性列は JSON テキストなので、`json_extract(col, '$.key')` で参照できます。

各ファミリーが何を測定しているか、また自信に満ちた誤答を生む唯一の名前の衝突については、[テーブルについて](/docs/explanation/tables) をお読みください。

**許可される SQL:** `sigiro_*` テーブルに対する `SELECT` のみです。sigiro は書き込みと DDL を拒否します。また、ファイル、URL、S3 のリーダー（`read_csv`、`read_parquet`、`glob` など）もブロックします。JOIN、ウィンドウ関数、集計関数、`json_extract` はいずれも動作します。

**CTE は拒否されます。** `WITH` 句は、その内容にかかわらず検証に失敗します。派生テーブルのサブクエリとして書き換えてください: `SELECT ... FROM (SELECT ...) t`。ホスト型サービスでは、1 つの共有カタログがすべてのテナントを保持しているため、`information_schema` もブロックされます。

**例:**

```sql
-- Slowest operations, last hour
SELECT service_name, span_name,
       approx_quantile(duration, 0.95) / 1000.0 AS p95_ms, count(*) AS n
FROM sigiro_spans
WHERE timestamp > now() - INTERVAL '1 hour'
GROUP BY 1, 2 ORDER BY p95_ms DESC LIMIT 10;
```

```sql
-- Error-log rate per route, last 15 min
SELECT service_name, log_attributes ->> 'http.route' AS route, count(*) AS errors
FROM sigiro_logs
WHERE timestamp > now() - INTERVAL '15 minutes' AND severity_number >= 17
GROUP BY 1, 2 ORDER BY errors DESC;
```

## 5. 制限

| 制限                       | 値                                  | 超過時の sigiro の動作                                                 |
| -------------------------- | ----------------------------------- | ---------------------------------------------------------------------- |
| クエリのタイムアウト       | 10 秒                               | sigiro がクエリを拒否します（400）                                     |
| 結果行数                   | 100,000                             | sigiro がレスポンスを打ち切り、`x-sigiro-truncated: true` を設定します |
| クエリメモリ               | 約 400 MB                           | クエリが失敗します（400）                                              |
| 取り込みボディ             | 8 MB（展開後）                      | 413                                 |
| 同時実行クエリ             | テナントあたり 4                    | 超過分は空きを最大 5 秒待ち、その後 429（`Retry-After` 付き）          |
| SQL/API リクエストレート   | テナントあたり 5 req/s、バースト 20 | 429                                                                    |
| OTLP/HTTP リクエストレート | ソースあたり 50 req/s、バースト 100 | 429                                                                    |

sigiro にはセカンダリインデックスがありません。そのため、**すべてのクエリを `timestamp` で範囲指定してください**。範囲指定は速度をもたらします。また、タイムアウトと打ち切りも防ぎます。

## 6. データ保持

ベータの期間中は、sigiro を記録システムではなく、ライブなクエリサービスとして扱ってください。`sigiro.com` の保持期間はまだ公開していません。現在の期間は[ご相談ください](https://cal.com/akshatagarwl/30-minutes)。長期間保持する必要のあるデータはエクスポートしてください。

## 次のステップ

- [エージェントでコードを計測する](/docs/how-to/install-with-ai) — サービスにまだ OpenTelemetry がない場合
- [API リファレンス](/docs/reference) — 現在利用できるエンドポイントとリクエストプレイグラウンド
