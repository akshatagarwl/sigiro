---
title: エージェントでコードを計装する
description: エージェントにプロンプトを 1 つ渡すと、サービスに OpenTelemetry の自動計装を追加して sigiro に向けます。Claude Code、OpenCode、Codex で使えます。
sidebar:
  order: 1
---

このガイドでは、OpenTelemetry がまったく入っていないサービスに、エージェントに編集させて OpenTelemetry を追加する方法を説明します。以下のプロンプトをエージェントに貼り付けてください。エージェントがフレームワークを検出し、公式の自動計装をインストールし、エクスポーターを sigiro に向けます。

これは Claude Code、OpenCode、Codex のほか、ファイルを編集してパッケージマネージャーを実行できる任意のエージェントで機能します。

## sigiro スキルをインストールする

エージェントを起動する前に、公開スキルをインストールします。

```bash
npx skills add Arnav12114/skills
```

[`debug-with-sigiro` スキル](https://github.com/Arnav12114/skills) は、テレメトリが届いた後に sigiro を使う方法をエージェントに教えます。このガイドは計装に絞っています。

## 始める前に

送信先として sigiro サーバーが必要です。`sigiro serve` で起動するか、[クイックスタート](/docs/tutorials/quickstart#1-実行する) の Docker コマンドで起動してください。ホスト型 sigiro の場合は、エージェントを起動する前に、新しいアカウントなら `sigiro signup --name "Your Name" --email you@example.com` を（既存のアカウントならブラウザーを使う `sigiro auth login` を）実行してください。セルフホストのサーバーには鍵は不要です。

自動計装は Python、Node.js、Java、.NET、Ruby 向けに提供されています。対象のサービスが Go の場合、このプロンプトは使用しないでください。代わりに [Go のセクション](#go-ソースではなくビルドを計装する)へ進んでください。

## 1. エージェントにこのプロンプトを渡す

エンドポイントを置き換えてください。ホスト型の新しいアカウントでは、先にターミナルのサインアップコマンドを実行してください。既存のアカウントでは、トークンをプロンプトにコピーするのではなく、ログイン済みの CLI セッションを使ってください。

```
Instrument this codebase for sigiro observability.

SIGIRO ENDPOINT: http://localhost:4318  (self-hosted; for hosted sigiro use
                 https://<your-sigiro-host> with no port)
HOSTED AUTH: sigiro signup --name "Your Name" --email you@example.com  (new account;
             use sigiro auth login for an existing account; omit for self-hosted)

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
- For hosted sigiro only, read a bearer token with `sigiro auth token`; never paste it into source
- Set OTEL_SERVICE_NAME to identify this service (use the existing service name or repo name)
- Use the existing package manager (pip, npm, Maven, Gradle, gem, go get)
- For a new hosted account, use `sigiro signup --name "Your Name" --email you@example.com`;
  for an existing account, use `sigiro auth login`. Never hardcode credentials.
- If a file already initializes the OTel SDK, update it rather than re-initializing
- For automation only, read credentials from the environment; interactive users should
  use the OS credential store via `sigiro signup` or `sigiro auth login`
```

エージェントが言語を誤って検出した場合は、直接伝えてください。

```
Language: <python|nodejs|java|dotnet|ruby>
Framework: <fastapi|express|spring-boot|rails|...>
```

## 2. エージェントにやってはいけないことを伝える

この作業では、エージェントは求めた以上のことをします。エージェントが始める前に、次の制限を伝えてください。

- OTel Collector をインストールしない。sigiro がコレクターです。
- エンドポイントごとに手動のスパンを追加しない。自動計装がそれらをカバーします。
- データベースの接続文字列やビジネスロジックを変更しない。
- ダッシュボードやアラートの設定を行わない。この作業には含まれません。

作業の全体は、フレームワークを検出し、自動計装をインストールし、それを sigiro に向けることです。

## 3. 得られるカバレッジを確認する

| 言語    | 方法                                             | 対象                                                            |
| ------- | ------------------------------------------------ | --------------------------------------------------------------- |
| Python  | `opentelemetry-instrument` CLI + distro          | HTTP (FastAPI, Flask, Starlette), DB (psycopg2, asyncpg), Redis |
| Node.js | `@opentelemetry/auto-instrumentations-node`      | HTTP (Express, Fastify), DB (pg, mysql2, mongoose)              |
| Java    | `opentelemetry-javaagent.jar`                    | HTTP (Servlets, Spring), DB (JDBC), JVM メトリクス              |
| .NET    | `OpenTelemetry.AutoInstrumentation` startup hook | HTTP (ASP.NET Core), DB (EF Core)                               |
| Ruby    | `opentelemetry-instrumentation-all` gem          | HTTP (Rails), DB (ActiveRecord), Sidekiq                        |
| Go      | `otelc` によるコンパイル時計装                   | HTTP サーバースパン、Go ランタイムメトリクス、サードパーティライブラリ |

あるライブラリでスパンが得られない場合は、そのライブラリについてだけスパンを手で追加してください。

自動計装のカバー範囲は表から受ける印象より狭いため、次の 2 つの欠落を想定しておく価値があります。

**ログ。** 同梱のログブリッジが対象とするのは名前付きのロガーです。Node では pino、winston、bunyan、Python では標準の `logging` モジュールです。`console.log` や `print` で出力するサービスは、トレースとメトリクスは出しても**ログは出しません**。手でブリッジするのではなく、本物のロガーに移行してください。

**公式の計装が知らないデータベース。** 上の各 DB の項目は特定のドライバーを挙げています。`instrumentation-pg` は `node-postgres` にパッチを当てるため、`postgres.js` を使う Node のサービスではクエリのスパンが得られません。ORM でも欠落は埋まりません。`drizzle-orm` は `drizzle-orm/tracing` にトレーサーを同梱していますが、公開パッケージではその `await import('@opentelemetry/api')` がコメントアウトされているため、スパンを出力できません。DB の列を信頼する前にご自身のドライバーが一覧にあることを確認し、手書きのスパンよりもその ORM 向けに保守されているラッパーを優先してください。

## Go: ソースではなくビルドを計装する

Go ではこの作業をエージェントに任せないでください。Go ではビルドコマンドに接頭辞を付けるだけで、計装済みのバイナリが出来上がります。ソースファイルは変更しません。ツールは `otelc` で、OpenTelemetry プロジェクトがビルドしています。このツールは、あなたが管理していないサードパーティライブラリも計装します。エージェントは自分のコードしか編集できないため、これはエージェントにはできません。

お使いのプラットフォーム向けのバイナリを [v1.0.1 リリース](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/tag/v1.0.1) からダウンロードしてください。このリリースでは、macOS と Linux の arm64 と x86-64、および Windows の x86-64 向けに `otelc` が公開されています。

```bash
curl -fsSL -o otelc https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation/releases/download/v1.0.1/otelc-darwin-arm64
chmod +x otelc
./otelc version
```

このツールには `go install` は使えません。モジュールがコマンドのパスを公開していないためです。アセットがないプラットフォームでは、リポジトリをクローンして `make build` を実行してください。

次に、通常のビルドコマンドの前に `otelc` を付けます。

```bash
./otelc go build -o checkout .
```

最初のビルドでは OpenTelemetry のパッケージをダウンロードするため、通常のビルドより時間がかかります。`go.mod` ファイルは変更されません。

[クイックスタート](/docs/tutorials/quickstart#2-テレメトリを送信する) と同じ 2 つの変数を指定してバイナリを起動します。

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=checkout \
  ./checkout
```

バイナリは起動時にトレースプロバイダー、メータープロバイダー、ロガープロバイダーを作成します。SDK のコードを書く必要はありません。サービスは HTTP サーバースパンと Go ランタイムメトリクスを報告します。

制限が 2 つあります。このツールには Go 1.25 以降が必要で、現在のバージョンは 1.0.1（プロジェクトが 2026 年 7 月 14 日にタグ付け）です。対応するライブラリについては [アップストリームのドキュメント](https://github.com/open-telemetry/opentelemetry-go-compile-instrumentation) を読んでください。

## 4. 動作を確認する

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
OTEL_SERVICE_NAME=my-service \
  <your-app-start-command>
```

次に、sigiro がどのサービスを受信したかを確認します。

```bash
curl http://localhost:9999/v1/services
```

実際のトラフィックがあれば、数秒以内にサービスが表示されます。表示された後は、範囲を限定した `POST /v1/query` でスパンとログを調べられます。

サービスが表示されない場合は、次を確認してください。セルフホストのサーバーでは、ポートを確認します。HTTP は `:4318`、gRPC は `:4317` です。ホスト型サービスでは逆に、エンドポイントにポートが**付いていない**ことを確認し、localhost ではなく `https://<your-sigiro-host>/v1/services` を鍵を付けてクエリしてください。`sigiro status` を実行してサーバーが正常であることを確認します。エクスポーターの設定が誤っていると起動時に問題が報告されるため、ご自身のサービスのログで OTel の起動エラーを確認してください。

ホスト型サービスでは、ヘッダーが正確に `Authorization: Bearer <oauth-access-token>` であることを確認してください。セルフホストのサーバーはすべての認証ヘッダーを無視します。そのため、鍵が誤っていても、メッセージも 401 レスポンスもなく失敗します。

## 次のステップ

- [クイックスタート](/docs/tutorials/quickstart#3-最近のエラーを問い合わせる) — 最近のエラーを SQL で照会します
- [エージェント自身のテレメトリを sigiro に送信する](/docs/how-to/agent-telemetry) — Claude Code や Codex も sigiro に向けます
- [ホスト型 sigiro へのテレメトリ送信](/docs/how-to/hosted-onboarding) — サーバーを自分で運用したくない場合に使います
