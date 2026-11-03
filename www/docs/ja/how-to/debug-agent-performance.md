---
title: sigiro でエージェントをデバッグして最適化する
description: コーディングエージェントのセッションを記録し、時間とトークンをどこで使ったかを見つけ、その根拠をより小さく速いワークフローに変えます。
sidebar:
  order: 5
---

このガイドでは、エージェント自身の OpenTelemetry を使って 4 つの問いに答えます。

1. セッションは実時間をどこで使ったか。
2. どのモデル、ツール、操作が最も多くのトークンや呼び出しを消費したか。
3. どの失敗、リトライ、繰り返しの読み取りが役に立たなかったか。
4. 提案した最適化によって、比較可能な 2 回目の実行は改善したか。

1 つの遅いスパンやモデルの自己申告をもとに最適化しないでください。範囲を限定したセッションの時間枠を使い、操作の集計全体を順位付けし、すべての主張の裏付けとなる SQL を残してください。

## 1. 分離したローカル sigiro を起動する

最新リリースをインストールし、サーバーを起動します。

```bash
curl -fsSL https://sigiro.com/install | sh
export PATH="$HOME/.local/bin:$PATH"

export SIGIRO_DATA_DIR="$HOME/.local/share/sigiro-agent-debug"
unset SIGIRO_CATALOG_URL
unset SIGIRO_S3_BUCKET SIGIRO_S3_ENDPOINT SIGIRO_S3_REGION
unset SIGIRO_S3_KEY_ID SIGIRO_S3_SECRET
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN

sigiro serve
```

`SIGIRO_CATALOG_URL` を設定しない場合、sigiro はテレメトリをローカルの Parquet ファイルに保存し、そのメタデータをローカルの DuckDB カタログに保持します。PostgreSQL や S3 は使いません。データディレクトリを明示することで、この実験を他のローカルテレメトリから分離できます。

別のターミナルで、準備ができるのを待ちます。

```bash
sigiro healthcheck
curl http://127.0.0.1:9999/v1/services
```

次に、Claude Code、Codex、Hermes Agent、Pi のいずれかについて [エージェント自身のテレメトリを sigiro に送信する](/docs/how-to/agent-telemetry) に従ってください。

## 2. 役に立つテストセッションを記録する

完了を明確に確認できるタスクを選びます。開始前に次の項目を記録してください。

- 正確なプロンプト
- リポジトリのリビジョンと作業ツリーの状態
- エージェント、モデル、設定
- UTC の開始・終了タイムスタンプ
- キャッシュと依存関係がウォーム状態だったかどうか
- 期待する成果物と検証コマンド

タスクを、途中で介入せずに 1 回実行します。プロンプト、モデル、リポジトリの状態、キャッシュのウォーム度を同時に変えるベンチマークでは、なぜ速くなったのかを特定できません。

実行後、エクスポーターがデータを出力したことを確認します。

```bash
curl http://127.0.0.1:9999/v1/services
```

サービス名はエクスポーターが決めます。エージェントの製品名と同じだと仮定せず、ここで確認してください。

## 3. 範囲を限定したセッションの時間枠をクエリする

実行全体をカバーするリテラルの UTC タイムスタンプを指定して `/v1/query` を使います。まず操作の件数とエラースパンを集計し、その後で該当するトレース ID を調べます。結果は生の根拠であり、診断や因果関係の判定ではありません。返された行を完全なものとして扱う前に、レスポンスヘッダー `x-sigiro-truncated` を確認してください。

## 4. 実時間のコストを順位付けする

スパンを `span_name`（利用できる場合はルートやアドレスの属性）でグループ化し、次のパターンを確認します。

- 1 つのモデル呼び出しが p95 と合計時間の大半を占めている
- 多くの逐次的なツールスパンが、独立して実行できた可能性がある
- セットアップ、認証、依存関係のチェックが 1 つのセッション内で繰り返されている
- 失敗したツールが入力を変えずにリトライされている
- 同じファイル、URL、ステータスエンドポイントが繰り返し取得されている
- 子エージェントが、親がすでに行った探索を繰り返している
- 承認やユーザーの待ち時間が実行時間として数えられている
- 長い親スパンは子の時間を含んでいるだけで、それに加算してはならない

スパンの所要時間は包含的です。親のセッションスパンと、その子であるモデルやツールのスパンを、独立したコストのように合計しないでください。親でエンドツーエンドのレイテンシーを測り、子でその内訳を説明します。

カスタムクエリを書くときは、Parquet の統計でファイルをスキップできるように、リテラルの UTC 範囲を使ってください。

```bash
curl -sS -D /tmp/sigiro-headers -X POST http://127.0.0.1:9999/v1/query \
  -H 'content-type: text/plain' \
  --data "SELECT span_name,
                 count(*) AS calls,
                 sum(duration) / 1000000.0 AS total_seconds,
                 max(duration) / 1000000.0 AS slowest_seconds
          FROM sigiro_spans
          WHERE service_name = '<service-name>'
            AND timestamp >= TIMESTAMP '2026-09-10 12:00:00'
            AND timestamp <  TIMESTAMP '2026-09-10 13:00:00'
          GROUP BY span_name
          ORDER BY total_seconds DESC"
```

