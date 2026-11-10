---
title: CLI
description: sigiro のすべてのサブコマンド、その引数、読み取る環境変数（ホスト型のサインアップとログインを含む）。
sidebar:
  order: 2
---

`sigiro` には 8 つのサブコマンドがあります。クライアントコマンドは成功時にインデント付きの JSON を
出力します。サーバーコマンドはそのプロトコルまたはリスナーに接続されたまま動作し続けます。

ホスト型の新しいアカウントを作るには、`sigiro signup --name "Your Name" --email
you@example.com` を実行します。6 桁の OTP をメールで送り、ターミナルで安全に入力を求め、
個人ワークスペースをちょうど 1 つ作成して有効化し、OAuth デバイス認証を内部で完了して、
リフレッシュ／アクセス認証情報を OS の認証情報ストアに保存します。既存のアカウントでは、
ブラウザーを使う `sigiro auth login` を使ってください。`--no-browser` を付けると、代わりに
確認用 URL を表示します。`sigiro auth status`、`sigiro auth token`、`sigiro auth logout` で
保存済みのセッションを管理します。

グローバルフラグ `--token <TOKEN>` は、自動化のための明示的な上書きです。
`SIGIRO_ACCESS_TOKEN` より優先されます。オープンモードのセルフホストサーバーでは不要ですが、
静的ベアラー認証を設定したセルフホストサーバーでは必要です。

すべてのクライアントサブコマンド（`status`、`mcp`）は、次の順序で接続先サーバーを選びます。

1. `SIGIRO_ENDPOINT` が設定されていればそれ。
2. `sigiro auth login` または `sigiro signup` がセッションを保存していれば
   `https://sigiro.com`。
3. それ以外は `http://localhost:9999`。

そのため、ホスト型 sigiro にサインインした後は、ローカルで `sigiro serve` が動いていても、
これらのコマンドは `sigiro.com` と通信します。ローカルサーバーに接続するには
`SIGIRO_ENDPOINT=http://localhost:9999` を設定するか、`sigiro auth logout` を実行してください。

## ホスト型エージェント認証

sigiro は内部で公式の Agent Auth 実装を使っており、オンボーディング用の 2 つ目の CLI は
提供していません。委任ホストの登録、1 回限りの承認、最小権限の capability 付与、トークンの使用、
鍵のローテーション、失効については
[Agent Auth でエージェントをオンボーディングする](/docs/how-to/agent-auth) を参照してください。

## ingest-keys

ホスト型 sigiro で長時間動作するサービス向けの固定キーで、15 分で失効する `sigiro auth token` の代わりに使います。キーはログインが紐づくワークスペースに属し、`Authorization: Bearer <key>` として送信され、テレメトリの送信だけができます。

| コマンド                                  | 動作                                                         |
| ----------------------------------------- | ------------------------------------------------------------ |
| `sigiro ingest-keys create --name <NAME>` | キーを作成して一度だけ表示します。保持するのはハッシュだけです |
| `sigiro ingest-keys list`                 | ID、名前、最終使用、取り消し状態を一覧表示します。秘密は出しません |
| `sigiro ingest-keys revoke <ID>`          | キーを取り消します。以後、その送信元は `401` を受け取ります    |

キーを管理できるのはご自身のログインだけです。Agent Auth トークンやインジェストキーでは管理できません。

## serve

OTLP gRPC サーバー、OTLP HTTP サーバー、クエリ API を起動します。

| フラグ                    | 上書きする変数          | 既定値                  |
| ------------------------- | ----------------------- | ----------------------- |
| `--data-dir <PATH>`       | `SIGIRO_DATA_DIR`       | `~/.local/share/sigiro` |
| `--http-port <PORT>`      | `SIGIRO_HTTP_PORT`      | `9999`                  |
| `--otlp-http-port <PORT>` | `SIGIRO_OTLP_HTTP_PORT` | `4318`                  |
| `--otlp-grpc-port <PORT>` | `SIGIRO_GRPC_PORT`      | `4317`                  |

初回起動時に、クエリ拡張機能をデータディレクトリへダウンロードします。このダウンロードには
30〜90 秒かかり、その間は起動が進みません。

認証なしのサーバー (セルフホストの既定) は、Web ダッシュボードも
`http://localhost:9999/dashboard/` で提供します。ダッシュボードはバイナリに組み込まれて
います。別の静的ビルドを使うには、そのビルドを含むディレクトリ (`dashboard/dist`) を
`SIGIRO_DASHBOARD_DIR` に設定します。

## query

