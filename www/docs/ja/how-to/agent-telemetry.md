---
title: エージェント自身のテレメトリーを sigiro に送信する
description: >-
  Claude Code と Codex はどちらも、自身のセッションに関する OpenTelemetry を出力します。これらを sigiro
  に向けることで、トークン数、ツール呼び出し、セッションのレイテンシーを SQL で読み出せます。
sidebar:
  order: 3
---
このガイドでは、サービスの動作ではなく、エージェント自身の動作を記録する方法を説明します。Claude Code と Codex はどちらも、自身のセッションに関する OpenTelemetry を出力します。そのテレメトリーを sigiro に向けると、セッションがクエリ可能な行になります。

これは
[エージェントでコードを計装する](/docs/how-to/install-with-ai)
とは別の作業です。あちらは _あなたのサービス_ を sigiro に向けるものです。両方が必要であれば、両方を実施してください。

## 始める前に

sigiro サーバーが必要です。`sigiro serve` で起動するか、[クイックスタート](/docs/tutorials/quickstart#1-run-it)にある Docker コマンドで起動してください。

以下の設定では、どちらのエージェントもポート `4317` の OTLP gRPC 経由でエクスポートします。セルフホストのサーバーではキーは不要です。

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

ホスティングサービスを利用する場合は、
`OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer ${SIGIRO_API_KEY}"` を追加してください。キーはコミット対象のファイルに一切含めないようにします。

## Codex

Codex はテレメトリー設定を環境変数からではなく、独自の設定ファイルから読み取ります。`~/.codex/config.toml` を編集してください。

```toml
[otel]
environment = "dev"
log_user_prompt = false
exporter = "otlp-grpc"

[otel.exporter."otlp-grpc"]
endpoint = "http://localhost:4317"
```

ホスティングサービスを利用する場合は、
`headers = { authorization = "Bearer ${SIGIRO_API_KEY}" }` を追加してください。`SIGIRO_API_KEY` は Codex を起動する環境で設定します。

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