レスポンスヘッダー `x-sigiro-truncated` を確認してください。`true` の場合は、完全性を主張する前に、クエリの範囲を狭めるか、さらに集計してください。クエリエンドポイントは読み取り専用の `SELECT` を 1 つだけ受け付け、CTE は受け付けません。

## 5. 根拠を変更に変える

測定された主なコストに対処する、最も小さな変更を適用します。

- **直列化された独立ツール:** まとめてリクエストするか、独立した調査を並行して委任します。
- **繰り返される探索:** 前提条件を 1 回だけ集め、各子エージェントに再探索させるのではなく、その結果を渡します。
- **大きすぎるツール結果:** 取得元で絞り込み、要求するフィールドを減らし、ファイルの範囲を狭め、ログ全体をモデルに戻さないようにします。
- **過剰なモデルコンテキスト:** 重複した指示や古い履歴を取り除き、現在の判断に必要なファイルとレコードだけを取得します。
- **繰り返される失敗:** リトライの前に、欠けているパス、認証情報、依存関係、無効な引数を修正します。入力を変えないリトライは回復ではありません。
- **多すぎるモデル呼び出し:** 互いに依存する細かな質問は範囲を限定した 1 つのリクエストにまとめ、独立した作業は並列のままにします。
- **1 つのモデルに集中したコスト:** 機械的な探索には安価なモデルを使い、高性能なモデルはそれが必要な判断のために取っておきます。
- **長い承認待ち:** 実行速度の改善を主張する前に、人の待ち時間と計算時間を分けます。

トークン数だけから因果関係を推測しないでください。トークンが多いのは、有益な推論の場合も、繰り返されたコンテキストの場合もあります。関連するスパン、エラー、終了理由、ツールの順序を読んでください。

## 6. 最適化を検証する

同じリポジトリの状態と比較可能なキャッシュ条件で、同じプロンプトをもう一度実行します。同じ長さの UTC 時間枠を比較し、次を報告します。

- エンドツーエンドのセッション時間
- モデル呼び出しの回数と p95 レイテンシー
- 出力されている場合は、入力、出力、キャッシュ、推論のトークン数
- ツール呼び出しの回数と、子の累積時間
- 失敗した操作とリトライした操作の件数
- 要求された成果物が同じ検証コマンドに合格したかどうか

タスクに失敗した速い実行は最適化ではありません。比較に使った範囲限定の SQL とクエリ結果を残してください。

## 分析エージェントに直接アクセスさせる

sigiro は 2 つの機械向けインターフェースを公開しています。

- HTTP API 用の `GET http://127.0.0.1:9999/openapi.json`
- stdio MCP ツール用の `sigiro mcp`: `list_services` と `run_query`

Hermes Agent では、`~/.hermes/config.yaml` に次を追加し、Hermes の環境に Python の `mcp` パッケージがインストールされていることを確認してから、Hermes を再起動します。

```yaml
mcp_servers:
  sigiro:
    command: sigiro
    args: [mcp]
```

MCP に対応した他のエージェントも、それぞれの MCP 設定で同じ `sigiro mcp` コマンドを登録できます。エージェントのサービスの `PATH` が対話シェルより狭い場合は、`sigiro` の絶対パスを使ってください。

## 分析エージェント向けのプロンプト

### セッションのボトルネックを見つける

```text
Use Sigiro to analyze service <service-name> from <UTC start> to <UTC end>.
Use list_services, then run_query with literal UTC bounds to rank where the
session spent wall-clock time by operation and model. Distinguish inclusive
parent duration from child duration; do not double-count nested spans. Cite
trace IDs or SQL results for every conclusion. Check truncation before
claiming completeness. End with the three smallest changes most likely to reduce
end-to-end latency without weakening the task's verification.
```

### 無駄な作業を見つける

```text
Audit service <service-name> from <UTC start> to <UTC end> for work that did not
advance the task: repeated file or URL reads, unchanged retries, failed setup or
authentication calls, duplicate parent/child discovery, oversized tool results,
and serial calls that were independent. Query bounded raw telemetry, then inspect
complete rows for suspicious operations. Report counts, duration,
and trace IDs. Separate evidence from inference.
```

### モデルとトークンのコストを分析する

```text
Analyze GenAI telemetry for service <service-name> from <UTC start> to <UTC end>.
Break down calls, p95 latency, errors, finish reasons, and available input,
output, cache, and reasoning tokens by provider and model. Identify where token
volume is concentrated and correlate it with the surrounding operation or tool
sequence. Do not recommend a cheaper model unless the evidence shows which calls
are mechanical enough to move safely.
```

### 前後を比較する

```text
Compare service <service-name> in baseline window <UTC start/end> with optimized
window <UTC start/end>. Use equal query logic and literal TIMESTAMP bounds. Compare
end-to-end duration, model calls and p95, token categories, tool calls, failures,
and retries. Verify both runs completed the same acceptance check. Cite SQL and
rows for every delta, state missing telemetry plainly, and reject improvements
that only moved time into an unmeasured child process.
```

### セットアップと成果物の作業を分ける

```text
Analyze this agent session as two phases: prerequisite/setup work and delivery
work. Derive the boundary from span names and timestamps rather than assuming it.
Report duration, failures, retries, and model/tool calls in each phase. Identify
which setup checks can be cached or performed once, and which are required for
correctness. Include trace IDs and runnable drill-down SQL.
```