`sigiro query "<SQL>"` はローカルクライアントではなく、**フェデレーション型のコーディネーター**です。
1 つ以上のエッジを解決し、各エッジに部分クエリを送り、部分結果を 1 つの答えにまとめます。

| フラグ                    | 型         | 既定値 |
| ------------------------- | ---------- | ------ |
| `<SQL>`                   | 文字列     | なし   |
| `--edge <NAME=HOST:PORT>` | 繰り返し可 | なし   |
| `--full-scan`             | フラグ     | オフ   |

エッジは次の順序で解決します。`SIGIRO_EDGES`（カンマ区切りの `name=host:port`）と `--edge` は
そのまま使われ、どちらか一方があれば探索は行いません。どちらも設定されていない場合、sigiro は
`tailscale status --json` を実行し、`tag:sigiro-edge` タグの付いたすべてのピアを使います。
解決されたエッジが 0 個の場合、何もない相手に対して実行するのではなく、コマンドは失敗します。

したがって `sigiro query` は、ホスト型の `sigiro.com` や通常のローカルサーバーに対しては
**動作しません**。エッジは `SIGIRO_QUERY_SURFACE=quack` でデータを提供する必要があり、
`--edge` で指定するか tailnet 上で見つけられる必要があります。ホスト型 sigiro や 1 台のローカル
サーバーに対して SQL を実行するには、[`sigiro mcp`](#mcp) の `run_query` ツールを使うか、
SQL を `/v1/query` に POST してください。

```bash
curl -s https://sigiro.com/v1/query \
  -H "Authorization: Bearer $(sigiro auth token)" \
  --data "SELECT service_name, count(*) AS spans FROM sigiro_spans
          WHERE timestamp > now() - INTERVAL '1 hour' GROUP BY 1"
```

`--full-scan` を指定すると、`timestamp` に対する `WHERE` の範囲指定がないクエリを許可します。
既定では、sigiro はそのようなクエリを拒否します。

## status

`sigiro status` は、上記の方法で選ばれたサーバーの `/health` エンドポイントを読み取り、
ヘルスブロックと、直近 5 分間のスパン数とログ数を含む `recent_ingest` ブロックを出力します。
`endpoint` フィールドは、確認したサーバーを示します。`/health` には鍵は不要です。

## healthcheck

`sigiro healthcheck` はローカルの `/health` エンドポイントを確認し、サーバーの準備ができている
ときだけ `0` で終了します。distroless イメージにはシェルも `curl` もないため、これがコンテナの
`HEALTHCHECK` になります。

## mcp

`sigiro mcp` は stdio の Model Context Protocol サーバーを起動します。`list_services` と
`run_query` に加えて、ドキュメントツール（`search_docs`、`get_page`、`list_pages`、
`get_navigation`）を公開するため、分析エージェントは自分で HTTP リクエストを組み立てずに
sigiro を調べられます。上記の方法で選ばれたサーバーを使うので、`sigiro auth login` の後は
保存済みのセッションでホスト型ワークスペースを読み取り、そのセッションも自分で更新します。

stdio サーバーとしてエージェントに登録します。

```bash
claude mcp add sigiro -- sigiro mcp     # Claude Code
codex mcp add sigiro -- sigiro mcp      # Codex
```

エージェントがシェルより狭い `PATH` で起動する場合は、`sigiro` の絶対パスを使ってください。
`run_query` では、`timestamp > now() - INTERVAL '1 hour'` のようにタイムスタンプ値で
`timestamp` を絞り込んでください。結果の行ではタイムスタンプと `duration` が Unix エポックからの
マイクロ秒で表示されますが、`timestamp` を整数と比較すると失敗します。

ドキュメントツールは `https://sigiro.com/mcp/docs` のホスト型ドキュメント MCP を使います。
`SIGIRO_DOCS_MCP_URL` を設定した場合はそちらを使います。オフラインのとき、エンドポイントが数秒以内に
応答しないとき、またはエラーを返したときは、バイナリに組み込まれたこのドキュメントのコピーから
答えます。ツール名、引数、回答の形は変わりませんが、組み込みのコピーは実行中のリリース時点の
ドキュメントです。ネットワークを使わず常に組み込みのコピーを使うには、`SIGIRO_DOCS_MCP_URL` を
空の値に設定してください。

ホスト型 sigiro は、CLI のセッションを使わずにブラウザーで OAuth サインインするエージェント向けに、
同じツールを streamable HTTP で `https://sigiro.com/mcp` からも提供しています。サインインなしで
使えるドキュメントツールだけなら `https://sigiro.com/mcp/docs` にあります。
