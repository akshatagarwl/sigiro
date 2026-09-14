---
title: エージェント自身のテレメトリーを sigiro に送信する
description: >-
  Claude Code と Codex は自身のセッションの OpenTelemetry を出力します。sigiro
  に向ければ、トークン数やツール呼び出し、レイテンシーを SQL で読み出せます。
sidebar:
  order: 3
---

このガイドでは、サービスの動作ではなく、エージェント自身の動作を記録する方法を説明します。Claude Code と Codex はどちらも、自身のセッションに関する OpenTelemetry を出力します。そのテレメトリーを sigiro に向けると、セッションがクエリ可能な行になります。

これは
[エージェントでコードを計装する](/docs/how-to/install-with-ai)
とは別の作業です。あちらは _あなたのサービス_ を sigiro に向けるものです。両方が必要であれば、両方を実施してください。

## 始める前に

sigiro サーバーが必要です。`sigiro serve` で起動するか、[クイックスタート](/docs/tutorials/quickstart#1-実行する)にある Docker コマンドで起動してください。

以下の設定では、どちらのエージェントもポート `4317` の OTLP gRPC 経由でエクスポートします。セルフホストのサーバーではキーは不要です。

ホスティングサービスでは、代わりに **HTTP** 経由の OTLP を使用してください。ホスティング事業者が必ずしも gRPC ポートを公開しているとは限らないため、先に [ホスティング版 sigiro にテレメトリーを送信する](/docs/how-to/hosted-onboarding#2-opentelemetry-エクスポーターを-sigiro-に向ける) をお読みください。以下の各セクションの末尾には、ホスティング版向けの設定を記載しています。

## Claude Code

Claude Code はテレメトリー設定を環境変数から読み取ります。Claude Code を起動する環境で、次の変数を設定してください。シェルのプロファイル、`.envrc` ファイル、またはラッパースクリプトを使用します。

```bash
export CLAUDE_CODE_ENABLE_TELEMETRY=1
export CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1
export OTEL_METRICS_EXPORTER=otlp
export OTEL_LOGS_EXPORTER=otlp
export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_PROTOCOL=grpc
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4317
```

ホスティングサービスを利用する場合は、キーを追加するだけでなく、プロトコルとエンドポイントも変更してください。`api.sigiro.com` は 443 でのみ応答するため、そこに `:4317` や `:4318` のエンドポイントを指定すると接続に失敗し、エージェントは何も報告しません。

```bash
export OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf
export OTEL_EXPORTER_OTLP_ENDPOINT=https://api.sigiro.com
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${SIGIRO_ACCESS_TOKEN}"
```

キーはコミット対象のファイルに一切含めないようにします。使用するエンドポイントとポートはホスティング事業者から案内されるため、そのホストが `api.sigiro.com` でない場合は `https://<your-sigiro-host>` を使用してください。

デフォルトでは、失敗したツールは `TelemetrySafeError` や `ShellError` のような一般的なステータスしか報告しないことがあります。失敗の理由をデバッグ中に確認するには、詳細を明示的に有効にします。

```bash
export OTEL_LOG_TOOL_DETAILS=1
```

この設定を有効にすると、コマンド、ファイルパス、エラーテキストが送信される可能性があります。追加のプライバシー上のリスクを許容できる場合にだけ有効にしてください。

## Codex

Codex はテレメトリー設定を環境変数からではなく、独自の設定ファイルから読み取ります。`~/.codex/config.toml` を編集してください。

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-grpc]
endpoint = "http://localhost:4317"

[otel.metrics_exporter.otlp-grpc]
endpoint = "http://localhost:4317"

[otel.exporter.otlp-grpc]
endpoint = "http://localhost:4317"
```

Codex はシグナルごとに 1 つのエクスポーターを指定し、`exporter` は **ログ** 用のエクスポーターのみを指します。`trace_exporter` を設定しないとスパンが得られず、`metrics_exporter` を設定しないとメトリクスが得られません。どちらの誤りも警告なく発生します。

ホスティングサービスを利用する場合は、HTTP エクスポーターを使用し、各エンドポイントに **シグナルごとの完全なパス** を指定してください。Codex は記載された URL にそのまま送信し、何も付加しないため、ベース URL では誤ったルートに届き、sigiro に拒否されます。

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/traces"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_ACCESS_TOKEN}" }

[otel.metrics_exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/metrics"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_ACCESS_TOKEN}" }

[otel.exporter.otlp-http]
endpoint = "https://api.sigiro.com/v1/logs"
protocol = "binary"
headers = { authorization = "Bearer ${SIGIRO_ACCESS_TOKEN}" }
```

`otlp-http` では `protocol` が必須で、`binary` または `json` を指定します。`SIGIRO_ACCESS_TOKEN` は Codex を起動する環境で設定します。

Codex は自身を `codex` ではなく `codex_exec` として報告するため、以下の確認手順ではその名前を探してください。

## 両者に共通する 1 つの制限

これらの設定が対象とするのは、エージェントのプロセスのみです。エージェントが実行するコマンドは対象外です。エージェントがシェルツール経由で起動したアプリケーションが、必ずしもこれらの変数を継承するとは限りません。そのため、計装済みのサービスには、そのサービス自身のエクスポーター設定が別途必要です。それには
[エージェントでコードを計装する](/docs/how-to/install-with-ai) を利用してください。

## 動作を確認する

エージェントを起動し、プロンプトを 1 回実行してから、sigiro が受信したサービスを確認します。

```bash
curl http://localhost:9999/v1/services
```

エージェントは自身を 1 つのサービスとして報告します。表示されたら、セッションをクエリします。

```bash
curl -X POST http://localhost:9999/v1/query \
  --data "SELECT service_name, span_name, count(*) AS n
          FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour'
          GROUP BY 1, 2 ORDER BY n DESC"
```

エージェントはカウンターやゲージも報告します。これらは `sigiro_spans` ではなく `sigiro_metrics_*` テーブルに格納されます。どのファミリーが何を保持し、それぞれのファミリーがどのマシンを表しているかについては、[テーブルについて](/docs/explanation/tables)を参照してください。

## 次のステップ

- [エージェントでコードを計装する](/docs/how-to/install-with-ai) — あなたのサービスも sigiro に向ける
- [CLI リファレンス](/docs/reference/cli) — すべてのサブコマンドとフラグ
