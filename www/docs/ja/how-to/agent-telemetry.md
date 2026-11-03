---
title: エージェント自身のテレメトリを sigiro に送信する
description: Claude Code、Codex、Hermes Agent、Pi を sigiro に向けて、セッション、モデルのレイテンシー、トークン、ツール、エラー、リトライを調べます。
sidebar:
  order: 4
---

このガイドでは、コーディングエージェントが編集するアプリケーションの動作ではなく、エージェント自身の動作を記録します。得られたスパン、ログ、メトリクスから、遅いモデル呼び出し、繰り返されるツール、失敗した前提条件、過剰なコンテキスト、直列化された作業を見つけられます。

これは、_あなたのアプリケーション_ を sigiro に向ける [エージェントでコードを計装する](/docs/how-to/install-with-ai) とは別のものです。両方の視点が必要な場合は両方を使ってください。

## sigiro スキルをインストールする

エージェントを起動する前に、公開スキルをインストールします。

```bash
npx skills add Arnav12114/skills
```

[`debug-with-sigiro` スキル](https://github.com/Arnav12114/skills) は、sigiro でテレメトリを調べる方法をエージェントに教えます。このガイドのテレメトリ設定は変更しません。

## 始める前に

sigiro を起動し、API を確認します。

```bash
sigiro serve
curl http://127.0.0.1:9999/health
```

ローカルのレシーバーは `4318` で OTLP/HTTP を、`4317` で OTLP/gRPC を受け付けます。リクエストの認証はないため、信頼できないネットワークには公開しないでください。ホスト型サービスを使う場合は、新しいアカウントなら `sigiro signup --name "Your Name" --email you@example.com` を（既存のアカウントなら `sigiro auth login` を）実行し、代わりに [ホスト型 sigiro へのテレメトリ送信](/docs/how-to/hosted-onboarding) に従ってください。

テレメトリには、プロンプト、応答、ファイルの内容、コマンド、ユーザー識別子、環境の詳細が含まれることがあります。以下の例では控えめな取得設定を選んでいます。より多くの内容を出力する前に、すべてのエクスポーターを確認してください。

## Claude Code

Claude Code には OpenTelemetry のエクスポート機能が組み込まれています。次のキーを `~/.claude/settings.json` の `env` オブジェクトに追加します。これで、IDE が起動するセッションも含め、すべてのセッションがテレメトリを送信します。

```json
{
  "env": {
    "CLAUDE_CODE_ENABLE_TELEMETRY": "1",
    "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA": "1",
    "OTEL_TRACES_EXPORTER": "otlp",
    "OTEL_METRICS_EXPORTER": "otlp",
    "OTEL_LOGS_EXPORTER": "otlp",
    "OTEL_EXPORTER_OTLP_PROTOCOL": "http/protobuf",
    "OTEL_EXPORTER_OTLP_ENDPOINT": "http://127.0.0.1:4318",
    "OTEL_METRIC_EXPORT_INTERVAL": "10000"
  }
}
```

ファイルにすでに `env` オブジェクトがある場合は、キーをそこにマージしてください。同じ変数は、`claude` を起動するシェルでの `export` 文としても機能します。

Claude Code のトレーシングはベータ版です。`CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1` と `OTEL_TRACES_EXPORTER=otlp` で有効になります。トレースがないと診断の `gen_ai` と `operation_summaries` セクションは空になりますが、ログやメトリクスを送ったサービスは引き続き `/v1/services` に表示されます。お使いのバージョンについては [Claude Code monitoring](https://code.claude.com/docs/en/monitoring-usage) を参照してください。

新しい Claude Code セッションを開始してプロンプトを 1 つ実行し、10 秒待ってから、以下の確認手順を実行してください。

### Claude Code が既定で送信するもの

すべてのメトリクス、ログ、スパンには、`user.email`、`user.account_uuid`、`user.account_id`、`user.id`、`organization.id`、`session.id`、`terminal.type` が付きます。共有サーバーやホスト型サーバーにテレメトリを送る前に、これらの ID 属性について検討してください。

次のセクションの設定がない場合、テレメトリには各ツールの名前は含まれますが、引数は含まれません。

- **ツール呼び出し:** ツール名、成否、所要時間、入力と結果のバイト数、権限の判定。
- **Bash コマンド:** 固定リストにある最初のプログラムだけ（`bash_argv0`。例: `git`、または `other`）と、そのカテゴリ（`bash_command_class`）。
- **ツールの失敗:** `ShellError` や `TelemetrySafeError` などのカテゴリだけ。
- **プロンプト:** `<REDACTED>` とプロンプトの長さ。
- **モデルリクエスト:** `claude_code.api_request` イベントのモデル、トークン、最初のトークンまでの時間、`cost_usd`。

### より詳しく取得する

以下の各設定は、コマンド、ファイルの内容、会話のテキストなど、より多くの内容をエクスポートします。露出を受け入れられる設定だけを有効にし、新しいセッションを開始してください。

| 設定                                   | 追加されるもの                                                                                                                                                                  |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `OTEL_LOG_TOOL_DETAILS=1`              | ツールスパンの `full_command`、`file_path`、`skill_name`、`subagent_type`。ツールイベントの `tool_input` と `tool_parameters`。エラーの全文。MCP サーバー名と接続エラー。 |
| `OTEL_LOG_TOOL_CONTENT=1`              | 各ツールの入力と出力を含む `tool.output` スパンイベント。トレースが必要です。                                                                                                  |
| `OTEL_LOG_USER_PROMPTS=1`              | `claude_code.user_prompt` イベントと `claude_code.interaction` スパンのプロンプトテキスト。                                                                                    |
| `OTEL_LOG_ASSISTANT_RESPONSES=1`       | `claude_code.assistant_response` イベントの応答テキスト。設定しない場合は `OTEL_LOG_USER_PROMPTS` に従います。応答を伏せたままにするには `0` を設定します。                    |
| `OTEL_LOG_RAW_API_BODIES=1`            | API のリクエストとレスポンスの JSON 全体を、`claude_code.api_request_body` と `claude_code.api_response_body` イベントとして出力します。リクエストボディには会話履歴が含まれます。 |
| `OTEL_METRICS_INCLUDE_VERSION=true`    | メトリクスとイベントの `app.version`。                                                                                                                                          |
| `OTEL_METRICS_INCLUDE_ENTRYPOINT=true` | メトリクスとイベントの `app.entrypoint`。                                                                                                                                       |

失敗したツールを見ると違いが分かります。既定の設定では、失敗した Read は `TelemetrySafeError`、失敗した Bash コマンドは `ShellError` を報告します。`OTEL_LOG_TOOL_DETAILS=1` を設定すると、同じ失敗が、失敗したコマンドやパスとともに `File does not exist. Note: your current working directory is …` や `Shell command failed` として報告されます。

Claude Code は内容の属性を `CLAUDE_CODE_OTEL_CONTENT_MAX_LENGTH`（既定は 61,440 文字）で切り詰めます。長い会話のリクエストボディは、その長さで切れた状態で届きます。sigiro はこのサイズの値を保存します。それより大きな上限はテストしていません。

### 詳細なベータトレーシング

詳細なベータトレーシングでは、各ステップでモデルが受け取ったものと返したものも記録します。同じ `env` オブジェクトに次のキーを追加します。

```json
"ENABLE_BETA_TRACING_DETAILED": "1",
"BETA_TRACING_ENDPOINT": "http://127.0.0.1:4318"
```

これらのキーを設定すると、Claude Code はログとトレースを `OTEL_EXPORTER_OTLP_ENDPOINT` ではなく `BETA_TRACING_ENDPOINT` に送ります。メトリクスは引き続き `OTEL_EXPORTER_OTLP_ENDPOINT` に送られます。両方に同じ sigiro のベース URL を使ってください。

詳細なベータトレーシングで追加されるもの:

- フックの実行ごとの `claude_code.hook` スパン（`OTEL_LOG_TOOL_DETAILS=1` の場合はフックの設定を含む）
- インタラクション、モデルリクエスト、ツールの各スパンの `new_context`: そのステップで Claude Code がモデルのコンテキストに追加したテキスト
- モデルリクエストスパンの `response.model_output`、`query_source`、`system_prompt_preview`、`system_prompt_length`、`system_prompt_hash`、`system_reminders`、`tools`
- `claude_code.system_prompt` ログイベント

`system_reminders` はモデルリクエストごとに数十キロバイトになることがあります。Claude Code はこれらの属性を安定したスパンスキーマに含めていないため、名前が変わる可能性があります。対話型セッションでこれらが得られるのは、組織が Anthropic の許可リストに入っている場合だけです。`claude -p` と Agent SDK のセッションには許可リストは不要です。Claude Code は、プロジェクトとローカルの設定ファイルにあるこれら 2 つのキーを無視します。

### サブプロセスからのトレース

Claude Code は各 Bash コマンドに `TRACEPARENT` 変数を渡します。Claude Code の Bash ツールが起動した `claude -p` の実行は、親セッションのトレースに加わります。その `claude_code.interaction` スパンには `parent.source: env` が付き、`/v1/traces/{trace_id}` は両方のセッションのスパンを返します。`session.id` で絞り込んで分けてください。

### Claude Code のテレメトリを読む

```text
claude_code.interaction                  one user prompt
├── claude_code.llm_request              one model request
├── claude_code.tool                     permission wait and execution
│   ├── claude_code.tool.blocked_on_user permission wait
│   │   └── claude_code.llm_request      auto mode permission check
│   └── claude_code.tool.execution       execution
└── claude_code.hook                     detailed beta tracing only
```

- `claude_code.tool` の所要時間には権限の待ち時間が含まれます。実行時間には `claude_code.tool.execution` を、待ち時間には `claude_code.tool.blocked_on_user` を使ってください。
- auto モードでは、Claude Code は一部のツール呼び出しを別のモデルリクエストで確認します。このリクエストは `claude_code.tool.blocked_on_user` の下にある `claude_code.llm_request` スパンで、`query_source_safe = 'auto_mode'` が付きます。Claude Code 2.1.273 では、確認 1 回あたり約 1.4 秒かかりました。このスパンにはトークン数がありますが、Claude Code はこれに対して `claude_code.api_request` イベントも `claude_code.cost.usage` のデータポイントも送りません。Claude Code のコスト合計にはこれらの確認は含まれません。
- 失敗したツールは、スパンのステータスと、その `claude_code.tool_result` イベントの `success = false` を設定します。Claude Code はこれに対して ERROR 重大度のログを送りません。
- `claude_code.api_request` イベントと `claude_code.cost.usage` メトリクスは、同じコスト合計を報告します。

これらのクエリのタイムスタンプは、セッションを囲むリテラルの UTC 範囲に置き換えてください。日別・モデル別のコスト:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT strftime(timestamp, '%Y-%m-%d') AS usage_date,
                 json_extract_string(log_attributes, 'model') AS model,
                 count(*) AS requests,
                 round(sum(CAST(json_extract_string(log_attributes, 'cost_usd') AS DOUBLE)), 2) AS cost_usd
          FROM sigiro_logs
          WHERE service_name = 'claude-code'
            AND body = 'claude_code.api_request'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY usage_date, model
          ORDER BY usage_date, cost_usd DESC"
```

ツールごとの呼び出し数、失敗数、p95 所要時間:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT json_extract_string(log_attributes, 'tool_name') AS tool,
                 count(*) AS calls,
                 count(*) FILTER (WHERE json_extract_string(log_attributes, 'success') = 'false') AS failures,
                 quantile_cont(CAST(json_extract_string(log_attributes, 'duration_ms') AS DOUBLE), 0.95) AS p95_ms
          FROM sigiro_logs
          WHERE service_name = 'claude-code'
            AND body = 'claude_code.tool_result'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY tool
          ORDER BY calls DESC"
```

権限の待ち時間と実行時間の比較:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT span_name,
                 count(*) AS spans,
                 round(sum(duration) / 60000000.0, 1) AS total_minutes,
                 median(duration) / 1000 AS p50_ms
          FROM sigiro_spans
          WHERE service_name = 'claude-code'
            AND span_name IN ('claude_code.tool.blocked_on_user', 'claude_code.tool.execution')
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY span_name"
```

Claude Code のコスト合計に含まれない、auto モードの権限確認:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT json_extract_string(span_attributes, 'model') AS model,
                 count(*) AS checks,
                 round(sum(duration) / 60000000.0, 1) AS total_minutes,
                 sum(CAST(json_extract_string(span_attributes, 'cache_read_tokens') AS BIGINT)) AS cache_read_tokens,
                 sum(CAST(json_extract_string(span_attributes, 'output_tokens') AS BIGINT)) AS output_tokens
          FROM sigiro_spans
          WHERE service_name = 'claude-code'
            AND span_name = 'claude_code.llm_request'
            AND json_extract_string(span_attributes, 'query_source_safe') = 'auto_mode'
            AND timestamp >= TIMESTAMP '2026-09-10 00:00:00'
            AND timestamp <  TIMESTAMP '2026-09-11 00:00:00'
          GROUP BY model"
```

## Codex

Codex はテレメトリの設定を `~/.codex/config.toml` から読み取ります。

```toml
[otel]
environment = "dev"
log_user_prompt = false

[otel.trace_exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"

[otel.metrics_exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"

[otel.exporter.otlp-grpc]
endpoint = "http://127.0.0.1:4317"
```

`exporter` はログのエクスポーターで、`trace_exporter` や `metrics_exporter` の代わりにはなりません。Codex は自身を `codex_exec` として報告するため、確認の際はそのサービス名を探してください。

## Hermes Agent

Hermes は、コミュニティ製の [`briancaffey/hermes-otel`](https://github.com/briancaffey/hermes-otel) プラグインを使います。

```bash
hermes plugins install briancaffey/hermes-otel/hermes_otel
```

プラグインの依存パッケージは、Hermes を実行するのと同じ Python 環境にインストールしてください。仮想環境が `~/git/hermes-agent/venv` にあるソースチェックアウトの場合:

```bash
~/git/hermes-agent/venv/bin/pip install \
  -r ~/.hermes/plugins/hermes_otel/requirements.txt
```

お使いのインストールに対応する `pip` のパスを使ってください。無関係なシステムの Python にこれらのパッケージをインストールしても、Hermes からは見えません。

`~/.hermes/hermes_otel.yaml` を作成します。

```yaml
backends:
  - type: otlp
    name: sigiro
    endpoint: http://127.0.0.1:4318/v1/traces
    metrics: true
    logs: true

capture_logs: true
capture_previews: false
capture_sender_id: false
capture_conversation_history: false

# Optional host-level evidence for local performance work.
host_metrics: true
host_metrics_gpu: auto
host_metrics_interval_ms: 1000
```

プラグインをインストールまたは変更した後は Hermes を再起動してください。汎用の OTLP バックエンドは、トレースのエンドポイントから `/v1/metrics` と `/v1/logs` を導きます。セッション、モデル、API、ツール、スキル、承認、サブエージェントの入れ子のスパンと、トークン、所要時間、コスト、リトライ、リソースのメトリクスを出力します。

このプラグインでは `capture_previews` の既定値は `true` です。上の明示的な `false` により、プロンプト、応答、ツールの引数、ツールの結果のプレビューはエクスポートされません。`capture_logs: true` を指定すると、インポートしたライブラリの Python レコードがエクスポートされることがあります。ロガーの範囲をより狭くしたい場合は、プラグインの `log_attach_logger` 設定を使ってください。

## Pi コーディングエージェント

Pi の公式パッケージ `@earendil-works/pi-telemetry` はテレメトリの型を定義していますが、現在 OTLP エクスポーターは提供していません。そのため、以下の設定ではコミュニティが保守する [`@devkade/pi-opentelemetry`](https://github.com/devkade/pi-opentelemetry) 拡張機能を使います。採用する前に内容を確認してください。

```bash
pi install npm:@devkade/pi-opentelemetry

export PI_OTEL_ENABLE=1
export OTEL_SERVICE_NAME=pi-coding-agent

export OTEL_TRACES_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://127.0.0.1:4318/v1/traces

export OTEL_METRICS_EXPORTER=otlp
export OTEL_EXPORTER_OTLP_METRICS_ENDPOINT=http://127.0.0.1:4318/v1/metrics
export OTEL_METRIC_EXPORT_INTERVAL=10000

export PI_OTEL_PRIVACY_PROFILE=strict
export PI_OTEL_PAYLOAD_MAX_BYTES=32768

pi
```

Pi の中では、`/otel-status` がエクスポーターの状態を報告し、`/otel-open-trace` が現在のトレースを開きます。この拡張機能のドキュメントにはトレースとメトリクスの記載はありますが、ログのエクスポートはありません。`strict` プライバシープロファイルは、既定の `detailed-with-redaction` プロファイルより公開する情報が少なくなります。

## テレメトリが届いたことを確認する

設定したエージェントで実際のプロンプトを 1 つ実行し、サービス名を確認します。

```bash
curl http://127.0.0.1:9999/v1/services
```

エージェントのエントリを見つけ、`signals.spans`、`signals.logs`、`signals.metrics` を確認します。スパンが 0 でもログやメトリクスが 0 でないサービスは、テレメトリを送信しています。インストールの失敗ではありません。スパンベースの診断（`gen_ai` と `operation_summaries`）を期待できるのは、`signals.spans` が 0 でない場合だけです。トレースを有効にしたのにスパンが 0 の場合は、エージェントのトレースエクスポーターの設定を確認してください。出力される語彙を推測しないでください。件数が 0 でないシグナルのテーブルだけをクエリします。2 つのタイムスタンプは、テストセッションを囲むリテラルの UTC 範囲に置き換えてください。スパンの場合:

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT service_name, span_name, count(*) AS occurrences
          FROM sigiro_spans
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY service_name, span_name
          ORDER BY occurrences DESC"
```

`signals.logs` が 0 でなければ、ログを調べます。

```bash
curl -sS -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT service_name, count(*) AS records
          FROM sigiro_logs
          WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY service_name ORDER BY records DESC"
```

`signals.metrics` が 0 でなければ、OpenTelemetry の各テーブル種別にわたってメトリクス名を確認します。

```bash
for table in gauge sum histogram exp_histogram; do
  curl -sS -X POST http://127.0.0.1:9999/v1/query \
    -H 'content-type: text/plain' \
    --data "SELECT metric_name, count(*) AS points
            FROM sigiro_metrics_${table}
            WHERE timestamp >= TIMESTAMP '2026-09-10 12:00:00'
              AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
            GROUP BY metric_name ORDER BY points DESC"
done
```

リテラルの `TIMESTAMP` 範囲を使うと、sigiro は Parquet ファイルを絞り込めます。`now()` から作った述語はすべてのファイルを読み、ローカルの履歴が増えるにつれて徐々に遅くなります。

次は、[sigiro でエージェントをデバッグして最適化する](/docs/how-to/debug-agent-performance) に進んでください。
