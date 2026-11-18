---
title: エージェントでコードを計装する
description: >-
  エージェントにプロンプトを 1 つ渡すだけです。サービスに OpenTelemetry の自動計装を追加し、sigiro
  に向けます。Claude Code、OpenCode、Codex で動作します。
sidebar:
  order: 1
---

このガイドでは、計装がまったく入っていないサービスに OpenTelemetry を追加する方法を、エージェントに編集させながら説明します。以下のプロンプトをエージェントに貼り付けてください。エージェントがフレームワークを検出し、公式の自動計装をインストールし、エクスポーターの向き先を sigiro に設定します。

これは Claude Code、OpenCode、Codex、あるいはファイルを編集してパッケージマネージャーを実行できる任意のエージェントで動作します。

## はじめる前に

送信先として sigiro サーバーが必要です。`sigiro serve` で起動するか、[クイックスタート](/docs/tutorials/quickstart#1-実行する)にある Docker コマンドで起動してください。ホスト版の新規ユーザーは `sigiro signup --name "Your Name" --email you@example.com`、既存アカウントは `sigiro auth login` を実行してください。セルフホストのサーバーには認証情報は不要です。

自動計装は Python、Node.js、Java、.NET、Ruby 向けに提供されています。対象のサービスが Go の場合、このプロンプトは使用しないでください。代わりに [Go のセクション](#go-ソースではなくビルドを計装する)へ進んでください。

## 1. エージェントに次のプロンプトを渡す

エンドポイントを置き換えてください。ホステッドサービスを使用しない場合は、アクセストークンの行を削除してください。

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (self-hosted; for hosted sigiro use
                 https://<your-sigiro-host> with no port)
HOSTED AUTH: sigiro signup --name "Your Name" --email you@example.com  (new account; use sigiro auth login for an existing account; omit for self-hosted)

Goal: add OpenTelemetry auto-instrumentation so this service emits traces,
metrics, and logs to sigiro. Make the minimum change that works — prefer
auto-instrumentation over hand-written spans.

Rules:
- Detect the language and framework automatically
- Use official OTel auto-instrumentation where it exists (Python opentelemetry-instrument,
  Java javaagent, Node.js auto-instrumentations-node, etc.)
- If auto-instrumentation is not available for this language/framework, add
  manual OTel SDK spans for HTTP handlers and database calls
- Set OTEL_EXPORTER_OTLP_ENDPOINT to the SIGIRO ENDPOINT above exactly as given.
  A self-hosted server listens on :4318; a hosted one carries no port. Do not add
  or remove a port
- ホスト版では `sigiro auth token` で bearer token を読み取り、ソースコードにコピーしない
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- ホスト版の新規ユーザーは `sigiro signup --name "Your Name" --email you@example.com`、既存ユーザーは `sigiro auth login` を使い、認証情報をハードコードしない
- If a file already initializes the OTel SDK, update it rather than re-initializing
- 自動化の場合のみ環境変数から認証情報を読み取り、対話的な利用では `sigiro signup` または `sigiro auth login` を使う
```

エージェントが誤った言語を検出した場合は、直接指示してください。

```
Language: <python|nodejs|java|dotnet|ruby>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. やってはいけないことをエージェントに伝える

ここでエージェントは、必要以上のことをやりがちです。作業を始める前に、次の制限を明示してください。

- OTel Collector をインストールしないでください。sigiro がコレクターです。
- エンドポイントごとに手書きのスパンを追加しないでください。自動計装がカバーします。
- データベース接続文字列やビジネスロジックを変更しないでください。
- ダッシュボードやアラート設定をセットアップしないでください。sigiro にはどちらもありません。

タスクの全体はこれだけです。フレームワークを検出し、自動計装をインストールし、その向き先を sigiro に設定することです。

## 3. 得られるカバレッジを確認する

| 言語    | アプローチ                                               | カバー範囲                                                             |
| ------- | -------------------------------------------------------- | ---------------------------------------------------------------------- |
| Python  | `opentelemetry-instrument` CLI + ディストリビューション  | HTTP（FastAPI、Flask、Starlette）、DB（psycopg2、asyncpg）、Redis      |
| Node.js | `@opentelemetry/auto-instrumentations-node`              | HTTP（Express、Fastify）、DB（pg、mysql2、mongoose）                   |
| Java    | `opentelemetry-javaagent.jar`                            | HTTP（サーブレット、Spring）、DB（JDBC）、JVM メトリクス               |
| .NET    | `OpenTelemetry.AutoInstrumentation` スタートアップフック | HTTP（ASP.NET Core）、DB（EF Core）                                    |
| Ruby    | `opentelemetry-instrumentation-all` gem                  | HTTP（Rails）、DB（ActiveRecord）、Sidekiq                             |
| Go      | `otelc` コンパイル時計装                                 | HTTP サーバースパン、Go ランタイムメトリクス、サードパーティライブラリ |

あるライブラリでスパンがまったく取得できない場合は、そのライブラリに限って手動でスパンを追加してください。

自動計装のカバー範囲は上の表が示すよりも狭いため、次の 2 つの欠落はあらかじめ想定しておく価値があります。

**ログ。** 同梱のログブリッジがカバーするのは名前付きロガーです。Node では pino、winston、bunyan、Python では標準の `logging` モジュールです。`console.log` や `print` で書き出しているサービスでは、トレースとメトリクスは生成されますが、**ログは生成されません**。手作業でブリッジするのではなく、本物のロガーへ移行してください。

**公式の計装が知らないデータベース。** 上の各 DB のエントリは、特定のドライバーを名指ししています。`instrumentation-pg` がパッチを当てるのは `node-postgres` であるため、`postgres.js` を使う Node のサービスではクエリスパンが取得できません。ORM を使ってもこの欠落は埋まりません。`drizzle-orm` は `drizzle-orm/tracing` にトレーサーを同梱していますが、公開パッケージではその `await import('@opentelemetry/api')` がコメントアウトされているため、スパンを発行できません。DB の列を信頼する前に、自分が使っているドライバーがリストに載っていることを確認してください。また、手書きのスパンよりも、その ORM 向けにメンテナンスされているラッパーを選んでください。

## Go: ソースではなくビルドを計装する

Go の場合、この作業をエージェントに任せないでください。Go ではビルドコマンドに接頭辞を付けるだけで、計装済みのバイナリが生成されます。ソースファイルは 1 つも変更しません。ツールは `otelc` で、OpenTelemetry プロジェクトがビルドしています。このツールは、自分で管理していないサードパーティライブラリも計装します。エージェントは自分のコードしか編集できないため、これはエージェントには不可能です。

お使いのプラットフォーム向けのバイナリを [v1.0.1 リリース](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/tag/v1.0.1)からダウンロードしてください。このリリースでは、macOS と Linux 向けの arm64 および x86-64 版、Windows 向けの x86-64 版の `otelc` が公開されています。

```bash
curl -fsSL -o otelc https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/download/v1.0.1/otelc-darwin-arm64
chmod +x otelc
./otelc version
```

このツールでは `go install` は使えません。モジュールがコマンドパスを公開していないためです。アセットが提供されていないプラットフォームでは、リポジトリをクローンして `make build` を実行してください。

続いて、通常のビルドコマンドの前に `otelc` を付けます。

```bash
./otelc go build -o checkout .
```

初回のビルドでは OpenTelemetry のパッケージをダウンロードするため、通常のビルドより時間がかかります。`go.mod` ファイルは変更されません。

[クイックスタート](/docs/tutorials/quickstart#2-テレメトリを送信する)で使用しているのと同じ 2 つの変数を指定して、バイナリを起動します。

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=checkout \
  ./checkout
```

バイナリは起動時にトレースプロバイダー、メータープロバイダー、ロガープロバイダーを作成します。ユーザー側の SDK コードは不要です。サービスは HTTP サーバースパンと Go ランタイムメトリクスをレポートします。

制限が 2 つあります。このツールには Go 1.25 以降が必要であり、現在のバージョンは 1.0.1 で、プロジェクトが 2026 年 7 月 14 日にタグ付けしたものです。カバーされるライブラリについては、[アップストリームのドキュメント](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation)を参照してください。

## 4. 動作していることを確認する

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=my-service \
  <your-app-start-command>
```

次に、どのサービスを受信したかを sigiro に問い合わせます。

```bash
curl http://localhost:9999/v1/services
```

実際のトラフィックが発生してから数秒以内にサービスが表示されます。表示された後は、`sigiro diagnose my-service` が読み取れるデータが揃っています。

サービスが表示されない場合は、次の点を確認してください。セルフホストのサーバーでは、ポートを確認します。HTTP は `:4318`、gRPC は `:4317` です。ホステッドサービスでは、その逆に、エンドポイントにポートが**付いていない**ことを確認し、localhost ではなく `https://<your-sigiro-host>/v1/services` にアクセストークンを添えて問い合わせてください。`sigiro status` を実行して、サーバーが正常であることを確認します。自分のサービスのログを読み、OTel の起動時エラーを確認してください。エクスポーターの設定が誤っている場合、起動時に障害がレポートされるためです。

ホステッドサービスでは、ヘッダーが正確に `Authorization: Bearer <oauth...en>` であることを確認してください。セルフホストのサーバーはすべての認証ヘッダーを無視します。そのため、誤ったトークンを指定しても、メッセージも 401 レスポンスもなく失敗します。

## 次のステップ

- [クイックスタート](/docs/tutorials/quickstart#3-何が変わったかを尋ねる) — 何がどう変わったのかを尋ねる
- [エージェント自身のテレメトリを sigiro に送る](/docs/how-to/agent-telemetry) — Claude Code や Codex の向き先も sigiro にする
- [ホステッド sigiro にテレメトリを送る](/docs/how-to/hosted-onboarding) — サーバーを自分で運用したくない場合はこちら
