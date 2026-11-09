---
title: ホスト型 sigiro へのテレメトリ送信
description: ターミナルからホスト型 sigiro のアカウントを作成し、OpenTelemetry を送信して、SQL でデータを照会します。
sidebar:
  order: 3
---

このガイドでは、当社が運用する sigiro ホストへ OpenTelemetry を送信する方法と、そのデータを照会する方法を説明します。インフラの運用は不要です。

最初から最後までの流れは次のとおりです。

1. [sigiro.com/sign-in](https://sigiro.com/sign-in) で、またはターミナルで `sigiro signup` を実行してサインアップします。どちらの場合も個人ワークスペースが 1 つ作られます。
2. CLI をインストールし、そのワークスペースに接続します（`sigiro auth login`）。
3. 以下の `OTEL_*` 変数をエクスポートしてアプリを起動します。アプリは `https://sigiro.com` に送信します。インジェスト専用の別ホストはありません。
4. [sigiro.com/dashboard](https://sigiro.com/dashboard/) でデータを確認するか、SQL で照会します。

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

**アカウントがまだなく、ターミナルから始める場合。** シェルを離れずにアカウントを作成します。

```bash
sigiro signup --name "Your Name" --email you@example.com
sigiro auth status
```

`sigiro signup` は 6 桁の OTP をメールで送信し、ターミナルで安全に入力を求めます。個人ワークスペースをちょうど 1 つ作成して有効化し、ブラウザーなしで OAuth デバイス認証を内部で完了し、リフレッシュ／アクセス認証情報をオペレーティングシステムの認証情報ストアに保存します。

**すべてのログインは 1 つのワークスペースに結び付けられ、ブラウザーがその名前を示します。** 承認ページには、CLI が使うワークスペースが表示されます。アカウントが複数のワークスペースに属している場合はそこで選択し、選択するまで何も承認されません。この選択はログインの一部です。セッションを更新しても、後で別のワークスペースに参加しても、同じワークスペースのままです。そのワークスペースから削除されるとログインは終了します。切り替えるには `sigiro auth login` をもう一度実行してください。

Web のサインアップと `sigiro signup` は同じプロビジョニング処理を呼び出し、アカウントにワークスペースがないときにだけ個人ワークスペースを作成します。そのため、既存のアカウントに対して `sigiro signup` を実行しても冪等です。そのアカウントにサインインするだけで、2 つ目のアカウントや 2 つ目のワークスペースは作りません。すでに複数のワークスペースに属しているアカウントでは処理を止め、`sigiro auth login` を実行するよう求めます。勝手にどれかを選ぶことはありません。

`sigiro auth status` は、CLI が結び付けられているアカウントとワークスペースを表示します。`sigiro auth logout` は保存済みの認証情報を失効させて削除します。どちらのフローも認証情報をシェル履歴に残さず、OAuth API を呼び出したりトークンを手でコピーしたりする必要もありません。

ここでは人のサインアップと OAuth ログインを扱います。ソフトウェアに独自の委任 ID と鍵が必要な場合は、[Agent Auth オンボーディング](/docs/how-to/agent-auth) を使ってください。

ホスト型の認証は Better Auth が提供します。ワークスペースは分離の単位です。データ、メンバー、クエリは他のすべてのワークスペースから分離されています。API のフィールドやエラーメッセージによってはテナントまたは組織と呼ばれますが、3 つの言葉はすべて同じものを指します。
どちらのコマンドも、既定ではホスト型サービス `https://sigiro.com` を対象とします。

同じトークンで OTLP のインジェストと API クエリの両方を認証します。

## 2. OpenTelemetry エクスポーターを sigiro に向ける

標準の OTLP over HTTP を使います。認証には標準の `Authorization: Bearer` ヘッダーを使います。

**OTLP/HTTP:**

```bash
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $(sigiro auth token)"
export OTEL_EXPORTER_OTLP_ENDPOINT="https://sigiro.com"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_SERVICE_NAME="checkout"   # the name your service shows up under
```

ヘッダーを先に設定してください。`OTEL_EXPORTER_OTLP_ENDPOINT` をすでにエクスポートしていると、`sigiro` CLI が自身のテレメトリをそこへ送ろうとして `SIGIRO_OTEL_AUTH_KEY is required` という警告を表示します。この警告は無害で、トークンはそのまま出力されます。

**トークンは 15 分で失効します。** `sigiro auth token` は保存済みセッションを更新してから、短期間有効なアクセストークンを出力します。このトークンで起動したアプリは 15 分間データを送信し、その後エクスポーターは "The access token has expired" という理由付きの HTTP `401` を受け取ります。これはアプリ自身のログにしか表示されません。sigiro を試すにはこれで十分です。

**長時間動作するサービスはインジェストキーで送信します。** CLI がサインインしているワークスペース用に作成します。

```bash
sigiro ingest-keys create --name checkout
```

キー `sigiro_<id>_<secret>` と、対応する `otel_exporter_otlp_headers` の値が出力されます。キーはこの場で保存してください。表示されるのは一度だけで、sigiro はそのハッシュしか保持しません。その後、トークンの代わりにキーを使います。

```bash
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer sigiro_<id>_<secret>"
```

キーは失効しません。できるのはテレメトリの送信だけで、キーを使ったクエリは `403` になります。`sigiro ingest-keys list` は各キーの ID と最終使用日時を表示し、`sigiro ingest-keys revoke <id>` はキーを即座に停止します。

**アプリにまだ OpenTelemetry がない場合。** ゼロコード計装を使えば、上の変数だけでトレース、ログ、メトリクスを送信でき、コードの変更は不要です。Node.js の場合:

```bash
npm install @opentelemetry/api @opentelemetry/auto-instrumentations-node
node --require @opentelemetry/auto-instrumentations-node/register app.js
```

Python の場合:

```bash
pip install opentelemetry-distro opentelemetry-exporter-otlp-proto-http
opentelemetry-bootstrap -a install
export OTEL_PYTHON_LOGGING_AUTO_INSTRUMENTATION_ENABLED=true  # also send logs
opentelemetry-instrument python app.py
```

その他の言語や、エージェントに設定させたい場合は、[エージェントでコードを計装する](/docs/how-to/install-with-ai) に従ってください。

**エンドポイントにポートは含まれません。** ホスト型 sigiro は標準の HTTPS ポートで OTLP を終端するため、`https://sigiro.com` がエンドポイントの全体です。セルフホストの[クイックスタート](/docs/tutorials/quickstart#2-テレメトリを送信する)から `:4318` をコピーしないでください。`sigiro.com` ではそのポートは接続を拒否し、接続できなかった SDK は、その失敗をここではなく自身のサービスログに報告します。

**`sigiro.com` では OTLP/gRPC は利用できません。** ポート `4317` は閉じており、gRPC のサービスパスはルーティングされていません。HTTP を使用してください。

**注 — カスタムホストやセルフホストの場合。** 本ページの内容は、当社が運用し CLI が既定で使用するホスト `https://sigiro.com` を前提としています。あなたや運用担当者が別の場所で sigiro を運用している場合は、各例のホストを置き換え、CLI がそこを対象とするように `SIGIRO_ENDPOINT` を設定してください。その場合、ポート、gRPC の可否、データ保持はそのデプロイの設定次第ですので、本ページを前提にせず運用担当者にご確認ください。なお `sigiro signup` と `sigiro auth login` は、引き続きホスト型サービス `https://sigiro.com` に対して認証します。

SDK は各シグナルを標準の OTLP パス（`/v1/traces`、`/v1/logs`、`/v1/metrics`、`/v1development/profiles`）に送信し、そのパスは上記のエンドポイントに付加されます。エクスポーターがシグナルごとのエンドポイントを受け取る形式の場合は、フルパスを指定してください。HTTP は gzip をサポートし、エンドポイントは TLS を使用します。1 リクエストは展開後のサイズで **8 MB 以下**である必要があります。SDK のデフォルトのバッチサイズは、この上限を十分に下回ります。

プロファイルも同じエンドポイントを使います。Go プログラム、OpenTelemetry eBPF プロファイラー、Collector からの送り方は [プロファイルを sigiro に送信する](/docs/how-to/send-profiles) を参照してください。

## 3. データが届いていることを確認する

アプリにいくつかリクエストを送ります。フラッシュ間隔は約 1 秒のため、数秒待ってください。その後 [sigiro.com/dashboard](https://sigiro.com/dashboard/) を開き、同じメールアドレスでサインインします。サービスが `OTEL_SERVICE_NAME` の名前で、トレースとログとともに一覧に表示されます。

ターミナルから確認する場合は次のとおりです。

```bash
curl -s "https://sigiro.com/v1/services" \
  -H "Authorization: Bearer $(sigiro auth token)"
```

サービスが `services` 配列に、シグナルごとの件数とともに表示されます。表示されない場合は、アプリ自身のログでエクスポーターのエラーを確認してください。`401` には理由（トークンの失効、キーの取り消しまたは不明、ヘッダーの欠落）が含まれ、接続エラーはたいていエンドポイントに `:4318` が残っていることが原因です。

**まだアプリを計装していない場合。** テスト用のスパンを 1 つ手で送って経路を確認します。OTLP/HTTP は protobuf だけでなく JSON も受け付けます。

```bash
now=$(date +%s)000000000
curl -s -w '%{http_code}\n' https://sigiro.com/v1/traces \
  -H "Authorization: Bearer $(sigiro auth token)" \
  -H "content-type: application/json" \
  --data '{"resourceSpans":[{"resource":{"attributes":[{"key":"service.name","value":{"stringValue":"hello-sigiro"}}]},
    "scopeSpans":[{"spans":[{"traceId":"5b8efff798038103d269b633813fc60c","spanId":"eee19b7ec3c1b174",
    "name":"GET /hello","kind":2,"startTimeUnixNano":"'"$now"'","endTimeUnixNano":"'"$now"'"}]}]}]}'
```

`200` は sigiro がスパンを受け付けたことを意味します。数秒後に `hello-sigiro` が `/v1/services` に表示されます。

この確認に `sigiro query` は使えません。これはセルフホストのエッジをフェデレーションするコマンドで、`sigiro.com` に対しては失敗します。ターミナルやエージェントからクエリするには、上のように `curl` を使うか、保存済みのログインで同じワークスペースを読む [`sigiro mcp`](/docs/reference/cli#mcp) を使ってください。

```bash
claude mcp add sigiro -- sigiro mcp
```

## 4. データを照会する（SQL API）

- **エンドポイント:** メインの HTTPS ポートで `POST /v1/query`。
- **認証:** `Authorization: Bearer $(sigiro auth token)`。
- **ボディ:** JSON ではなく、生の SQL 文字列。
- **レスポンス:** 行オブジェクトの JSON 配列。ヘッダー `x-sigiro-truncated: true|false` は、sigiro が結果を打ち切ったかどうかを示します。

各ワークスペースは分離されたカタログを持つため、参照できるのは**自分自身の**ワークスペースのデータのみです。

**テーブル:** `sigiro_spans`、`sigiro_logs`、`sigiro_metrics_gauge`、`sigiro_metrics_sum`、`sigiro_metrics_histogram`、`sigiro_metrics_exp_histogram`、`sigiro_profiles`。属性列は JSON テキストなので、`json_extract(col, '$.key')` または `col ->> 'key'` で参照できます。

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
| 取り込みボディ             | 8 MB（展開後）                      | 413                                                                    |
| 同時実行クエリ             | テナントあたり 4                    | 超過分は空きを最大 5 秒待ち、その後 429（`Retry-After` 付き）          |
| SQL/API リクエストレート   | テナントあたり 5 req/s、バースト 20 | 429                                                                    |
| OTLP/HTTP リクエストレート | ソースあたり 50 req/s、バースト 100 | 429                                                                    |

sigiro にはセカンダリインデックスがありません。そのため、**すべてのクエリを `timestamp` で範囲指定してください**。範囲指定は速度をもたらします。また、タイムアウトと打ち切りも防ぎます。

## 6. データ保持

ベータの期間中は、sigiro を記録システムではなく、ライブなクエリサービスとして扱ってください。`sigiro.com` の保持期間はまだ公開していません。現在の期間は[ご相談ください](https://cal.com/akshatagarwl/30-minutes)。長期間保持する必要のあるデータはエクスポートしてください。

## 次のステップ

- [ダッシュボード](https://sigiro.com/dashboard/) — サインインしたワークスペースのサービス、トレース、ログをブラウザーで確認できます
- [エージェントでコードを計装する](/docs/how-to/install-with-ai) — サービスにまだ OpenTelemetry がない場合
- [API リファレンス](/docs/reference) — サーバーの `/openapi.json` が現在記述している HTTP 操作
